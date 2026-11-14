#!/usr/bin/env bash
# Kernel/proc fixtures, not a claim that an actual remote host was reachable.
set -euo pipefail
ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
# shellcheck source=../src/confirmation.sh
source "$ROOT/src/confirmation.sh"
# shellcheck source=../src/validation.sh
source "$ROOT/src/validation.sh"
fixture_dir=$(mktemp -d); trap 'rm -rf -- "$fixture_dir"' EXIT
proc=$fixture_dir/proc
mkdir -p "$proc/sys/kernel/random" "$proc/100/fd" "$proc/200/fd" "$proc/300/fd"
printf '01234567-89ab-cdef-0123-456789abcdef\n' > "$proc/sys/kernel/random/boot_id"
stat_fixture() {
  local pid=$1 parent=$2 ticks=$3 value i
  value="$pid (comm with ) spaces) S $parent"
  for ((i=2;i<19;i++)); do value+=' 0'; done
  printf '%s %s\n' "$value" "$ticks" > "$proc/$pid/stat"
  printf '0::/system.slice/sshd.service\n' > "$proc/$pid/cgroup"
  printf 'Uid:\t0\t0\t0\t0\n' > "$proc/$pid/status"
}
stat_fixture 100 200 101
stat_fixture 200 300 90
stat_fixture 300 1 50
ln -s 'socket:[9001]' "$proc/200/fd/3"
jq -cn '{schema:1,contexts:[{user:"root",addr:"192.0.2.1",laddr:"192.0.2.2",lport:2222},{user:"alice",addr:"192.0.2.1",laddr:"192.0.2.2",lport:2222}],confirmation:{ssh_cgroups:["/system.slice/sshd.service"]}}' > "$fixture_dir/enrollment.json"
# Override executable verification solely for this nonroot proc fixture. Real
# production verifies root ownership, executable hash and enrolled cgroup.
dmb_trusted_path() { :; }
_dmb_confirmation_binary() { [[ $4 == 200 || $4 == 300 ]] || return 1; printf '/usr/sbin/sshd\n'; }
_dmb_confirmation_local_user() { case $1 in 0) printf 'root\n';;1000)printf 'alice\n';;*)return 1;;esac; }
_dmb_confirmation_ss() { printf '0 0 192.0.2.2:2222 192.0.2.1:54321 users:(("sshd",pid=200,fd=3)) ino:9001\n'; }
SSH_CONNECTION='203.0.113.99 1 127.0.0.1 22'
export SSH_CONNECTION
transport=$(_dmb_confirmation_transport_at "$fixture_dir/enrollment.json" "$proc" 100)
jq -e '.socket_inode=="9001" and .local_port==2222 and .peer_address=="192.0.2.1" and .pid==200 and .start_ticks=="90" and .login_uid==0 and .login_user=="root"' <<< "$transport" >/dev/null
# sudo/root callers do not turn an unprivileged SSH login into a root login.
# The socket may belong to the privileged monitor in a different cgroup.
mkdir -p "$proc/150/fd"
stat_fixture 100 150 101
stat_fixture 150 200 95
printf 'Uid:\t1000\t1000\t1000\t1000\n' > "$proc/150/status"
printf '0::/user.slice/session.scope\n' > "$proc/150/cgroup"
printf '1000\n' > "$proc/100/loginuid"
_dmb_confirmation_binary() { [[ $4 == 150 || $4 == 200 || $4 == 300 ]] || return 1; printf '/usr/sbin/sshd\n'; }
unprivileged=$(_dmb_confirmation_transport_at "$fixture_dir/enrollment.json" "$proc" 100)
jq -e '.login_uid==1000 and .login_user=="alice" and .pid==200' <<< "$unprivileged" >/dev/null
printf '2000\n' > "$proc/100/loginuid"
if _dmb_confirmation_transport_at "$fixture_dir/enrollment.json" "$proc" 100 >/dev/null; then exit 1; fi
printf '4294967295' > "$proc/100/loginuid"
stat_fixture 100 200 101
_dmb_confirmation_binary() { [[ $4 == 200 || $4 == 300 ]] || return 1; printf '/usr/sbin/sshd\n'; }
# Changing PID/start ticks cannot turn a multiplexed existing TCP transport into
# a fresh connection; inode/tuple identity is the freshness boundary.
changed=$(jq '.pid=999 | .start_ticks="99999"' <<< "$transport")
if _dmb_confirmation_fresh "$transport" "$changed"; then exit 1; fi
fresh=$(jq '.socket_inode="9002" | .peer_port=54322 | .start_ticks="300"' <<< "$transport")
_dmb_confirmation_fresh "$transport" "$fresh"
if _dmb_confirmation_fresh null "$fresh"; then exit 1; fi
# An unrelated socket cannot be attributed to our ancestor using its PID label.
_dmb_confirmation_ss() { printf '0 0 192.0.2.2:2222 192.0.2.1:54321 users:(("sshd",pid=200,fd=3)) ino:9009\n'; }
if _dmb_confirmation_transport_at "$fixture_dir/enrollment.json" "$proc" 100 >/dev/null; then exit 1; fi
# No enrolled sshd ancestry means no SSH authorization despite forged env.
_dmb_confirmation_binary() { return 1; }
if _dmb_confirmation_transport_at "$fixture_dir/enrollment.json" "$proc" 100 >/dev/null; then exit 1; fi
# State-level proof checks independently enforce target listener and deadline.
mkdir "$fixture_dir/tx"
boot=$(cat /proc/sys/kernel/random/boot_id)
read -r uptime _ < /proc/uptime; uptime=${uptime%%.*}
jq -cn --arg boot "$boot" --argjson deadline "$((uptime+30))" '{id:"fixture",generation:3,confirmation_round:0,confirmation_min_start_ticks:200,boot_id:$boot,deadline_uptime:$deadline}' > "$fixture_dir/tx/state.json"
jq -cn --argjson origin "$transport" '{origin_transport:$origin,confirm_port:2222}' > "$fixture_dir/tx/plan.json"
printf '%064d\n' 0 > "$fixture_dir/tx/challenge.sha256"
_dmb_confirmation_root() { :; }
_dmb_confirmation_manifest() { printf '%s\n' "$fixture_dir/enrollment.json"; }
dmb_confirmation_transport() { printf '%s\n' "$fresh"; }
_dmb_confirmation_read_challenge() { (($1>0 && $1<=30)) || return 1; printf '%064d\n' 0; }
proof=$(dmb_confirmation_check "$fixture_dir/tx" ssh)
jq -e '.transaction_id=="fixture" and .generation==3 and .remote_access_verified==true' <<< "$proof" >/dev/null
# Another authenticated network context cannot certify untested Match Address.
original_fresh=$fresh
fresh=$(jq '.peer_address="198.51.100.1"' <<< "$fresh")
if dmb_confirmation_check "$fixture_dir/tx" ssh >/dev/null 2>&1; then exit 1; fi
fresh=$original_fresh
# A fresh root login does not establish reachability for the intended user alice.
jq '.confirm_user="alice"' "$fixture_dir/tx/plan.json" > "$fixture_dir/wrong-user.json"
mv "$fixture_dir/wrong-user.json" "$fixture_dir/tx/plan.json"
if dmb_confirmation_check "$fixture_dir/tx" ssh >/dev/null 2>&1; then exit 1; fi
jq 'del(.confirm_user)' "$fixture_dir/tx/plan.json" > "$fixture_dir/correct-user.json"
mv "$fixture_dir/correct-user.json" "$fixture_dir/tx/plan.json"
# A different session opened before application is not fresh authentication.
fresh=$(jq '.start_ticks="150"' <<< "$fresh")
if dmb_confirmation_check "$fixture_dir/tx" ssh >/dev/null 2>&1; then exit 1; fi
fresh=$(jq '.start_ticks="300"' <<< "$fresh")
jq '.confirm_port=22' "$fixture_dir/tx/plan.json" > "$fixture_dir/wrong.json"
mv "$fixture_dir/wrong.json" "$fixture_dir/tx/plan.json"
if dmb_confirmation_check "$fixture_dir/tx" ssh >/dev/null 2>&1; then exit 1; fi
jq --argjson deadline "$((uptime-1))" '.deadline_uptime=$deadline' "$fixture_dir/tx/state.json" > "$fixture_dir/expired.json"
mv "$fixture_dir/expired.json" "$fixture_dir/tx/state.json"
if dmb_confirmation_check "$fixture_dir/tx" ssh >/dev/null 2>&1; then exit 1; fi
printf 'PASS authenticated-ancestry/socket and login-user binding, fresh transport, target listener and deadline (fixtures)\n'
