#!/usr/bin/env bash
set -uo pipefail
ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
source "$ROOT/src/core.sh"
source "$ROOT/src/validation.sh"
source "$ROOT/src/adapters/routing.sh"
work=$(mktemp -d);trap 'rm -rf -- "$work"' EXIT
fail() { printf 'FAIL routing: %s\n' "$*" >&2;exit 1; }
reject() { if "$@" >/dev/null 2>&1;then fail "accepted: $*";fi; }
for ctx in '{"family":5}' '{"source":"127.1"}' '{"table":0}' '{"mark":-1}' '{"foo":1}' '{"vrf":"bad;cmd"}' '{"family":6,"peer":"192.0.2.1"}';do reject dmb_route_context_valid "$ctx";done
dmb_route_context_valid '{"family":6,"source":"2001:db8::1","peer":"2001:db8::2","vrf":"blue","table":100,"mark":20,"encapsulation_overhead":50}' || fail context
# Use a real read-only loopback sysctl and mocked link mutation.
printf '{"ifindex":1,"ifname":"lo","address":"00:00:00:00:00:00","mtu":1500,"max_mtu":9000}\n' > "$work/link"
dmb_route_link() { [[ $1 == lo ]] || return 2;cat "$work/link"; }
dmb_run() {
  local timeout_seconds=$1;shift
  if [[ $1 == ip && $2 == link && $3 == set && $4 == dev && $5 == lo && $6 == mtu ]];then
    jq -c --argjson mtu "$7" '.mtu=$mtu' "$work/link" > "$work/link.next" && mv "$work/link.next" "$work/link"
  else fail "unexpected command $timeout_seconds $*";fi
}
dmb_mtu_plan lo 1400 runtime > "$work/plan" || fail plan
dmb_mtu_apply "$work/plan" 0 "$work" || fail apply
dmb_mtu_verify "$work/plan" 0 || fail verify
dmb_mtu_restore "$work/plan" 0 "$work" || fail restore
dmb_mtu_verify_restore "$work/plan" 0 || fail verify-restore
reject dmb_mtu_plan lo 1100 runtime
jq '.steps[0].after.mtu=68' "$work/plan" > "$work/forged"
reject dmb_mtu_apply "$work/forged" 0 "$work"
[[ $(jq .mtu "$work/link") == 1500 ]] || fail forged-plan-mutated-link
reject dmb_mtu_plan lo 99999 runtime
reject dmb_mtu_plan lo 1400 persistent
reject dmb_mtu_plan 'eth0;id' 1400 runtime
reject dmb_iface_plan lo down runtime '{}'
# Interface identity replacement is observable even with a matching MTU.
jq '.ifindex=99' "$work/link" > "$work/new";mv "$work/new" "$work/link"
reject dmb_mtu_verify_restore "$work/plan" 0
printf 'Routing context validation, MTU apply/inverse, identity-change and unsafe link-state refusals passed.\n'
