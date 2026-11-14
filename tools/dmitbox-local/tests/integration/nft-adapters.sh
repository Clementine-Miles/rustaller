#!/usr/bin/env bash
# Native kernel nftables tests. This wrapper always enters a fresh user/net
# namespace; --inside alone cannot enable mutation in the caller's namespace.
set -Eeuo pipefail
project=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
if [[ ${1-} != --inside ]]; then
  for cmd in unshare ip jq timeout readlink; do
    command -v "$cmd" >/dev/null || { printf 'SKIP: native dependency %s unavailable\n' "$cmd"; exit 77; }
  done
  unshare --user --map-root-user --net true >/dev/null 2>&1 || { printf 'SKIP: disposable user/network namespaces unavailable\n'; exit 77; }
  current=$(readlink /proc/self/ns/net)
  exec {origin_namespace_fd}</proc/self/ns/net
  exec unshare --user --map-root-user --net bash "$0" --inside "$current" "$origin_namespace_fd"
fi
current=$(readlink /proc/self/ns/net)
[[ ${3-} =~ ^[0-9]+$ ]] || exit 77
origin=$(readlink "/proc/self/fd/$3") || { printf 'SKIP: inherited original namespace descriptor unavailable\n'; exit 77; }
read -r mapped_start mapped_uid mapped_count < /proc/self/uid_map
[[ $mapped_uid =~ ^[0-9]+$ && $mapped_start == 0 && $mapped_count == 1 && $origin == "${2-}" && $current != "$origin" && $EUID == 0 ]] || { printf 'REFUSED: not inside a fresh disposable user/network namespace\n' >&2; exit 77; }
shift 3
native="$project/.cache/vm-validation/tools/usr/sbin/nft"
if [[ ! -x $native ]]; then native=$(command -v nft) || { printf 'SKIP: native nft missing\n'; exit 77; }; fi
export PATH="${native%/*}:$PATH"
export LD_LIBRARY_PATH="$project/.cache/vm-validation/tools/usr/lib/x86_64-linux-gnu${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
# shellcheck source=../../src/core.sh
source "$project/src/core.sh"
# shellcheck source=../../src/validation.sh
source "$project/src/validation.sh"
# shellcheck source=../../src/adapters/nft.sh
source "$project/src/adapters/nft.sh"
nft() { "$native" "$@"; }
if ! nft -j list ruleset >/dev/null 2>&1; then printf 'SKIP: disposable namespace kernel NET_ADMIN unavailable\n'; exit 77; fi
# This development container hides /sys/module. Override only that visibility
# prerequisite after the explicitly requested isolated native probe succeeded.
# Production has no flag/environment override for its facility check.
dmb_nft_facility_ready() { return 0; }
printf 'MOCKED: sysfs facility visibility; nft queries/batches below use the real isolated kernel\n'
work=$(mktemp -d)
manager_pid=''
cleanup() { if [[ -n $manager_pid ]]; then kill "$manager_pid" 2>/dev/null || :; wait "$manager_pid" 2>/dev/null || :; fi; rm -rf -- "$work"; }
trap cleanup EXIT
active_plan=''
failure() {
  local code=$?
  printf 'Native nft integration failed at line %s (exit %s)\n' "$1" "$code" >&2
  if [[ -n $active_plan && -f $active_plan ]]; then
    jq -S '.steps[0].after' "$active_plan" > "$work/expected-resource.json"
    dmb_nft_capture "$active_plan" 0 | jq -S . > "$work/observed-resource.json" || :
    diff -u "$work/expected-resource.json" "$work/observed-resource.json" >&2 || :
  fi
  return "$code"
}
trap 'failure "$LINENO"' ERR
DMB_STATE_DIR="$work/state"
export DMB_RUNTIME_DIR="$work/run"
mkdir -p "$DMB_STATE_DIR/enrollments" "$work/tx/snapshots"
# Fixture trust is limited to the test's private directory; production trust is
# not changed and no test/bypass flag exists in the distributed program.
dmb_trusted_path() { [[ $1 == "$work" || $1 == "$work/"* ]] && [[ ! -L $1 ]]; }
assert_fail() { if "$@" > "$work/rejected.log" 2>&1; then printf 'Expected refusal: %s\n' "$*" >&2; exit 1; fi; }
canon() { dmb_nft_semantic; }
full_state() { nft -j list ruleset | canon; }
foreign_state() { nft -j list ruleset | jq '.nftables|=map(select((.table?.family!="inet" or .table?.name!="dmitbox_local") and (to_entries[0].value.family!="inet" or to_entries[0].value.table!="dmitbox_local")))' | canon; }
write_enrollment() {
  local selector pipeline
  selector=$(jq -ce '.steps[0].nft' "$1")
  pipeline=$(dmb_nft_pipeline_hash "$selector" <(nft -j list ruleset))
  jq -c --arg pipeline "$pipeline" '{schema:1,nft:.steps[0].nft,pipeline_sha256:$pipeline}' "$1" > "$DMB_STATE_DIR/enrollments/nft.json"
}
owner=0123456789abcdef0123456789abcdef

