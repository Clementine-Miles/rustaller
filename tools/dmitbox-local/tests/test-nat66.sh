#!/usr/bin/env bash
# Privileged tools are mocked here. No host netfilter/network state is touched.
set -euo pipefail
ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
# shellcheck source=../src/core.sh
source "$ROOT/src/core.sh"
# shellcheck source=../src/validation.sh
source "$ROOT/src/validation.sh"
# shellcheck source=../src/adapters/routing.sh
source "$ROOT/src/adapters/routing.sh"
# shellcheck source=../src/adapters/pool.sh
source "$ROOT/src/adapters/pool.sh"
# shellcheck source=../src/adapters/nft.sh
source "$ROOT/src/adapters/nft.sh"
# shellcheck source=../src/adapters/nat66.sh
source "$ROOT/src/adapters/nat66.sh"
# shellcheck source=../src/transaction.sh
source "$ROOT/src/transaction.sh"
NAT_TEST_WORK=$(mktemp -d); trap 'rm -rf -- "$NAT_TEST_WORK"' EXIT
export DMB_STATE_DIR=$NAT_TEST_WORK/state DMB_RUNTIME_DIR=$NAT_TEST_WORK/run
mkdir -p "$DMB_STATE_DIR/enrollments" "$NAT_TEST_WORK/tx/snapshots"
dmb_trusted_path() { [[ $1 == "$NAT_TEST_WORK" || $1 == "$NAT_TEST_WORK/"* ]] && [[ ! -L $1 ]]; }
dmb_nft_manager_guard() { [[ ! -f $NAT_TEST_WORK/foreign-manager ]]; }
# This command-mock suite also supplies its kernel facility visibility.
dmb_nft_facility_ready() { return 0; }
dmb_nat66_pool_context() { [[ ! -f $NAT_TEST_WORK/context-changed ]]; }
dmb_pool_capture() { [[ ! -f $NAT_TEST_WORK/pool-owner-changed ]] || return 76; jq -cS --argjson i "$2" '.steps[$i].after' "$1"; }
reject() { if "$@" > "$NAT_TEST_WORK/rejected.log" 2>&1; then printf 'Unexpected success: %s\n' "$*" >&2; exit 1; fi; }
foreign='{"nftables":[{"table":{"family":"ip","name":"container_nat"}},{"chain":{"family":"ip","table":"container_nat","name":"postrouting","type":"nat","hook":"postrouting","prio":100,"policy":"accept"}}]}'
printf '%s\n' "$foreign" > "$NAT_TEST_WORK/ruleset"
nft() {
  printf 'nft %s\n' "$*" >> "$NAT_TEST_WORK/commands"
  if [[ $* == '-j list ruleset' ]]; then cat "$NAT_TEST_WORK/ruleset"; return; fi
  if [[ $* == '-j -c -f '* ]]; then [[ ! -f $NAT_TEST_WORK/bad-batch ]]; return; fi
  [[ $* == '-j -f '* ]] || return 64
  local batch=${*: -1} state
  # Validate mutation scope separately from the adapter's expected-state model.
  jq -e 'all(.nftables[]; (has("delete") and .delete.table=={family:"ip6",name:"dmitbox_nat66"}) or (has("create") and .create.table.family=="ip6" and .create.table.name=="dmitbox_nat66") or (has("add") and (.add.chain.table//.add.rule.table)=="dmitbox_nat66"))' "$batch" >/dev/null || return
  state=$(jq -c --argjson foreign "$foreign" '$foreign|.nftables += []' <<< '{}') || return
  state=$(jq -c --argjson foreign "$state" '$foreign|.nftables += [input.nftables[]|.create // .add // empty]' <(printf '{}\n') "$batch") || return
  printf '%s\n' "$state" > "$NAT_TEST_WORK/ruleset"
}
conntrack() {
  [[ ! -f $NAT_TEST_WORK/ct-fail ]] || return 1
  cat "$NAT_TEST_WORK/flows"
  printf 'conntrack v1.4.8 (conntrack-tools): %s flow entries have been shown.\n' "$(awk 'NF{n++} END{print n+0}' "$NAT_TEST_WORK/flows")" >&2
}
dmb_run() { local seconds=$1; shift; case $1 in nft) shift; nft "$@";; conntrack) shift; conntrack "$@";; *) timeout --kill-after=1 "$seconds" "$@";; esac; }
dmb_tx_uptime() { printf '%s\n' "$(cut -d. -f1 /proc/uptime)"; }
: > "$NAT_TEST_WORK/flows"
cat > "$NAT_TEST_WORK/verifier" <<'VERIFY'
#!/bin/bash
set -euo pipefail
request=$(cat)
[[ ! -f $1/fail-verifier ]] || exit 3
digest=$(printf '%s\n' "$request" | sha256sum | cut -d' ' -f1)
jq -cnS --argjson r "$request" --arg hash "$digest" '{schema:1,request_sha256:$hash,nonce:$r.nonce,phase:$r.phase,verifier_id:$r.verifier_id,prefix:$r.prefix,source_address:$r.expected_source,location:"external",result:"reachable",observations:[$r.members[]|{address:.,reachable:true,direction:(if $r.phase=="nat66-outbound" then "outbound" else "inbound" end),observed_source:.}]}'
VERIFY
chmod 700 "$NAT_TEST_WORK/verifier"
verifier_hash=$(sha256sum "$NAT_TEST_WORK/verifier" | cut -d' ' -f1)
pool=$(jq -cn --arg path "$NAT_TEST_WORK/verifier" --arg hash "$verifier_hash" --arg work "$NAT_TEST_WORK" '{schema:1,id:"p1",owner_id:"0123456789abcdef0123456789abcdef",iface:"test0",prefix:"2001:470:ff00::/64",anchor:"2001:470:ff00::1",verifier:{path:$path,sha256:$hash,args:[$work],identity:"fixture-remote",source_address:"2001:470:ee00::1"}}')
printf '%s\n' "$pool" > "$DMB_STATE_DIR/enrollments/pool-p1.json"
config='{"schema":1,"operation":"enable","enrollment":"p1","members":["2001:470:ff00::10","2001:470:ff00::11"],"external_probes":true}'
ownership=$DMB_STATE_DIR/transactions/tx-0123456789abcdef0123456789abcdef
mkdir -p "$ownership"
jq -c '{schema:1,adapter:"pool",resources:["pool:p1"],steps:[.members|to_entries[]|{id:("member-"+(.key|tostring)),resource:"pool:p1",adapter:"pool",enrollment:"p1",address:.value,after:{exists:true,address:.value}}]}' <<< "$config" > "$ownership/plan.json"
printf '{"state":"COMMITTED"}\n' > "$ownership/state.json"
jq -cn --arg hash "$(sha256sum "$ownership/plan.json" | cut -d' ' -f1)" '{sha256:$hash}' > "$ownership/plan-hash.json"
touch "$NAT_TEST_WORK/pool-owner-changed"
reject dmb_nat66_plan "$config"
rm "$NAT_TEST_WORK/pool-owner-changed"
dmb_nat66_plan "$config" > "$NAT_TEST_WORK/enable.json"
[[ ! -e $NAT_TEST_WORK/tx/snapshots/nat66-0-nat66-preflight-proof.json ]]
# New-flow random mapping has exactly N entries, no zero-slot omission or marks.
jq -e '.steps[0].after.nftables[2].rule.expr[-1].snat.addr.map|.key.numgen.mod==2 and .data.set==[[0,"2001:470:ff00::10"],[1,"2001:470:ff00::11"]]' "$NAT_TEST_WORK/enable.json" >/dev/null
[[ $(printf '{"numgen":{"mode":"random","mod":2}}' | dmb_nat66_semantic) == "$(printf '{"numgen":{"mode":"random","mod":2,"offset":0}}' | dmb_nat66_semantic)" ]]
! rg -q '"(mangle|mark)"' "$NAT_TEST_WORK/enable.json"
reject dmb_nat66_plan "$(jq '.external_probes=false' <<< "$config")"
reject dmb_nat66_plan "$(jq '.members=["2001:db8::1"]' <<< "$config")"
reject dmb_nat66_plan "$(jq '.members += [.members[0]]' <<< "$config")"
dmb_nat66_snapshot "$NAT_TEST_WORK/enable.json" 0 "$NAT_TEST_WORK/tx"
touch "$NAT_TEST_WORK/fail-verifier"
reject dmb_nat66_apply "$NAT_TEST_WORK/enable.json" 0 "$NAT_TEST_WORK/tx"
[[ $(cat "$NAT_TEST_WORK/ruleset") == "$foreign" ]]
rm "$NAT_TEST_WORK/fail-verifier"
touch "$NAT_TEST_WORK/context-changed"
reject dmb_nat66_apply "$NAT_TEST_WORK/enable.json" 0 "$NAT_TEST_WORK/tx"
rm "$NAT_TEST_WORK/context-changed"
touch "$NAT_TEST_WORK/bad-batch"
reject dmb_nat66_apply "$NAT_TEST_WORK/enable.json" 0 "$NAT_TEST_WORK/tx"
[[ $(cat "$NAT_TEST_WORK/ruleset") == "$foreign" ]]
rm "$NAT_TEST_WORK/bad-batch"
dmb_nat66_apply "$NAT_TEST_WORK/enable.json" 0 "$NAT_TEST_WORK/tx"
dmb_nat66_verify "$NAT_TEST_WORK/enable.json" 0 "$NAT_TEST_WORK/tx"
reject dmb_nat66_pool_removal_check test0 2001:470:ff00::10
printf 'tcp 6 100 ESTABLISHED src=2001:470:ff00::1 dst=2001:470:ee00::1 sport=50000 dport=443 src=2001:470:ee00::1 dst=2001:470:ff00::10 sport=443 dport=50000 mark=33554432 use=1\n' > "$NAT_TEST_WORK/flows"
[[ $(dmb_nat66_flow_count 2001:470:ff00::1 '["2001:470:ff00::10"]') == 1 ]]
# Independent inverse leaves conntrack translations and pool addresses intact.
touch "$NAT_TEST_WORK/context-changed" "$NAT_TEST_WORK/fail-verifier"
dmb_nat66_restore "$NAT_TEST_WORK/enable.json" 0 "$NAT_TEST_WORK/tx"
dmb_nat66_verify_restore "$NAT_TEST_WORK/enable.json" 0 "$NAT_TEST_WORK/tx"
[[ $(cat "$NAT_TEST_WORK/ruleset") == "$foreign" ]]
reject dmb_nat66_pool_removal_check test0 2001:470:ff00::10
[[ $(dmb_nat66_flow_count 2001:470:ff00::1 '["2001:470:ff00::10"]') == 1 ]]
: > "$NAT_TEST_WORK/flows"
dmb_nat66_pool_removal_check test0 2001:470:ff00::10
dmb_nat66_restore "$NAT_TEST_WORK/enable.json" 0 "$NAT_TEST_WORK/tx"
rm "$NAT_TEST_WORK/context-changed" "$NAT_TEST_WORK/fail-verifier"
dmb_nat66_apply "$NAT_TEST_WORK/enable.json" 0 "$NAT_TEST_WORK/tx"
disable=$(jq '.operation="disable"' <<< "$config")
dmb_nat66_plan "$disable" > "$NAT_TEST_WORK/disable.json"
dmb_nat66_apply "$NAT_TEST_WORK/disable.json" 0 "$NAT_TEST_WORK/tx"
dmb_nat66_verify "$NAT_TEST_WORK/disable.json" 0 "$NAT_TEST_WORK/tx"
dmb_nat66_restore "$NAT_TEST_WORK/disable.json" 0 "$NAT_TEST_WORK/tx"
dmb_nat66_verify_restore "$NAT_TEST_WORK/disable.json" 0 "$NAT_TEST_WORK/tx"
# Third-state configuration is preserved, never clobbered by an inverse.
jq '.nftables += [{rule:{family:"ip6",table:"dmitbox_nat66",chain:"postrouting",expr:[{accept:null}]}}]' "$NAT_TEST_WORK/ruleset" > "$NAT_TEST_WORK/changed"
mv "$NAT_TEST_WORK/changed" "$NAT_TEST_WORK/ruleset"
reject dmb_nat66_restore "$NAT_TEST_WORK/enable.json" 0 "$NAT_TEST_WORK/tx"
touch "$NAT_TEST_WORK/foreign-manager"
reject dmb_nat66_plan "$config"
rm "$NAT_TEST_WORK/foreign-manager"
# Strict tuple parsing, counter-free semantic state, and malformed history.
printf 'tcp src=bad dst=2001:470::1\n' > "$NAT_TEST_WORK/flows"
reject dmb_nat66_flow_count 2001:470:ff00::1 '["2001:470:ff00::10"]'
printf '{}\n' > "$DMB_STATE_DIR/nat66-used/corrupt.json"
reject dmb_nat66_pool_removal_check test0 2001:470:ff00::10
printf 'PASS NAT66 typed map, explicit probes, fail-before-publication, independent inverse, retained flows/address guards, conflict and idempotence (mock tools)\n'
