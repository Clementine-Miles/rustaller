#!/usr/bin/env bash
set -euo pipefail
project=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
# shellcheck source=../src/core.sh
source "$project/src/core.sh"
# shellcheck source=../src/transaction.sh
source "$project/src/transaction.sh"
# shellcheck source=../src/validation.sh
source "$project/src/validation.sh"
# shellcheck source=../src/adapters/runtime.sh
source "$project/src/adapters/runtime.sh"
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
mkdir "$work/tx" "$work/tx/snapshots"
printf '{"uuid":{},"swapdevices":[]}\n' > "$work/native.json"
printf cubic > "$work/congestion"
printf 0 > "$work/probes"
# These fixtures replace privilege/native host mutation only inside this test.
dmb_require_root() { :; }
dmb_trusted_path() { [[ $1 == "$work" || $1 == "$work/"* ]]; }
dmb_run() { shift; "$@"; }
dmb_atomic_write() { local file=$1 mode=$2; cat > "$file"; chmod "$mode" "$file"; }
dmb_audit() { jq -ce . <<< "${2:-'{}'}" >> "$work/audit"; }
stat() {
  if [[ $1 == -c && ${*: -1} == "$work/"* ]]; then
    case "$2" in %u|%g) printf '0\n'; return;; '%u:%g:%a') printf '0:0:600\n'; return;; '%s:%h:%u:%a') command stat -c '%s:%h:0:600' -- "${*: -1}"; return;; esac
  fi
  command stat "$@"
}
sysctl() {
  printf '%s\n' "$*" >> "$work/sysctl.calls"
  if [[ $1 == -n ]]; then
    case "$2" in net.ipv4.tcp_available_congestion_control) printf 'cubic bbr\n';;
      net.ipv4.tcp_congestion_control) cat "$work/congestion";;
      net.core.default_qdisc) printf 'fq_codel\n';;
      *) return 1;; esac
  elif [[ $1 == -w && $2 == net.ipv4.tcp_congestion_control=* ]]; then printf '%s' "${2#*=}" > "$work/congestion"
  else return 1; fi
}
iperf3() {
  local count
  count=$(cat "$work/probes"); printf '%s' "$((count+1))" > "$work/probes"
  [[ ${MOCK_PROBE_FAIL:-0} == 0 ]] || return 1
  printf '{"end":{"sum_received":{"bits_per_second":100000000}}}\n'
}
findmnt() { printf '{"filesystems":[{"fstype":"%s","options":"rw,relatime"}]}\n' "${MOCK_FS:-ext4}"; }
fallocate() {
  if [[ ${MOCK_ENOSPC:-0} == 1 ]]; then command truncate -s 4096 -- "${*: -1}"; return 1; fi
  command fallocate "$@"
}
mkswap() {
  local path=${*: -1} identity uuid=$2 output
  identity=$(command stat -c '%d:%i' -- "$path")
  output=$(jq -c --arg identity "$identity" --arg uuid "$uuid" '.uuid[$identity]=$uuid' "$work/native.json")
  printf '%s\n' "$output" > "$work/native.json"
}
blkid() {
  local path=${*: -1} identity uuid
  identity=$(command stat -c '%d:%i' -- "$path")
  uuid=$(jq -er --arg identity "$identity" '.uuid[$identity]' "$work/native.json") || return 2
  if [[ $* == *'-s TYPE'* ]]; then printf 'swap\n'; else printf '%s\n' "$uuid"; fi
}
swapon() {
  if [[ $1 == --show ]]; then jq '{swapdevices}' "$work/native.json"; return; fi
  local path=${*: -1} priority=$2 output
  [[ -f $path ]] || return 1
  output=$(jq -c --arg path "$path" --argjson priority "$priority" '.swapdevices += [{name:$path,type:"file",size:16773120,used:0,prio:$priority}]' "$work/native.json")
  printf '%s\n' "$output" > "$work/native.json"
}
swapoff() {
  [[ ${MOCK_SWAPOFF_FAIL:-0} == 0 ]] || return 1
  local path=${*: -1} output
  output=$(jq -c --arg path "$path" '.swapdevices|=map(select(.name!=$path))' "$work/native.json")
  printf '%s\n' "$output" > "$work/native.json"
}
ln() {
  if [[ ${MOCK_CREATE_RACE:-0} == 1 ]]; then printf 'foreign resource\n' > "${*: -1}"; fi
  command ln "$@"
}
assert_fail() { if "$@" > "$work/expected-failure.out" 2>&1; then printf 'Expected failure: %s\n' "$*" >&2; exit 1; fi; }