nft -f - <<'NFT'
table inet container_fixture {
 set published { type inet_service; elements = { 1234, 5678 }; }
 chain forwarding { type filter hook forward priority 0; policy accept; counter accept; }
 chain docker_fixture { ip saddr 172.17.0.0/16 counter accept; }
}
table ip administrator_nat {
 chain postrouting { type nat hook postrouting priority srcnat; policy accept; ip saddr 172.17.0.0/16 oifname "eth0" masquerade; }
}
NFT
foreign_before=$(foreign_state)
step=$(dmb_nft_plan_exclusive "$owner" '[{"address":"0.0.0.0","port":2222},{"address":"::","port":2222}]' '[{"action":"allow","protocol":"tcp","port":443},{"action":"deny","protocol":"tcp","port":8080}]')
# Dynamic counters must disappear while statement/rule ordering remains stable.
step=$(jq '.after.nftables[2].rule.expr |= [{counter:{}}]+.' <<< "$step")
jq -cn --argjson step "$step" '{schema:1,adapter:"nft",resources:[$step.resource],steps:[$step+{id:"native-exclusive",before:{present:false,nftables:[]}}]}' > "$work/exclusive.json"
write_enrollment "$work/exclusive.json"
active_plan="$work/exclusive.json"
dmb_nft_snapshot "$work/exclusive.json" 0 "$work/tx"
dmb_nft_apply "$work/exclusive.json" 0 "$work/tx"
dmb_nft_verify "$work/exclusive.json" 0 "$work/tx"
[[ $(foreign_state) == "$foreign_before" ]]
printf 'native nft: exclusive apply and foreign NAT/container/set preservation PASS\n'

ip link set lo up
before=$(dmb_nft_capture "$work/exclusive.json" 0)
# A refused TCP connection still sends local SYN/RST packets through INPUT.
# This requires no optional ping binary and cannot leave this private namespace.
timeout 2 bash -c 'exec 3<>/dev/tcp/127.0.0.1/9' >/dev/null 2>&1 || :
after=$(dmb_nft_capture "$work/exclusive.json" 0)
[[ $before == "$after" ]]
nft -j list chain inet dmitbox_local input | jq -e '[.nftables[]|.rule?.expr[]?|.counter?.packets // empty]|any(.>0)' >/dev/null
printf 'native nft: live counter changes excluded from semantic fingerprint PASS\n'

before=$(full_state)
cat > "$work/bad.json" <<'JSON'
{"nftables":[{"add":{"rule":{"family":"inet","table":"dmitbox_local","chain":"input","expr":[{"accept":null}]}}},{"add":{"chain":{"family":"inet","table":"does_not_exist","name":"invalid_batch"}}}]}
JSON
assert_fail nft -j -c -f "$work/bad.json"
[[ $(full_state) == "$before" ]]
assert_fail nft -j -f "$work/bad.json"
[[ $(full_state) == "$before" ]]
printf 'native nft: malformed check and atomic publication preserve complete state PASS\n'

# An active competing manager must block changes even if it started after plan.
mkfifo "$work/manager.fifo"
bash -c 'printf ufw > /proc/self/comm; exec 3<> "$1"; read -r -t 30 -u 3 unused' _ "$work/manager.fifo" &
manager_pid=$!
for _ in {1..100}; do [[ $(cat "/proc/$manager_pid/comm") == ufw ]] && break; sleep 0.01; done
assert_fail dmb_nft_apply "$work/exclusive.json" 0 "$work/tx"
[[ $(full_state) == "$before" ]]
kill "$manager_pid"; wait "$manager_pid" 2>/dev/null || :; manager_pid=''
printf 'native nft: concurrent firewall manager detection PASS\n'

# Simulate only the boot-id evidence, while exercising real native absence,
# immutable snapshot/authority checks and exclusive-table recreation. This is
# not a guest reboot or a proof of external packet access.
mkdir -p "$work/reset/snapshots"
jq '.steps[0].before=.steps[0].after|.steps[0].after.nftables[1].chain.policy="accept"' "$work/exclusive.json" > "$work/reset-plan.json"
jq -cn --arg boot "$(dmb_nft_boot_id)" '{boot_id:$boot}' > "$work/reset/state.json"
dmb_nft_snapshot "$work/reset-plan.json" 0 "$work/reset"
dmb_nft_apply "$work/reset-plan.json" 0 "$work/reset"
nft 'delete table inet dmitbox_local'
assert_fail dmb_nft_restore "$work/reset-plan.json" 0 "$work/reset"
dmb_nft_boot_id() { printf '11111111-1111-1111-1111-111111111111\n'; }
printf '\n' >> "$work/reset/snapshots/nft-0-authority.json"
assert_fail dmb_nft_restore "$work/reset-plan.json" 0 "$work/reset"
cp "$DMB_STATE_DIR/enrollments/nft.json" "$work/reset/snapshots/nft-0-authority.json"
nft 'add table inet dmitbox_local'
assert_fail dmb_nft_restore "$work/reset-plan.json" 0 "$work/reset"
nft 'delete table inet dmitbox_local'
dmb_nft_owned_reset "$work/reset-plan.json" 0 "$work/reset"
dmb_nft_restore "$work/reset-plan.json" 0 "$work/reset"
dmb_nft_verify_restore "$work/reset-plan.json" 0 "$work/reset"
[[ $(foreign_state) == "$foreign_before" ]]
printf 'native nft: exclusive boot-reset inverse and same-boot/tampered-authority/foreign-name refusals PASS (boot-id mocked)\n'

