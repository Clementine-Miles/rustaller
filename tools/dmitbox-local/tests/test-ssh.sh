#!/usr/bin/env bash
# Mock-driven policy and staged-planner tests; no host SSH reload or file edits.
set -euo pipefail
ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
# shellcheck source=../src/validation.sh
source "$ROOT/src/validation.sh"
# shellcheck source=../src/confirmation.sh
source "$ROOT/src/confirmation.sh"
# shellcheck source=../src/adapters/nft.sh
source "$ROOT/src/adapters/nft.sh"
# shellcheck source=../src/adapters/ssh.sh
source "$ROOT/src/adapters/ssh.sh"
fixture_dir=$(mktemp -d); trap 'rm -rf -- "$fixture_dir"' EXIT
owner=0123456789abcdef0123456789abcdef
fragment=$fixture_dir/ports.conf
printf '# dmitbox-local:%s\nPort 22\nPort 2222\n' "$owner" > "$fragment"
[[ $(dmb_ssh_ports "$fragment" "$owner") == '[22,2222]' ]]
printf 'PasswordAuthentication yes\n' >> "$fragment"
if dmb_ssh_ports "$fragment" "$owner" >/dev/null; then exit 1; fi
printf '# dmitbox-local:%s\nPort 22\n' "$owner" > "$fragment"
# Authentication policy hashing ignores only intentional listener changes.
a=$(printf 'port 22\nlistenaddress 0.0.0.0:22\npasswordauthentication no\nauthenticationmethods publickey,password\n' | dmb_ssh_effective_policy)
b=$(printf 'port 2222\nlistenaddress 0.0.0.0:2222\npasswordauthentication no\nauthenticationmethods publickey,password\n' | dmb_ssh_effective_policy)
c=$(printf 'port 22\npasswordauthentication yes\nauthenticationmethods any\n' | dmb_ssh_effective_policy)
[[ $a == "$b" && $a != "$c" ]]
# The fake native sshd parses the generated shadow's include; this fixture tests
# replacement and planner choreography, not OpenSSH semantics or accessibility.
cat > "$fixture_dir/sshd" <<'SCRIPT'
#!/usr/bin/env bash
set -eu
context=''
while (($#)); do
  if [[ $1 == -f ]]; then config=$2; shift 2
  elif [[ $1 == -C ]]; then context=$2; shift 2
  else shift; fi
done
include=$(awk '$1=="Include"{print $2}' "$config")
while read -r key value; do [[ $key != Port ]] || printf 'port %s\n' "$value"; done < "$include"
if [[ ${MOCK_SSHD_MATCH_CHANGE:-0} == 1 && $context == *lport=2222 ]]; then
  printf 'passwordauthentication yes\nauthenticationmethods any\n'
else printf 'passwordauthentication no\nauthenticationmethods publickey,password\n'; fi
printf 'usedns %s\n' "${MOCK_SSHD_USE_DNS:-no}"
while read -r key value; do [[ $key != Port ]] || printf 'listenaddress 0.0.0.0:%s\n' "$value"; done < "$include"
SCRIPT
chmod 700 "$fixture_dir/sshd"
printf 'Include %s\nMatch User alice\n  AuthenticationMethods publickey,password\n' "$fragment" > "$fixture_dir/sshd_config"
jq -cn --arg owner "$owner" --arg cfg "$fixture_dir/sshd_config" --arg fragment "$fragment" --arg sshd "$fixture_dir/sshd" '{schema:1,driver:"systemd-service",owner_id:$owner,config:$cfg,fragment:$fragment,sshd:$sshd,contexts:[{user:"alice",host:"192.0.2.1",addr:"192.0.2.1",laddr:"192.0.2.2",lport:22}]}' > "$fixture_dir/ssh.json"
selector='{"mode":"exclusive","family":"inet","table":"dmitbox_local","chain":"input","parent_chain":null,"owner_id":"0123456789abcdef0123456789abcdef"}'
nft_baseline=$(dmb_nft_plan_state "$selector" '[{"address":"0.0.0.0","port":22}]')
jq -cn --argjson s "$selector" --arg pipeline "$(printf '%064d' 0)" '{schema:1,nft:$s,pipeline_sha256:$pipeline}' > "$fixture_dir/nft.json"
# Explicit library-only seams. Production entrypoint never sources these mocks.
dmb_ssh_manifest() { printf '%s\n' "$fixture_dir/ssh.json"; }
dmb_ssh_guard() { :; }
dmb_trusted_path() { :; }
dmb_scratch_tmp() { mktemp -d "$fixture_dir/native.XXXXXXXX"; }
dmb_run() { shift; if [[ $1 == ss ]]; then return 0; fi; "$@"; }
dmb_file_state() { jq -cn --arg hash "$(sha256sum "$1" | cut -d' ' -f1)" '{exists:true,type:"file",sha256:$hash,mode:"600",uid:0,gid:0,acl_b64:"",xattrs_b64:""}'; }
dmb_nft_enrollment_path() { printf '%s\n' "$fixture_dir/nft.json"; }
dmb_nft_capture() { printf '%s\n' "$nft_baseline"; }
dmb_confirmation_transport() { printf '{"kind":"ssh","local_port":22,"socket_inode":"1","boot_id":"fixture"}\n'; }
dmb_ssh_listener_check() { :; }
jq '.contexts[0].host="unproven.example"' "$fixture_dir/ssh.json" > "$fixture_dir/hostname.json"
if dmb_ssh_simulate "$fixture_dir/hostname.json" "$(base64 -w0 "$fragment")" >/dev/null 2>&1; then exit 1; fi
export MOCK_SSHD_USE_DNS=yes
if dmb_ssh_simulate "$fixture_dir/ssh.json" "$(base64 -w0 "$fragment")" >/dev/null 2>&1; then exit 1; fi
unset MOCK_SSHD_USE_DNS
plan=$(dmb_ssh_plan_port 2222)
jq -e '.schema==1 and .confirm_port==2222 and (.resources|length)==2 and (.steps|length)==4 and
  .steps[0].adapter=="nft" and .steps[1].adapter=="ssh" and
  .steps[2].phase=="commit" and .steps[3].phase=="commit" and
  .steps[1].after==.steps[2].before and
  .steps[1].after_effective[0].ports==[22,2222] and
  .steps[2].after_effective[0].ports==[2222] and .steps[2].after_effective[0].context.lport==2222' <<< "$plan" >/dev/null
# The SSH adapter owns only its tagged listener grants, retaining other rules.
jq -e --arg tag "dmitbox-local:ssh:$owner" '.steps[0].after.nftables | any(.[];.rule.comment?==$tag)' <<< "$plan" >/dev/null
printf '%s\n' "$plan" > "$fixture_dir/plan.json"
dmb_ssh_plan_shape "$fixture_dir/plan.json"
dmb_ssh_payload_check "$fixture_dir/plan.json" 1 "$fixture_dir/ssh.json"
dmb_ssh_payload_check "$fixture_dir/plan.json" 2 "$fixture_dir/ssh.json"
dmb_ssh_firewall_check "$fixture_dir/plan.json" "$fixture_dir/ssh.json"
# Hand-edited public plans receive the same coordination/auth-policy checks.
jq '.steps=[.steps[1]]' "$fixture_dir/plan.json" > "$fixture_dir/unguarded.json"
if dmb_ssh_plan_shape "$fixture_dir/unguarded.json" >/dev/null; then exit 1; fi
jq '.steps[1].after_effective[0].policy_sha256="forged"' "$fixture_dir/plan.json" > "$fixture_dir/auth-forged.json"
if dmb_ssh_payload_check "$fixture_dir/auth-forged.json" 1 "$fixture_dir/ssh.json" >/dev/null; then exit 1; fi
jq '.steps[0].after.nftables+= [{rule:{expr:[{drop:null}]}}]' "$fixture_dir/plan.json" > "$fixture_dir/firewall-forged.json"
if dmb_ssh_firewall_check "$fixture_dir/firewall-forged.json" "$fixture_dir/ssh.json" >/dev/null; then exit 1; fi
# Match LocalPort can silently weaken authentication without changing files.
# Simulation must use the NEW connection port, preserving the old policy hash.
export MOCK_SSHD_MATCH_CHANGE=1
if dmb_ssh_plan_port 2222 >/dev/null 2>&1; then exit 1; fi
unset MOCK_SSHD_MATCH_CHANGE
# Reapplication when the target is already configured refuses before mutation.
printf '# dmitbox-local:%s\nPort 2222\n' "$owner" > "$fragment"
if dmb_ssh_plan_port 2222 >/dev/null 2>&1; then exit 1; fi
# An ambiguous or absent explicit anchor is refused by the native simulation.
printf 'Include /etc/ssh/sshd_config.d/*.conf\n' > "$fixture_dir/sshd_config"
if dmb_ssh_simulate "$fixture_dir/ssh.json" "$(base64 -w0 "$fragment")" >/dev/null 2>&1; then exit 1; fi
printf 'PASS SSH owned Port parser, authentication-policy preservation, staged plan and refusals (mock-driven)\n'
