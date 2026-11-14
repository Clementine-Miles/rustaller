#!/usr/bin/env bash
set -euo pipefail
export LC_ALL=C
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf -- "$TMP"' EXIT
mkdir -p "$TMP/bin" "$TMP/state" "$TMP/etc" "$TMP/units" "$TMP/tx"
printf '1500\n' > "$TMP/state/mtu.eth0"
printf '1500\n' > "$TMP/state/mtu.eth1"
printf 'up\n' > "$TMP/state/admin.eth0"
printf 'up\n' > "$TMP/state/admin.eth1"
printf '9.9.9.9\n' > "$TMP/state/dns"
printf '1\n' > "$TMP/state/nft_counter"
for k in all default lo; do printf '0\n' > "$TMP/state/v6-$k"; done
printf 'nameserver 9.9.9.9\n' > "$TMP/etc/resolv.conf"
printf 'old gai config\n' > "$TMP/etc/gai.conf"
export FAKE_STATE="$TMP/state"
cat > "$TMP/bin/ip" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail
s="$FAKE_STATE"
case "${1:-}" in
  -4|-6)
    if [[ "$2" == route && "$3" == get ]]; then
      [[ "$1" == -4 ]] && echo '1.1.1.1 via 192.0.2.1 dev eth0 src 192.0.2.10' || echo '2606:4700:4700::1111 via 2001:db8::1 dev eth0'
      exit 0
    fi
    if [[ "$2" == route && "$3" == show ]]; then
      echo 'default via gateway dev eth0'
      exit 0
    fi
    exit 0 ;;
  -br) echo 'eth0 UP 192.0.2.10/24'; exit 0 ;;
  -o)
    iface=eth0
    if [[ "${4:-}" == dev ]]; then iface="$5"; fi
    if [[ "${3:-}" == dev ]]; then iface="$4"; fi
    [[ -f "$s/mtu.$iface" ]] || exit 1
    mtu="$(cat "$s/mtu.$iface")"; admin="$(cat "$s/admin.$iface")"
    [[ "$admin" == up ]] && flags='BROADCAST,MULTICAST,UP,LOWER_UP' || flags='BROADCAST,MULTICAST'
    printf '2: %s: <%s> mtu %s qdisc fq state UP mode DEFAULT\n' "$iface" "$flags" "$mtu"
    exit 0 ;;
  link)
    if [[ "$2" == show ]]; then [[ -f "$s/mtu.${4:-}" ]] && exit 0; exit 1; fi
    if [[ "$2" == set && "$3" == dev ]]; then
      iface="$4"; [[ -f "$s/mtu.$iface" ]] || exit 1
      case "${5:-}" in
        mtu) printf '%s\n' "$6" > "$s/mtu.$iface" ;;
        up|down) printf '%s\n' "$5" > "$s/admin.$iface" ;;
        *) exit 1 ;;
      esac
      exit 0
    fi ;;
esac
exit 1
MOCK
cat > "$TMP/bin/sysctl" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail
if [[ "$1" == -n ]]; then
  key="${2##*.}"; [[ "$2" == net.ipv6.conf.*.disable_ipv6 ]] && key="${2#net.ipv6.conf.}" && key="${key%.disable_ipv6}" && cat "$FAKE_STATE/v6-$key" && exit 0
  printf 'bbr\n'; exit 0
fi
if [[ "$1" == -w ]]; then
  expression="$2"; key="${expression%%=*}"; value="${expression#*=}"
  [[ "$key" == net.ipv6.conf.*.disable_ipv6 ]] || exit 1
  key="${key#net.ipv6.conf.}"; key="${key%.disable_ipv6}"
  if [[ "${FAKE_SYSCTL_FAIL_KEY:-}" == "$key" && "$value" == 1 ]]; then exit 5; fi
  printf '%s\n' "$value" > "$FAKE_STATE/v6-$key"; echo "$expression"; exit 0
fi
exit 0
MOCK
cat > "$TMP/bin/systemctl" <<'MOCK'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$FAKE_STATE/systemctl.log"
if [[ "${1:-}" == is-active ]]; then
  case "${*: -1}" in
    NetworkManager|ssh.socket|sshd.socket) exit 1 ;;
    *.timer) [[ "${FAKE_TIMER_FAIL:-0}" == 1 ]] && exit 1 || exit 0 ;;
  esac
fi
exit 0
MOCK
cat > "$TMP/bin/resolvectl" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail
case "${1:-}" in
  dns)
    if (( $# == 2 )); then printf 'Link 2 (%s): %s\n' "$2" "$(tr '\n' ' ' < "$FAKE_STATE/dns")"; exit 0; fi
    : > "$FAKE_STATE/dns"
    shift 2
    printf '%s\n' "$@" > "$FAKE_STATE/dns" ;;
  revert) : > "$FAKE_STATE/dns" ;;
  flush-caches) : ;;
  *) exit 1 ;;