# Status reads do not change sysctls, call iperf, or install packages.
dmb_tcp_report | jq -e '.current=="cubic" and .performance=="unmeasured"' >/dev/null
[[ $(cat "$work/probes") == 0 && ! $(cat "$work/sysctl.calls") =~ '-w' ]]
printf '{"schema":1,"requested":true,"target":"192.0.2.9","port":5201,"seconds":2}' > "$work/measure.json"
dmb_tcp_plan bbr "$work/measure.json" runtime > "$work/tcp-plan.json"
[[ $(cat "$work/probes") == 0 ]]
# Engine supplies exact precondition after locking.
jq '.steps[0].before={value:"cubic"}' "$work/tcp-plan.json" > "$work/tcp.json"
dmb_tcp_snapshot "$work/tcp.json" 0 "$work/tx"
dmb_tcp_apply "$work/tcp.json" 0 "$work/tx"
[[ $(cat "$work/congestion") == bbr ]]
dmb_tcp_verify "$work/tcp.json" 0 "$work/tx"
jq -e '.ratio==1 and .baseline.bits_per_second>0' "$work/tx/snapshots/tcp-0-after.json" >/dev/null
MOCK_PROBE_FAIL=1 assert_fail dmb_tcp_verify "$work/tcp.json" 0 "$work/tx"
# An external administrator's third value must not be overwritten.
printf reno > "$work/congestion"
assert_fail dmb_tcp_restore "$work/tcp.json" 0 "$work/tx"
[[ $(cat "$work/congestion") == reno ]]
printf bbr > "$work/congestion"
dmb_tcp_restore "$work/tcp.json" 0 "$work/tx"
dmb_tcp_restore "$work/tcp.json" 0 "$work/tx"
[[ $(cat "$work/congestion") == cubic ]]
! grep -Eq 'sysctl --system|modprobe|net.core.default_qdisc=' "$work/sysctl.calls"
printf '{"schema":1,"requested":true,"target":"999.1.1.1","port":5201,"seconds":2}' > "$work/bad-measure.json"
assert_fail dmb_tcp_plan bbr "$work/bad-measure.json" runtime

