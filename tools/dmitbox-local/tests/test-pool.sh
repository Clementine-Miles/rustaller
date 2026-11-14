#!/usr/bin/env bash
# Mock-driven pool tests; verifier responses here are fixtures, not external proof.
set -euo pipefail
ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
source "$ROOT/src/core.sh"
source "$ROOT/src/validation.sh"
source "$ROOT/src/adapters/routing.sh"
source "$ROOT/src/adapters/ipv6.sh"
source "$ROOT/src/adapters/pool.sh"
umask 077
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
DMB_STATE_DIR=$work/state
mkdir -p "$DMB_STATE_DIR/enrollments"
tx=$work/tx-11111111111111111111111111111111
mkdir -p "$tx/snapshots"
state=$work/addresses.json
status=$work/verifier-status
printf 'reachable\n' > "$status"
printf '[{"family":"inet6","local":"2001:4860:abcd:1234::1","prefixlen":64,"scope":"global","protocol":"static","valid_life_time":4294967295,"preferred_life_time":4294967295}]\n' > "$state"
fixture_link='{"ifindex":42,"ifname":"pool0","address":"02:00:00:00:00:01","linkinfo":{"info_kind":"veth"}}'
fail_add=none
mode=preferred
# Fixture-only trust and native interfaces. No production flag selects them.
dmb_trusted_path() { [[ $1 == "$work"/* && ! -L $1 ]]; }
dmb_ipv6_unmanaged() { [[ $1 == pool0 ]]; }
dmb_route_link() { [[ $1 == pool0 ]] && printf '%s\n' "$fixture_link"; }
dmb_atomic_write() { local path=$1 filemode=$2; cat > "$path"; chmod "$filemode" "$path"; }
sleep() { :; }
dmb_run() {
  local seconds=$1 cmd=$2; shift 2
  [[ $seconds =~ ^[0-9]+$ ]] || return 1
  if [[ $cmd != ip ]]; then "$cmd" "$@"; return; fi
  case "$*" in
    '-j -6 address show dev pool0'|'-j -6 address show'|'-N -j -6 address show dev pool0') jq -cn --argjson data "$(cat "$state")" '[{ifindex:42,ifname:"pool0",addr_info:$data}]';;
    '-j -6 route show table all dev pool0') printf '[{"dst":"2001:4860:abcd:1234::/64","dev":"pool0","protocol":"kernel","metric":256}]\n';;
    '-6 address add '* )
      [[ $fail_add != before ]] || return 70
      local address=${4%/128} tmp
      tmp=$(jq -c --arg address "$address" --arg mode "$mode" '.+[{family:"inet6",local:$address,prefixlen:128,scope:"global",protocol:"186",noprefixroute:true,valid_life_time:4294967295,preferred_life_time:4294967295,tentative:($mode=="tentative"),dadfailed:($mode=="failed")}]' "$state")
      printf '%s\n' "$tmp" > "$state"
      [[ $fail_add != after ]] || return 70;;
    '-6 address del '* )
      local address=${4%/128} tmp
      tmp=$(jq -c --arg address "$address" 'map(select(.local!=$address))' "$state")
      printf '%s\n' "$tmp" > "$state";;
    *) printf 'Unexpected mock native operation: %s\n' "$*" >&2; return 1;;
  esac
}
cat > "$work/verifier" <<'VERIFIER'
#!/bin/bash
set -euo pipefail
request=$(cat)
digest=$(printf '%s\n' "$request" | sha256sum); digest=${digest%% *}
result=$(cat "$1")
jq -cnS --argjson request "$request" --arg hash "$digest" --arg result "$result" \
  '$request|{schema:1,request_sha256:$hash,nonce:.nonce,phase:.phase,verifier_id:.verifier_id,prefix:.prefix,location:"external",result:$result,source_address:.expected_source,observations:[.addresses[]|{address:.,reachable:true,direction:"inbound"}]}'
VERIFIER
chmod 700 "$work/verifier"
hash=$(sha256sum "$work/verifier"); hash=${hash%% *}
config=$(jq -cn --arg path "$work/verifier" --arg hash "$hash" --arg status "$status" '{schema:1,id:"example",prefix_authorized:true,owner_id:"22222222222222222222222222222222",iface:"pool0",prefix:"2001:4860:abcd:1234::/64",anchor:"2001:4860:abcd:1234::1",verifier:{path:$path,sha256:$hash,args:[$status],identity:"fixture-separate-host",source_address:"2606:4700:ffff::1"}}')
dmb_pool_enroll "$config" > "$DMB_STATE_DIR/enrollments/pool-example.json"
planconfig='{"enrollment":"example","count":2,"external_probes":true}'
dmb_pool_plan "$planconfig" > "$work/plan.json"
jq -e '.steps|length==2' "$work/plan.json" > /dev/null
[[ $(jq length "$state") == 1 ]] || { printf 'Planning mutated addresses\n' >&2; exit 1; }
for ((i=0;i<2;i++)); do
  dmb_pool_snapshot "$work/plan.json" "$i" "$tx"
  address=$(jq -r --argjson i "$i" '.steps[$i].address' "$work/plan.json")
  dmb_ip_in_cidr "$address" 2001:4860:abcd:1234::/64
done
# Bypass only native-manager/sysctl fixture prerequisites; exercise the real
# pinned program, request/nonce binding, bounded output and proof checks.
dmb_pool_preflight() {
  local manifest
  manifest=$(cat "$3/snapshots/pool-enrollment-example.json")
  dmb_pool_verifier "$1" "$2" "$3" anchor "$(jq -r .anchor <<< "$manifest")"
}
for ((i=0;i<2;i++)); do dmb_pool_apply "$work/plan.json" "$i" "$tx"; dmb_pool_verify "$work/plan.json" "$i" "$tx"; done
[[ $(jq length "$state") == 3 ]]
for ((i=1;i>=0;i--)); do dmb_pool_restore "$work/plan.json" "$i" "$tx"; dmb_pool_verify_restore "$work/plan.json" "$i" "$tx"; dmb_pool_restore "$work/plan.json" "$i" "$tx"; done
[[ $(jq length "$state") == 1 ]]

# Partial netlink command failure after creation remains identifiable/restorable.
fail_add=after
if dmb_pool_apply "$work/plan.json" 0 "$tx"; then printf 'Accepted partial native failure\n' >&2; exit 1; fi
dmb_pool_restore "$work/plan.json" 0 "$tx"
fail_add=none
for mode in failed tentative; do
  if dmb_pool_apply "$work/plan.json" 0 "$tx"; then printf 'Accepted failed/pending DAD\n' >&2; exit 1; fi
  dmb_pool_restore "$work/plan.json" 0 "$tx"
done
mode=preferred
printf 'blocked\n' > "$status"
if dmb_pool_apply "$work/plan.json" 0 "$tx"; then printf 'Accepted blocked anchor probe\n' >&2; exit 1; fi
[[ $(jq length "$state") == 1 ]]
printf 'reachable\n' > "$status"
dmb_pool_apply "$work/plan.json" 0 "$tx"
printf 'blocked\n' > "$status"
if dmb_pool_verify "$work/plan.json" 0 "$tx"; then printf 'Accepted blocked member probe\n' >&2; exit 1; fi
dmb_pool_restore "$work/plan.json" 0 "$tx"
printf 'reachable\n' > "$status"

# A third administrator state must survive a refused restore.
dmb_pool_apply "$work/plan.json" 0 "$tx"
address=$(jq -r '.steps[0].address' "$work/plan.json")
tmp=$(jq -c --arg address "$address" 'map(if .local==$address then .protocol="187" else . end)' "$state")
printf '%s\n' "$tmp" > "$state"
if dmb_pool_restore "$work/plan.json" 0 "$tx"; then printf 'Restored a foreign ownership tag\n' >&2; exit 1; fi
jq -e --arg address "$address" 'any(.[];.local==$address and .protocol=="187")' "$state" >/dev/null
tmp=$(jq -c --arg address "$address" 'map(if .local==$address then .protocol="186" else . end)' "$state")
printf '%s\n' "$tmp" > "$state"
dmb_pool_restore "$work/plan.json" 0 "$tx"

# Verifier integrity, permission to probe, prefix-family and membership failures.
printf '# changed\n' >> "$work/verifier"
if dmb_pool_verifier "$work/plan.json" 0 "$tx" member "$address"; then printf 'Accepted a changed verifier\n' >&2; exit 1; fi
if dmb_pool_plan '{"enrollment":"example","count":1,"external_probes":false}'; then exit 1; fi
for value in '2001:db8::/64' 'fe80::/64' '2001:4860:abcd:1234::/56' '192.0.2.0/24'; do
  bad=$(jq -c --arg prefix "$value" '.prefix=$prefix' <<< "$config")
  if dmb_pool_enroll "$bad"; then printf 'Accepted invalid pool prefix\n' >&2; exit 1; fi
done
printf 'PASS: pool mock lifecycle, independent idempotent restore, partial native failure, DAD failure/timeout, blocked probes, ownership conflict, verifier integrity and strict prefix validation\n'
printf 'UNVERIFIED: real remote verifier honesty/reachability, native DAD and reboot recovery\n'
