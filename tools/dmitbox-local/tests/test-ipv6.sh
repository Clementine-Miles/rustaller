#!/usr/bin/env bash
set -uo pipefail
ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
source "$ROOT/src/core.sh"
source "$ROOT/src/validation.sh"
source "$ROOT/src/adapters/routing.sh"
source "$ROOT/src/adapters/ipv6.sh"
work=$(mktemp -d);export DMB_STATE_DIR=$work/state;trap 'rm -rf -- "$work"' EXIT
fail() { printf 'FAIL ipv6: %s\n' "$*" >&2;exit 1; }
reject() { if "$@" >/dev/null 2>&1;then fail "accepted: $*";fi; }
reject dmb_ipv6_plan disable '{}'
reject dmb_ipv6_plan disable '{"iface":"eth0","force":true}'
# Mock read-only inventory; mutation functions are never called in the host.
dmb_ipv6_unmanaged() { dmb_route_iface_valid "$1"; }
printf '{"link":{"ifindex":2,"ifname":"test0","address":"00:11:22:33:44:55"},"addresses":[],"routes":[],"sysctl":{"disable_ipv6":0,"autoconf":0,"accept_ra":0,"addr_gen_mode":1}}\n' > "$work/state"
dmb_ipv6_inventory() { cat "$work/state"; }
dmb_ipv6_plan disable '{"iface":"test0"}' > "$work/plan" || fail empty-static-plan
jq -e '.steps[0].before.sysctl.disable_ipv6==0 and .steps[0].after.sysctl.disable_ipv6==1' "$work/plan" >/dev/null || fail desired
for change in '.addresses=[{"local":"2001:db8::1","prefixlen":64}]' '.routes=[{"dst":"default","gateway":"fe80::1"}]' '.sysctl.autoconf=1' '.sysctl.accept_ra=1' '.sysctl.addr_gen_mode=0';do
  jq "$change" "$work/state" > "$work/changed";mv "$work/state" "$work/original";mv "$work/changed" "$work/state"
  reject dmb_ipv6_plan disable '{"iface":"test0"}'
  mv "$work/original" "$work/state"
done
# Claimed owner does not override failed real ownership discovery.
dmb_ipv6_unmanaged() { return 2; }
reject dmb_ipv6_plan disable '{"iface":"test0"}'
printf 'IPv6 explicit selection and destructive-state fail-closed planning tests passed; no addressed-link recovery is claimed.\n'
# Granular static plans retain each resource separately, including every table.
dmb_ipv6_unmanaged() { return 0; }
dmb_ipv6_enrollment() { return 0; }
cat > "$work/static" <<'JSON'
{"link":{"ifindex":2,"ifname":"test0","address":"00:11:22:33:44:55","kind":"veth","master":null},"addresses":[{"family":"inet6","local":"2001:db8::10","prefixlen":64,"scope":"global","valid_life_time":4294967295,"preferred_life_time":4294967295},{"family":"inet6","local":"2001:db8::11","prefixlen":64,"scope":"global","valid_life_time":4294967295,"preferred_life_time":4294967295}],"routes":[{"dst":"2001:db8::/64","dev":"test0","protocol":"kernel","metric":256,"flags":[],"pref":"medium"},{"dst":"default","dev":"test0","table":100,"gateway":"fe80::1","protocol":"static","metric":42,"flags":[],"pref":"medium"}],"sysctl":{"disable_ipv6":0,"autoconf":0,"accept_ra":0,"addr_gen_mode":0}}
JSON
dmb_ipv6_inventory_full() { cat "$work/static"; }
dmb_ipv6_static_plan disable test0 > "$work/static-plan" || fail static-plan
jq -e '.steps|length==5 and .[0].id=="ipv6-autogen" and .[1].kind=="route" and .[1].selector.table==100 and .[2].selector.local=="2001:db8::10" and .[3].selector.local=="2001:db8::11" and .[4].id=="ipv6-disable"' "$work/static-plan" >/dev/null || fail granular-order
jq -e '([.steps[].resource]|unique|length)==5 and (.resources|length)==7' "$work/static-plan" >/dev/null || fail independent-resource-versions
for change in '.addresses[0].valid_life_time=600' '.addresses[0].temporary=true' '.routes[1].multipath=[{}]' '.routes[1].protocol="ra"' '.link.master="vrf-blue"';do
  jq "$change" "$work/static" > "$work/changed"
  reject dmb_ipv6_static_validate "$(cat "$work/changed")"
done
printf 'Static IPv6 plan decomposition, address restoration order, multiple tables and unsupported-state rejection passed (mocks).\n'
