#!/usr/bin/env bash
# Pure parsing plus native-command mocks; no host network writes or probes.
set -uo pipefail
ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
source "$ROOT/src/core.sh"
source "$ROOT/src/validation.sh"
source "$ROOT/src/adapters/dns.sh"
work=$(mktemp -d);trap 'rm -rf -- "$work"' EXIT
fail() { printf 'FAIL: %s\n' "$*" >&2;exit 1; }
expect_reject() { if "$@" >/dev/null 2>&1;then fail "accepted unsafe input: $*";fi; }
for bad in '' '1.2.3.999' '01.2.3.4' '1.2.3' '1.1.1.1 8.8.8.8' '1.1.1.1;id' '::1' '::' 'fe80::1%eth0' 'ff02::1';do expect_reject dmb_dns_servers "$bad";done
expect_reject dmb_dns_servers 1.1.1.1 1.1.1.1
expect_reject dmb_dns_iface_valid --version
expect_reject dmb_dns_numeric_list '["--interface=other0"]'
expect_reject dmb_dns_numeric_list '["192.0.2.1","2001:db8::1"]' 4
[[ $(dmb_dns_servers '2001:4860:4860:0:0:0:0:8888') == 2001:4860:4860::8888 ]] || fail canonical
printf '{"dns4":["192.0.2.53"],"dns6":[],"auto4":"no","auto6":"no"}\n' > "$work/nm.json"
printf '["192.0.2.53"]\n' > "$work/resolved.json"
printf 'nameserver 192.0.2.53\nsearch example.test\noptions edns0 trust-ad\n# administrator comment\n' > "$work/resolv.conf"
out=$(dmb_dns_content "$work/resolv.conf" static eth0 '["1.1.1.1"]') || fail content
[[ $out == *'search example.test'* && $out == *'options edns0 trust-ad'* && $out == *'# administrator comment'* && $out != *'nameserver 192.0.2.53'* ]] || fail preserved-content
# Read-only owner mock isolates command semantics from distro detection.
dmb_dns_owner() { printf '{"owner":"nm","identity":"aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"}\n'; }
dmb_run() { shift;"$@"; }
ip() { printf '[{"ifindex":2,"ifname":"eth0","address":"00:11:22:33:44:55"}]\n'; }
nmcli() {
  if [[ $1 == -g ]];then
    case $2 in ipv4.dns) jq -r '.dns4|join(",")' "$work/nm.json";;ipv6.dns) jq -r '.dns6|join(",")' "$work/nm.json";;ipv4.ignore-auto-dns) jq -r .auto4 "$work/nm.json";;ipv6.ignore-auto-dns) jq -r .auto6 "$work/nm.json";;*)return 2;;esac
  elif [[ $1 == connection && $2 == modify ]];then
    jq -cn --arg d4 "$6" --arg d6 "$8" --arg a4 "${10}" --arg a6 "${12}" '{dns4:($d4|split(",")|map(select(length>0))),dns6:($d6|split(",")|map(select(length>0))),auto4:$a4,auto6:$a6}' > "$work/nm.next" && mv "$work/nm.next" "$work/nm.json"
  else fail 'forbidden broad NM reapply';fi
}
resolvectl() {
  case $1 in
    status) return 0;;
    dns) if (($#==2));then printf 'Link 2 (eth0): %s\n' "$(jq -r 'join(" ")' "$work/resolved.json")";else shift 2;printf '%s\n' "$@" | jq -Rsc 'split("\n")|map(select(length>0))' > "$work/resolved.json";fi;;
    domain) printf 'Link 2 (eth0): ~example.test search.example.test\n';;
    default-route) printf 'Link 2 (eth0): yes\n';;
    dnssec) printf 'Link 2 (eth0): yes\n';;
    dnsovertls) printf 'Link 2 (eth0): opportunistic\n';;
    *) fail 'forbidden broad resolved command';;
  esac
}
dmb_dns_plan eth0 custom persistent 1.1.1.1 2606:4700:4700::1111 > "$work/plan.json" || fail plan
jq -e '.steps|length==2' "$work/plan.json" >/dev/null || fail split-steps
jq -e '([.steps[].resource]|unique|length)==2 and (.resources|length)==3' "$work/plan.json" >/dev/null || fail resource-locking
jq -e '.steps[0].adapter=="dns_nm" and .steps[1].adapter=="dns_resolved"' "$work/plan.json" >/dev/null || fail adapter-order
jq '.steps[1].after.dns=["--interface=other0"]' "$work/plan.json" > "$work/forged-runtime"
expect_reject dmb_dns_resolved_apply "$work/forged-runtime" 1 "$work"
jq '.steps[0].after.dns4=["--temporary"]' "$work/plan.json" > "$work/forged-profile"
expect_reject dmb_dns_nm_apply "$work/forged-profile" 0 "$work"
before_runtime=$(cat "$work/resolved.json")
dmb_dns_nm_apply "$work/plan.json" 0 "$work" || fail profile-apply
dmb_dns_nm_verify "$work/plan.json" 0 || fail profile-readback
[[ $(cat "$work/resolved.json") == "$before_runtime" ]] || fail premature-runtime-mutation
dmb_dns_resolved_apply "$work/plan.json" 1 "$work" || fail runtime-apply
dmb_dns_resolved_verify "$work/plan.json" 1 || fail runtime-readback
dmb_dns_resolved_restore "$work/plan.json" 1 "$work" || fail runtime-restore
dmb_dns_resolved_verify_restore "$work/plan.json" 1 || fail runtime-restoration-readback
dmb_dns_nm_restore "$work/plan.json" 0 "$work" || fail profile-restore
dmb_dns_nm_verify_restore "$work/plan.json" 0 || fail profile-restoration-readback
expect_reject dmb_dns_plan eth0 cloudflare persistent 8.8.8.8
expect_reject dmb_dns_plan eth0 custom persistent '1.1.1.1 8.8.8.8'
printf 'DNS parsing, separate profile/runtime mutations, policy-preserving readback and inverse mocks passed.\n'
# Explicit probe outcomes: a received DNS error is not a filtered timeout.
probe_status=NOERROR
probe_answer='example.test. 60 IN A 192.0.2.1'
dig() { [[ $probe_status == TIMEOUT ]] && return 9;printf ';; ->>HEADER<<- opcode: QUERY, status: %s, id: 1\n%s\n' "$probe_status" "$probe_answer"; }
dmb_dns_probe example.test > "$work/probe" || fail positive-probe
jq -e '.verdict=="resolved"' "$work/probe" >/dev/null || fail positive-verdict
probe_status=SERVFAIL;probe_answer=''
rc=0;dmb_dns_probe example.test > "$work/probe" || rc=$?
[[ $rc == 1 ]] && jq -e '.verdict=="dns-error-response"' "$work/probe" >/dev/null || fail explicit-dns-error
probe_status=TIMEOUT
rc=0;dmb_dns_probe example.test > "$work/probe" || rc=$?
[[ $rc == 3 ]] && jq -e '.verdict=="inconclusive"' "$work/probe" >/dev/null || fail blocked-probe
# Restoring a committed DNS snapshot creates a new inverse plan and refuses
# ambiguous historical ownership, rather than ordering by wall-clock timestamps.
DMB_STATE_DIR=$work/state
mkdir -p "$DMB_STATE_DIR/transactions/tx-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
cp "$work/plan.json" "$DMB_STATE_DIR/transactions/tx-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/plan.json"
printf '{"state":"COMMITTED"}\n' > "$DMB_STATE_DIR/transactions/tx-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/state.json"
dmb_trusted_path() { [[ -e $1 || ${2:-} == allow_missing ]]; }
# A typed DNS step cannot redirect a known owner to a security configuration.
jq -cn '{steps:[{owner:"static",identity:"/etc/resolv.conf",file:"/etc/ssh/sshd_config"}]}' > "$work/forged-file"
expect_reject dmb_dns_file_path "$work/forged-file" 0
dmb_dns_nm_apply "$work/plan.json" 0 "$work" || fail restore-fixture-profile
dmb_dns_resolved_apply "$work/plan.json" 1 "$work" || fail restore-fixture-runtime
dmb_dns_restore_plan eth0 > "$work/inverse" || fail inverse-plan
jq -e '.steps[0].adapter=="dns_resolved" and .steps[1].adapter=="dns_nm" and .steps[0].after.dns==["192.0.2.53"] and .steps[1].after.dns4==["192.0.2.53"]' "$work/inverse" >/dev/null || fail inverse-order
dmb_dns_resolved_apply "$work/inverse" 0 "$work" || fail inverse-runtime
dmb_dns_nm_apply "$work/inverse" 1 "$work" || fail inverse-profile
dmb_dns_nm_verify_restore "$work/plan.json" 0 || fail original-profile-restored
dmb_dns_resolved_verify_restore "$work/plan.json" 1 || fail original-runtime-restored
