#!/usr/bin/env bash
# Actual kernel operations only in a fresh user+network namespace. Ownership
# discovery and filesystem ancestry are separate fixture mocks, explicitly noted.
set -uo pipefail
ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)
for cmd in unshare ip jq bash timeout;do command -v "$cmd" >/dev/null || { printf 'SKIP: missing %s\n' "$cmd";exit 77; };done
unshare --user --map-root-user --net true || { printf 'SKIP: isolated user/network namespaces unavailable\n';exit 77; }
unshare --user --map-root-user --net bash -s -- "$ROOT" <<'ISOLATED'
set -euo pipefail
ROOT=$1
source "$ROOT/src/core.sh"
source "$ROOT/src/validation.sh"
source "$ROOT/src/adapters/routing.sh"
source "$ROOT/src/adapters/ipv6.sh"
work=$(mktemp -d);trap 'rm -rf -- "$work"' EXIT
export DMB_STATE_DIR=$work/state
mkdir -p "$DMB_STATE_DIR/enrollments"
dmb_trusted_path() { [[ $1 == "$work/"* || $1 == /proc/sys/net/ipv6/conf/dmbv60/* ]] && [[ ! -L $1 ]]; }
dmb_ipv6_unmanaged() { [[ $1 == dmbv60 ]]; }
ip link add dmbv60 type veth peer name dmbv61
for dev in dmbv60 dmbv61;do
  printf '0\n' > "/proc/sys/net/ipv6/conf/$dev/autoconf"
  printf '0\n' > "/proc/sys/net/ipv6/conf/$dev/accept_ra"
  printf '1\n' > "/proc/sys/net/ipv6/conf/$dev/addr_gen_mode"
  ip link set "$dev" up
done
ip -6 address add fe80::10/64 dev dmbv60
ip -6 address add 2001:db8::10/64 dev dmbv60
ip -6 address add 2001:db8::11/64 dev dmbv60
ip -6 address add fe80::11/64 dev dmbv61
ip -6 route add default via fe80::11 dev dmbv60 table 100 proto static metric 42
ip -6 route add 2001:db8:2::/64 via fe80::11 dev dmbv60 table 200 proto static metric 70
for ((attempt=0;attempt<50;attempt++));do
  if ip -j -6 address show dev dmbv60 | jq -e 'all(.[].addr_info[]?;.tentative!=true and .dadfailed!=true)' >/dev/null;then break;fi
  sleep 0.1
done
ip -j -6 address show dev dmbv60 | jq -e 'all(.[].addr_info[]?;.tentative!=true and .dadfailed!=true)' >/dev/null
dmb_ipv6_inventory_full dmbv60 | jq -cS . > "$work/before"
dmb_ipv6_enroll dmbv60 > "$DMB_STATE_DIR/enrollments/ipv6-dmbv60.json"
dmb_ipv6_static_plan disable dmbv60 > "$work/plan"
count=$(jq '.steps|length' "$work/plan")
for ((i=0;i<count;i++));do
  [[ $(dmb_ipv6_static_capture "$work/plan" "$i" | jq -cS .) == "$(jq -cS --argjson i "$i" '.steps[$i].before' "$work/plan")" ]]
  dmb_ipv6_static_apply "$work/plan" "$i" "$work"
  dmb_ipv6_static_verify "$work/plan" "$i" "$work"
done
[[ $(cat /proc/sys/net/ipv6/conf/dmbv60/disable_ipv6) == 1 ]]
ip -j -6 address show dev dmbv60 | jq -e 'all(.[];.addr_info==[])' >/dev/null
# A foreign address on the peer remains intact throughout the transaction.
ip -j -6 address show dev dmbv61 | jq -e 'any(.[].addr_info[]?;.local=="fe80::11")' >/dev/null
for ((i=count-1;i>=0;i--));do
  dmb_ipv6_static_restore "$work/plan" "$i" "$work"
  dmb_ipv6_static_verify_restore "$work/plan" "$i" "$work"
done
dmb_ipv6_inventory_full dmbv60 | jq -cS . > "$work/restored"
cmp "$work/before" "$work/restored"
# An independently executable inverse is idempotent after full restoration.
for ((i=count-1;i>=0;i--));do dmb_ipv6_static_restore "$work/plan" "$i" "$work";done
# Inject interruption after every completed native primitive. Resume only the
# touched inverse operations, then prove the whole original per-link inventory.
for ((cut=1;cut<=count;cut++));do
  for ((i=0;i<cut;i++));do dmb_ipv6_static_apply "$work/plan" "$i" "$work";done
  for ((i=cut-1;i>=0;i--));do dmb_ipv6_static_restore "$work/plan" "$i" "$work";done
  dmb_ipv6_inventory_full dmbv60 | jq -cS . > "$work/resumed"
  cmp "$work/before" "$work/resumed"
done
printf 'PASS: native static IPv6 granular disable/inverse, DAD, address order, multiple routing tables, peer preservation, idempotent inverse, interruption after every primitive\n' 
printf 'MOCKED: manager ownership discovery and fixture filesystem ancestry; UNVERIFIED: reboot identity and external IPv6/SSH reachability\n'
ISOLATED
