#!/usr/bin/env bash
# Native address/DAD primitives. The external verifier is explicitly mocked;
# this cannot qualify upstream routability or the full transaction/recovery path.
set -euo pipefail
ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)
for tool in unshare ip jq timeout; do command -v "$tool" >/dev/null || exit 77; done
unshare --user --map-root-user --net true 2>/dev/null || { printf 'SKIP: isolated namespaces unavailable\n'; exit 77; }
parent=$(readlink /proc/self/ns/net)
umask 077
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
unshare --user --map-root-user --net bash -s -- "$ROOT" "$parent" "$work" <<'ISOLATED'
set -euo pipefail
root=$1; parent=$2; work=$3
[[ $(readlink /proc/self/ns/net) != "$parent" ]] || exit 1
source "$root/src/core.sh"
source "$root/src/validation.sh"
source "$root/src/adapters/routing.sh"
source "$root/src/adapters/ipv6.sh"
source "$root/src/adapters/pool.sh"
DMB_LOCK_FDS=()
ip link add pool0 type veth peer name peer0
ip link set pool0 up
ip link set peer0 up
# Test fixture boundaries only; native ip operations and DAD are not mocked.
dmb_pool_preflight() { return 0; }
dmb_pool_verifier() { printf 'Fixture: external proof was not requested or established\n' >&2; }
make_plan() {
  local address=$1 plan before after
  plan=$(jq -cn --arg address "$address" --arg ns "$(stat -Lc %i /proc/self/ns/net)" --argjson link "$(dmb_route_link pool0 | jq '{ifindex,ifname,address,master:(.master//null),kind:(.linkinfo.info_kind//"ether")}')" '{steps:[{id:"pool-address-0",adapter:"pool",iface:"pool0",prefix:"2001:4860:abcd:1234::/64",address:$address,enrollment:"fixture",enrollment_sha256:("0"*64),external_probes:true,link:$link,netns_inode:$ns,after:{}}]}')
  before=$(dmb_pool_capture <(printf '%s\n' "$plan") 0)
  after=$(jq -cS --arg address "$address" '.+{exists:true,address:$address,prefixlen:128,scope:"global",protocol:"186",noprefixroute:true,permanent:true}' <<< "$before")
  jq -cS --argjson before "$before" --argjson after "$after" '.steps[0].before=$before|.steps[0].after=$after' <<< "$plan" > "$work/plan.json"
}
make_plan 2001:4860:abcd:1234::10
dmb_pool_apply "$work/plan.json" 0 "$work"
dmb_pool_verify "$work/plan.json" 0 "$work"
ip -j -6 addr show dev pool0 | jq -e 'any(.[].addr_info[]?;.local=="2001:4860:abcd:1234::10" and (.tentative//false|not) and (.dadfailed//false|not) and .noprefixroute==true and .protocol=="0xba")' >/dev/null
dmb_pool_restore "$work/plan.json" 0 "$work"
dmb_pool_verify_restore "$work/plan.json" 0 "$work"
dmb_pool_restore "$work/plan.json" 0 "$work"

# DAD receives a real response from the other end of the veth pair.
ip -6 addr add 2001:4860:abcd:1234::11/128 dev peer0 nodad
make_plan 2001:4860:abcd:1234::11
if dmb_pool_apply "$work/plan.json" 0 "$work"; then printf 'FAIL: duplicate address was accepted\n' >&2; exit 1; fi
ip -j -6 addr show dev pool0 | jq -e 'any(.[].addr_info[]?;.local=="2001:4860:abcd:1234::11" and .dadfailed==true)' >/dev/null
dmb_pool_restore "$work/plan.json" 0 "$work"
dmb_pool_verify_restore "$work/plan.json" 0 "$work"
ip -j -6 addr show dev peer0 | jq -e 'any(.[].addr_info[]?;.local=="2001:4860:abcd:1234::11")' >/dev/null
printf 'PASS: native pool /128 creation, preferred DAD, protocol ownership, duplicate-DAD failure, independent idempotent removal and preservation of peer address\n'
printf 'UNVERIFIED: external verifier, upstream /64 routability, enrollment-owner discovery and reboot recovery\n'
ISOLATED
