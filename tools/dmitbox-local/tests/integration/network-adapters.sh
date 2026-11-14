#!/usr/bin/env bash
# Native adapter checks in a disposable namespace, not a recovery-backend test.
set -euo pipefail
ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)
for tool in unshare ip jq timeout; do
  command -v "$tool" >/dev/null || { printf 'SKIP: missing %s\n' "$tool"; exit 77; }
done
[[ -f $ROOT/src/adapters/routing.sh && -f $ROOT/src/core.sh && -f $ROOT/src/validation.sh ]] || {
  printf 'SKIP: required adapter modules are unavailable\n'; exit 77;
}
if ! unshare --user --map-root-user --net true 2>/dev/null; then
  printf 'SKIP: isolated namespaces unavailable\n'; exit 77
fi
parent_net=$(readlink /proc/self/ns/net)
umask 077
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
unshare --user --map-root-user --net bash -s -- "$ROOT" "$parent_net" "$work" <<'ISOLATED'
set -euo pipefail
root=$1; parent=$2; work=$3
[[ $(readlink /proc/self/ns/net) != "$parent" ]] || exit 1
# These source paths are developer-controlled test inputs; production entrypoints
# do not expose a module-source or environment override interface.
source "$root/src/core.sh"
source "$root/src/validation.sh"
source "$root/src/adapters/routing.sh"
DMB_LOCK_FDS=()
ip link add dmb0 type veth peer name dmb1
ip link set lo up
ip link set dmb0 up
ip link set dmb1 up
ip address add 192.0.2.10/24 dev dmb0
ip address add 198.51.100.10/24 dev dmb1
ip address add 2001:db8:1::10/64 dev dmb0 nodad
ip address add 2001:db8:2::10/64 dev dmb1 nodad
ip route add default via 192.0.2.1 dev dmb0
ip route add table 100 default via 198.51.100.1 dev dmb1
ip rule add priority 1000 from 198.51.100.10/32 table 100
ip -6 route add default via 2001:db8:1::1 dev dmb0
ip -6 route add table 100 default via 2001:db8:2::1 dev dmb1
ip -6 rule add priority 1000 from 2001:db8:2::10/128 table 100
dmb_route_report '{}' > "$work/report.json"
jq -e '.network_probes==false and ([.links[].ifname]|index("dmb0")!=null and index("dmb1")!=null) and any(.routes.ipv4[]; (.table|tostring)=="100") and any(.routes.ipv6[]; (.table|tostring)=="100")' "$work/report.json" >/dev/null

# The plan does not mutate the link. Invocation below deliberately tests the
# primitive in isolation; it does not bypass the production transaction CLI.
dmb_mtu_plan dmb0 1400 runtime '{}' > "$work/mtu.json"
ip -j link show dev dmb0 | jq -e '.[0].mtu==1500' >/dev/null
dmb_mtu_apply "$work/mtu.json" 0 "$work"
dmb_mtu_verify "$work/mtu.json" 0 "$work"
ip -j link show dev dmb0 | jq -e '.[0].mtu==1400' >/dev/null
dmb_mtu_restore "$work/mtu.json" 0 "$work"
dmb_mtu_verify_restore "$work/mtu.json" 0 "$work"
ip -j link show dev dmb0 | jq -e '.[0].mtu==1500' >/dev/null
if dmb_mtu_plan dmb0 1279 runtime '{}' > /dev/null 2>&1; then
  printf 'FAIL: accepted MTU below IPv6 minimum\n' >&2; exit 1
fi
if dmb_mtu_plan dmb0 1400 persistent '{}' > /dev/null 2>&1; then
  printf 'FAIL: accepted unsupported persistent owner\n' >&2; exit 1
fi
if dmb_iface_plan dmb0 down runtime '{}' > /dev/null 2>&1; then
  printf 'FAIL: accepted unsafe generic link-down operation\n' >&2; exit 1
fi

# IPv6-only and IPv4-only address inventories remain distinct; no public probe
# or default-route heuristic is used to choose a link for either inventory.
ip -4 address flush dev dmb0
ip -4 address flush dev dmb1
dmb_route_report '{}' | jq -e '[.addresses[]|select(.ifname!="lo")|.addr_info[]|select(.scope=="global")]|length>0 and all(.[];.family=="inet6")' >/dev/null
ip -6 address flush dev dmb0
ip -6 address flush dev dmb1
ip address add 192.0.2.10/24 dev dmb0
dmb_route_report '{}' | jq -e '[.addresses[]|select(.ifname!="lo")|.addr_info[]|select(.scope=="global")]|length>0 and all(.[];.family=="inet")' >/dev/null

ip link delete dmb0
if dmb_mtu_capture "$work/mtu.json" 0 > /dev/null 2>&1; then
  printf 'FAIL: missing interface was captured as existing\n' >&2; exit 1
fi
printf 'PASS: native report, source-routing inventory, MTU apply/readback/restore, IPv6 minimum, missing-interface rejection\n'
printf 'UNVERIFIED: external PMTU, SSH access and independent rollback/reboot behavior\n'
ISOLATED
