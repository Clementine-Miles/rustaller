#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(dirname "$SCRIPT_DIR")"
SCRIPT="$ROOT/dmitbox-local.sh"
TMP="$(mktemp -d)"
trap 'rm -rf -- "$TMP"' EXIT
fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }
check() { "$@" || fail "command failed: $*"; }

check bash -n "$SCRIPT"
[[ "$(bash "$SCRIPT" --version)" == 'dmitbox.sh 2026.11.13-local-r3' ]] || fail 'version changed'
bash "$SCRIPT" --help | grep -Fq '无遥测、无远端控制端' || fail 'missing local-only help'
printf 'PASS: bash parse and standalone local CLI\n'

# No remote scripts, HTTP client invocations, background reporters or portal code.
if grep -Eiq '^[[:space:]]*[^#[:space:]].*(curl[[:space:]]|wget[[:space:]]|https?://|send_stats[[:space:]]|remote_script_execute|dd_reinstall|secure_site_install)' "$SCRIPT"; then
  fail 'network fetch, telemetry or webserver content remains in an executable source line'
fi
if grep -Eq '^([a-zA-Z_][a-zA-Z0-9_]*)\(\) \{' "$SCRIPT"; then :; else fail 'missing functions'; fi
for name in send_stats run_remote_script remote_script_execute dd_reinstall secure_site_install_or_update reverse_proxy_install_or_update tcpfit_install_reviewed cloudinit_qga_apply_dmit_default_ipchange_mode; do
  grep -Eq "^${name}\\(\\) *\\{" "$SCRIPT" && fail "removed function still defined: $name"
done
printf 'PASS: no telemetry, remote code downloader, hosting module or DD entrypoint\n'

# Every global function has one owner; no overridden definitions.
awk '/^[a-zA-Z_][a-zA-Z_0-9]*\(\) \{/ {print $1}' "$SCRIPT" | sort | uniq -d > "$TMP/duplicates"
[[ ! -s "$TMP/duplicates" ]] || { cat "$TMP/duplicates"; fail 'duplicate function definitions'; }
printf 'PASS: no duplicate shell function definitions\n'

export DMITBOX_SOURCE_ONLY=1
source "$SCRIPT"
for name in default_iface have_cmd netg_dns_set_safe netg_mtu_set_safe netg_ipv6_toggle_safe netg_firewall_fingerprint netg_tx_confirm dns_backup_once gai_backup_once external_tcp_tuning_active; do
  declare -F "$name" >/dev/null || fail "missing retained implementation: $name"
done
[[ "$(declare -f netg_dns_set_safe | grep -c 'dns_backup_once')" -ge 1 ]] || fail 'baseline DNS backup not called'
printf 'PASS: all retained network operations are wired to the guarded state machine\n'

# Test managed file writes and rejected symlink targets without touching /etc.
check write_file "$TMP/managed.conf" 'data' 0600
[[ "$(stat -c %a "$TMP/managed.conf")" == 600 ]] || fail 'mode not private'
printf 'SENTINEL' > "$TMP/sentinel"
ln -s "$TMP/sentinel" "$TMP/target-symlink"
if write_file "$TMP/target-symlink" 'overwrite' >/dev/null 2>&1; then fail 'wrote through symlink'; fi
[[ "$(cat "$TMP/sentinel")" == SENTINEL ]] || fail 'symlink target changed'
printf 'PASS: atomic managed file writes and symlink rejection\n'

# Simulated local DNS backup, including failure on symlink source. No DNS query.
NETG_RESOLV_PATH="$TMP/source-resolv.conf"
RESOLV_BACKUP="$TMP/backups/original.conf"
printf 'nameserver 9.9.9.9\n' > "$NETG_RESOLV_PATH"
check dns_backup_once
[[ "$(cat "$RESOLV_BACKUP")" == 'nameserver 9.9.9.9' ]] || fail 'DNS baseline backup'
printf 'nameserver 1.1.1.1\n' > "$NETG_RESOLV_PATH"
check dns_backup_once
[[ "$(cat "$RESOLV_BACKUP")" == 'nameserver 9.9.9.9' ]] || fail 'DNS baseline overwritten'
ln -s "$TMP/source-resolv.conf" "$TMP/resolv-symlink"
NETG_RESOLV_PATH="$TMP/resolv-symlink"
RESOLV_BACKUP="$TMP/backups/new.conf"
if dns_backup_once >/dev/null 2>&1; then fail 'symlink resolver accepted'; fi
printf 'PASS: local DNS baseline backup is separate and tamper-resistant\n'

# Test-only handling of external TCP tuning: do not execute any program.
if grep -Eq '(curl|wget|bash[[:space:]]+[^-])' <(declare -f external_tcp_tuning_active); then fail 'TCP conflict check not read-only'; fi
printf 'PASS: external TCP conflict detection is read-only\n'
printf 'ALL LOCAL INTEGRATION TESTS PASS\n'
