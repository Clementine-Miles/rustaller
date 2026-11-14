#!/usr/bin/env bash
# Native NAT66 batch/readback only, confined to a new user and network namespace.
# No external verifier or delegated prefix is simulated as real Internet proof.
set -euo pipefail
ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)
if [[ ${1-} != --inside ]]; then
  exec {nat66_origin_fd}</proc/self/ns/net
  exec unshare --user --map-root-user --net bash "$0" --inside "$nat66_origin_fd"
fi
[[ ${2-} =~ ^[0-9]+$ && $EUID == 0 ]] || exit 77
[[ $(readlink /proc/self/ns/net) != "$(readlink "/proc/self/fd/$2")" ]] || exit 77
native=$(command -v nft || :)
[[ -n $native ]] || native=$ROOT/.cache/vm-validation/tools/usr/sbin/nft
[[ -x $native ]] || { printf 'SKIP native nft unavailable\n'; exit 77; }
export PATH=${native%/*}:$PATH
export LD_LIBRARY_PATH=$ROOT/.cache/vm-validation/tools/usr/lib/x86_64-linux-gnu${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}
# shellcheck source=../../src/core.sh
source "$ROOT/src/core.sh"
# shellcheck source=../../src/adapters/nft.sh
source "$ROOT/src/adapters/nft.sh"
# shellcheck source=../../src/adapters/nat66.sh
source "$ROOT/src/adapters/nat66.sh"
WORK=$(mktemp -d); trap 'rm -rf -- "$WORK"' EXIT
"$native" -j list ruleset >/dev/null || { printf 'SKIP namespace NET_ADMIN unavailable\n'; exit 77; }
"$native" 'add table ip unrelated_nat'
"$native" 'add chain ip unrelated_nat postrouting { type nat hook postrouting priority 100; policy accept; }'
foreign=$("$native" -j list ruleset | dmb_nft_semantic)
pool='{"owner_id":"0123456789abcdef0123456789abcdef","iface":"fixture0","anchor":"2001:470:ff00::1","prefix":"2001:470:ff00::/64"}'
members='["2001:470:ff00::10","2001:470:ff00::11"]'
expected=$(dmb_nat66_state "$pool" "$members")
jq '{nftables:[.nftables[]|{add:.}]}' <<< "$expected" > "$WORK/random.json"
# Verify native aliases independently with fixed SNAT; this does not exercise
# the random-map expression or establish any translated external connection.
fixed=$(jq -cS '.nftables[2].rule.expr[-1].snat.addr="2001:470:ff00::10"' <<< "$expected")
jq '{nftables:[.nftables[]|{add:.}]}' <<< "$fixed" > "$WORK/fixed.json"
"$native" -j -c -f "$WORK/fixed.json"
"$native" -j -f "$WORK/fixed.json"
actual=$(dmb_nat66_from_ruleset "$pool" "$("$native" -j list ruleset)")
[[ $actual == "$fixed" ]]
"$native" delete table ip6 dmitbox_nat66
[[ $("$native" -j list ruleset | dmb_nft_semantic) == "$foreign" ]]
printf 'PASS native IPv6 SNAT typed batch/readback, original-direction/new-flow selector and unrelated IPv4 NAT preservation\n'
if ! "$native" -j -c -f "$WORK/random.json" > "$WORK/random-check.log" 2>&1; then
  [[ $("$native" -j list ruleset | dmb_nft_semantic) == "$foreign" ]]
  cat "$WORK/random-check.log"
  printf 'SKIP random NAT66 kernel expression unavailable; fixed-SNAT evidence does not qualify the random-map adapter\n'
  exit 77
fi
"$native" -j -f "$WORK/random.json"
actual=$(dmb_nat66_from_ruleset "$pool" "$("$native" -j list ruleset)")
[[ $actual == "$expected" ]]
"$native" delete table ip6 dmitbox_nat66
[[ $("$native" -j list ruleset | dmb_nft_semantic) == "$foreign" ]]
printf 'PASS native first-packet random SNAT map syntax/readback/inverse; UNVERIFIED external NAT connectivity and conntrack drain\n'
