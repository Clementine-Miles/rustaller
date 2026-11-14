#!/usr/bin/env bash
# This validates test isolation and Linux facilities, not DMITBox recovery.
set -euo pipefail
for tool in unshare ip jq; do
  command -v "$tool" >/dev/null || { printf 'SKIP: missing %s\n' "$tool"; exit 77; }
done
parent_net=$(readlink /proc/self/ns/net)
if ! unshare --user --map-root-user --net true 2>/dev/null; then
  printf 'SKIP: user/network namespace creation unavailable\n'; exit 77
fi
unshare --user --map-root-user --net bash -s -- "$parent_net" <<'ISOLATED'
set -euo pipefail
[[ $(readlink /proc/self/ns/net) != "$1" ]] || { printf 'Isolation failed\n' >&2; exit 1; }
[[ $(id -u) == 0 ]] || exit 1
# This mutation occurs only after checking a freshly created network namespace.
ip link add dmb0 type veth peer name dmb1
ip link set lo up
ip link set dmb0 up
ip link set dmb1 up
ip address add 192.0.2.10/24 dev dmb0
ip address add 198.51.100.10/24 dev dmb1
ip address add 2001:db8:1::10/64 dev dmb0 nodad
ip address add 2001:db8:2::10/64 dev dmb1 nodad
ip route add default via 192.0.2.1 dev dmb0 metric 100
ip route add default via 198.51.100.1 dev dmb1 metric 200
ip route add table 100 default via 198.51.100.1 dev dmb1
ip rule add priority 1000 from 198.51.100.10/32 table 100
ip -6 route add default via 2001:db8:1::1 dev dmb0 metric 100
ip -6 route add table 100 default via 2001:db8:2::1 dev dmb1
ip -6 rule add priority 1000 from 2001:db8:2::10/128 table 100
ip -j route get 203.0.113.1 from 198.51.100.10 | jq -e '.[0].dev=="dmb1" and (.[0].table|tostring)=="100"' >/dev/null
ip -6 -j route get 2001:db8:ffff::1 from 2001:db8:2::10 | jq -e '.[0].dev=="dmb1" and (.[0].table|tostring)=="100"' >/dev/null
ip link set dmb0 mtu 1400
ip -j link show dev dmb0 | jq -e '.[0].mtu==1400' >/dev/null
ip -j route show table all
ip -6 -j route show table all
printf 'PASS: isolated dual-stack, multiple interfaces, source-routing and MTU facilities\n'
printf 'This test does not prove external reachability, DAD, reboot recovery, DNS or nftables behavior.\n'
ISOLATED