# Represents failure before SSH reload: the independent firewall inverse must
# remove only its owned table. No SSH command is needed to execute this inverse.
dmb_nft_restore "$work/exclusive.json" 0 "$work/tx"
dmb_nft_verify_restore "$work/exclusive.json" 0 "$work/tx"
dmb_nft_restore "$work/exclusive.json" 0 "$work/tx"
[[ $(foreign_state) == "$foreign_before" ]]
printf 'native nft: independent idempotent pre-SSH-reload inverse PASS\n'

nft -f - <<'NFT'
table inet administrator_filter {
 set published { type inet_service; elements = { 80, 443 }; }
 chain input {
  type filter hook input priority 0; policy accept;
  tcp dport 1000 counter accept comment "foreign-before";
  tcp dport 1001 counter accept comment "foreign-after";
 }
 chain untouched { counter accept; }
}
NFT
scoped_foreign_before=$(full_state)
step=$(dmb_nft_plan_scoped "$owner" administrator_filter input dmb_ssh '[{"address":"0.0.0.0","port":2222}]')
jq -cn --argjson step "$step" '{schema:1,adapter:"nft",resources:[$step.resource],steps:[$step+{id:"native-scoped",before:{present:false,nftables:[]}}]}' > "$work/scoped.json"
write_enrollment "$work/scoped.json"
active_plan="$work/scoped.json"
dmb_nft_apply "$work/scoped.json" 0 "$work/tx"
dmb_nft_verify "$work/scoped.json" 0 "$work/tx"
foreign_order=$(nft -j list chain inet administrator_filter input | jq -c '[.nftables[]|.rule? // empty|select(.comment|startswith("foreign-"))|.comment]')
[[ $foreign_order == '["foreign-before","foreign-after"]' ]]
jump_handle=$(nft -j -a list chain inet administrator_filter input | jq -er --arg comment "dmitbox-local:$owner" '.nftables[]|.rule? // empty|select(.comment==$comment)|.handle')
# Update child policy while retaining the parent jump at its original handle.
jq '.steps[0].before=.steps[0].after|.steps[0].after.nftables |= map(if has("rule") and .rule.chain=="dmb_ssh" then .rule.expr[-2].match.right=2223 else . end)' "$work/scoped.json" > "$work/scoped-update.json"
active_plan="$work/scoped-update.json"
dmb_nft_apply "$work/scoped-update.json" 0 "$work/tx"
dmb_nft_verify "$work/scoped-update.json" 0 "$work/tx"
[[ $(nft -j -a list chain inet administrator_filter input | jq -er --arg comment "dmitbox-local:$owner" '.nftables[]|.rule? // empty|select(.comment==$comment)|.handle') == "$jump_handle" ]]
[[ $(nft -j list chain inet administrator_filter input | jq -c '[.nftables[]|.rule? // empty|select(.comment|startswith("foreign-"))|.comment]') == "$foreign_order" ]]
dmb_nft_restore "$work/scoped-update.json" 0 "$work/tx"
dmb_nft_restore "$work/scoped.json" 0 "$work/tx"
[[ $(full_state) == "$scoped_foreign_before" ]]
printf 'native nft: same-table scoped child, stable jump position and exact foreign preservation PASS\n'

# The enrolled foreign parent is not ours to rewrite or silently adopt. A later
# administrator edit changes its version even when our child is still absent.
nft 'add rule inet administrator_filter input tcp dport 1002 accept comment "later-administrator"'
before=$(full_state)
assert_fail dmb_nft_capture "$work/scoped.json" 0
assert_fail dmb_nft_apply "$work/scoped.json" 0 "$work/tx"
[[ $(full_state) == "$before" ]]
printf 'native nft: enrolled parent pipeline version conflict refusal PASS\n'

# A second INPUT hook creates competing ownership; refuse before touching it.
nft 'add table inet conflicting_input'
nft 'add chain inet conflicting_input input { type filter hook input priority 10; policy drop; }'
before=$(full_state)
assert_fail dmb_nft_capture "$work/scoped.json" 0
[[ $(full_state) == "$before" ]]
printf 'native nft: foreign INPUT hook conflict refusal PASS\n'
printf 'nft integration: PASS (native kernel nft, fresh user and network namespace)\n'
