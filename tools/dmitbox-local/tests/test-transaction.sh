#!/usr/bin/env bash
# Isolated fixtures only: no host networking/services are changed.
set -euo pipefail
ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
# shellcheck source=../src/core.sh
source "$ROOT/src/core.sh"
# shellcheck source=../src/transaction.sh
source "$ROOT/src/transaction.sh"
fixture=$(mktemp -d)
trap 'rm -rf -- "$fixture"' EXIT
DMB_STATE_DIR=$fixture/state
DMB_LOCK_DIR=$fixture/locks
mkdir -p -- "$DMB_STATE_DIR" "$DMB_LOCK_DIR"
dmb_require_root() { return 0; }
dmb_tx_namespace_check() { return 0; }
# The real namespace guard is exercised separately from the nonroot engine
# fixture; independent init recovery cannot cross a network namespace.
(
  source "$ROOT/src/transaction.sh"
  readlink() { case $1 in /proc/self/ns/net) printf 'net:[2]\n';; /proc/1/ns/net) printf 'net:[1]\n';; *) printf 'same:[1]\n';; esac; }
  if dmb_tx_namespace_check >/dev/null 2>&1; then exit 1; fi
)
dmb_trusted_path() { [[ $1 == "$fixture"/* && ( -e $1 || ${2:-} == allow_missing ) ]]; }
# Fixtures run as the unprivileged test user in an isolated temporary tree.
dmb_atomic_write() { local tmp; tmp=$(mktemp "${1%/*}/.write.XXXXXXXX") || return; cat > "$tmp" && chmod "$2" "$tmp" && mv -T "$tmp" "$1"; }
dmb_recovery_arm() { : > "$2/fixture-armed"; }
dmb_recovery_ready() { [[ -f $2/fixture-armed ]]; }
dmb_recovery_disarm() { return 0; }
dmb_confirmation_prepare() { return 0; }
dmb_confirmation_now_ticks() { printf '1234\n'; }
dmb_confirmation_check() {
  local tx=$1 id fd hash generation
  id=${tx##*/}; generation=$(jq .generation "$tx/state.json")
  exec {fd}>"$DMB_LOCK_DIR/$id.lock"
  flock -n -x "$fd" || { printf 'A lock was held across confirmation input\n' >&2; return 1; }
  exec {fd}>&-
  hash=$(printf fixture | dmb_tx_hash)
  exec {fd}>"$DMB_LOCK_DIR/resource-$hash.lock"
  flock -n -x "$fd" || return 1
  exec {fd}>&-
  jq -cn --arg id "$id" --argjson generation "$generation" --arg transport "$generation" '{transaction_id:$id,generation:$generation,transport_id:$transport}'
}
dmb_fs_capture() { [[ ! -f $fixture/fail-capture ]] || return 1; jq -cn --arg value "$(cat "$fixture/resource")" '{value:$value}'; }
dmb_fs_snapshot() { return 0; }
dmb_fs_apply() {
  jq -r --argjson i "$2" '.steps[$i].after.value' "$1" > "$fixture/resource"
  [[ ! -f $fixture/fail-apply ]]
}
dmb_fs_verify() { [[ ! -f $fixture/fail-verify ]]; }
dmb_fs_restore() {
  [[ ! -f $fixture/fail-restore ]] || return 1
  jq -r --argjson i "$2" '.steps[$i].before.value' "$1" > "$fixture/resource"
}
dmb_fs_verify_restore() { [[ ! -f $fixture/fail-restore-verify ]]; }
# Unit mocks isolate the state engine; independent artifact spawning is tested
# separately by test-recovery.sh. There is no production fixture switch.
dmb_tx_adapter_run() { dmb_tx_adapter_dispatch "$1" "${2##*/}" "$3"; }
plan=$fixture/plan.json
make_plan() {
  printf old > "$fixture/resource"
  jq -cn '{schema:1,adapter:"fs",resources:["fixture"],steps:[{id:"first",resource:"fixture",after:{value:"new"}}]}' > "$plan"
}
state() { jq -r .state "$DMB_STATE_DIR/transactions/$1/state.json"; }
expect() { [[ $1 == "$2" ]] || { printf 'Expected %s; got %s\n' "$2" "$1" >&2; exit 1; }; }
make_plan
id=$(dmb_tx_prepare "$plan")
expect "$(state "$id")" PREPARED
# Atomic reservation prevents a second administrator planning the same resource.
set +e
(dmb_tx_prepare "$plan") > "$fixture/second.out" 2> "$fixture/second.err"
rc=$?
set -e
expect "$rc" 75
expect "$(find "$DMB_STATE_DIR/reservations" -type f | wc -l)" 1
dmb_tx_apply "$id" > /dev/null
expect "$(state "$id")" VERIFIED
jq -e '.confirmation_min_start_ticks|type=="number"' "$DMB_STATE_DIR/transactions/$id/state.json" > /dev/null
expect "$(cat "$fixture/resource")" new
dmb_tx_rollback "$id"
expect "$(state "$id")" ROLLED_BACK
expect "$(cat "$fixture/resource")" old
dmb_tx_rollback "$id"
expect "$(find "$DMB_STATE_DIR/reservations" -type f | wc -l)" 0
printf 'PASS: reservation exclusion; apply/rollback; idempotent rollback\n'

# Failure after the resource mutation but before DONE still restores INTENT.
make_plan
id=$(dmb_tx_prepare "$plan")
touch "$fixture/fail-apply"
if dmb_tx_apply "$id" > /dev/null; then exit 1; fi
rm "$fixture/fail-apply"
expect "$(cat "$fixture/resource")" old
expect "$(state "$id")" ROLLED_BACK
printf 'PASS: failure after write uses durable INTENT inverse\n'

make_plan
id=$(dmb_tx_prepare "$plan")
dmb_tx_apply "$id" > /dev/null
printf foreign > "$fixture/resource"
if dmb_tx_rollback "$id"; then exit 1; else expect "$?" 76; fi
expect "$(cat "$fixture/resource")" foreign
expect "$(state "$id")" CONFLICT
printf new > "$fixture/resource"
dmb_tx_rollback "$id"
expect "$(cat "$fixture/resource")" old
printf 'PASS: conflicting administrator state is preserved; explicit retry works\n'

make_plan
id=$(dmb_tx_prepare "$plan")
dmb_tx_apply "$id" > /dev/null
touch "$fixture/fail-restore"
if dmb_tx_rollback "$id"; then exit 1; fi
expect "$(state "$id")" ROLLBACK_FAILED
expect "$(cat "$fixture/resource")" new
rm "$fixture/fail-restore"
dmb_tx_rollback "$id"
expect "$(state "$id")" ROLLED_BACK
printf 'PASS: failed inverse remains retryable and never reports restored\n'

make_plan
id=$(dmb_tx_prepare "$plan")
dmb_tx_apply "$id" > /dev/null
touch "$fixture/fail-restore-verify"
if dmb_tx_rollback "$id"; then exit 1; fi
expect "$(state "$id")" ROLLBACK_FAILED
rm "$fixture/fail-restore-verify"
dmb_tx_rollback "$id"
expect "$(state "$id")" ROLLED_BACK
printf 'PASS: matching hashes do not bypass failed restore postconditions\n'

dmb_ssh_capture() { dmb_fs_capture "$@"; }
dmb_ssh_plan_shape() { return 0; } # Engine fixture; real SSH policy has its own suite.
dmb_ssh_apply() { dmb_fs_apply "$@"; }
dmb_ssh_verify() { dmb_fs_verify "$@"; }
dmb_ssh_restore() { dmb_fs_restore "$@"; }
# Commit-stage tightening requires a second confirmation and inverses in order.
make_plan
jq '.adapter="ssh" | .steps += [{id:"final",resource:"fixture",phase:"commit",after:{value:"final"}}]' "$plan" > "$fixture/p2"
mv "$fixture/p2" "$plan"
id=$(dmb_tx_prepare "$plan")
dmb_tx_apply "$id" > /dev/null
dmb_tx_confirm "$id" > /dev/null
expect "$(state "$id")" VERIFIED
expect "$(cat "$fixture/resource")" final
expect "$(jq .confirmation_round "$DMB_STATE_DIR/transactions/$id/state.json")" 1
dmb_tx_confirm "$id" > /dev/null
expect "$(state "$id")" COMMITTED
expect "$(find "$DMB_STATE_DIR/reservations" -type f | wc -l)" 0
printf 'PASS: staged policy, second fresh proof, no locks while reading proof\n'

make_plan
id=$(dmb_tx_prepare "$plan")
tx=$DMB_STATE_DIR/transactions/$id
jq '.deadline_uptime=0' "$tx/state.json" > "$fixture/expired"
mv "$fixture/expired" "$tx/state.json"
if dmb_tx_apply "$id" > /dev/null; then exit 1; fi
expect "$(cat "$fixture/resource")" old
dmb_tx_rollback "$id"
printf 'PASS: expired transaction never mutates a resource\n'

# Immutable plan content must match its recorded fingerprint.
make_plan
id=$(dmb_tx_prepare "$plan")
printf '\n' >> "$DMB_STATE_DIR/transactions/$id/plan.json"
if dmb_tx_apply "$id" > /dev/null; then exit 1; fi
expect "$(cat "$fixture/resource")" old
printf 'PASS: modified durable plan is refused\n'
# Restore exact canonical fixture bytes solely to clean up this test reservation.
jq -cS . "$DMB_STATE_DIR/transactions/$id/plan.json" > "$fixture/repaired"
mv "$fixture/repaired" "$DMB_STATE_DIR/transactions/$id/plan.json"
dmb_tx_rollback "$id"

# Actual concurrent preparation: exactly one durable resource reservation wins.
printf old > "$fixture/resource"
jq '.resources=["parallel"] | .steps[0].resource="parallel"' "$plan" > "$fixture/parallel-plan.json"
set +e
dmb_tx_prepare "$fixture/parallel-plan.json" > "$fixture/a.out" 2> "$fixture/a.err" &
a=$!
dmb_tx_prepare "$fixture/parallel-plan.json" > "$fixture/b.out" 2> "$fixture/b.err" &
b=$!
wait "$a"; ar=$?
wait "$b"; br=$?
set -e
[[ $ar:$br == 0:75 || $ar:$br == 75:0 ]]
if [[ $ar == 0 ]]; then id=$(cat "$fixture/a.out"); else id=$(cat "$fixture/b.out"); fi
dmb_tx_rollback "$id"
printf 'PASS: concurrent administrators produce exactly one reservation winner\n'

jq '.resources=["prepare-failure"] | .steps[0].resource="prepare-failure"' "$plan" > "$fixture/fail-plan.json"
touch "$fixture/fail-capture"
if dmb_tx_prepare "$fixture/fail-plan.json" > /dev/null; then exit 1; fi
rm "$fixture/fail-capture"
hash=$(printf prepare-failure | dmb_tx_hash)
[[ ! -f $DMB_STATE_DIR/reservations/$hash.json ]]
printf 'PASS: failed preparation releases only its own reservation metadata\n'

# Caller-chosen labels cannot bypass locks on the same concrete file target.
make_plan
jq --arg path "$fixture/resource" '.resources=["aliasA"] | .steps[0].resource="aliasA" | .steps[0].path=$path' "$plan" > "$fixture/alias-a.json"
jq '.resources=["aliasB"] | .steps[0].resource="aliasB"' "$fixture/alias-a.json" > "$fixture/alias-b.json"
id=$(dmb_tx_prepare "$fixture/alias-a.json")
if dmb_tx_prepare "$fixture/alias-b.json" > /dev/null 2>&1; then exit 1; else expect "$?" 75; fi
dmb_tx_rollback "$id"
printf 'PASS: canonical target ownership defeats alternate resource labels\n'

make_plan
jq '.adapter="ssh" | .steps[0].restore_rank=10 | .steps += [{id:"final",resource:"fixture",phase:"commit",restore_rank:10,after:{value:"final"}}]' "$plan" > "$fixture/staged.json"
id=$(dmb_tx_prepare "$fixture/staged.json")
dmb_tx_apply "$id" > /dev/null
dmb_tx_confirm "$id" > /dev/null
dmb_tx_rollback "$id"
expect "$(cat "$fixture/resource")" old
expect "$(state "$id")" ROLLED_BACK
printf 'PASS: equal restore priorities preserve reverse stage order\n'

dmb_ssh_verify_restore() { [[ ! -f $fixture/runtime-stale ]]; }
dmb_ssh_restore() { dmb_fs_restore "$@" || return; rm -f "$fixture/runtime-stale"; }
make_plan
jq '.adapter="ssh"' "$plan" > "$fixture/reconcile.json"
id=$(dmb_tx_prepare "$fixture/reconcile.json")
dmb_tx_apply "$id" > /dev/null
# Simulate a crash after restoring the file but before reloading the service.
printf old > "$fixture/resource"
touch "$fixture/runtime-stale"
dmb_tx_rollback "$id"
expect "$(state "$id")" ROLLED_BACK
[[ ! -f $fixture/runtime-stale ]]
printf 'PASS: inverse retry reconciles runtime even when file already matches before\n'

make_plan
jq '.resources=["expected-version"] | .steps[0].resource="expected-version" | .steps[0].before={value:"outdated"}' "$plan" > "$fixture/expected.json"
if dmb_tx_prepare "$fixture/expected.json" > /dev/null 2>&1; then exit 1; else expect "$?" 76; fi
expect "$(cat "$fixture/resource")" old
hash=$(printf expected-version | dmb_tx_hash)
[[ ! -f $DMB_STATE_DIR/reservations/$hash.json ]]
printf 'PASS: input expected versions are validated rather than silently rebased\n'

# A reset predicate is adapter-specific authority, never a general third-state
# override. This fixture models a proven kernel reset, not foreign edits.
dmb_ssh_owned_reset() { [[ -f $fixture/proven-reset && $(cat "$fixture/resource") == kernel-reset ]]; }
make_plan
jq '.adapter="ssh"' "$plan" > "$fixture/reset.json"
id=$(dmb_tx_prepare "$fixture/reset.json")
dmb_tx_apply "$id" > /dev/null
printf foreign > "$fixture/resource"
touch "$fixture/proven-reset"
if dmb_tx_rollback "$id";then exit 1;else expect "$?" 76;fi
expect "$(cat "$fixture/resource")" foreign
printf kernel-reset > "$fixture/resource"
dmb_tx_rollback "$id"
expect "$(cat "$fixture/resource")" old
expect "$(state "$id")" ROLLED_BACK
rm "$fixture/proven-reset"
printf 'PASS: only a successful pinned adapter predicate authorizes a known kernel reset\n'

# Lock acquisition must reject objects that block before flock or truncate a
# symlink target. These private fixture objects never touch host lock paths.
mkfifo "$fixture/lock-fifo"
if dmb_tx_lock_path "$fixture/lock-fifo";then exit 1;fi
ln -s "$fixture/resource" "$fixture/lock-symlink"
if dmb_tx_lock_path "$fixture/lock-symlink";then exit 1;fi
rm "$DMB_LOCK_DIR/global.lock"
ln -s "$fixture/resource" "$DMB_LOCK_DIR/global.lock"
if dmb_tx_prepare "$plan" >/dev/null 2>&1;then exit 1;fi
expect "$(cat "$fixture/resource")" old
printf 'PASS: symlink and FIFO lock paths are refused before redirection\n'