esac
MOCK
cat > "$TMP/bin/nft" <<'MOCK'
#!/usr/bin/env bash
[[ "$1 $2 $3" == '-a list ruleset' ]] || exit 1
printf 'table inet guarded {\n chain input { type filter hook input priority 0; policy drop; }\n counter packets %s bytes 234 accept # handle %s\n}\n' "$(cat "$FAKE_STATE/nft_counter")" "$(cat "$FAKE_STATE/nft_counter")"
MOCK
cat > "$TMP/bin/iptables-save" <<'MOCK'
#!/usr/bin/env bash
printf '*filter\n:INPUT ACCEPT [%s:400]\nCOMMIT\n' "$(cat "$FAKE_STATE/nft_counter")"
MOCK
cat > "$TMP/bin/ip6tables-save" <<'MOCK'
#!/usr/bin/env bash
printf '*filter\n:INPUT ACCEPT [%s:410]\nCOMMIT\n' "$(cat "$FAKE_STATE/nft_counter")"
MOCK
cat > "$TMP/bin/getent" <<'MOCK'
#!/usr/bin/env bash
[[ "$1 $2" == 'hosts example.com' ]] && { echo '93.184.215.14 example.com'; exit 0; }
exit 1
MOCK
chmod 755 "$TMP/bin/"*
export PATH="$TMP/bin:$PATH"
export DMITBOX_SOURCE_ONLY=1
source "$ROOT/dmitbox-local.sh"
# Resource paths are redirected to test-only fixture directories.
NETG_TX_ROOT="$TMP/tx"
NETG_UNIT_DIR="$TMP/units"
NETG_RESOLV_PATH="$TMP/etc/resolv.conf"
NETG_TIMEOUT=180
NETG_TEST_MODE=1
GAI_CONF="$TMP/etc/gai.conf"
BACKUP_BASE="$TMP/backup"
RESOLV_BACKUP="$TMP/backup/resolv.conf.orig"
IPV6_SYSCTL_FILE="$TMP/etc/ipv6-disabled.conf"
IPV6_FIX_SYSCTL_FILE="$TMP/etc/ipv6-enabled.conf"
IPV6_POOL_CONF="$TMP/etc/pool.conf"
IPV6_RAND_CONF="$TMP/etc/rand.conf"
is_systemd(){ return 0; }
is_resolved_active(){ return 1; }
fail(){ printf 'FAIL: %s\n' "$*" >&2; exit 1; }
assert(){ "$@" || fail "assert failed: $*"; }
lasttx(){ ls -td "$NETG_TX_ROOT"/tx-* | head -1; }

hash1="$(netg_firewall_fingerprint)"
printf '9999\n' > "$FAKE_STATE/nft_counter"
hash2="$(netg_firewall_fingerprint)"
[[ "$hash1" == "$hash2" ]] || fail 'firewall hash changes on counters/handles'
echo 'PASS: canonical nft/iptables SHA256 ignores counters and handles'

assert netg_ipv4_valid 192.0.2.10
! netg_ipv4_valid 300.0.0.1 || fail 'accepted invalid IPv4'
! netg_ipv4_valid '1.2.3.4.' || fail 'accepted trailing dot'
assert netg_iface_valid eth0
! netg_iface_valid evil || fail 'accepted nonexistent iface'
! netg_iface_set_safe eth0 down || fail 'allowed primary interface shutdown'
assert netg_route_test 1.1.1.1 2606:4700:4700::1111 >/dev/null

echo 'PASS: input validation, primary interface guard, read-only route diagnostics'

# The timer must arm before any mutation, and never fails open.
export FAKE_TIMER_FAIL=1
if netg_mtu_set_safe eth0 1400 2>/dev/null; then fail 'timer arm failure allowed changes'; fi
[[ "$(cat "$FAKE_STATE/mtu.eth0")" == 1500 ]] || fail 'MTU changed without rollback timer'
export FAKE_TIMER_FAIL=0

assert netg_dns_set_safe cloudflare eth0 >/dev/null
DNS_TX="$(basename "$(lasttx)")"
[[ "$(grep -c '^nameserver ' "$NETG_RESOLV_PATH")" == 3 ]] || fail 'resolver lines missing'
[[ -f "$NETG_UNIT_DIR/dmitbox-net-$DNS_TX.timer" ]] || fail 'no rollback timer unit'
[[ -f "$NETG_UNIT_DIR/dmitbox-net-$DNS_TX.service" ]] || fail 'no boot rollback service'
assert netg_tx_confirm "$DNS_TX" >/dev/null
[[ -f "$NETG_TX_ROOT/$DNS_TX/confirmed" ]] || fail 'transaction not confirmed'
if netg_tx_rollback_now "$DNS_TX" >/dev/null; then fail 'confirmed tx may rollback'; fi

assert netg_dns_set_safe google eth0 >/dev/null
DNS_TX2="$(basename "$(lasttx)")"
assert netg_tx_rollback_now "$DNS_TX2" >/dev/null
grep -q '^nameserver 1.1.1.1$' "$NETG_RESOLV_PATH" || fail 'DNS rollback did not restore immediate predecessor'
echo 'PASS: DNS change, hash, explicit confirm, reverse to pre-transaction state'