# New Swap file: verified allocation -> no-replace link -> explicit activation.
new_swap_plan() {
  local target=$1 output=$2
  dmb_swap_plan_create "$target" 16 77 > "$work/plan.tmp"
  jq '.steps[0].before={exists:false}|.steps[1].before=.steps[0].after' "$work/plan.tmp" > "$output"
}
new_swap_plan "$work/swapfile" "$work/swap.json"
dmb_swap_apply "$work/swap.json" 0 "$work/tx"
dmb_swap_verify "$work/swap.json" 0 "$work/tx"
[[ $(command stat -c %h -- "$work/swapfile") == 1 ]]
dmb_swap_apply "$work/swap.json" 1 "$work/tx"
dmb_swap_verify "$work/swap.json" 1 "$work/tx"
jq -e '.swapdevices[0].prio==77' "$work/native.json" >/dev/null
# Failed swapoff must propagate and retain the file and activation evidence.
MOCK_SWAPOFF_FAIL=1 assert_fail dmb_swap_restore "$work/swap.json" 1 "$work/tx"
[[ -f $work/swapfile ]]
dmb_swap_restore "$work/swap.json" 1 "$work/tx"
dmb_swap_restore "$work/swap.json" 1 "$work/tx"
dmb_swap_restore "$work/swap.json" 0 "$work/tx"
dmb_swap_cleanup "$work/swap.json" 0 "$work/tx"
[[ ! -e $work/swapfile ]]
# No pre-existing resources are overwritten, and unsupported fs fails early.
printf 'keep me\n' > "$work/existing"
assert_fail dmb_swap_plan_create "$work/existing" 16 1
[[ $(cat "$work/existing") == 'keep me' ]]
MOCK_FS=btrfs assert_fail dmb_swap_plan_create "$work/unsupported" 16 1
[[ ! -e $work/unsupported ]]
# Filesystem exhaustion only leaves independently removable private staging.
new_swap_plan "$work/enospc" "$work/enospc.json"
MOCK_ENOSPC=1 assert_fail dmb_swap_apply "$work/enospc.json" 0 "$work/tx"
[[ ! -e $work/enospc ]]
dmb_swap_cleanup "$work/enospc.json" 0 "$work/tx"
# Atomic check-and-create refuses a competing resource and preserves it.
new_swap_plan "$work/racing" "$work/racing.json"
MOCK_CREATE_RACE=1 assert_fail dmb_swap_apply "$work/racing.json" 0 "$work/tx"
[[ $(cat "$work/racing") == 'foreign resource' ]]
dmb_swap_cleanup "$work/racing.json" 0 "$work/tx"
[[ $(cat "$work/racing") == 'foreign resource' ]]
# Malformed path ownership metadata is rejected before touching resources.
jq '.steps[0].owner_id="../../escape"' "$work/swap.json" > "$work/malicious.json"
assert_fail dmb_swap_apply "$work/malicious.json" 0 "$work/tx"
# Restart at the atomic link/unlink boundary: independently recorded inode is
# sufficient to clean our two names, and an active inode must be retained.
new_swap_plan "$work/crash" "$work/crash.json"
dmb_swap_apply "$work/crash.json" 0 "$work/tx"
crash_owner=$(jq -r '.steps[0].owner_id' "$work/crash.json")
crash_stage="$work/tx/snapshots/swap-$crash_owner.stage"
command ln -- "$work/crash" "$crash_stage"
swapon --priority 33 -- "$crash_stage"
assert_fail dmb_swap_cleanup "$work/crash.json" 0 "$work/tx"
[[ -e $work/crash && -e $crash_stage ]]
swapoff -- "$crash_stage"
dmb_swap_cleanup "$work/crash.json" 0 "$work/tx"
[[ ! -e $work/crash && ! -e $crash_stage ]]
dmb_swap_cleanup "$work/crash.json" 0 "$work/tx"

# Explicit package maintenance must refuse concurrent pending network work.
dmb_tx_init() { mkdir -p "$work/state/transactions" "$work/locks"; }
dmb_state_root() { printf '%s\n' "$work/state"; }
dmb_lock_root() { printf '%s\n' "$work/locks"; }
apt-get() { printf '%s\n' "$*" >> "$work/package.calls"; }
dmb_tx_init
mkdir "$work/state/transactions/tx-fixture"
printf '{"state":"ARMED"}' > "$work/state/transactions/tx-fixture/state.json"
assert_fail dmb_maintenance_run update-packages
[[ ! -e $work/package.calls ]]
printf '{"state":"COMMITTED"}' > "$work/state/transactions/tx-fixture/state.json"
dmb_maintenance_run update-packages
[[ $(cat "$work/package.calls") == $'update\nupgrade' ]]
jq -se 'all(.[]; .rollback==false)' "$work/audit" >/dev/null
before_probe=$(cat "$work/probes")
dmb_maintenance_report system | jq -e '.network_probes==false' >/dev/null
dmb_maintenance_report cleanup | jq -e '.mutations==false' >/dev/null
[[ $(cat "$work/probes") == "$before_probe" ]]
assert_fail dmb_runtime_timezone ../../etc/shadow
printf 'runtime: PASS (mocked sysctl/iperf/swap/packages; real private allocation, hard links and flock)\n' 
