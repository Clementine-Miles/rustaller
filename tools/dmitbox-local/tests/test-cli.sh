#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd -- "$(dirname -- "$0")/.." && pwd -P)
artifact=$ROOT/dist/dmitbox-local.sh
[[ -f $artifact ]] || { printf 'Build the standalone artifact first\n'; exit 1; }
# Read only the generated literal IFS assignment, not the CLI entry point.
ifs_line=$(sed -n '/^IFS=/p' "$artifact")
[[ $ifs_line == "IFS="* && $ifs_line != *$'\n'* ]]
[[ $(bash -c "$ifs_line"'; printf "%s" "$IFS" | od -An -tx1' | tr -d ' \n') == 20090a ]]
output=$(DMITBOX_ALLOW_UNGUARDED_FIREWALL=1 NETG_TEST_MODE=1 DMB_STATE_DIR=/tmp/forged bash "$artifact" --help)
[[ $output == *'--net-confirm'* && $output == *'--apply-plan'* ]]
[[ $(bash "$artifact" --version) == 4.* ]]
! bash "$artifact" --not-a-command >/dev/null 2>&1
! bash "$artifact" --net-confirm >/dev/null 2>&1
! bash "$artifact" --plan dns >/dev/null 2>&1
! bash "$artifact" --internal-adapter apply tx-forged 0 >/dev/null 2>&1
! bash "$artifact" --net-mtu-set eth0 1500 >/dev/null 2>&1
if ((EUID!=0)); then
  code=0; bash "$artifact" --net-rollback tx-00000000000000000000000000000000 >/dev/null 2>&1 || code=$?
  [[ $code != 0 ]]
fi
printf 'PASS standalone CLI, invalid input, removed bypasses and privilege gates\n'