assert netg_mtu_set_safe eth0 1420 >/dev/null
MTU_TX="$(basename "$(lasttx)")"
[[ "$(cat "$FAKE_STATE/mtu.eth0")" == 1420 ]] || fail 'MTU not applied'
assert bash "$NETG_TX_ROOT/$MTU_TX/rollback.sh"
[[ "$(cat "$FAKE_STATE/mtu.eth0")" == 1500 ]] || fail 'independent MTU rollback failed'
echo 'PASS: independent worker can restore MTU without sourcing DMITBox'

assert netg_iface_set_safe eth1 down >/dev/null
IF_TX="$(basename "$(lasttx)")"
[[ "$(cat "$FAKE_STATE/admin.eth1")" == down ]] || fail 'interface did not go down'
assert netg_tx_rollback_now "$IF_TX" >/dev/null
[[ "$(cat "$FAKE_STATE/admin.eth1")" == up ]] || fail 'interface state not restored'
echo 'PASS: secondary interface down + automatic rollback restoration'

assert netg_ipv6_toggle_safe disable >/dev/null
IP_TX="$(basename "$(lasttx)")"
[[ "$(cat "$FAKE_STATE/v6-all")" == 1 ]] || fail 'IPv6 not toggled'
assert netg_tx_rollback_now "$IP_TX" >/dev/null
[[ "$(cat "$FAKE_STATE/v6-all")" == 0 ]] || fail 'IPv6 rollback failed'
[[ ! -f "$IPV6_SYSCTL_FILE" ]] || fail 'IPv6 persisted config not restored'
echo 'PASS: IPv6 owned sysctl and runtime rollback'

assert netg_gai_set_safe v4 >/dev/null
GAI_TX="$(basename "$(lasttx)")"
grep -q '::ffff:0:0/96 100' "$GAI_CONF" || fail 'gai not changed'
assert netg_tx_rollback_now "$GAI_TX" >/dev/null
grep -q 'old gai config' "$GAI_CONF" || fail 'gai not restored'
echo 'PASS: glibc precedence managed rollback'

is_resolved_active(){ return 0; }
assert netg_dns_set_safe quad9 eth0 >/dev/null
R_TX="$(basename "$(lasttx)")"
assert netg_tx_rollback_now "$R_TX" >/dev/null
[[ "$(cat "$FAKE_STATE/dns")" == '9.9.9.9' ]] || fail 'resolvectl prior per-link DNS not restored'
echo 'PASS: systemd-resolved per-link previous override restored'

# Same SSH session should be insufficient unless explicitly in test mode.
NETG_TEST_MODE=0
export SSH_CONNECTION='198.51.100.5 4567 192.0.2.10 22'
assert netg_mtu_set_safe eth0 1492 >/dev/null
SESS_TX="$(basename "$(lasttx)")"
if netg_tx_confirm "$SESS_TX" >/dev/null 2>&1; then fail 'confirmed in original session'; fi
export SSH_CONNECTION='198.51.100.5 7890 192.0.2.10 22'
assert netg_tx_confirm "$SESS_TX" >/dev/null
[[ "$(cat "$FAKE_STATE/mtu.eth0")" == 1492 ]] || fail 'confirmed change lost'
echo 'PASS: original-session confirmation rejected; distinct session accepted'

# Refuse symlink target, and leave original untouched.
ln -s "$TMP/etc/resolv.conf" "$TMP/etc/linked-resolv.conf"
NETG_RESOLV_PATH="$TMP/etc/linked-resolv.conf"
is_resolved_active(){ return 1; }
if netg_dns_set_safe google eth0 2>/dev/null; then fail 'symlink target overwritten'; fi
[[ "$(grep -c nameserver "$TMP/etc/resolv.conf")" == 3 ]] || fail 'symlink target unexpectedly altered'
echo 'PASS: resolver symlink guarded'

# A partial sysctl failure should restore the already modified keys.
NETG_TEST_MODE=1
export FAKE_SYSCTL_FAIL_KEY=default
if netg_ipv6_toggle_safe disable >/dev/null 2>&1; then fail 'accepted partial IPv6 sysctl failure'; fi
[[ "$(cat "$FAKE_STATE/v6-all")" == 0 ]] || fail 'partial sysctl failure did not restore all flag'
[[ "$(cat "$FAKE_STATE/v6-default")" == 0 ]] || fail 'partial sysctl failure changed default flag'
unset FAKE_SYSCTL_FAIL_KEY
echo 'PASS: partially failed IPv6 modification gets rolled back'

is_systemd(){ return 1; }
if netg_mtu_set_safe eth0 1484 >/dev/null 2>&1; then fail 'non-systemd operation allowed'; fi
[[ "$(cat "$FAKE_STATE/mtu.eth0")" == 1492 ]] || fail 'non-systemd refused but still changed MTU'
echo 'PASS: unsupported init fails closed'

echo 'ALL NETWORK GUARD TESTS PASS' 
