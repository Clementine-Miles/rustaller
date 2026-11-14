#!/usr/bin/env bash
# Process isolation fixtures; never starts host init units or touches networking.
set -euo pipefail
ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
fixture=$(mktemp -d)
trap 'rm -rf -- "$fixture"' EXIT
DMB_STATE_DIR=$fixture/state
DMB_LOCK_DIR=$fixture/locks
mkdir -p -- "$DMB_STATE_DIR" "$DMB_LOCK_DIR"
# shellcheck source=../src/core.sh
source "$ROOT/src/core.sh"
# shellcheck source=../src/transaction.sh
source "$ROOT/src/transaction.sh"
# shellcheck source=../src/recovery/supervisor.sh
source "$ROOT/src/recovery/supervisor.sh"
# A fixture worker intentionally includes test helpers in its own bytes. The
# production artifact has no environment variable or option enabling these mocks.
worker=$fixture/worker.sh
{
  printf '#!/usr/bin/env bash\nset -euo pipefail\n'
  printf 'fixture=%q\nDMB_STATE_DIR=%q\nDMB_LOCK_DIR=%q\n' "$fixture" "$DMB_STATE_DIR" "$DMB_LOCK_DIR"
  printf 'source %q\n' "$ROOT/src/core.sh" "$ROOT/src/transaction.sh" "$ROOT/src/recovery/supervisor.sh"
  cat <<'WORKER'
dmb_require_root() { return 0; }
dmb_tx_namespace_check() { return 0; }
dmb_trusted_path() { [[ $1 == "$fixture"/* && ( -e $1 || ${2:-} == allow_missing ) ]]; }
dmb_atomic_write() { local tmp; tmp=$(mktemp "${1%/*}/.write.XXXXXXXX") || return; cat > "$tmp" && chmod "$2" "$tmp" && mv -T "$tmp" "$1"; }
dmb_confirmation_prepare() { return 0; }
dmb_recovery_ready() { return 0; }
dmb_recovery_disarm() { return 0; }
dmb_fs_capture() { jq -cn --arg value "$(cat "$fixture/resource")" '{value:$value}'; }
dmb_fs_snapshot() { return 0; }
dmb_fs_apply() {
  jq -r --argjson i "$2" '.steps[$i].after.value' "$1" > "$fixture/resource"
  : > "$fixture/started"
  if [[ -f $fixture/block ]]; then sleep 60; fi
}
dmb_fs_verify() { return 0; }
dmb_fs_restore() { jq -r --argjson i "$2" '.steps[$i].before.value' "$1" > "$fixture/resource"; }
if [[ ${1:-} == --internal-adapter-guarded ]]; then shift; dmb_tx_adapter_guarded "$@"; fi
WORKER
} > "$worker"
# shellcheck disable=SC1090
source "$worker"
dmb_recovery_arm() {
  cp -- "$worker" "$2/worker.sh"
  local hash
  hash=$(sha256sum "$2/worker.sh"); hash=${hash%% *}
  jq -cn --arg id "$1" --arg hash "$hash" '{id:$id,version:1,backend:"systemd",nonce:"0123456789abcdef0123456789abcdef",worker_sha256:$hash}' > "$2/recovery.json"
}
plan=$fixture/plan.json
jq -cn '{schema:1,adapter:"fs",resources:["fixture"],steps:[{id:"one",resource:"fixture",after:{value:"new"}}]}' > "$plan"
printf old > "$fixture/resource"
id=$(dmb_tx_prepare "$plan")
dmb_tx_apply "$id" > /dev/null
[[ $(cat "$fixture/resource") == new ]]
[[ ! -e $DMB_STATE_DIR/transactions/$id/active.json ]]
dmb_tx_rollback "$id"
[[ $(cat "$fixture/resource") == old ]]
printf 'PASS: pinned guarded adapter executes in recorded independent process group\n'

# SIGKILL cannot run a shell trap. The independent recovery path must cancel the
# orphaned active process group before it can acquire locks and apply inverses.
id=$(dmb_tx_prepare "$plan")
touch "$fixture/block"
rm -f "$fixture/started"
dmb_tx_apply "$id" > "$fixture/actor.log" 2>&1 &
actor=$!
for ((tries=0;tries<100;tries++)); do [[ ! -f $fixture/started ]] || break; sleep 0.05; done
[[ -f $fixture/started ]]
kill -KILL "$actor"
wait "$actor" 2>/dev/null || :
dmb_recovery_attempt_rollback "$id"
[[ $(cat "$fixture/resource") == old ]]
[[ $(jq -r .state "$DMB_STATE_DIR/transactions/$id/state.json") == ROLLED_BACK ]]
rm "$fixture/block"
printf 'PASS: forced actor termination; orphan cancellation; independent inverse\n'

# Expiration while a mutation is running must cancel it and use the original
# monotonic deadline; changing wall-clock time is irrelevant to this path.
id=$(dmb_tx_prepare "$plan")
touch "$fixture/block"
rm -f "$fixture/started"
dmb_tx_apply "$id" > "$fixture/expired-actor.log" 2>&1 &
actor=$!
for ((tries=0;tries<100;tries++)); do [[ ! -f $fixture/started ]] || break; sleep 0.05; done
[[ -f $fixture/started ]]
tx=$DMB_STATE_DIR/transactions/$id
jq '.deadline_uptime=0' "$tx/state.json" > "$fixture/expired"
mv "$fixture/expired" "$tx/state.json"
dmb_recovery_watch "$id" 0123456789abcdef0123456789abcdef
wait "$actor" 2>/dev/null || :
[[ $(cat "$fixture/resource") == old ]]
[[ $(jq -r .state "$tx/state.json") == ROLLED_BACK ]]
rm "$fixture/block"
printf 'PASS: monotonic expiration cancels an in-flight mutator before inverse\n'

# Pin verification is mandatory even when the local script still parses.
id=$(dmb_tx_prepare "$plan")
tx=$DMB_STATE_DIR/transactions/$id
dmb_recovery_arm "$id" "$tx"
printf '\n# tampered\n' >> "$tx/worker.sh"
if dmb_recovery_valid_record "$id" "$tx"; then exit 1; fi
printf 'PASS: tampered pinned worker is refused\n'

# An active timer alone is never accepted. Readiness needs nonce/hash/boot/PID
# identity and the expected worker cmdline; this stale record must fail.
jq -cn --arg id "$id" --arg boot "$(dmb_recovery_boot_id)" --argjson pid "$$" '{id:$id,nonce:"wrong",worker_sha256:"wrong",boot_id:$boot,pid:$pid}' > "$tx/ready.json"
cp -- "$worker" "$tx/worker.sh"
# Re-source only the supervisor so the actual readiness implementation is used.
# shellcheck source=../src/recovery/supervisor.sh
source "$ROOT/src/recovery/supervisor.sh"
if dmb_recovery_ready "$id" "$tx"; then exit 1; fi
printf 'PASS: service presence/stale ready file does not establish armed recovery\n'

# Render both explicit recovery backends without starting host services.
dmb_recovery_render_watch_unit "$id" "$tx" 0123456789abcdef0123456789abcdef > "$fixture/unit"
dmb_recovery_render_timer "$id" 90 > "$fixture/timer"
dmb_recovery_render_openrc_watch "$id" "$tx" 0123456789abcdef0123456789abcdef > "$fixture/openrc"
grep -Fq 'KillMode=control-group' "$fixture/unit"
grep -Fq 'OnActiveSec=90s' "$fixture/timer"
grep -Fq 'supervisor="supervise-daemon"' "$fixture/openrc"
bash -n "$fixture/openrc"
printf 'PASS: systemd monotonic deadline and OpenRC supervise-daemon renderings\n'
printf 'UNVERIFIED: real systemd/OpenRC boot ordering, guest reboot, filesystem I/O faults\n'

# Boot recovery also handles interruption before an independent worker exists.
# No host init command is executed by these fixtures.
dmb_recovery_disarm() { return 0; }
jq '.resources=["prepared-reboot"]|.steps[0].resource="prepared-reboot"' "$plan" > "$fixture/prepared-plan.json"
id=$(dmb_tx_prepare "$fixture/prepared-plan.json")
[[ ! -e $DMB_STATE_DIR/transactions/$id/worker.sh ]]
# Simulate constructor SIGKILL before PREPARED becomes durable.
orphan="tx-11111111111111111111111111111111"
orphan_tx=$DMB_STATE_DIR/transactions/$orphan
mkdir -p "$orphan_tx/journal"
jq -cS '.resources=["constructor-reboot"]|.steps[0].resource="constructor-reboot"' "$plan" > "$orphan_tx/plan.json"
jq -cn --arg id "$orphan" '{id:$id,pid:99999999,start_ticks:"1",boot_id:"previous-boot"}' > "$orphan_tx/preparation.json"
hash=$(printf constructor-reboot | dmb_tx_hash)
jq -cn --arg id "$orphan" '{transaction_id:$id,resource:"constructor-reboot"}' > "$DMB_STATE_DIR/reservations/$hash.json"
dmb_recovery_boot
[[ $(jq -r .state "$DMB_STATE_DIR/transactions/$id/state.json") == ROLLED_BACK ]]
[[ ! -e $DMB_STATE_DIR/reservations/$hash.json ]]
printf 'PASS: boot cleans constructor interruption and PREPARED without worker\n'

jq '.resources=["corrupt-armed"]|.steps[0].resource="corrupt-armed"' "$plan" > "$fixture/corrupt-plan.json"
id=$(dmb_tx_prepare "$fixture/corrupt-plan.json")
dmb_tx_state_write "$DMB_STATE_DIR/transactions/$id" ARMED
if dmb_recovery_boot; then exit 1; fi
[[ $(jq -r .state "$DMB_STATE_DIR/transactions/$id/state.json") == ARMED ]]
printf 'PASS: ARMED with missing pinned recovery worker fails closed at boot\n'

# Prefixes do not confer ownership of existing init/scanner files. Exercise the
# durable publication registry using real private files, never host init paths.
owned=$fixture/owned-recovery-unit
printf foreign > "$owned"
if dmb_recovery_owned_write "$owned" 600 <<< generated; then exit 1; fi
[[ $(cat "$owned") == foreign ]]
rm "$owned"
printf version1 | dmb_recovery_owned_write "$owned" 600
[[ $(cat "$owned") == version1 ]]
version1=$(sha256sum "$owned");version1=${version1%% *}
version2=$(printf version2 | sha256sum);version2=${version2%% *}
# Crash before publishing the replacement: current before-version is retained.
jq --arg path "$owned" --arg before "$version1" --arg after "$version2" '.files[$path]={phase:"INTENT",before:$before,after:$after}' "$DMB_STATE_DIR/recovery-files.json" > "$fixture/registry-next"
mv "$fixture/registry-next" "$DMB_STATE_DIR/recovery-files.json"
printf version2 | dmb_recovery_owned_write "$owned" 600 "$version2"
[[ $(cat "$owned") == version2 ]]
# Crash after publication but before DONE: the intended after-version is owned.
jq --arg path "$owned" '.files[$path].phase="INTENT"' "$DMB_STATE_DIR/recovery-files.json" > "$fixture/registry-next"
mv "$fixture/registry-next" "$DMB_STATE_DIR/recovery-files.json"
printf version3 | dmb_recovery_owned_write "$owned" 600
printf administrator > "$owned"
if dmb_recovery_owned_write "$owned" 600 <<< replacement;then exit 1;fi
if dmb_recovery_owned_remove "$owned";then exit 1;fi
[[ $(cat "$owned") == administrator ]]
printf version3 > "$owned"
dmb_recovery_owned_remove "$owned"
dmb_recovery_owned_remove "$owned"
[[ ! -e $owned ]]
printf first | dmb_recovery_owned_write "$fixture/parallel-first" 600 &
first_writer=$!
printf second | dmb_recovery_owned_write "$fixture/parallel-second" 600 &
second_writer=$!
wait "$first_writer"; wait "$second_writer"
[[ $(cat "$fixture/parallel-first") == first && $(cat "$fixture/parallel-second") == second ]]
jq -e --arg first "$fixture/parallel-first" --arg second "$fixture/parallel-second" '.files[$first].phase=="DONE" and .files[$second].phase=="DONE"' "$DMB_STATE_DIR/recovery-files.json" >/dev/null
printf 'PASS: recovery-file ownership, interrupted publication, foreign edits and idempotent cleanup\n'
printf 'PASS: simultaneous recovery publications retain both ownership records\n'
