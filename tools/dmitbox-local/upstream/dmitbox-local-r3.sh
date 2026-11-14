#!/usr/bin/env bash
set -euo pipefail
IFS=$'\n\t'

SCRIPT_NAME="dmitbox.sh"
SCRIPT_VERSION="2026.11.13-local-r3"

# managed files
TUNE_SYSCTL_FILE="/etc/sysctl.d/99-dmit-tcp-tune.conf"
DMIT_TCP_DEFAULT_FILE="/etc/sysctl.d/99-dmit-tcp-dmitdefault.conf"
IPV6_SYSCTL_FILE="/etc/sysctl.d/99-dmit-ipv6.conf"
IPV6_FIX_SYSCTL_FILE="/etc/sysctl.d/99-dmit-ipv6-fix.conf"
GAI_CONF="/etc/gai.conf"
BACKUP_BASE="/root/dmit-backup"
TCP_SYSCTL_BACKUP="${BACKUP_BASE}/tcp-sysctl.orig"
IPV6_SYSCTL_BACKUP="${BACKUP_BASE}/ipv6-sysctl.orig"

# DNS backup
RESOLV_BACKUP="${BACKUP_BASE}/resolv.conf.orig"

# SSH backup & drop-in
SSH_ORIG_TGZ="${BACKUP_BASE}/ssh-orig.tgz"
SSH_DROPIN_ORIG_MARKER="${BACKUP_BASE}/ssh-dropin-existed.orig"
SSH_DROPIN_DIR="/etc/ssh/sshd_config.d"
SSH_DROPIN_FILE="${SSH_DROPIN_DIR}/00-dmitbox.conf"
SSH_LEGACY_DROPIN_FILE="${SSH_DROPIN_DIR}/99-dmitbox.conf"
SSH_KEYS_BACKUP_DIR="${BACKUP_BASE}/ssh-keys"
SSH_EPHEMERAL_KEY_BASE="/tmp"

# IPv6 pool + persist
IPV6_POOL_CONF="/etc/dmit-ipv6-pool.conf"
IPV6_POOL_SERVICE="/etc/systemd/system/dmit-ipv6-pool.service"

# IPv6 random outbound (nftables NAT66)
IPV6_RAND_CONF="/etc/dmit-ipv6-rand.conf"
IPV6_RAND_NFT="/etc/nftables.d/dmitbox-ipv6-rand.nft"
IPV6_RAND_SERVICE="/etc/systemd/system/dmit-ipv6-rand.service"

# system maintenance
SWAP_FILE="/swapfile"
SWAP_CONF="/etc/dmitbox-swap.conf"
SWAPPINESS_FILE="/etc/sysctl.d/99-dmitbox-swappiness.conf"
FSTAB_SWAP_BACKUP="${BACKUP_BASE}/fstab.before-dmitbox-swap"
FAIL2BAN_JAIL_FILE="/etc/fail2ban/jail.d/99-dmitbox-sshd.local"
FAIL2BAN_BACKUP="${BACKUP_BASE}/fail2ban-sshd.orig"
FAIL2BAN_ORIG_MARKER="${BACKUP_BASE}/fail2ban-sshd-existed.orig"

# common firewall management (only records rules created by this script)
COMMON_FIREWALL_REGISTRY="/etc/dmitbox-firewall.rules"
COMMON_FIREWALL_MARKER="# managed by dmitbox.sh - common firewall rules"
COMMON_FIREWALL_CREATED=0
COMMON_FIREWALL_BACKEND=""
COMMON_FIREWALL_BACKUP_DIR="${BACKUP_BASE}/firewall"
COMMON_FIREWALL_LAST_BACKUP=""

# Runtime detection (having systemctl installed does not mean systemd is PID 1)
SYSTEMD_RUNTIME_DIR="/run/systemd/system"

RUN_MODE="${RUN_MODE:-menu}" # menu | cli

# colors (no red)
c_reset="\033[0m"
c_dim="\033[2m"
c_bold="\033[1m"
c_green="\033[32m"
c_yellow="\033[33m"
c_cyan="\033[36m"
c_white="\033[37m"
MENU_AFTER_HEADER=0

if [[ -n "${NO_COLOR:-}" || "${TERM:-}" == "dumb" ]]; then
  c_reset=""
  c_dim=""
  c_bold=""
  c_green=""
  c_yellow=""
  c_cyan=""
  c_white=""
fi

ok()   { echo -e "${c_green}✔${c_reset} $*"; }
info() { echo -e "${c_cyan}➜${c_reset} $*"; }
warn() { echo -e "${c_yellow}⚠${c_reset} $*"; }

need_root() {
  if [[ "${EUID}" -ne 0 ]]; then
    warn "请用 root 运行：sudo bash ${SCRIPT_NAME}"
    exit 1
  fi
}

have_cmd() { command -v "$1" >/dev/null 2>&1; }
ts_now() { date +"%Y%m%d-%H%M%S"; }
ensure_dir() { mkdir -p "$1"; }

command_with_timeout() {
  local seconds="${1:-8}"
  shift || return 1
  is_uint_in_range "$seconds" 1 3600 || seconds=8
  if have_cmd timeout; then
    timeout "$seconds" "$@"
  else
    "$@"
  fi
}

has_tty() {
  [[ -r /dev/tty ]] && ( : </dev/tty ) 2>/dev/null
}

is_uint_in_range() {
  local value="${1:-}" min="${2:-0}" max="${3:-2147483647}"
  [[ "$value" =~ ^[0-9]{1,10}$ ]] || return 1
  value=$((10#$value))
  (( value >= min && value <= max ))
}

valid_username() {
  local user="${1:-}"
  [[ "$user" =~ ^[a-z_][a-z0-9_-]{0,31}$ ]]
}

read_tty() {
  local __var="$1" __prompt="$2" __default="${3:-}"
  local __val=""
  if [[ "$__prompt" == "请输入编号 > " ]]; then
    printf -v __prompt '%b' "  ${c_bold}${c_white}请选择${c_reset} ${c_cyan}›${c_reset} "
  fi
  if has_tty; then
    read -r -p "$__prompt" __val </dev/tty || true
  else
    read -r -p "$__prompt" __val || true
  fi
  __val="${__val:-$__default}"
  printf -v "$__var" "%s" "$__val"
}

read_tty_secret() {
  local __var="$1" __prompt="$2"
  local __val=""
  if has_tty; then
    read -r -s -p "$__prompt" __val </dev/tty || true
    echo >&2 || true
  else
    read -r -s -p "$__prompt" __val || true
    echo >&2 || true
  fi
  printf -v "$__var" "%s" "$__val"
}

soft_clear() {
  printf "\033[2J\033[H" 2>/dev/null || true
  printf "\033[3J" 2>/dev/null || true
  if have_cmd clear; then clear >/dev/null 2>&1 || true; fi
}

pause_for_return() {
  local msg="${1:-↩ 按回车返回...}" answer=""
  [[ "$RUN_MODE" == "menu" ]] || return 0
  echo
  printf "%s" "$msg"
  # read_tty() always prefers /dev/tty.  Keep pause prompts on the same input
  # source so pipe/process-substitution launch methods cannot consume Enter
  # from a different stdin stream.
  if has_tty; then
    IFS= read -r answer </dev/tty || true
  elif [[ -t 0 ]]; then
    IFS= read -r answer || true
  else
    sleep 2
  fi
  echo
}

pause_up() {
  pause_for_return "↩ 按回车返回..."
}

pause_main() {
  pause_for_return "↩ 按回车继续..."
}





write_file() {
  local path="${1:?missing path}" content="${2-}" requested_mode="${3:-}"
  local dir="" tmp="" mode="0644"
  dir="$(dirname -- "$path")" || return 1
  [[ ! -L "$path" ]] || { warn "拒绝覆盖符号链接：$path"; return 1; }
  if [[ -e "$path" && ! -f "$path" ]]; then
    warn "目标不是普通文件：$path"
    return 1
  fi
  mkdir -p -- "$dir" || return 1
  if [[ -n "$requested_mode" ]]; then
    [[ "$requested_mode" =~ ^0?[0-7]{3}$ ]] || return 1
    mode="$requested_mode"
  elif [[ -f "$path" ]]; then
    mode="$(stat -c '%a' -- "$path" 2>/dev/null || echo 644)"
    [[ "$mode" =~ ^[0-7]{3,4}$ ]] || mode=644
  fi
  tmp="$(mktemp "${dir}/.$(basename -- "$path").XXXXXX")" || return 1
  if ! printf '%s\n' "$content" > "$tmp" || ! chmod "$mode" "$tmp" || ! mv -f -- "$tmp" "$path"; then
    rm -f -- "$tmp" 2>/dev/null || true
    return 1
  fi
}

sysctl_apply_all() { sysctl --system >/dev/null 2>&1 || true; }

# ---------------- pkg helper ----------------
run_with_spinner() {
  # usage: run_with_spinner "title" cmd...
  local title="$1"; shift
  local log=""
  log="/tmp/dmitbox-$(ts_now).log"

  info "$title"
  echo -e "${c_dim}  处理中，请稍候；需要时可按 Ctrl+C 中断。详细日志：${log}${c_reset}"

  ("$@") >"$log" 2>&1 &
  local pid=$!
  local spin="|/-\\"
  local i=0
  while kill -0 "$pid" >/dev/null 2>&1; do
    local j=$(( i % 4 ))
    # shellcheck disable=SC2059
    printf "\r${c_dim}…安装/配置进行中 %c  (log: %s)${c_reset}" "${spin:j:1}" "$log"
    sleep 0.2
    i=$((i+1))
  done
  local rc=0
  wait "$pid" || rc=$?
  printf "\r\033[K" || true
  if [[ $rc -ne 0 ]]; then
    warn "命令返回非 0（rc=$rc）。最近日志如下："
    tail -n 40 "$log" 2>/dev/null || true
  else
    ok "完成"
  fi
  return $rc
}

pkg_install() {
  local -a pkgs=("$@")
  local quiet=1 rc=0
  (( ${#pkgs[@]} > 0 )) || return 0
  [[ "${RUN_MODE:-menu}" == "menu" ]] && quiet=0
  export DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=a

  if have_cmd apt-get; then
    if (( quiet == 0 )); then
      info "正在安装：${pkgs[*]}"
      run_with_spinner "apt-get update" apt-get -o DPkg::Lock::Timeout=30 -y update || \
        warn "索引更新失败，尝试使用现有软件包索引继续安装"
      run_with_spinner "apt-get install ${pkgs[*]}" \
        apt-get -o DPkg::Lock::Timeout=30 -y \
          -o Dpkg::Options::=--force-confdef \
          -o Dpkg::Options::=--force-confold install "${pkgs[@]}" || rc=$?
    else
      apt-get -o DPkg::Lock::Timeout=30 -qq update >/dev/null 2>&1 || true
      apt-get -o DPkg::Lock::Timeout=30 -y install "${pkgs[@]}" >/dev/null 2>&1 || rc=$?
    fi
  elif have_cmd dnf; then
    (( quiet == 1 )) || info "正在安装：${pkgs[*]}"
    if (( quiet == 1 )); then dnf -y install "${pkgs[@]}" >/dev/null 2>&1 || rc=$?;
    else dnf -y install "${pkgs[@]}" || rc=$?; fi
  elif have_cmd yum; then
    (( quiet == 1 )) || info "正在安装：${pkgs[*]}"
    if (( quiet == 1 )); then yum -y install "${pkgs[@]}" >/dev/null 2>&1 || rc=$?;
    else yum -y install "${pkgs[@]}" || rc=$?; fi
  elif have_cmd apk; then
    (( quiet == 1 )) || info "正在安装：${pkgs[*]}"
    if (( quiet == 1 )); then apk add --no-cache "${pkgs[@]}" >/dev/null 2>&1 || rc=$?;
    else apk add --no-cache "${pkgs[@]}" || rc=$?; fi
  else
    warn "未识别包管理器：请手动安装 ${pkgs[*]}"
    return 127
  fi

  if (( rc != 0 )); then
    warn "软件安装失败（exit=${rc}）：${pkgs[*]}"
    return "$rc"
  fi
  return 0
}

# ---------------- helpers ----------------
ipv6_status() {
  local a d
  a="$(sysctl -n net.ipv6.conf.all.disable_ipv6 2>/dev/null || echo "N/A")"
  d="$(sysctl -n net.ipv6.conf.default.disable_ipv6 2>/dev/null || echo "N/A")"
  echo "all=$a default=$d"
}

has_ipv6_global_addr() { ip -6 addr show scope global 2>/dev/null | grep -q "inet6 "; }
has_ipv6_default_route() { ip -6 route show default 2>/dev/null | grep -q "^default "; }

libc_kind() {
  if have_cmd getconf && getconf GNU_LIBC_VERSION >/dev/null 2>&1; then echo "glibc"; return 0; fi
  if have_cmd ldd && ldd --version 2>&1 | head -n 1 | grep -qi musl; then echo "musl"; return 0; fi
  if have_cmd ldd && ldd --version 2>&1 | grep -qi "glibc"; then echo "glibc"; return 0; fi
  echo "unknown"
}

is_systemd() { have_cmd systemctl && [[ -d "$SYSTEMD_RUNTIME_DIR" ]]; }
is_resolved_active() { is_systemd && systemctl is-active --quiet systemd-resolved 2>/dev/null; }


dns_resolve_ok() {
  have_cmd getent && command_with_timeout 6 getent hosts example.com >/dev/null 2>&1
}

human_bytes() {
  local bytes="${1:-0}"
  [[ "$bytes" =~ ^[0-9]+$ ]] || bytes="0"
  if have_cmd numfmt; then
    numfmt --to=iec-i --suffix=B "$bytes" 2>/dev/null || echo "${bytes} B"
  else
    awk -v b="$bytes" 'BEGIN {
      split("B KiB MiB GiB TiB", u, " "); i=1;
      while (b >= 1024 && i < 5) { b /= 1024; i++ }
      printf "%.1f %s", b, u[i]
    }'
  fi
}

os_pretty_name() {
  if [[ -r /etc/os-release ]]; then
    (
      # shellcheck disable=SC1091
      . /etc/os-release
      echo "${PRETTY_NAME:-${NAME:-Linux}}"
    )
  else
    echo "Linux"
  fi
}

confirm_word() {
  local expected="$1" prompt="$2" answer=""
  read_tty answer "$prompt" ""
  [[ "$answer" == "$expected" ]]
}

# ---------------- banner ----------------
menu_width() {
  local columns="${DMITBOX_MENU_COLUMNS:-${COLUMNS:-}}" width=0
  if ! is_uint_in_range "$columns" 20 1000; then
    columns="$(tput cols 2>/dev/null || true)"
  fi
  if ! is_uint_in_range "$columns" 20 1000; then
    columns="$(stty size </dev/tty 2>/dev/null | awk '{print $2}' || true)"
  fi
  is_uint_in_range "$columns" 20 1000 || columns=80
  width=$((columns - 4))
  (( width < 56 )) && width=56
  (( width > 92 )) && width=92
  printf '%s\n' "$width"
}

menu_repeat() {
  local character="${1:-─}" count="${2:-0}" output="" i=0
  is_uint_in_range "$count" 0 1000 || count=0
  for ((i=0; i<count; i++)); do output+="$character"; done
  printf '%s' "$output"
}

menu_display_width() {
  local text="${1:-}" clean="" width=""
  clean="$(printf '%b' "$text" | sed -E $'s/\x1B\\[[0-9;]*[[:alpha:]]//g')"
  width="$( (export LC_ALL=C.UTF-8; printf '%s' "$clean" | wc -L) 2>/dev/null || true)"
  if [[ "$width" =~ ^[0-9]+$ ]]; then
    printf '%s\n' "$width"
  else
    printf '%s\n' "${#clean}"
  fi
}

menu_wrap_segments() {
  local text="${1:-}" max_width="${2:-50}" rest="" segment="" line="" candidate=""
  is_uint_in_range "$max_width" 10 1000 || max_width=50
  if (( $(menu_display_width "$text") <= max_width )); then
    printf '%s\n' "$text"
    return 0
  fi
  rest="$text"
  while [[ "$rest" == *" · "* ]]; do
    segment="${rest%% · *}"
    rest="${rest#* · }"
    candidate="$segment"
    [[ -n "$line" ]] && candidate="${line} · ${segment}"
    if [[ -n "$line" ]] && (( $(menu_display_width "$candidate") > max_width )); then
      printf '%s\n' "$line"
      line="$segment"
    else
      line="$candidate"
    fi
  done
  candidate="$rest"
  [[ -n "$line" ]] && candidate="${line} · ${rest}"
  if [[ -n "$line" ]] && (( $(menu_display_width "$candidate") > max_width )); then
    printf '%s\n' "$line"
    printf '%s\n' "$rest"
  else
    printf '%s\n' "$candidate"
  fi
}

menu_rule() {
  local width=""
  width="$(menu_width)"
  printf "  ${c_dim}%s${c_reset}\n" "$(menu_repeat '─' "$width")"
}

menu_box_top() {
  local width="$1" left='╭─ DMIT BOX ' right=" v${SCRIPT_VERSION} ─╮" fill=0
  fill=$((width - $(menu_display_width "$left") - $(menu_display_width "$right")))
  (( fill < 1 )) && fill=1
  printf "  ${c_cyan}${c_bold}╭─ DMIT BOX %s v%s ─╮${c_reset}\n" \
    "$(menu_repeat '─' "$fill")" "$SCRIPT_VERSION"
}

menu_box_rule() {
  local width="$1" left="${2:-├}" right="${3:-┤}"
  printf "  ${c_cyan}%s%s%s${c_reset}\n" "$left" "$(menu_repeat '─' "$((width - 2))")" "$right"
}

menu_box_row() {
  local text="$1" width="$2" text_width=0 padding=0
  text_width="$(menu_display_width "$text")"
  padding=$((width - text_width - 6))
  if (( padding < 0 )); then
    printf "  ${c_cyan}│${c_reset}  "
    printf '%b\n' "$text"
    return 0
  fi
  printf "  ${c_cyan}│${c_reset}  "
  printf '%b' "$text"
  printf '%*s' "$padding" ''
  printf "  ${c_cyan}│${c_reset}\n"
}

menu_compact_uptime() {
  local seconds="${1:-}" days=0 hours=0 minutes=0
  [[ "$seconds" =~ ^[0-9]+$ ]] || { printf 'N/A\n'; return 1; }
  seconds=$((10#$seconds))
  days=$((seconds / 86400))
  hours=$(((seconds % 86400) / 3600))
  minutes=$(((seconds % 3600) / 60))
  if (( days > 0 )); then
    printf '%s天%s小时\n' "$days" "$hours"
  elif (( hours > 0 )); then
    printf '%s小时%s分\n' "$hours" "$minutes"
  elif (( minutes > 0 )); then
    printf '%s分钟\n' "$minutes"
  else
    printf '<1分钟\n'
  fi
}

menu_runtime_status() {
  local layout_width="${1:-$(menu_width)}"
  local uptime_seconds="" uptime_text="N/A" load_one="N/A" cpu_count="N/A"
  local mem_percent="N/A" disk_percent="N/A"
  local load_display="N/A" mem_display="N/A" disk_display="N/A"
  local load_color="$c_green" mem_color="$c_green" disk_color="$c_green"

  uptime_seconds="$(awk '{print int($1)}' /proc/uptime 2>/dev/null || true)"
  if [[ "$uptime_seconds" =~ ^[0-9]+$ ]]; then
    uptime_text="$(menu_compact_uptime "$uptime_seconds")"
  fi

  load_one="$(awk '{print $1}' /proc/loadavg 2>/dev/null || true)"
  [[ "$load_one" =~ ^[0-9]+([.][0-9]+)?$ ]] || load_one="N/A"
  cpu_count="$(getconf _NPROCESSORS_ONLN 2>/dev/null || nproc 2>/dev/null || true)"
  is_uint_in_range "$cpu_count" 1 1048576 || cpu_count="N/A"

  mem_percent="$(awk '
    /^MemTotal:/ {total=$2}
    /^MemAvailable:/ {available=$2}
    /^MemFree:/ {free_mem=$2}
    /^Buffers:/ {buffers=$2}
    /^Cached:/ {cached=$2}
    END {
      if (!available) available=free_mem+buffers+cached
      if (total > 0) printf "%.0f", (total-available)*100/total
    }
  ' /proc/meminfo 2>/dev/null || true)"
  is_uint_in_range "$mem_percent" 0 100 || mem_percent="N/A"

  disk_percent="$(LC_ALL=C df -P / 2>/dev/null | awk 'NR == 2 {gsub(/%/, "", $5); print $5}' || true)"
  is_uint_in_range "$disk_percent" 0 100 || disk_percent="N/A"

  if [[ "$load_one" != "N/A" && "$cpu_count" != "N/A" ]] && \
     awk -v load="$load_one" -v cpus="$cpu_count" 'BEGIN {exit !(load >= cpus)}'; then
    load_color="$c_yellow"
  fi
  if [[ "$load_one" != "N/A" && "$cpu_count" != "N/A" ]]; then
    load_display="${load_one}/${cpu_count}核"
  fi
  if [[ "$mem_percent" != "N/A" ]] && (( mem_percent >= 80 )); then mem_color="$c_yellow"; fi
  if [[ "$disk_percent" != "N/A" ]] && (( disk_percent >= 85 )); then disk_color="$c_yellow"; fi
  [[ "$mem_percent" == "N/A" ]] || mem_display="${mem_percent}%"
  [[ "$disk_percent" == "N/A" ]] || disk_display="${disk_percent}%"

  if (( layout_width < 68 )); then
    printf "${c_dim}运行${c_reset} ${c_cyan}%s${c_reset}" "$uptime_text"
    printf "  ${c_dim}· 负载${c_reset} ${load_color}%s${c_reset}\n" "$load_display"
    printf "${c_dim}内存${c_reset} ${mem_color}%s${c_reset}" "$mem_display"
    printf "  ${c_dim}· 根盘${c_reset} ${disk_color}%s${c_reset}\n" "$disk_display"
  else
    printf "${c_dim}运行${c_reset} ${c_cyan}%s${c_reset}" "$uptime_text"
    printf "  ${c_dim}· 负载${c_reset} ${load_color}%s${c_reset}" "$load_display"
    printf "  ${c_dim}· 内存${c_reset} ${mem_color}%s${c_reset}" "$mem_display"
    printf "  ${c_dim}· 根盘${c_reset} ${disk_color}%s${c_reset}\n" "$disk_display"
  fi
}

menu_header() {
  local title="$1" subtitle="${2:-}" width="" status_line="" subtitle_line=""
  soft_clear
  width="$(menu_width)"
  menu_box_top "$width"
  menu_box_row "${c_bold}${c_white}${title}${c_reset}" "$width"
  if [[ -n "$subtitle" ]]; then
    while IFS= read -r subtitle_line; do
      [[ -n "$subtitle_line" ]] && menu_box_row "${c_dim}${subtitle_line}${c_reset}" "$width"
    done < <(menu_wrap_segments "$subtitle" "$((width - 6))")
  fi
  menu_box_rule "$width"
  while IFS= read -r status_line; do
    [[ -n "$status_line" ]] && menu_box_row "$status_line" "$width"
  done < <(menu_runtime_status "$width")
  menu_box_rule "$width"
  menu_box_rule "$width" '╰' '╯'
  MENU_AFTER_HEADER=1
}

menu_section() {
  local title="$1" width="" used=0 fill=0
  width="$(menu_width)"
  used=$((4 + $(menu_display_width "$title")))
  fill=$((width - used))
  (( fill < 3 )) && fill=3
  echo
  MENU_AFTER_HEADER=0
  printf "  ${c_cyan}${c_bold}▸ %s${c_reset} ${c_dim}%s${c_reset}\n" \
    "$title" "$(menu_repeat '─' "$fill")"
}

menu_item() {
  local key="$1" title="$2" detail="${3:-}" width="" item_width=0
  if (( MENU_AFTER_HEADER == 1 )); then
    echo
    MENU_AFTER_HEADER=0
  fi
  width="$(menu_width)"
  item_width=$((10 + $(menu_display_width "$title") + $(menu_display_width "$detail")))
  printf "  ${c_cyan}${c_bold}[%2s]${c_reset}  ${c_bold}${c_white}%s${c_reset}" "$key" "$title"
  if [[ -n "$detail" ]] && (( item_width > width )); then
    printf "\n        ${c_dim}└─ %s${c_reset}" "$detail"
  elif [[ -n "$detail" ]]; then
    printf "  ${c_dim}› %s${c_reset}" "$detail"
  fi
  printf "\n"
}

menu_back_item() {
  echo
  menu_item "0" "返回上级"
  menu_rule
}

banner() {
  menu_header "DMITBox 本地工具箱" "网络配置 · 主机安全 · 诊断 · 系统维护（无遥测）"
}

# ---------------- 环境快照 ----------------
env_snapshot() {
  ( umask 077; mkdir -p -- "$BACKUP_BASE" ) || return 1
  local bdir=""
  bdir="${BACKUP_BASE}/snapshot-$(ts_now)"
  ( umask 077; mkdir -p -- "$bdir"; chmod 700 -- "$bdir" ) || return 1
  info "环境快照 → ${bdir}"

  for p in /etc/sysctl.conf /etc/sysctl.d /etc/gai.conf /etc/modprobe.d /etc/default/grub /etc/network /etc/netplan /etc/systemd/network /etc/resolv.conf /etc/ssh/sshd_config /etc/ssh/sshd_config.d /etc/ufw /etc/firewalld "$COMMON_FIREWALL_REGISTRY"; do
    if [[ -e "$p" ]]; then
      mkdir -p "${bdir}$(dirname "$p")"
      cp -a "$p" "${bdir}${p}" 2>/dev/null || true
    fi
  done

  {
    echo "time=$(date)"
    echo "uname=$(uname -a)"
    echo "libc=$(libc_kind)"
    echo "iface=$(default_iface 2>/dev/null || true)"
    echo "timezone=$( (timedatectl show -p Timezone --value 2>/dev/null || cat /etc/timezone 2>/dev/null || true) )"
    echo "time_sync=$(time_sync_status_text 2>/dev/null || echo unavailable)"
    echo "ipv6_sysctl=$(ipv6_status)"
    echo
    echo "== ip -br a =="; ip -br a 2>/dev/null || true
    echo
    echo "== ip -4 route =="; ip -4 route 2>/dev/null || true
    echo
    echo "== ip -6 addr =="; ip -6 addr show 2>/dev/null || true
    echo
    echo "== ip -6 route =="; ip -6 route show 2>/dev/null || true
    echo
    echo "== resolv.conf =="; sed -n '1,80p' /etc/resolv.conf 2>/dev/null || true
    echo
    echo "== qdisc =="; tc qdisc show 2>/dev/null || true
    echo
    echo "== bbr =="; cat /proc/sys/net/ipv4/tcp_available_congestion_control 2>/dev/null || true
    echo
    echo "== sshd -T (if available) =="; (sshd -T 2>/dev/null | sed -n '1,220p' || true)
    echo
    echo "== firewall managers =="
    (LC_ALL=C ufw status verbose 2>/dev/null || true)
    (firewall-cmd --state 2>/dev/null || true)
    (firewall-cmd --get-active-zones 2>/dev/null || true)
    echo
    echo "== nftables tables =="; (nft list tables 2>/dev/null || true)
    echo
    echo "== iptables policies =="; (iptables -S 2>/dev/null | sed -n '1,160p' || true)
  } > "${bdir}/state.txt"

  ok "已保存：${bdir}"
  echo "查看：less -S ${bdir}/state.txt"
}

# ---------------- 时区：中国 ----------------
set_timezone_china() {
  info "时区：设置为中国（Asia/Shanghai）"
  pkg_install tzdata

  if have_cmd timedatectl; then
    timedatectl set-timezone Asia/Shanghai >/dev/null 2>&1 || true
  fi

  if [[ -e /usr/share/zoneinfo/Asia/Shanghai ]]; then
    ln -sf /usr/share/zoneinfo/Asia/Shanghai /etc/localtime || true
    echo "Asia/Shanghai" > /etc/timezone 2>/dev/null || true
  fi

  local tz
  tz="$( (timedatectl show -p Timezone --value 2>/dev/null || cat /etc/timezone 2>/dev/null || echo "unknown") )"
  ok "当前时区：$tz"
}

# ---------------- 重启网络服务 ----------------
_ipv6_find_disable_sources() {
  echo -e "${c_yellow}${c_bold}--- IPv6 开启失败排查 ---${c_reset}"
  echo -e "${c_dim}[启动参数]${c_reset} $(cat /proc/cmdline 2>/dev/null || true)"
  if grep -qw "ipv6.disable=1" /proc/cmdline 2>/dev/null; then
    warn "发现 ipv6.disable=1：必须改 GRUB/引导并重启"
  fi
  echo
  echo -e "${c_dim}[sysctl 覆盖]${c_reset}"
  (grep -RIn --line-number -E 'net\.ipv6\.conf\.(all|default|lo)\.disable_ipv6[[:space:]]*=[[:space:]]*1' \
    /etc/sysctl.conf /etc/sysctl.d 2>/dev/null || true) | sed -n '1,140p'
  echo
  echo -e "${c_dim}[模块黑名单]${c_reset}"
  (grep -RIn --line-number -E '^[[:space:]]*blacklist[[:space:]]+ipv6|^[[:space:]]*install[[:space:]]+ipv6[[:space:]]+/bin/true' \
    /etc/modprobe.d 2>/dev/null || true) | sed -n '1,140p'
  echo -e "${c_yellow}${c_bold}------------------------${c_reset}"
}


gai_backup_once() {
  local orig="${BACKUP_BASE}/gai.conf.orig"
  if [[ -e "$orig" || -L "$orig" ]]; then
    [[ -f "$orig" && ! -L "$orig" ]] || return 1
    return 0
  fi
  [[ -f "$GAI_CONF" && ! -L "$GAI_CONF" ]] || {
    warn "原始 gai.conf 不存在或不可安全备份；拒绝猜测其原值"
    return 1
  }
  ensure_dir "$BACKUP_BASE" || return 1
  cp -p -- "$GAI_CONF" "$orig" || return 1
  chmod 600 "$orig" || return 1
}

# Never download or execute tcpfit; detect only incompatible existing configurations.
external_tcp_tuning_active() {
  [[ -s /etc/sysctl.d/99-tcpfit.conf || -s /etc/systemd/system/tcpfit-qdisc.service ]]
}

tcp_available_congestion_controls() {
  cat /proc/sys/net/ipv4/tcp_available_congestion_control 2>/dev/null
}

bbr_check() {
  echo "================ BBR 检测 ================"
  echo "kernel=$(uname -r)"
  local avail cur
  avail="$(tcp_available_congestion_controls 2>/dev/null || true)"
  cur="$(sysctl -n net.ipv4.tcp_congestion_control 2>/dev/null || echo "N/A")"
  echo "当前=${cur}"
  echo "可用=${avail:-N/A}"
  if echo " $avail " | grep -q " bbr "; then
    ok "支持 bbr（实现取决于内核）"
  else
    warn "未看到 bbr（可能内核不含/模块不可用）"
  fi
  echo "=========================================="
}

tcp_managed_keys() {
  printf '%s\n' \
    net.core.default_qdisc \
    net.ipv4.tcp_congestion_control \
    net.core.netdev_max_backlog \
    net.core.somaxconn \
    net.ipv4.tcp_max_syn_backlog \
    net.core.rmem_max \
    net.core.wmem_max \
    net.ipv4.tcp_rmem \
    net.ipv4.tcp_wmem \
    net.ipv4.tcp_mtu_probing \
    net.ipv4.tcp_fastopen \
    net.ipv4.tcp_syncookies \
    net.ipv4.tcp_adv_win_scale \
    net.ipv4.tcp_sack \
    net.ipv4.tcp_timestamps \
    kernel.panic \
    vm.swappiness
}

tcp_runtime_snapshot_write() {
  local destination="$1" tmp="" key="" value="" saved=0
  tmp="${destination}.tmp.$$"
  ensure_dir "$(dirname "$destination")" || return 1
  : > "$tmp" || return 1

  while IFS= read -r key; do
    value="$(sysctl -n "$key" 2>/dev/null || true)"
    [[ -n "$value" ]] || continue
    printf '%s=%s\n' "$key" "$value" >> "$tmp" || {
      rm -f -- "$tmp" >/dev/null 2>&1 || true
      return 1
    }
    saved=$((saved + 1))
  done < <(tcp_managed_keys)

  if (( saved == 0 )); then
    rm -f -- "$tmp" >/dev/null 2>&1 || true
    return 1
  fi
  chmod 600 "$tmp" >/dev/null 2>&1 || true
  mv -f -- "$tmp" "$destination"
}

tcp_runtime_snapshot_restore() {
  local source="$1" line="" key="" value="" restored=0 failed=0
  [[ -s "$source" && ! -L "$source" ]] || return 1

  while IFS= read -r line; do
    [[ "$line" == *=* ]] || continue
    key="${line%%=*}"
    value="${line#*=}"
    tcp_managed_keys | grep -Fxq "$key" || continue
    if sysctl -w "${key}=${value}" >/dev/null 2>&1; then
      restored=$((restored + 1))
    else
      failed=$((failed + 1))
    fi
  done < "$source"

  (( restored > 0 )) || return 1
  (( failed == 0 )) || return 2
  return 0
}

tcp_backup_runtime_once() {
  [[ -s "$TCP_SYSCTL_BACKUP" ]] && return 0

  ensure_dir "$BACKUP_BASE"
  if ! tcp_runtime_snapshot_write "$TCP_SYSCTL_BACKUP"; then
    warn "未能读取当前 TCP/sysctl 参数，已取消修改"
    return 1
  fi
  ok "已备份修改前 TCP/sysctl 参数：$TCP_SYSCTL_BACKUP"
}

tcp_restore_runtime_backup() {
  [[ -s "$TCP_SYSCTL_BACKUP" ]] || return 1
  local rc=0
  tcp_runtime_snapshot_restore "$TCP_SYSCTL_BACKUP" || rc=$?
  return "$rc"
}

tcp_fixed_profile_content() {
  cat <<'EOF'
# managed by dmitbox.sh - fixed common TCP tuning
# Static compatibility profile: no bandwidth or RTT measurement.
net.core.default_qdisc = fq
net.ipv4.tcp_congestion_control = bbr

net.core.netdev_max_backlog = 16384
net.core.somaxconn = 8192
net.ipv4.tcp_max_syn_backlog = 8192

net.core.rmem_max = 67108864
net.core.wmem_max = 67108864
net.ipv4.tcp_rmem = 4096 87380 67108864
net.ipv4.tcp_wmem = 4096 65536 67108864

net.ipv4.tcp_mtu_probing = 1
net.ipv4.tcp_fastopen = 3
net.ipv4.tcp_syncookies = 1
EOF
}

tcp_value_normalize() {
  awk '{$1=$1; print}' <<< "${1:-}"
}

tcp_fixed_profile_keys() {
  tcp_fixed_profile_content | awk -F= '
    /^[[:space:]]*#/ || /^[[:space:]]*$/ {next}
    {
      key=$1
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", key)
      if (key != "") print key
    }'
}

tcp_fixed_runtime_verify() {
  local line="" key="" expected="" actual=""
  while IFS= read -r line; do
    [[ "$line" == *=* ]] || continue
    key="${line%%=*}"
    expected="${line#*=}"
    key="$(tcp_value_normalize "$key")"
    expected="$(tcp_value_normalize "$expected")"
    [[ -n "$key" ]] || continue
    actual="$(sysctl -n "$key" 2>/dev/null || true)"
    actual="$(tcp_value_normalize "$actual")"
    [[ -n "$actual" && "$actual" == "$expected" ]] || return 1
  done < <(tcp_fixed_profile_content | sed -E '/^[[:space:]]*(#|$)/d')
  return 0
}

tcp_fixed_status_text() {
  if external_tcp_tuning_active; then
    printf '未启用（检测到外部调优配置）\n'
  elif [[ -s "$TUNE_SYSCTL_FILE" ]]; then
    if tcp_fixed_runtime_verify; then
      printf '已应用且运行正常\n'
    else
      printf '已配置但运行值不一致\n'
    fi
  else
    printf '未应用\n'
  fi
}

tcp_fixed_preflight() {
  local available="" key="" unsupported=()
  if external_tcp_tuning_active; then
    warn "检测到已有第三方 TCP 调优配置，拒绝覆盖或叠加"
    info "请先自行检查并移除第三方配置；本工具不会接管其回滚"
    return 1
  fi
  have_cmd sysctl || { warn "系统缺少 sysctl，无法应用 TCP 参数"; return 1; }
  if [[ -L "$TUNE_SYSCTL_FILE" || -L "$DMIT_TCP_DEFAULT_FILE" ]]; then
    warn "检测到 TCP 配置路径是符号链接，为避免覆盖未知文件已取消"
    return 1
  fi
  if have_cmd modprobe; then
    modprobe tcp_bbr >/dev/null 2>&1 || true
    modprobe sch_fq >/dev/null 2>&1 || true
  fi
  available="$(tcp_available_congestion_controls 2>/dev/null || true)"
  case " ${available} " in
    *" bbr "*) ;;
    *)
      warn "当前内核未提供 BBR，固定通用调优已取消"
      info "可先使用【检测 BBR 支持】确认；本脚本不会为此替换第三方内核"
      return 1
      ;;
  esac
  while IFS= read -r key; do
    [[ -n "$key" ]] || continue
    sysctl -n "$key" >/dev/null 2>&1 || unsupported+=("$key")
  done < <(tcp_fixed_profile_keys)
  if (( ${#unsupported[@]} > 0 )); then
    warn "当前内核缺少固定配置所需参数，未写入任何配置："
    printf '    - %s\n' "${unsupported[@]}"
    return 1
  fi
  return 0
}

tcp_fixed_transaction_restore() {
  local rollback="$1" rc=0
  if [[ -f "$rollback/had-fixed" ]]; then
    cp -a "$rollback/fixed.conf" "$TUNE_SYSCTL_FILE" || rc=1
  else
    rm -f -- "$TUNE_SYSCTL_FILE" >/dev/null 2>&1 || rc=1
  fi
  if [[ -f "$rollback/had-dmit" ]]; then
    cp -a "$rollback/dmit.conf" "$DMIT_TCP_DEFAULT_FILE" || rc=1
  else
    rm -f -- "$DMIT_TCP_DEFAULT_FILE" >/dev/null 2>&1 || rc=1
  fi
  sysctl_apply_all
  tcp_runtime_snapshot_restore "$rollback/runtime.snapshot" >/dev/null 2>&1 || rc=1
  return "$rc"
}

tcp_fixed_apply() {
  local rollback="" candidate="" staged="" log="" ifc="" root_qdisc="" rc=0
  tcp_fixed_preflight || return 1

  if [[ -s "$TUNE_SYSCTL_FILE" ]] && \
     cmp -s "$TUNE_SYSCTL_FILE" <(tcp_fixed_profile_content) && \
     tcp_fixed_runtime_verify; then
    ok "固定通用调优已经正常生效，无需重复应用"
    return 0
  fi

  menu_section "固定配置预览"
  print_kv "拥塞控制 / 队列" "BBR / FQ"
  print_kv "TCP 缓冲上限" "64 MiB（固定值）"
  print_kv "连接队列" "somaxconn 8192 / SYN backlog 8192"
  print_kv "其他" "MTU 探测、TCP Fast Open、SYN Cookie"
  print_kv "固定状态" "$(tcp_fixed_status_text)"
  ifc="$(default_iface)"
  if have_cmd tc && [[ -n "$ifc" ]]; then
    root_qdisc="$(tc qdisc show dev "$ifc" 2>/dev/null | awk '/ root / {print $2; exit}' || true)"
    [[ -n "$root_qdisc" ]] && print_kv "当前出口队列" "${ifc} / ${root_qdisc}"
  fi
  warn "该模式不测速，也不会按带宽、RTT 或内存推导参数；适合作为快速兼容预设"
  info "不会覆盖当前网卡已有的 CAKE、HTB、TBF、NetEm 等手工 tc 规则"
  [[ -s "$DMIT_TCP_DEFAULT_FILE" ]] && info "现有 DMIT 默认参数将切换为固定通用参数"
  confirm_word "FIXED" "确认应用固定通用调优请输入 FIXED > " || { info "已取消"; return 0; }

  tcp_backup_runtime_once || return 1
  rollback="$(mktemp -d /tmp/dmitbox-tcp-fixed.XXXXXX)" || {
    warn "无法创建临时回滚点"
    return 1
  }
  candidate="${rollback}/candidate.conf"
  log="${rollback}/sysctl.log"
  tcp_fixed_profile_content > "$candidate" || {
    warn "无法生成固定调优配置"
    rm -rf -- "$rollback"
    return 1
  }
  chmod 600 "$candidate" >/dev/null 2>&1 || true
  if ! tcp_runtime_snapshot_write "$rollback/runtime.snapshot"; then
    warn "创建临时运行参数快照失败，未进行修改"
    rm -rf -- "$rollback"
    return 1
  fi
  if [[ -e "$TUNE_SYSCTL_FILE" ]]; then
    write_file "$rollback/had-fixed" 1
    cp -a "$TUNE_SYSCTL_FILE" "$rollback/fixed.conf" || rc=1
  fi
  if [[ -e "$DMIT_TCP_DEFAULT_FILE" ]]; then
    write_file "$rollback/had-dmit" 1
    cp -a "$DMIT_TCP_DEFAULT_FILE" "$rollback/dmit.conf" || rc=1
  fi
  if (( rc != 0 )); then
    warn "创建配置文件回滚点失败，未进行修改"
    rm -rf -- "$rollback"
    return 1
  fi

  ensure_dir "$(dirname "$TUNE_SYSCTL_FILE")" || rc=1
  staged="${TUNE_SYSCTL_FILE}.new.$$"
  if (( rc == 0 )); then
    cp "$candidate" "$staged" || rc=1
    chmod 644 "$staged" >/dev/null 2>&1 || rc=1
    mv -f -- "$staged" "$TUNE_SYSCTL_FILE" || rc=1
    rm -f -- "$DMIT_TCP_DEFAULT_FILE" >/dev/null 2>&1 || rc=1
  fi
  if (( rc == 0 )); then
    sysctl -p "$TUNE_SYSCTL_FILE" > "$log" 2>&1 || rc=$?
  fi
  if (( rc == 0 )) && ! tcp_fixed_runtime_verify; then
    rc=1
    printf '%s\n' '应用后运行值校验不一致' >> "$log"
  fi
  if (( rc != 0 )); then
    warn "固定通用调优应用失败，正在恢复修改前状态"
    tail -n 20 "$log" 2>/dev/null || true
    tcp_fixed_transaction_restore "$rollback" || \
      warn "自动回滚不完整，请进入【恢复修改前参数】再次恢复"
    rm -f -- "$staged" >/dev/null 2>&1 || true
    rm -rf -- "$rollback"
    return 1
  fi

  rm -rf -- "$rollback"
  ok "固定通用调优已应用并通过逐项运行值校验"
  info "持久配置：${TUNE_SYSCTL_FILE}"
  if [[ -n "$root_qdisc" && "$root_qdisc" != "fq" ]]; then
    info "当前网卡仍使用 ${root_qdisc}；脚本未覆盖已有活动 tc 队列"
  fi
  bbr_check
}

tcp_restore_default() {
  local restore_rc=0
  if external_tcp_tuning_active; then
    warn "检测到第三方 TCP 配置，拒绝覆盖"
    info "请先独立恢复第三方调优配置"
    return 1
  fi
  info "TCP：恢复脚本修改前的参数"
  if ! rm -f -- "$TUNE_SYSCTL_FILE" "$DMIT_TCP_DEFAULT_FILE" >/dev/null 2>&1; then
    warn "无法移除脚本管理的 TCP 持久配置，已停止恢复"
    return 1
  fi
  sysctl_apply_all
  tcp_restore_runtime_backup || restore_rc=$?
  case "$restore_rc" in
    0) ok "已恢复脚本首次修改前的 TCP/sysctl 参数" ;;
    2) warn "原始备份已读取，但部分参数当前内核不支持或恢复失败" ;;
    *)
      warn "未找到可用的原始参数备份：已移除持久调优文件并重新加载系统 sysctl"
      warn "不同发行版默认值不同，不再强行写入过时的 pfifo_fast；重启后由系统默认配置接管"
      ;;
  esac
  bbr_check
}

# ---------------- DNS / MTU 用户界面（调用统一事务引擎） ----------------
dns_switch_menu() {
  local ifc="" c=""
  ifc="$(default_iface)" || { warn "无法确定出口网卡，拒绝猜测 eth0"; return 1; }
  while true; do
    menu_header "DNS 管理（本地）" "出口网卡：${ifc} · 更改需在新 SSH 会话确认"
    menu_item 1 "Cloudflare" "1.1.1.1 / 1.0.0.1"
    menu_item 2 "Google" "8.8.8.8 / 8.8.4.4"
    menu_item 3 "Quad9" "9.9.9.9 / 149.112.112.112"
    menu_back_item
    read_tty c "请输入编号 > " ""
    case "$c" in
      1) netg_dns_set_safe cloudflare "$ifc" || true; pause_up ;;
      2) netg_dns_set_safe google "$ifc" || true; pause_up ;;
      3) netg_dns_set_safe quad9 "$ifc" || true; pause_up ;;
      0|q|Q) return 0 ;;
      *) warn "无效选项"; pause_up ;;
    esac
  done
}

mtu_current() {
  local ifc=""
  ifc="$(default_iface)" || return 1
  netg_link_mtu "$ifc"
}

ping_payload_ok_v4() {
  local host="$1" payload="$2"
  ping -4 -c 1 -W 1 -M "do" -s "$payload" "$host" >/dev/null 2>&1
}

mtu_probe_v4_value() {
  local host="1.1.1.1"
  if ! ping -4 -c 1 -W 1 "$host" >/dev/null 2>&1; then host="8.8.8.8"; fi
  if ! ping -4 -c 1 -W 1 "$host" >/dev/null 2>&1; then
    echo -e "${c_yellow}⚠ IPv4 ping 不通，无法探测 MTU（先检查网络）${c_reset}" >&2
    return 1
  fi

  echo -e "${c_cyan}➜${c_reset} MTU 探测：对 ${host} 做 DF 探测" >&2
  local lo=1200 hi=1472 mid best=0
  while [[ $lo -le $hi ]]; do
    mid=$(( (lo + hi) / 2 ))
    if ping_payload_ok_v4 "$host" "$mid"; then
      best="$mid"; lo=$((mid + 1))
    else
      hi=$((mid - 1))
    fi
  done

  if [[ "$best" -le 0 ]]; then
    echo -e "${c_yellow}⚠ 未探测到可用值${c_reset}" >&2
    return 1
  fi

  local mtu=$((best + 28))
  echo -e "${c_green}✔${c_reset} 推荐 MTU=${mtu}" >&2
  echo "$mtu"
}

mtu_menu() {
  local ifc="" mtu="" choice=""
  ifc="$(default_iface)" || { warn "无法识别出口网卡"; return 1; }
  while true; do
    menu_header "MTU 管理（受保护）" "运行时修改 + 独立超时回滚；不覆盖网络管理器持久配置"
    menu_item 1 "查看当前 MTU" "$(mtu_current)"
    menu_item 2 "探测建议 MTU" "只读 ping 探测，不修改"
    menu_item 3 "临时设置 MTU" "SHA-256 快照、新连接确认"
    menu_back_item
    read_tty choice "请输入编号 > " ""
    case "$choice" in
      1) mtu_current; pause_up ;;
      2) mtu_probe_v4_value || true; pause_up ;;
      3)
        read_tty mtu "输入 MTU（1280-9000）> " ""
        netg_mtu_set_safe "$ifc" "$mtu" || true
        pause_up ;;
      0|q|Q) return 0 ;;
      *) warn "无效选项"; pause_up ;;
    esac
  done
}

print_kv() { printf "%-20s %s\n" "$1" "$2"; }

health_check_core() {
  local ifc; ifc="$(default_iface 2>/dev/null || true)"
  local ipv6_sysctl; ipv6_sysctl="$(ipv6_status)"
  local v6_addr="NO" v6_route="NO" v4_route="NO" v6_route_test="NO" dns_ok="NO"

  has_ipv6_global_addr && v6_addr="YES"
  has_ipv6_default_route && v6_route="YES"
  ip -4 route get 1.1.1.1 >/dev/null 2>&1 && v4_route="YES"
  ip -6 route get 2606:4700:4700::1111 >/dev/null 2>&1 && v6_route_test="YES"
  dns_resolve_ok && dns_ok="YES"

  echo -e "${c_bold}${c_white}本地网络体检${c_reset}  ${c_dim}(接口: $ifc)${c_reset}"
  echo -e "${c_dim}----------------------------------------------${c_reset}"

  print_kv "IPv4 路由"       "$( [[ "$v4_route" == "YES" ]] && echo -e "${c_green}存在${c_reset}" || echo -e "${c_yellow}缺失${c_reset}" )"
  print_kv "DNS 解析"        "$( [[ "$dns_ok" == "YES" ]] && echo -e "${c_green}正常${c_reset}" || echo -e "${c_yellow}异常${c_reset}" )"
  print_kv "IPv6 sysctl 开关" "$ipv6_sysctl"
  print_kv "IPv6 公网地址"   "$( [[ "$v6_addr" == "YES" ]] && echo -e "${c_green}有${c_reset}" || echo -e "${c_yellow}无${c_reset}" )"
  print_kv "IPv6 默认路由"   "$( [[ "$v6_route" == "YES" ]] && echo -e "${c_green}有${c_reset}" || echo -e "${c_yellow}无${c_reset}" )"
  print_kv "IPv6 路由查询"    "$( [[ "$v6_route_test" == "YES" ]] && echo -e "${c_green}存在${c_reset}" || echo -e "${c_yellow}缺失${c_reset}" )"
  print_kv "当前 MTU"        "$(mtu_current || echo N/A)"
  echo -e "${c_dim}----------------------------------------------${c_reset}"

  if [[ "$dns_ok" != "YES" && "$v4_route" == "YES" ]]; then
    warn "像 DNS 问题：试试【DNS 切换】"
  fi
  if [[ "$v6_addr" == "NO" || "$v6_route" == "NO" ]]; then
    warn "IPv6 缺地址/路由：使用【网络安全工作台】检查 IPv6 诊断信息"
  fi
}

health_check_only() {
  health_check_core
  ok "体检完成（未改动任何配置）"
}

health_check_autofix() {
  local fixed=0
  health_check_core
  echo
  info "低风险修复：只刷新 resolved 缓存；IPv6 与路由修改必须显式确认"

  if ! has_ipv6_global_addr || ! has_ipv6_default_route; then
    warn "IPv6 地址或路由不完整：仅提示诊断，不自动改变 sysctl、RA 或重启网络服务"
    info "如需修改，请进入【IPv6 管理】使用定时回滚保护操作"
  fi

  if is_resolved_active && have_cmd resolvectl; then
    info "刷新 systemd-resolved DNS 缓存"
    resolvectl flush-caches >/dev/null 2>&1 || true
    fixed=1
  fi

  echo
  health_check_core
  if [[ "$fixed" -eq 1 ]]; then
    ok "已执行自动修复动作"
  else
    ok "无需修复"
  fi
}

# =============================================================================
# DMITBox network guard (KejiLion-inspired diagnostics; independent implementation)
# No network action runs when this module is sourced.
# =============================================================================
NETG_TX_ROOT="${NETG_TX_ROOT:-/var/lib/dmitbox/network-transactions}"
NETG_UNIT_DIR="${NETG_UNIT_DIR:-/etc/systemd/system}"
NETG_RESOLV_PATH="${NETG_RESOLV_PATH:-/etc/resolv.conf}"
NETG_TIMEOUT="${NETG_TIMEOUT:-180}"
NETG_TEST_MODE="${NETG_TEST_MODE:-0}"

netg_err() { printf 'DMITBox network guard: %s\n' "$*" >&2; }
netg_sha() {
  if have_cmd sha256sum; then sha256sum | awk '{print $1}'
  elif have_cmd shasum; then shasum -a 256 | awk '{print $1}'
  else return 1; fi
}
netg_fhash() {
  if [[ -f "$1" && ! -L "$1" ]]; then netg_sha < "$1"; else printf 'absent\n'; fi
}
netg_iface_valid() {
  [[ "${1:-}" =~ ^[a-zA-Z0-9_.:-]{1,32}$ ]] && ip link show dev "$1" >/dev/null 2>&1
}
netg_if_for() {
  local family="$1" dest="$2" result=""
  result="$(ip "-$family" route get "$dest" 2>/dev/null || true)"
  awk '{for(i=1;i<=NF;i++) if($i=="dev") {print $(i+1); exit}}' <<< "$result"
}
default_iface() {
  local iface=""
  iface="$(netg_if_for 4 1.1.1.1)"
  [[ -n "$iface" ]] || iface="$(netg_if_for 6 2606:4700:4700::1111)"
  [[ -n "$iface" ]] && netg_iface_valid "$iface" || return 1
  printf '%s\n' "$iface"
}
netg_ipv4_valid() {
  local value="${1:-}" a b c d e octet
  [[ "$value" =~ ^([0-9]{1,3}\.){3}[0-9]{1,3}$ ]] || return 1
  IFS=. read -r a b c d e <<< "$value"
  [[ -z "$e" ]] || return 1
  for octet in "$a" "$b" "$c" "$d"; do
    [[ "$octet" =~ ^[0-9]{1,3}$ ]] && (( 10#$octet <= 255 )) || return 1
  done
}
netg_ipv6_valid() {
  # Syntax check only for resolver values; reject options and shell tokens.
  local value="${1:-}"
  [[ ${#value} -le 45 && "$value" == *:* && "$value" =~ ^[0-9A-Fa-f:.]+$ ]]
}
netg_addr_valid() { netg_ipv4_valid "$1" || netg_ipv6_valid "$1"; }
netg_sysctl_one() { sysctl -n "$1" 2>/dev/null; }
netg_link_mtu() {
  local line
  line="$(ip -o link show dev "$1" 2>/dev/null)" || return 1
  awk '{for(i=1;i<=NF;i++)if($i=="mtu"){print $(i+1);exit}}' <<< "$line"
}
netg_link_admin() {
  local line flags
  line="$(ip -o link show dev "$1" 2>/dev/null)" || return 1
  flags="${line#*<}"; flags="${flags%%>*}"
  [[ "$flags" != "$line" ]] || return 1
  if [[ ",$flags," == *,UP,* ]]; then printf 'up\n'; else printf 'down\n'; fi
}

# Canonicalize observation-only firewall rules. Counters and nft handles are not policy.
# Preserve rule order; do not sort. Runtime sets with timeouts still cause legitimate drift.
netg_firewall_fingerprint() {
  local tmp="" used=0
  tmp="$(mktemp)" || return 1
  if have_cmd nft; then
    if ! LC_ALL=C nft -a list ruleset > "${tmp}.nft" 2>/dev/null; then
      rm -f -- "$tmp" "${tmp}.nft"; return 1
    fi
    sed -E 's/[[:space:]]*# handle [0-9]+([[:space:]]|$)/ /g; s/counter packets [0-9]+ bytes [0-9]+/counter/g' "${tmp}.nft" >> "$tmp"
    rm -f -- "${tmp}.nft"
    used=1
  fi
  local engine
  for engine in iptables-save ip6tables-save; do
    if have_cmd "$engine"; then
      printf '\n[%s]\n' "$engine" >> "$tmp"
      if ! "$engine" > "${tmp}.xt" 2>/dev/null; then
        rm -f -- "$tmp" "${tmp}.xt"; return 1
      fi
      sed -E 's/ \[[0-9]+:[0-9]+\]$ / [0:0] /; s/ \[[0-9]+:[0-9]+\]$/ [0:0]/' "${tmp}.xt" >> "$tmp"
      rm -f -- "${tmp}.xt"
      used=1
    fi
  done
  if (( used == 0 )); then rm -f -- "$tmp"; return 2; fi
  netg_sha < "$tmp"
  local rc=$?
  rm -f -- "$tmp"
  return "$rc"
}
netg_report() {
  local iface4 iface6 fw
  iface4="$(netg_if_for 4 1.1.1.1)"
  iface6="$(netg_if_for 6 2606:4700:4700::1111)"
  printf '\n[IPv4 / IPv6]\n'
  ip -br addr show 2>/dev/null || true
  printf '\n[路由]\n'
  ip -4 route show default 2>/dev/null || true
  ip -6 route show default 2>/dev/null || true
  printf '\n[出口网卡]\nIPv4: %s\nIPv6: %s\n' "${iface4:-未找到}" "${iface6:-未找到}"
  printf '\n[网卡状态]\n'
  ip -o link show 2>/dev/null || true
  printf '\n[DNS]\n'
  if [[ -L "$NETG_RESOLV_PATH" ]]; then
    printf 'resolv.conf -> %s\n' "$(readlink "$NETG_RESOLV_PATH")"
  fi
  if [[ -r "$NETG_RESOLV_PATH" ]]; then sed -n '1,35p' "$NETG_RESOLV_PATH"; fi
  if is_resolved_active && have_cmd resolvectl; then resolvectl dns 2>/dev/null || true; fi
  printf '\n[内核 / 队列]\n'
  sysctl net.ipv4.tcp_congestion_control net.core.default_qdisc \
    net.ipv6.conf.all.disable_ipv6 net.ipv6.conf.default.disable_ipv6 2>/dev/null || true
  printf '\n[防火墙 SHA-256 - 仅审计]\n'
  fw="$(netg_firewall_fingerprint 2>/dev/null)" && printf '%s\n' "$fw" || printf '不可用（缺少工具或无权限）\n'
  printf '\n'
}
netg_route_test() {
  local target4="${1:-1.1.1.1}" target6="${2:-2606:4700:4700::1111}"
  netg_ipv4_valid "$target4" || { netg_err "无效 IPv4 目标"; return 2; }
  netg_ipv6_valid "$target6" || { netg_err "无效 IPv6 目标"; return 2; }
  printf '[IPv4 route get %s]\n' "$target4"
  ip -4 route get "$target4" 2>&1 || true
  printf '\n[IPv6 route get %s]\n' "$target6"
  ip -6 route get "$target6" 2>&1 || true
  if have_cmd ping; then
    printf '\n[IPv4 ICMP - 最多 3 次]\n'
    command_with_timeout 12 ping -4 -n -c 3 -W 2 "$target4" 2>&1 || true
    printf '\n[IPv6 ICMP - 最多 3 次]\n'
    command_with_timeout 12 ping -6 -n -c 3 -W 2 "$target6" 2>&1 || true
  fi
  if have_cmd tracepath; then
    printf '\n[IPv4 tracepath]\n'
    command_with_timeout 12 tracepath -n "$target4" 2>&1 || true
    printf '\n[IPv6 tracepath]\n'
    command_with_timeout 12 tracepath -6 -n "$target6" 2>&1 || true
  fi
}

netg_txid_valid() { [[ "${1:-}" =~ ^tx-[a-zA-Z0-9]{8}$ ]]; }
netg_txdir() {
  netg_txid_valid "${1:-}" || return 2
  [[ ! -L "$NETG_TX_ROOT" && -d "$NETG_TX_ROOT" ]] || return 1
  printf '%s/%s\n' "$NETG_TX_ROOT" "$1"
}
netg_resource_capture() {
  local kind="$1" iface="$2"
  case "$kind" in
    dns-file) [[ ! -L "$NETG_RESOLV_PATH" ]] || return 1; cat -- "$NETG_RESOLV_PATH" ;;
    dns-resolved) resolvectl dns "$iface" ;;
    mtu) netg_link_mtu "$iface" ;;
    iface) netg_link_admin "$iface" ;;
    gai) if [[ -f "$GAI_CONF" ]]; then cat -- "$GAI_CONF"; else printf 'absent\n'; fi ;;
    ssh)
      netg_fhash /etc/ssh/sshd_config
      netg_fhash "$SSH_DROPIN_FILE"
      netg_fhash "$SSH_LEGACY_DROPIN_FILE"
      ssh_current_ports || return 1
      ;;
    ipv6)
      local key
      for key in all default lo; do printf '%s=' "$key"; netg_sysctl_one "net.ipv6.conf.$key.disable_ipv6" || return 1; done
      printf 'disable_conf='; netg_fhash "$IPV6_SYSCTL_FILE"
      printf 'enable_conf='; netg_fhash "$IPV6_FIX_SYSCTL_FILE"
      ;;
    *) return 2 ;;
  esac
}
netg_tx_record() {
  local tx="$1" kind="$2" iface="$3" name="$4" target="$5"
  printf '%s\n' "$kind" > "$tx/kind"
  printf '%s\n' "$iface" > "$tx/iface"
  printf '%s\n' "$name" > "$tx/label"
  printf '%s\n' "$target" > "$tx/target"
  printf '%s\n' "${SSH_CONNECTION:-}" > "$tx/origin-ssh"
  printf '%s\n' "$NETG_UNIT_DIR" > "$tx/unit-dir"
  printf '%s\n' "$(date -u +%FT%TZ)" > "$tx/created"
  chmod 600 "$tx"/*
}
netg_backup_file() {
  local tx="$1" source="$2" slot="$3"
  [[ ! -L "$source" ]] || return 1
  if [[ -e "$source" ]]; then
    [[ -f "$source" ]] || return 1
    cp -p -- "$source" "$tx/$slot.previous" || return 1
    : > "$tx/$slot.present"
  else
    : > "$tx/$slot.absent"
  fi
}
netg_capture_resolved_servers() {
  # resolvectl dns IFACE example: "Link 2 (eth0): 1.1.1.1 2606:..."
  local iface="$1" out="$2" line tok
  line="$(resolvectl dns "$iface")" || return 1
  : > "$out"
  # Remove only the leading "Link N (iface):" label, not the colons inside IPv6 addresses.
  line="$(printf '%s\n' "$line" | sed -E '1s/^Link[[:space:]]+[0-9]+[[:space:]]+\([^)]*\):[[:space:]]*//')"
  line="$(printf '%s\n' "$line" | tr '\n\t' '  ')"
  local -a words=()
  IFS=' ' read -r -a words <<< "$line"
  for tok in "${words[@]}"; do
    netg_addr_valid "$tok" || return 1
    printf '%s\n' "$tok" >> "$out"
  done
}
netg_tx_snapshot() {
  local tx="$1" kind="$2" iface="$3"
  case "$kind" in
    dns-file)
      [[ ! -L "$NETG_RESOLV_PATH" && -f "$NETG_RESOLV_PATH" ]] || return 1
      netg_backup_file "$tx" "$NETG_RESOLV_PATH" dns
      printf '%s\n' "$NETG_RESOLV_PATH" > "$tx/dns-path"
      ;;
    dns-resolved) netg_capture_resolved_servers "$iface" "$tx/dns-servers.previous" || return 1 ;;
    mtu)
      netg_link_mtu "$iface" > "$tx/mtu.previous" || return 1
      is_uint_in_range "$(cat "$tx/mtu.previous")" 576 65535 || return 1
      ;;
    iface) netg_link_admin "$iface" > "$tx/iface.previous" || return 1 ;;
    gai) netg_backup_file "$tx" "$GAI_CONF" gai; printf '%s\n' "$GAI_CONF" > "$tx/gai-path" ;;
    ssh)
      netg_backup_file "$tx" /etc/ssh/sshd_config ssh-main || return 1
      netg_backup_file "$tx" "$SSH_DROPIN_FILE" ssh-dropin || return 1
      netg_backup_file "$tx" "$SSH_LEGACY_DROPIN_FILE" ssh-legacy || return 1
      printf '%s\n' "$SSH_DROPIN_FILE" > "$tx/ssh-dropin-path"
      printf '%s\n' "$SSH_LEGACY_DROPIN_FILE" > "$tx/ssh-legacy-path"
      ;;
    ipv6)
      local k
      for k in all default lo; do netg_sysctl_one "net.ipv6.conf.$k.disable_ipv6" > "$tx/ipv6-$k.previous" || return 1; done
      netg_backup_file "$tx" "$IPV6_SYSCTL_FILE" ipv6-disabled || return 1
      netg_backup_file "$tx" "$IPV6_FIX_SYSCTL_FILE" ipv6-enabled || return 1
      printf '%s\n' "$IPV6_SYSCTL_FILE" > "$tx/ipv6-disabled-path"
      printf '%s\n' "$IPV6_FIX_SYSCTL_FILE" > "$tx/ipv6-enabled-path"
      ;;
    *) return 2 ;;
  esac
  netg_resource_capture "$kind" "$iface" > "$tx/state.before" || return 1
  netg_sha < "$tx/state.before" > "$tx/hash.before" || return 1
  netg_firewall_fingerprint > "$tx/firewall.before" 2>/dev/null || : > "$tx/firewall.before"
}
netg_make_undo_worker() {
  local tx="$1"
  cat > "$tx/rollback.sh" <<'WORKER' || return 1
#!/usr/bin/env bash
set -euo pipefail
IFS=$'\n\t'
tx="$(cd -- "$(dirname -- "$0")" && pwd -P)"
[[ -d "$tx" && ! -L "$tx" ]] || exit 1
exec 9> "$tx/lock"
flock -x 9
if [[ -e "$tx/confirmed" || -e "$tx/rolled-back" ]]; then exit 0; fi
[[ -e "$tx/pending" ]] || exit 1
kind="$(cat "$tx/kind")"
iface="$(cat "$tx/iface")"
unit_dir="$(cat "$tx/unit-dir")"
unit="dmitbox-net-$(basename "$tx")"
restore_file() {
  local name="$1" target="$2" staging=""
  [[ "$target" == /* && ! -L "$target" ]] || return 1
  if [[ -f "$tx/$name.present" ]]; then
    staging="$(mktemp "${target}.dmitbox-undo.XXXXXX")" || return 1
    cp -p -- "$tx/$name.previous" "$staging" || { rm -f -- "$staging"; return 1; }
    mv -f -- "$staging" "$target" || { rm -f -- "$staging"; return 1; }
  elif [[ -f "$tx/$name.absent" ]]; then
    rm -f -- "$target" || return 1
  else
    return 1
  fi
}
undo() {
  local k old target
  case "$kind" in
    dns-file) restore_file dns "$(cat "$tx/dns-path")" ;;
    dns-resolved)
      local -a old_servers=()
      mapfile -t old_servers < "$tx/dns-servers.previous"
      if (( ${#old_servers[@]} > 0 )); then
        resolvectl dns "$iface" "${old_servers[@]}" || return 1
      else
        resolvectl revert "$iface" || return 1
      fi
      resolvectl flush-caches >/dev/null 2>&1 || true
      ;;
    gai) restore_file gai "$(cat "$tx/gai-path")" ;;
    ssh)
      restore_file ssh-main /etc/ssh/sshd_config || return 1
      restore_file ssh-dropin "$(cat "$tx/ssh-dropin-path")" || return 1
      restore_file ssh-legacy "$(cat "$tx/ssh-legacy-path")" || return 1
      sshd -t || return 1
      systemctl reload sshd >/dev/null 2>&1 || systemctl reload ssh >/dev/null 2>&1 || \
        systemctl restart sshd >/dev/null 2>&1 || systemctl restart ssh >/dev/null 2>&1 || return 1
      ;;
    mtu)
      old="$(cat "$tx/mtu.previous")"
      [[ "$old" =~ ^[0-9]{3,5}$ ]] || return 1
      ip link set dev "$iface" mtu "$old" || return 1
      ;;
    iface)
      old="$(cat "$tx/iface.previous")"
      case "$old" in up|down) ip link set dev "$iface" "$old" || return 1 ;; *) return 1 ;; esac
      ;;
    ipv6)
      restore_file ipv6-disabled "$(cat "$tx/ipv6-disabled-path")" || return 1
      restore_file ipv6-enabled "$(cat "$tx/ipv6-enabled-path")" || return 1
      for k in all default lo; do
        old="$(cat "$tx/ipv6-$k.previous")"
        [[ "$old" == 0 || "$old" == 1 ]] || return 1
        sysctl -w "net.ipv6.conf.$k.disable_ipv6=$old" >/dev/null || return 1
      done
      ;;
    *) return 1 ;;
  esac
}
if undo; then
  printf 'rolled-back\n' > "$tx/rolled-back"
  date -u +%FT%TZ > "$tx/rolled-back-at"
  if command -v systemctl >/dev/null 2>&1; then
    systemctl disable "$unit.timer" "$unit.service" >/dev/null 2>&1 || true
    rm -f -- "$unit_dir/$unit.timer" "$unit_dir/$unit.service" || true
    systemctl daemon-reload >/dev/null 2>&1 || true
  fi
else
  printf 'rollback-failed\n' > "$tx/rollback-failed"
  exit 1
fi
WORKER
  bash -n "$tx/rollback.sh" || return 1
  chmod 700 "$tx/rollback.sh" || return 1
}
netg_timer_cleanup() {
  local tx="$1" unit="dmitbox-net-$(basename "$1")"
  systemctl disable --now "$unit.timer" >/dev/null 2>&1 || true
  systemctl disable "$unit.service" >/dev/null 2>&1 || true
  rm -f -- "$NETG_UNIT_DIR/$unit.timer" "$NETG_UNIT_DIR/$unit.service" || return 1
  systemctl daemon-reload >/dev/null 2>&1 || return 1
}
netg_timer_arm() {
  local tx="$1" unit="dmitbox-net-$(basename "$1")" shellpath="" secs="$NETG_TIMEOUT"
  is_systemd || { netg_err "受保护写操作目前要求 systemd PID 1（其他 init 暂拒绝执行）"; return 1; }
  have_cmd systemctl && have_cmd flock && have_cmd bash || return 1
  is_uint_in_range "$secs" 60 3600 || { netg_err "回滚时限必须为 60-3600 秒"; return 1; }
  [[ "$NETG_UNIT_DIR" =~ ^/[a-zA-Z0-9_/.-]+$ && "$tx" =~ ^/[a-zA-Z0-9_/.-]+$ ]] || return 1
  [[ ! -L "$NETG_UNIT_DIR" ]] || return 1
  mkdir -p -- "$NETG_UNIT_DIR" || return 1
  shellpath="$(command -v bash)" || return 1
  [[ "$shellpath" == /* && "$shellpath" != *' '* ]] || return 1
  write_file "$NETG_UNIT_DIR/$unit.service" "[Unit]
Description=DMITBox network failsafe rollback $unit
After=local-fs.target

[Service]
Type=oneshot
ExecStart=$shellpath $tx/rollback.sh

[Install]
WantedBy=multi-user.target" 0644 || return 1
  write_file "$NETG_UNIT_DIR/$unit.timer" "[Unit]
Description=DMITBox network failsafe timer $unit

[Timer]
OnActiveSec=${secs}s
AccuracySec=1s
Unit=${unit}.service

[Install]
WantedBy=timers.target" 0644 || { netg_timer_cleanup "$tx"; return 1; }
  if ! systemctl daemon-reload >/dev/null 2>&1 || \
     ! systemctl enable "$unit.service" >/dev/null 2>&1 || \
     ! systemctl enable --now "$unit.timer" >/dev/null 2>&1 || \
     ! systemctl is-active --quiet "$unit.timer"; then
    netg_err "回滚定时任务未成功启动；禁止修改系统配置"
    netg_timer_cleanup "$tx" || true
    return 1
  fi
}
netg_tx_begin() {
  local kind="$1" iface="$2" label="$3" target="$4" tx="" pending=""
  [[ -d "$NETG_TX_ROOT" && ! -L "$NETG_TX_ROOT" ]] || mkdir -p -m 700 -- "$NETG_TX_ROOT" || return 1
  [[ ! -L "$NETG_TX_ROOT" ]] || return 1
  chmod 700 -- "$NETG_TX_ROOT" || return 1
  # Refuse overlapping guarded operations until prior change is confirmed/rolled back.
  for pending in "$NETG_TX_ROOT"/tx-*/pending; do
    [[ -f "$pending" ]] || continue
    if [[ ! -f "${pending%/*}/confirmed" && ! -f "${pending%/*}/rolled-back" ]]; then
      netg_err "仍有未确认的网络变更：${pending%/*}；请先确认或回滚"
      return 1
    fi
  done
  tx="$(mktemp -d "$NETG_TX_ROOT/tx-XXXXXXXX")" || return 1
  chmod 700 "$tx" || return 1
  netg_tx_record "$tx" "$kind" "$iface" "$label" "$target" || return 1
  if ! netg_tx_snapshot "$tx" "$kind" "$iface"; then
    netg_err "无法完成变更前快照，已拒绝操作"
    rm -rf -- "$tx"
    return 1
  fi
  netg_make_undo_worker "$tx" || { rm -rf -- "$tx"; return 1; }
  : > "$tx/pending"
  if ! netg_timer_arm "$tx"; then
    rm -rf -- "$tx"
    return 1
  fi
  printf '%s\n' "$tx"
}
netg_tx_validate_applied() {
  local tx="$1" kind iface target val d
  kind="$(cat "$tx/kind")"
  iface="$(cat "$tx/iface")"
  target="$(cat "$tx/target")"
  case "$kind" in
    dns-file)
      [[ ! -L "$NETG_RESOLV_PATH" ]] || return 1
      if [[ "$(cat "$tx/label")" == restore ]]; then
        [[ -f "$RESOLV_BACKUP" ]] && cmp -s "$RESOLV_BACKUP" "$NETG_RESOLV_PATH"
        return $?
      fi
      while IFS= read -r d; do
        [[ -z "$d" ]] && continue
        grep -Fqx -- "nameserver $d" "$NETG_RESOLV_PATH" || return 1
      done < "$tx/dns-servers.desired"
      ;;
    dns-resolved)
      val="$(resolvectl dns "$iface")" || return 1
      [[ "$(cat "$tx/label")" != reset ]] || return 0
      while IFS= read -r d; do
        [[ -z "$d" ]] && continue
        [[ " $val " == *" $d "* || "$val" == *" $d" ]] || return 1
      done < "$tx/dns-servers.desired"
      ;;
    gai)
      if [[ "$target" == restore ]]; then
        [[ -f "${BACKUP_BASE}/gai.conf.orig" ]] && cmp -s "${BACKUP_BASE}/gai.conf.orig" "$GAI_CONF"
        return $?
      fi
      if [[ "$target" == v4 ]]; then
        grep -Eq '^precedence[[:space:]]+::ffff:0:0/96[[:space:]]+100([[:space:]]*)$' "$GAI_CONF" || return 1
      else
        ! grep -Fq '# DMITBox managed: prefer IPv4' "$GAI_CONF" || return 1
      fi
      ;;
    ssh)
      sshd -t >/dev/null 2>&1 || return 1
      case " $(ssh_current_ports) " in *" $target "*) ;; *) return 1 ;; esac
      port_in_use "$target" || return 1
      ;;
    mtu) [[ "$(netg_link_mtu "$iface")" == "$target" ]] ;;
    iface) [[ "$(netg_link_admin "$iface")" == "$target" ]] ;;
    ipv6)
      for val in all default lo; do [[ "$(netg_sysctl_one "net.ipv6.conf.$val.disable_ipv6")" == "$target" ]] || return 1; done
      ;;
    *) return 2 ;;
  esac
}
netg_apply_mutation() {
  local tx="$1" kind iface target provider dns_content="" v6_file="" k d
  kind="$(cat "$tx/kind")"; iface="$(cat "$tx/iface")"; target="$(cat "$tx/target")"
  case "$kind" in
    dns-file)
      [[ ! -L "$NETG_RESOLV_PATH" ]] || return 1
      if [[ "$(cat "$tx/label")" == restore ]]; then
        [[ -f "$RESOLV_BACKUP" && ! -L "$RESOLV_BACKUP" ]] || return 1
        local staged=""
        staged="$(mktemp "${NETG_RESOLV_PATH}.restore.XXXXXX")" || return 1
        cp -p -- "$RESOLV_BACKUP" "$staged" || { rm -f -- "$staged"; return 1; }
        mv -f -- "$staged" "$NETG_RESOLV_PATH" || { rm -f -- "$staged"; return 1; }
        return 0
      fi
      dns_content="# managed by DMITBox network guard"
      while IFS= read -r d; do dns_content+=$'\n'"nameserver $d"; done < "$tx/dns-servers.desired"
      write_file "$NETG_RESOLV_PATH" "$dns_content" 0644 || return 1
      ;;
    dns-resolved)
      if [[ "$(cat "$tx/label")" == reset ]]; then
        resolvectl revert "$iface" || return 1
        resolvectl flush-caches >/dev/null 2>&1 || true
        return 0
      fi
      local -a servers=()
      mapfile -t servers < "$tx/dns-servers.desired"
      resolvectl dns "$iface" "${servers[@]}" || return 1
      resolvectl flush-caches >/dev/null 2>&1 || true
      ;;
    gai)
      [[ ! -L "$GAI_CONF" ]] || return 1
      local old=""
      [[ ! -f "$GAI_CONF" ]] || old="$(cat "$GAI_CONF")"
      if [[ "$target" == restore ]]; then
        [[ -f "${BACKUP_BASE}/gai.conf.orig" && ! -L "${BACKUP_BASE}/gai.conf.orig" ]] || return 1
        local staged=""
        staged="$(mktemp "${GAI_CONF}.restore.XXXXXX")" || return 1
        cp -p -- "${BACKUP_BASE}/gai.conf.orig" "$staged" || { rm -f -- "$staged"; return 1; }
        mv -f -- "$staged" "$GAI_CONF" || { rm -f -- "$staged"; return 1; }
        return 0
      fi
      old="$(printf '%s\n' "$old" | awk '
        /^# (DMITBox|dmitbox.sh) managed: prefer IPv4$/ {skip=1; next}
        skip && /^precedence[[:space:]]+::ffff:0:0\/96[[:space:]]+100[[:space:]]*$/ {skip=0; next}
        {skip=0; print}
      ')"
      if [[ "$target" == v4 ]]; then old+=$'\n'"# DMITBox managed: prefer IPv4"$'\n'"precedence ::ffff:0:0/96 100"; fi
      write_file "$GAI_CONF" "$old" 0644 || return 1
      ;;
    mtu) ip link set dev "$iface" mtu "$target" || return 1 ;;
    iface) ip link set dev "$iface" "$target" || return 1 ;;
    ipv6)
      if [[ "$target" == 1 ]]; then
        rm -f -- "$IPV6_FIX_SYSCTL_FILE" || return 1
        write_file "$IPV6_SYSCTL_FILE" "# managed by dmitbox network guard
net.ipv6.conf.all.disable_ipv6=1
net.ipv6.conf.default.disable_ipv6=1
net.ipv6.conf.lo.disable_ipv6=1" 0644 || return 1
      else
        rm -f -- "$IPV6_SYSCTL_FILE" || return 1
        write_file "$IPV6_FIX_SYSCTL_FILE" "# managed by dmitbox network guard
net.ipv6.conf.all.disable_ipv6=0
net.ipv6.conf.default.disable_ipv6=0
net.ipv6.conf.lo.disable_ipv6=0" 0644 || return 1
      fi
      for k in all default lo; do
        sysctl -w "net.ipv6.conf.$k.disable_ipv6=$target" >/dev/null || return 1
      done
      ;;
    *) return 2 ;;
  esac
}
netg_tx_rollback_now() {
  local id="$1" tx=""
  tx="$(netg_txdir "$id")" || return 1
  [[ ! -L "$tx" && -f "$tx/rollback.sh" && ! -f "$tx/confirmed" ]] || return 1
  bash "$tx/rollback.sh" || return 1
  printf '事务 %s 已回滚。\n' "$id"
}
netg_tx_status() {
  local id="$1" tx="" label=""
  tx="$(netg_txdir "$id")" || return 1
  [[ -d "$tx" && ! -L "$tx" ]] || return 1
  label="$(cat "$tx/label")"
  printf '事务: %s\n操作: %s\n创建: %s\n' "$id" "$label" "$(cat "$tx/created")"
  if [[ -f "$tx/confirmed" ]]; then echo '状态: 已确认'
  elif [[ -f "$tx/rolled-back" ]]; then echo '状态: 已回滚'
  elif [[ -f "$tx/rollback-failed" ]]; then echo '状态: 回滚失败，请人工检查'
  else echo '状态: 等待外部确认（计时回滚已启用）'; fi
  printf '变更前资源 SHA-256: %s\n' "$(cat "$tx/hash.before")"
  [[ ! -s "$tx/hash.after" ]] || printf '变更后资源 SHA-256: %s\n' "$(cat "$tx/hash.after")"
  [[ ! -s "$tx/firewall.before" ]] || printf '变更前防火墙 SHA-256: %s\n' "$(cat "$tx/firewall.before")"
  [[ ! -s "$tx/firewall.after" ]] || printf '变更后防火墙 SHA-256: %s\n' "$(cat "$tx/firewall.after")"
}
netg_tx_confirm() {
  local id="$1" tx="" before_session="" current_session="" expected_hash="" current_hash=""
  tx="$(netg_txdir "$id")" || return 1
  [[ -f "$tx/pending" && ! -L "$tx" && ! -e "$tx/rolled-back" && ! -e "$tx/rollback-failed" ]] || return 1
  exec 8> "$tx/lock"
  flock -x 8 || return 1
  [[ ! -f "$tx/confirmed" && ! -f "$tx/rolled-back" ]] || return 1
  before_session="$(cat "$tx/origin-ssh")"
  current_session="${SSH_CONNECTION:-}"
  if [[ "$NETG_TEST_MODE" != 1 && -n "$before_session" ]] &&
     { [[ -z "$current_session" ]] || [[ "$current_session" == "$before_session" ]]; }; then
    netg_err "无法确认来自新的 SSH 会话（SSH_CONNECTION 缺失或仍是旧连接）"
    return 1
  fi
  if [[ "$NETG_TEST_MODE" != 1 && "$(cat "$tx/kind")" == ssh && -n "$before_session" ]]; then
    local cli_ip cli_port srv_ip srv_port
    IFS=' ' read -r cli_ip cli_port srv_ip srv_port <<< "$current_session"
    if [[ "$srv_port" != "$(cat "$tx/target")" ]]; then
      netg_err "SSH 端口变更必须由已连接到新端口的 SSH 会话确认"
      return 1
    fi
  fi
  netg_tx_validate_applied "$tx" || { netg_err "资源运行状态已偏离目标值，不允许确认"; return 1; }
  expected_hash="$(cat "$tx/hash.after")"
  current_hash="$(netg_resource_capture "$(cat "$tx/kind")" "$(cat "$tx/iface")" | netg_sha)" || return 1
  [[ "$current_hash" == "$expected_hash" ]] || { netg_err "变更后资源 SHA-256 不一致，拒绝确认"; return 1; }
  # Mark first while holding the shared flock: a concurrent timeout worker will be a no-op.
  printf '%s\n' "$(date -u +%FT%TZ)" > "$tx/confirmed"
  if ! netg_timer_cleanup "$tx"; then
    netg_err "事务已确认，但 systemd 单元清理失败；请检查残留定时器"
    return 1
  fi
  printf '事务 %s：已确认，自动回滚已取消。\n' "$id"
}
netg_dns_provider_servers() {
  case "$1" in
    cloudflare)
      printf '%s\n' 1.1.1.1 1.0.0.1 2606:4700:4700::1111 ;;
    google)
      printf '%s\n' 8.8.8.8 8.8.4.4 2001:4860:4860::8888 ;;
    quad9)
      printf '%s\n' 9.9.9.9 149.112.112.112 2620:fe::fe ;;
    *) return 2 ;;
  esac
}
dns_backup_once() {
  local source="$NETG_RESOLV_PATH" destination="$RESOLV_BACKUP"
  [[ -f "$source" && ! -L "$source" ]] || return 1
  [[ ! -L "$destination" ]] || return 1
  if [[ -e "$destination" ]]; then
    [[ -f "$destination" ]] || return 1
    return 0
  fi
  ( umask 077; mkdir -p -- "$(dirname "$destination")" ) || return 1
  cp -p -- "$source" "$destination" || return 1
  chmod 600 "$destination" || return 1
}

netg_dns_set_safe() {
  local provider="${1:-}" iface="${2:-}" kind="" tx="" servers="" server=""
  netg_dns_provider_servers "$provider" >/dev/null || { netg_err "仅支持 cloudflare / google / quad9"; return 2; }
  netg_iface_valid "$iface" || { netg_err "目标网卡无效"; return 2; }
  if is_systemd && systemctl is-active --quiet NetworkManager 2>/dev/null; then
    netg_err "NetworkManager 已启用；请用 nmcli 修改对应连接，不向其控制的 resolver 强行注入 DNS"
    return 1
  fi
  if is_resolved_active && have_cmd resolvectl; then
    kind=dns-resolved
  elif [[ -f "$NETG_RESOLV_PATH" && ! -L "$NETG_RESOLV_PATH" ]]; then
    kind=dns-file
  else
    netg_err "resolv.conf 由其他网络管理器接管；不允许强制覆盖符号链接"
    return 1
  fi
  # Implement the DNS transaction with its desired server list staged first.
  if [[ "$kind" == dns-file ]]; then dns_backup_once || return 1; fi
  tx="$(netg_tx_begin "$kind" "$iface" "$provider" "$provider")" || return 1
  netg_dns_provider_servers "$provider" > "$tx/dns-servers.desired" || return 1
  chmod 600 "$tx/dns-servers.desired"
  local before_now=""
  before_now="$(netg_resource_capture "$kind" "$iface" | netg_sha)" || return 1
  if [[ "$before_now" != "$(cat "$tx/hash.before")" ]]; then
    netg_err "DNS 状态在变更前已变化"; netg_tx_rollback_now "$(basename "$tx")" || true; return 1
  fi
  if ! netg_apply_mutation "$tx" || ! netg_tx_validate_applied "$tx"; then
    netg_err "DNS 应用验证失败，执行自动回滚"
    netg_tx_rollback_now "$(basename "$tx")" || true
    return 1
  fi
  # getent is an end-to-end resolver probe, not only a config readback.
  if have_cmd getent && ! command_with_timeout 8 getent hosts example.com >/dev/null 2>&1; then
    netg_err "DNS 解析未通过，恢复原来的解析设置"
    netg_tx_rollback_now "$(basename "$tx")" || true
    return 1
  fi
  netg_tx_record_after "$tx" || { netg_tx_rollback_now "$(basename "$tx")" || true; return 1; }
  netg_tx_print_pending "$tx"
}
netg_dns_restore_safe() {
  local iface="${1:-}" kind="" label="" tx=""
  netg_iface_valid "$iface" || return 2
  if is_systemd && systemctl is-active --quiet NetworkManager 2>/dev/null; then
    netg_err "NetworkManager 已启用；需由其持久化 DNS，拒绝直接恢复 resolver"
    return 1
  fi
  if is_resolved_active && have_cmd resolvectl; then
    kind=dns-resolved; label=reset
  elif [[ -f "$NETG_RESOLV_PATH" && ! -L "$NETG_RESOLV_PATH" ]]; then
    if is_systemd && systemctl is-active --quiet NetworkManager 2>/dev/null; then
      netg_err "NetworkManager 当前持有 DNS 配置，不直接恢复 resolv.conf"; return 1
    fi
    kind=dns-file; label=restore
    [[ -f "$RESOLV_BACKUP" && ! -L "$RESOLV_BACKUP" ]] || {
      netg_err "没有原始 DNS 文件备份，拒绝猜测原始值"; return 1;
    }
  else
    netg_err "当前 DNS 管理器不可安全操作"; return 1
  fi
  tx="$(netg_tx_begin "$kind" "$iface" "$label" "$label")" || return 1
  local current_hash=""
  current_hash="$(netg_resource_capture "$kind" "$iface" | netg_sha)" || return 1
  if [[ "$current_hash" != "$(cat "$tx/hash.before")" ]]; then
    netg_err "DNS 快照之后资源发生并发变化，停止恢复"
    netg_tx_rollback_now "$(basename "$tx")" || true
    return 1
  fi
  if ! netg_apply_mutation "$tx" || ! netg_tx_validate_applied "$tx"; then
    netg_tx_rollback_now "$(basename "$tx")" || true
    return 1
  fi
  if have_cmd getent && ! command_with_timeout 8 getent hosts example.com >/dev/null 2>&1; then
    netg_err "恢复后 DNS 仍无法解析，正在回滚"; netg_tx_rollback_now "$(basename "$tx")" || true
    return 1
  fi
  netg_tx_record_after "$tx" || { netg_tx_rollback_now "$(basename "$tx")" || true; return 1; }
  netg_tx_print_pending "$tx"
}
netg_tx_record_after() {
  local tx="$1" kind iface
  [[ -f "$tx/pending" && ! -f "$tx/rolled-back" && ! -f "$tx/confirmed" ]] || return 1
  kind="$(cat "$tx/kind")"; iface="$(cat "$tx/iface")"
  netg_resource_capture "$kind" "$iface" > "$tx/state.after" || return 1
  netg_sha < "$tx/state.after" > "$tx/hash.after" || return 1
  netg_firewall_fingerprint > "$tx/firewall.after" 2>/dev/null || : > "$tx/firewall.after"
  [[ ! -s "$tx/hash.after" ]] || chmod 600 "$tx/state.after" "$tx/hash.after" "$tx/firewall.after"
}
netg_tx_print_pending() {
  local tx="$1" id="$(basename "$1")"
  printf '\n操作已通过本地状态校验，事务：%s\n' "$id"
  printf '前状态 SHA-256：%s\n' "$(cat "$tx/hash.before")"
  printf '后状态 SHA-256：%s\n' "$(cat "$tx/hash.after")"
  if [[ -s "$tx/firewall.before" && -s "$tx/firewall.after" ]] && \
     ! cmp -s "$tx/firewall.before" "$tx/firewall.after"; then
    netg_err "同期防火墙状态发生变化；请核对是否有其他服务修改规则"
  fi
  local script_path=""
  script_path="$(readlink -f -- "$0" 2>/dev/null || printf '%s' "$0")"
  printf '请在独立 SSH 会话测试访问，然后执行：sudo bash %s --net-confirm %s\n' "$script_path" "$id"
  printf '未确认将在 %s 秒后回滚；不要关闭当前 SSH 会话。\n' "$NETG_TIMEOUT"
  printf '注意：本地校验不代表外部可达性，必须在新连接中验证。\n' 
}
netg_safe_change() {
  local kind="$1" iface="$2" label="$3" target="$4" tx=""
  tx="$(netg_tx_begin "$kind" "$iface" "$label" "$target")" || return 1
  local before_now="" before_hash=""
  before_now="$(netg_resource_capture "$kind" "$iface" | netg_sha)" || return 1
  before_hash="$(cat "$tx/hash.before")"
  if [[ "$before_now" != "$before_hash" ]]; then
    netg_err "创建快照后资源状态变化，停止操作"
    netg_tx_rollback_now "$(basename "$tx")" || true
    return 1
  fi
  if ! netg_apply_mutation "$tx" || ! netg_tx_validate_applied "$tx"; then
    netg_err "变更后的目标值不符合预期，开始回滚"
    netg_tx_rollback_now "$(basename "$tx")" || true
    return 1
  fi
  netg_tx_record_after "$tx" || { netg_tx_rollback_now "$(basename "$tx")" || true; return 1; }
  netg_tx_print_pending "$tx"
}
netg_ipv6_toggle_safe() {
  local action="${1:-}" target=""
  case "$action" in enable) target=0 ;; disable) target=1 ;; *) return 2 ;; esac
  if [[ "$target" == 1 ]]; then
    if [[ "${SSH_CONNECTION:-}" == *:* ]]; then
      netg_err "当前 SSH 连接可能使用 IPv6，禁止禁用 IPv6"
      return 1
    fi
    [[ ! -f "$IPV6_POOL_CONF" && ! -f "$IPV6_RAND_CONF" ]] || {
      netg_err "检测到 DMITBox IPv6 地址池/NAT66：先单独处理服务，不自动破坏其状态"; return 1;
    }
  fi
  if [[ "$target" == 0 ]] && grep -Eq '(^|[[:space:]])ipv6.disable=1([[:space:]]|$)' /proc/cmdline 2>/dev/null; then
    netg_err "内核启动参数 ipv6.disable=1 需要由引导层处理；拒绝自动修改 GRUB"
    return 1
  fi
  if [[ -L "$IPV6_SYSCTL_FILE" || -L "$IPV6_FIX_SYSCTL_FILE" ]]; then
    netg_err "受管 IPv6 配置文件是符号链接，拒绝覆盖"
    return 1
  fi
  netg_safe_change ipv6 '-' "IPv6-$action" "$target"
}
netg_mtu_set_safe() {
  local iface="${1:-}" mtu="${2:-}" old=""
  netg_iface_valid "$iface" || { netg_err "目标网卡不存在"; return 2; }
  is_uint_in_range "$mtu" 1280 9000 || { netg_err "MTU 限制在 1280-9000，避免意外破坏 IPv6"; return 2; }
  old="$(netg_link_mtu "$iface")"
  [[ "$old" != "$mtu" ]] || { printf 'MTU 未改变：%s\n' "$mtu"; return 0; }
  netg_safe_change mtu "$iface" "MTU ${old}->${mtu}" "$mtu"
}
netg_guarded_ssh_firewall_permit() {
  local backend="$1" action="$2" protocol="$3" port="$4" source="$5" id="${NETG_SSH_TX_ID:-}" tx=""
  netg_txid_valid "$id" || return 1
  tx="$(netg_txdir "$id")" || return 1
  [[ -f "$tx/pending" && ! -f "$tx/confirmed" ]] || return 1
  [[ "$(cat "$tx/kind")" == ssh ]] || return 1
  [[ "$action" == allow-port && "$protocol" == tcp && "$source" == - ]] || return 1
  [[ "$port" == "$(cat "$tx/target")" ]] || return 1
  [[ "$backend" == ufw || "$backend" == firewalld ]] || return 1
  systemctl is-active --quiet "dmitbox-net-$id.timer" || return 1
}

netg_ssh_port_safe() {
  local new_port="${1:-}" tx="" current=""
  is_uint_in_range "$new_port" 1024 65535 || { netg_err "安全 SSH 端口建议使用 1024-65535"; return 2; }
  new_port="$((10#$new_port))"
  [[ -f /etc/ssh/sshd_config && ! -L /etc/ssh/sshd_config ]] || return 1
  if port_in_use "$new_port"; then netg_err "端口已被监听"; return 1; fi
  ssh_validate_config || return 1
  if is_systemd && { systemctl is-active --quiet ssh.socket || systemctl is-active --quiet sshd.socket; }; then
    netg_err "检测到 systemd SSH socket activation；旧端口流程会禁用 socket，拒绝在此模式自动操作"
    return 1
  fi
  tx="$(netg_tx_begin ssh '-' "SSH port" "$new_port")" || return 1
  current="$(netg_resource_capture ssh '-' | netg_sha)" || return 1
  if [[ "$current" != "$(cat "$tx/hash.before")" ]]; then
    netg_err "SSH 配置在备份后发生变化，拒绝覆盖"
    netg_tx_rollback_now "$(basename "$tx")" || true
    return 1
  fi
  if ! NETG_SSH_TX_ID="$(basename "$tx")" ssh_set_port_legacy "$new_port" || ! netg_tx_validate_applied "$tx"; then
    netg_err "SSH 端口切换未完成，启动回滚"
    netg_tx_rollback_now "$(basename "$tx")" || true
    return 1
  fi
  netg_tx_record_after "$tx" || { netg_tx_rollback_now "$(basename "$tx")" || true; return 1; }
  netg_tx_print_pending "$tx"
  printf '请通过 NEW PORT=%s 独立登录确认；新端口的防火墙放行规则不会被通用回滚脚本删除。\n' "$new_port"
}

netg_iface_set_safe() {
  local iface="${1:-}" action="${2:-}" v4if="" v6if="" client="" sshif=""
  netg_iface_valid "$iface" || { netg_err "目标网卡不存在"; return 2; }
  [[ "$action" == up || "$action" == down ]] || return 2
  if [[ "$action" == down ]]; then
    v4if="$(netg_if_for 4 1.1.1.1)"
    v6if="$(netg_if_for 6 2606:4700:4700::1111)"
    if [[ "$iface" == "$v4if" || "$iface" == "$v6if" ]]; then
      netg_err "拒绝关闭默认出口网卡：接口 DOWN 后无法保证 DHCP/路由自动恢复"
      return 1
    fi
    if [[ -n "${SSH_CONNECTION:-}" ]]; then
      client="${SSH_CONNECTION%% *}"
      if netg_ipv4_valid "$client"; then sshif="$(netg_if_for 4 "$client")"; fi
      if netg_ipv6_valid "$client"; then sshif="$(netg_if_for 6 "$client")"; fi
      if [[ "$iface" == "$sshif" ]]; then netg_err "拒绝关闭当前 SSH 回程接口"; return 1; fi
    fi
  fi
  netg_safe_change iface "$iface" "Interface-$action" "$action"
}
netg_gai_set_safe() {
  local target="${1:-}" kind="" lib=""
  [[ "$target" == v4 || "$target" == default || "$target" == restore ]] || return 2
  if [[ "$target" == restore && ! -f "${BACKUP_BASE}/gai.conf.orig" ]]; then
    netg_err "没有原始 gai.conf 备份，无法恢复"; return 1
  fi
  lib="$(libc_kind)"
  [[ "$lib" == glibc ]] || { netg_err "/etc/gai.conf 的 precedence 在当前非 glibc 系统不适用"; return 1; }
  [[ ! -L "$GAI_CONF" ]] || return 1
  if [[ "$target" != restore ]]; then gai_backup_once || return 1; fi
  netg_safe_change gai '-' "Address-order-$target" "$target"
}
netg_tx_list() {
  local path id
  [[ -d "$NETG_TX_ROOT" ]] || { printf '尚无网络保护事务。\n'; return 0; }
  for path in "$NETG_TX_ROOT"/tx-*; do
    [[ -d "$path" ]] || continue
    id="${path##*/}"
    printf '%s  ' "$id"
    if [[ -f "$path/confirmed" ]]; then echo confirmed
    elif [[ -f "$path/rolled-back" ]]; then echo rolled-back
    elif [[ -f "$path/rollback-failed" ]]; then echo rollback-failed
    else echo PENDING; fi
  done
}
netg_network_menu() {
  local choice="" iface="" action="" mtu="" id=""
  while true; do
    menu_header "网络安全工作台" "双栈诊断 · 路由测试 · 网卡管理 · SHA-256 · 超时回滚"
    menu_section "只读诊断"
    menu_item 1 "完整网络快照" "IPv4/IPv6、DNS、网卡、队列和防火墙指纹"
    menu_item 2 "路由与链路测试" "route get、ping、tracepath（如已安装）"
    menu_item 3 "防火墙策略 SHA-256" "按语义剔除计数器与规则 handle"
    menu_item 4 "事务列表" "查看待确认、已回滚和已确认的操作"
    menu_section "受保护变更（必须支持 systemd 定时器）"
    menu_item 5 "网卡 UP/DOWN" "禁止关闭默认出口与 SSH 回程网卡"
    menu_item 6 "设置网卡 MTU" "原值快照 + 自动回滚，暂不自动持久化"
    menu_item 7 "事务确认或手动回滚" "推荐从新的 SSH 会话执行 --net-confirm"
    menu_back_item
    read_tty choice "请输入编号 > " ""
    case "$choice" in
      1) netg_report || true; pause_up ;;
      2) netg_route_test || true; pause_up ;;
      3) netg_firewall_fingerprint || warn "无法取得完整 netfilter 状态"; pause_up ;;
      4) netg_tx_list; pause_up ;;
      5)
        read_tty iface "接口名称 > " ""
        read_tty action "目标状态（up/down）> " ""
        netg_iface_set_safe "$iface" "$action" || true
        pause_up ;;
      6)
        read_tty iface "接口名称 > " ""
        read_tty mtu "MTU（1280-9000）> " ""
        netg_mtu_set_safe "$iface" "$mtu" || true
        pause_up ;;
      7)
        netg_tx_list
        read_tty id "事务 ID > " ""
        read_tty action "输入 confirm 或 rollback > " ""
        case "$action" in
          confirm) netg_tx_confirm "$id" || true ;;
          rollback) netg_tx_rollback_now "$id" || true ;;
          *) warn "无效操作" ;;
        esac
        pause_up ;;
      0|q|Q) return 0 ;;
      *) warn "无效选项"; pause_up ;;
    esac
  done
}

ipv6_prefix64_normalize() {
  local raw="${1%%/*}" left="" right="" missing=0 part=""
  local -a full=() left_parts=() right_parts=()

  raw="${raw%%%*}"
  [[ -n "$raw" && "$raw" != *.* ]] || return 1

  if [[ "$raw" == *::* ]]; then
    [[ "${raw#*::}" != *::* ]] || return 1
    left="${raw%%::*}"
    right="${raw#*::}"
    [[ -z "$left" ]] || IFS=':' read -r -a left_parts <<< "$left"
    [[ -z "$right" ]] || IFS=':' read -r -a right_parts <<< "$right"
    missing=$((8 - ${#left_parts[@]} - ${#right_parts[@]}))
    (( missing >= 1 )) || return 1
    full=("${left_parts[@]}")
    while (( missing > 0 )); do
      full+=("0")
      missing=$((missing - 1))
    done
    full+=("${right_parts[@]}")
  else
    IFS=':' read -r -a full <<< "$raw"
    (( ${#full[@]} == 8 )) || return 1
  fi

  (( ${#full[@]} == 8 )) || return 1
  for part in "${full[@]}"; do
    [[ "$part" =~ ^[0-9A-Fa-f]{1,4}$ ]] || return 1
  done

  printf '%x:%x:%x:%x\n' \
    "$((16#${full[0]}))" "$((16#${full[1]}))" \
    "$((16#${full[2]}))" "$((16#${full[3]}))"
}

ipv6_prefix64_guess() {
  local ifc="${1:-$(default_iface)}"
  local a=""
  a="$(ip -6 addr show dev "$ifc" scope global 2>/dev/null | awk '/inet6/{print $2}' | grep -E '/64$' | head -n1 || true)"
  if [[ -n "$a" ]]; then
    ipv6_prefix64_normalize "$a" && return 0
  fi
  a="$(ip -6 route show 2>/dev/null | awk -v i="$ifc" '$1 ~ /\/64$/ && $0 ~ ("dev " i) {print $1; exit}' || true)"
  if [[ -n "$a" ]]; then
    ipv6_prefix64_normalize "$a" && return 0
  fi
  return 1
}

ipv6_list_global_128() {
  local ifc="${1:-$(default_iface)}"
  ip -6 addr show dev "$ifc" scope global 2>/dev/null \
    | awk '/inet6/{print $2}' \
    | grep -E '/128$' \
    | sed 's#/128##g'
}

ipv6_addr_exists() {
  local ifc="${1:-$(default_iface)}" addr="$2"
  ip -6 addr show dev "$ifc" 2>/dev/null | grep -q "inet6 ${addr}/128"
}

ipv6_rand_host_64() {
  if have_cmd hexdump; then
    hexdump -n8 -e '4/2 "%04x " 1' /dev/urandom 2>/dev/null | awk '{print $1 ":" $2 ":" $3 ":" $4}'
    return 0
  fi
  printf "%04x:%04x:%04x:%04x" $((RANDOM%65536)) $((RANDOM%65536)) $((RANDOM%65536)) $((RANDOM%65536))
}

ipv6_add_128() {
  local addr="$1" ifc="${2:-$(default_iface)}"
  local valid="${3:-forever}" pref="${4:-forever}"
  if [[ "$valid" == "forever" ]]; then
    ip -6 addr add "${addr}/128" dev "$ifc" >/dev/null 2>&1 || return 1
  else
    ip -6 addr add "${addr}/128" dev "$ifc" valid_lft "$valid" preferred_lft "$pref" >/dev/null 2>&1 || return 1
  fi
  ok "已添加：${addr}/128  (dev ${ifc})"
  return 0
}

ipv6_del_128() {
  local addr="$1" ifc="${2:-$(default_iface)}"
  ip -6 addr del "${addr}/128" dev "$ifc" >/dev/null 2>&1 || true
  ok "已删除：${addr}/128"
}

ipv6_gen_n_128() {
  local n="$1" mode="${2:-persist}"
  is_uint_in_range "$n" 1 256 || { warn "数量必须在 1-256 之间"; return 1; }
  local ifc; ifc="$(default_iface)"
  local p64; p64="$(ipv6_prefix64_guess "$ifc" || true)"
  [[ -n "${p64:-}" ]] || { warn "未识别到 /64 前缀（请确认有 /64 地址或 ::/64 路由）"; return 1; }

  local valid="forever" pref="forever"
  if [[ "$mode" == "temp" ]]; then
    valid="3600"
    pref="1200"
  fi

  local made=0 tries=0
  while [[ "$made" -lt "$n" && "$tries" -lt $((n*50)) ]]; do
    tries=$((tries+1))
    local host; host="$(ipv6_rand_host_64)"
    local addr="${p64}:${host}"
    if ipv6_addr_exists "$ifc" "$addr"; then
      continue
    fi
    if ipv6_add_128 "$addr" "$ifc" "$valid" "$pref"; then
      made=$((made+1))
    fi
  done

  if [[ "$made" -lt "$n" ]]; then
    warn "只生成了 ${made}/${n} 个（可能系统限制或重复过多）"
    return 1
  fi
  ok "完成：生成 ${made} 个 /128（${mode}）"
}

ipv6_pool_write_conf() {
  local ifc="$1" prefix64="$2" n="$3" i=0 body=""
  shift 3
  local -a addrs=("$@")
  is_uint_in_range "$n" 1 256 || return 1
  (( ${#addrs[@]} == n )) || return 1
  [[ "$ifc" =~ ^[A-Za-z0-9_.:-]{1,32}$ ]] || return 1
  [[ "$prefix64" =~ ^[0-9A-Fa-f:]{2,39}$ ]] || return 1
  body="IFACE=${ifc}
PREFIX64=${prefix64}
N=${n}"
  for ((i=0; i<n; i++)); do
    [[ "${addrs[i]}" =~ ^[0-9A-Fa-f:]{2,39}$ ]] || return 1
    body+=$'\n'"ADDR_${i}=${addrs[i]}"
  done
  write_file "$IPV6_POOL_CONF" "$body" 0600
}

ipv6_load_managed_conf() {
  local file="$1" line="" key="" value="" i=0 count=0
  local new_iface="" new_prefix="" new_n="" valid_addr=""
  local -a entries=()
  [[ -f "$file" && ! -L "$file" ]] || return 1
  [[ $(wc -c < "$file") -le 65536 ]] || return 1
  while IFS= read -r line || [[ -n "$line" ]]; do
    [[ -n "$line" && "$line" != \#* ]] || continue
    [[ "$line" == *=* ]] || return 1
    key="${line%%=*}"; value="${line#*=}"
    case "$key" in
      IFACE)
        [[ "$value" =~ ^[A-Za-z0-9_.:-]{1,32}$ ]] || return 1
        new_iface="$value" ;;
      PREFIX64)
        [[ "$value" =~ ^[0-9A-Fa-f:]{2,39}(/64)?$ ]] || return 1
        new_prefix="$value" ;;
      N)
        is_uint_in_range "$value" 1 256 || return 1
        new_n=$((10#$value)) ;;
      ADDR_*)
        [[ "$key" =~ ^ADDR_([0-9]{1,3})$ ]] || return 1
        i=$((10#${BASH_REMATCH[1]}))
        (( i < 256 )) || return 1
        [[ "$value" =~ ^[0-9A-Fa-f:]{2,39}$ && "$value" == *:* ]] || return 1
        entries[i]="$value" ;;
      *) return 1 ;;
    esac
  done < "$file"
  [[ -n "$new_iface" && -n "$new_prefix" && -n "$new_n" ]] || return 1
  (( ${#entries[@]} == new_n )) || return 1
  for ((i=0; i<new_n; i++)); do
    [[ -n "${entries[i]:-}" ]] || return 1
  done
  IFACE="$new_iface" PREFIX64="$new_prefix" N="$new_n"
  # Any previous active variables are reset to avoid stale entries after a
  # configuration shrinks from 256 to a smaller pool.
  for ((i=0; i<256; i++)); do unset "ADDR_${i}"; done
  for ((i=0; i<new_n; i++)); do
    printf -v "ADDR_${i}" '%s' "${entries[i]}"
  done
  return 0
}

ipv6_pool_load_conf() {
  ipv6_load_managed_conf "$IPV6_POOL_CONF"
}

ipv6_pool_apply_from_conf() {
  local i=0 key="" addr="" failed=0
  ipv6_pool_load_conf || return 1
  for ((i=0; i<N; i++)); do
    key="ADDR_${i}"; addr="${!key:-}"
    if ! ipv6_addr_exists "$IFACE" "$addr"; then
      if ! ip -6 addr add "${addr}/128" dev "$IFACE" >/dev/null 2>&1; then
        warn "IPv6 地址添加失败：${addr}/128 (${IFACE})"
        failed=$((failed + 1))
      fi
    fi
  done
  (( failed == 0 )) || return 1
  ok "已验证 IPv6 地址池应用请求（接口 ${IFACE}）"
}

ipv6_pool_persist_enable() {
  local ip_bin="" unit="" i=0 addr="" key=""
  is_systemd || { warn "无 systemd：地址池仅在运行时生效"; return 0; }
  ipv6_pool_load_conf || { warn "地址池配置校验失败"; return 1; }
  ip_bin="$(command -v ip)" || return 1
  [[ "$ip_bin" == /* && "$ip_bin" != *' '* ]] || return 1
  unit="# managed by ${SCRIPT_NAME}
[Unit]
Description=DMIT IPv6 Pool Apply
After=network-online.target
Wants=network-online.target

[Service]
Type=oneshot
RemainAfterExit=yes"
  for ((i=0; i<N; i++)); do
    key="ADDR_${i}"; addr="${!key:-}"
    [[ "$addr" =~ ^[0-9A-Fa-f:]{2,39}$ ]] || return 1
    unit+=$'\n'"ExecStart=${ip_bin} -6 addr replace ${addr}/128 dev ${IFACE}"
  done
  unit+=$'\n\n'"[Install]
WantedBy=multi-user.target"
  write_file "$IPV6_POOL_SERVICE" "$unit" || return 1
  systemctl daemon-reload >/dev/null 2>&1 || return 1
  systemctl enable --now dmit-ipv6-pool.service >/dev/null 2>&1 || return 1
  systemctl restart dmit-ipv6-pool.service >/dev/null 2>&1 || return 1
  systemctl is-active --quiet dmit-ipv6-pool.service || return 1
  ok "地址池持久化已启用：dmit-ipv6-pool.service"
}

ipv6_pool_disable() {
  if ipv6_pool_load_conf; then
    local i
    for ((i=0;i<N;i++)); do
      local v="ADDR_${i}"
      local addr="${!v:-}"
      [[ -n "$addr" ]] || continue
      ip -6 addr del "${addr}/128" dev "$IFACE" >/dev/null 2>&1 || true
    done
  fi

  rm -f "$IPV6_POOL_CONF" >/dev/null 2>&1 || true

  if is_systemd; then
    systemctl disable dmit-ipv6-pool.service >/dev/null 2>&1 || true
    systemctl stop dmit-ipv6-pool.service >/dev/null 2>&1 || true
    rm -f "$IPV6_POOL_SERVICE" >/dev/null 2>&1 || true
    systemctl daemon-reload >/dev/null 2>&1 || true
    # systemd 会保留已删除单元的失败状态；同步清理，避免健康检查显示
    # "not-found failed"。这里只处理本脚本管理的单元，不影响其他服务。
    systemctl reset-failed dmit-ipv6-pool.service >/dev/null 2>&1 || true
  fi
  ok "已关闭 IPv6 地址池（并清理持久化）"
}

ipv6_pool_status() {
  echo -e "${c_bold}${c_white}IPv6 地址池状态${c_reset}"
  local ifc; ifc="$(default_iface)"
  local p64; p64="$(ipv6_prefix64_guess "$ifc" || true)"
  echo -e "${c_dim}IFACE:${c_reset} ${ifc}"
  echo -e "${c_dim}PREFIX64:${c_reset} ${p64:-unknown}"
  echo
  echo -e "${c_dim}当前网卡 /64 与 /128：${c_reset}"
  ip -6 addr show dev "$ifc" scope global 2>/dev/null | sed -n '1,200p' || true
  echo
  echo -e "${c_dim}当前 /128 列表：${c_reset}"
  ipv6_list_global_128 "$ifc" || true
  echo
  if [[ -f "$IPV6_POOL_CONF" ]]; then
    echo -e "${c_dim}池配置：${c_reset} ${IPV6_POOL_CONF}"
    sed -n '1,120p' "$IPV6_POOL_CONF" 2>/dev/null || true
  else
    echo -e "${c_dim}池配置：${c_reset} (未启用)"
  fi
}

# ---------- 随机出网（每个新连接随机 /128） ----------
ipv6_rand_write_conf() {
  local ifc="$1" prefix64="$2" n="$3" i=0 body=""
  shift 3
  local -a addrs=("$@")
  is_uint_in_range "$n" 2 256 || return 1
  (( ${#addrs[@]} == n )) || return 1
  [[ "$ifc" =~ ^[A-Za-z0-9_.:-]{1,32}$ ]] || return 1
  [[ "$prefix64" =~ ^[0-9A-Fa-f:]{2,39}$ ]] || return 1
  body="IFACE=${ifc}
PREFIX64=${prefix64}::/64
N=${n}"
  for ((i=0; i<n; i++)); do
    [[ "${addrs[i]}" =~ ^[0-9A-Fa-f:]{2,39}$ ]] || return 1
    body+=$'\n'"ADDR_${i}=${addrs[i]}"
  done
  write_file "$IPV6_RAND_CONF" "$body" 0600
}

ipv6_rand_load_conf() {
  ipv6_load_managed_conf "$IPV6_RAND_CONF"
}

ipv6_rand_render_nft() {
  {
    echo "table inet dmitbox_rand6 {"
    echo "  chain outmark {"
    echo "    type route hook output priority mangle; policy accept;"
    echo "    ct state new oifname \"${IFACE}\" ip6 daddr != ${PREFIX64} ip6 daddr != fe80::/10 ip6 daddr != ff00::/8 ct mark set numgen random mod ${N};"
    echo "  }"
    echo "  chain post {"
    echo "    type nat hook postrouting priority srcnat; policy accept;"
    local i
    for ((i=0;i<N;i++)); do
      local addr_var="ADDR_${i}"
      local addr_val="${!addr_var:-}"
      echo "    oifname \"${IFACE}\" ct mark ${i} ip6 daddr != ${PREFIX64} ip6 daddr != fe80::/10 ip6 daddr != ff00::/8 snat to ${addr_val};"
    done
    echo "  }"
    echo "}"
  }
}

ipv6_rand_apply_nft_runtime() {
  pkg_install nftables >/dev/null 2>&1 || true
  have_cmd nft || { warn "未找到 nft 命令，无法启用随机出网 IPv6"; return 1; }

  mkdir -p "$(dirname "$IPV6_RAND_NFT")" >/dev/null 2>&1 || true
  ipv6_rand_render_nft > "$IPV6_RAND_NFT"

  if ! nft -c -f "$IPV6_RAND_NFT" >/dev/null 2>&1; then
    warn "nft 规则语法校验失败：$IPV6_RAND_NFT"
    echo
    nl -ba "$IPV6_RAND_NFT" | sed -n '1,200p'
    echo
    warn "你也可以手动跑：nft -c -f $IPV6_RAND_NFT"
    return 1
  fi

  nft delete table inet dmitbox_rand6 >/dev/null 2>&1 || true
  if ! nft -f "$IPV6_RAND_NFT" >/dev/null 2>&1; then
    warn "nft 规则加载失败：$IPV6_RAND_NFT"
    return 1
  fi

  ok "已启用（runtime）：每个新连接随机选择出网 IPv6（N=${N}）"
  return 0
}

ipv6_rand_persist_systemd() {
  local nft_bin=""
  is_systemd || { warn "无 systemd：随机出网仅运行时生效"; return 0; }
  nft_bin="$(command -v nft)" || return 1
  [[ "$nft_bin" == /* && "$nft_bin" != *' '* ]] || return 1
  write_file "$IPV6_RAND_SERVICE" "# managed by ${SCRIPT_NAME}
[Unit]
Description=DMIT IPv6 Random Outbound
After=network-online.target
Wants=network-online.target

[Service]
Type=oneshot
ExecStartPre=-${nft_bin} delete table inet dmitbox_rand6
ExecStart=${nft_bin} -f ${IPV6_RAND_NFT}
RemainAfterExit=yes

[Install]
WantedBy=multi-user.target" || return 1
  systemctl daemon-reload >/dev/null 2>&1 || return 1
  systemctl enable --now dmit-ipv6-rand.service >/dev/null 2>&1 || return 1
  systemctl restart dmit-ipv6-rand.service >/dev/null 2>&1 || return 1
  systemctl is-active --quiet dmit-ipv6-rand.service || return 1
  ok "随机出网已持久化：dmit-ipv6-rand.service"
}

ipv6_rand_enable_from_pool() {
  local want_n="$1"
  is_uint_in_range "$want_n" 2 256 || { warn "随机池大小必须在 2-256 之间"; return 1; }
  local ifc; ifc="$(default_iface)"
  local p64; p64="$(ipv6_prefix64_guess "$ifc" || true)"
  [[ -n "${p64:-}" ]] || { warn "未识别到 /64 前缀（prefix64）"; return 1; }

  local addrs=()
  while IFS= read -r ip; do
    [[ -n "$ip" ]] && addrs+=("$ip")
  done < <(ipv6_list_global_128 "$ifc")

  if [[ "${#addrs[@]}" -lt "$want_n" ]]; then
    warn "当前 /128 数量不足：需要 ${want_n} 个，但只有 ${#addrs[@]} 个"
    warn "请先在 IPv6 地址池里新增一些 /128，再启用随机出网"
    return 1
  fi

  local chosen=("${addrs[@]:0:$want_n}")
  ipv6_rand_write_conf "$ifc" "$p64" "$want_n" "${chosen[@]}" || return 1
  ipv6_rand_load_conf || { warn "写入配置失败"; return 1; }
  ipv6_rand_apply_nft_runtime || return 1
  ipv6_rand_persist_systemd || warn "随机出网仅在运行时生效"

  echo -e "${c_dim}已使用以下出网 IPv6 池：${c_reset}"
  printf "%s\n" "${chosen[@]}"
  return 0
}

ipv6_rand_disable() {
  if have_cmd nft; then
    nft delete table inet dmitbox_rand6 >/dev/null 2>&1 || true
  fi

  rm -f "$IPV6_RAND_NFT" >/dev/null 2>&1 || true
  rm -f "$IPV6_RAND_CONF" >/dev/null 2>&1 || true

  if is_systemd; then
    systemctl disable dmit-ipv6-rand.service >/dev/null 2>&1 || true
    systemctl stop dmit-ipv6-rand.service >/dev/null 2>&1 || true
    rm -f "$IPV6_RAND_SERVICE" >/dev/null 2>&1 || true
    systemctl daemon-reload >/dev/null 2>&1 || true
  fi

  ok "已关闭随机出网 IPv6（并清理持久化）"
}

ipv6_rand_selftest() {
  local n="${1:-10}"
  echo "IPv6 NAT66 本地运行态检查（不产生外部请求）"
  [[ -f "$IPV6_RAND_CONF" && ! -L "$IPV6_RAND_CONF" ]] || {
    warn "随机出网配置不存在"; return 1;
  }
  have_cmd nft || { warn "nft 不可用，不能检查运行态"; return 1; }
  if ! nft list table inet dmitbox_rand6; then
    warn "随机出网 nft 表不存在或无法读取"; return 1;
  fi
  ip -6 route show default 2>/dev/null || true
  info "本地检查只能证明规则存在，不能证明不同连接的公网源 IPv6 已被上游路由。"
  info "如需确认真实出网地址，请手动运行外部回显测试；脚本本身不向第三方发送公网 IP。"
}

ipv6_rand_status() {
  echo -e "${c_bold}${c_white}随机出网 IPv6 状态${c_reset}"
  if [[ -f "$IPV6_RAND_CONF" ]]; then
    echo -e "${c_dim}配置：${c_reset}${IPV6_RAND_CONF}"
    sed -n '1,120p' "$IPV6_RAND_CONF" 2>/dev/null || true
  else
    echo -e "${c_dim}未启用（配置文件不存在）${c_reset}"
  fi
  echo
  echo -e "${c_dim}nft 规则：${c_reset}"
  if have_cmd nft; then
    nft list table inet dmitbox_rand6 2>/dev/null || echo "(无)"
    echo
    echo -e "${c_dim}语法校验（nft -c）：${c_reset}"
    nft -c -f "$IPV6_RAND_NFT" >/dev/null 2>&1 && echo "OK" || echo "FAIL (查看：nl -ba $IPV6_RAND_NFT | sed -n '1,120p')"
  else
    echo "(未安装 nft)"
  fi
  echo
  info "本模式只提供本地 nftables 检查；源地址公网验证请由操作员自行发起。"
}

ipv6_pool_generate_and_enable_rand() {
  local n="$1"
  is_uint_in_range "$n" 2 256 || { warn "数量必须在 2-256 之间"; return 1; }
  local ifc; ifc="$(default_iface)"
  local p64; p64="$(ipv6_prefix64_guess "$ifc" || true)"
  [[ -n "${p64:-}" ]] || { warn "未识别到 /64 前缀"; return 1; }

  info "一键：生成 ${n} 个 /128（持久）并启用随机出网"
  local made=0 tries=0
  local addrs=()
  while [[ "$made" -lt "$n" && "$tries" -lt $((n*60)) ]]; do
    tries=$((tries+1))
    local host; host="$(ipv6_rand_host_64)"
    local addr="${p64}:${host}"
    if ipv6_addr_exists "$ifc" "$addr"; then
      continue
    fi
    if ipv6_add_128 "$addr" "$ifc" "forever" "forever"; then
      addrs+=("$addr")
      made=$((made+1))
    fi
  done

  if [[ "$made" -lt "$n" ]]; then
    warn "只生成了 ${made}/${n} 个"
    return 1
  fi

  ipv6_pool_write_conf "$ifc" "$p64" "$n" "${addrs[@]}" || {
    warn "地址已生成但持久化配置写入失败，请人工检查 /128 地址"
    return 1
  }
  ipv6_pool_persist_enable || warn "地址池仅在运行时生效"

  ipv6_rand_write_conf "$ifc" "$p64" "$n" "${addrs[@]}" || return 1
  if ! ipv6_rand_load_conf || ! ipv6_rand_apply_nft_runtime; then
    warn "IPv6 地址池已生成，但随机出网启用失败；请查看菜单中的随机出网状态"
    return 1
  fi
  ipv6_rand_persist_systemd || warn "随机出网仅在运行时生效"

  ok "完成：已生成 /128 池并启用随机出网"
}

ipv6_tools_menu() {
  local ifc; ifc="$(default_iface)"
  while true; do
    menu_header "IPv6 /64 地址池" "网卡：${ifc} · /128 地址与随机出网管理"
    menu_section "地址池"
    menu_item "1" "查看地址池状态" "/64 前缀、/128 地址与持久化配置"
    menu_item "2" "新增长期 /128" "运行时长期有效，重启后失效"
    menu_item "3" "新增临时 /128" "1 小时有效"
    menu_item "4" "删除一个 /128" "手动输入地址"
    menu_item "5" "关闭并清理地址池" "删除池内 /128 并取消持久化"
    menu_section "随机出网"
    menu_item "6" "从现有地址启用" "选择前 N 个 /128"
    menu_item "7" "一键生成并启用" "推荐：生成 N 个 /128 后立即启用"
    menu_item "8" "查看运行状态" "配置、nft 规则与语法检查"
    menu_item "9" "运行自检" "连续请求并检查源 IPv6 是否变化"
    menu_item "10" "关闭随机出网" "清理 nft 规则与持久化服务"
    menu_back_item
    local c=""
    read_tty c "请输入编号 > " ""

    case "$c" in
      1) ipv6_pool_status; pause_up ;;
      2)
        local n=""
        read_tty n "生成多少个 /128（默认 3）> " "3"
        is_uint_in_range "$n" 1 256 || { warn "数量必须在 1-256 之间"; pause_up; continue; }
        ipv6_gen_n_128 "$n" "persist" || true
        pause_up
        ;;
      3)
        local n=""
        read_tty n "生成多少个 /128（默认 1）> " "1"
        is_uint_in_range "$n" 1 256 || { warn "数量必须在 1-256 之间"; pause_up; continue; }
        ipv6_gen_n_128 "$n" "temp" || true
        pause_up
        ;;
      4)
        local a=""
        read_tty a "输入要删除的 /128（如 2605:...:....）> " ""
        [[ -n "$a" ]] || { warn "不能为空"; pause_up; continue; }
        ipv6_del_128 "$a" "$ifc" || true
        pause_up
        ;;
      5) ipv6_pool_disable || true; pause_up ;;
      6)
        local n=""
        read_tty n "随机池大小 N（建议 3~10，默认 5）> " "5"
        is_uint_in_range "$n" 2 256 || { warn "N 必须在 2-256 之间"; pause_up; continue; }
        ipv6_rand_enable_from_pool "$n" || true
        pause_up
        ;;
      7)
        local n=""
        read_tty n "生成并随机出网：N（建议 3~10，默认 5）> " "5"
        is_uint_in_range "$n" 2 256 || { warn "N 必须在 2-256 之间"; pause_up; continue; }
        ipv6_pool_generate_and_enable_rand "$n" || true
        pause_up
        ;;
      8) ipv6_rand_status; pause_up ;;
      9)
        local n=""
        read_tty n "自检次数（默认 10）> " "10"
        is_uint_in_range "$n" 2 100 || n="10"
        ipv6_rand_selftest "$n" || true
        pause_up
        ;;
      10) ipv6_rand_disable || true; pause_up ;;
      0|q|Q) return 0 ;;
      *) warn "无效选项"; pause_up ;;
    esac
  done
}


# ======================================================================
# Cloud-init / QEMU Guest Agent（换 IP 防失联）
# ======================================================================

# --- cloud-init safety helpers ---
vm_guest_status() {
  menu_header "KVM Guest Agent 检查（只读）" "不接管网络、不重置 cloud-init 状态"
  print_kv "虚拟化" "$(systemd-detect-virt 2>/dev/null || echo unknown)"
  if have_cmd cloud-init; then
    command_with_timeout 8 cloud-init status --long 2>/dev/null || true
  else
    info "未安装 cloud-init"
  fi
  if is_systemd; then
    systemctl status qemu-guest-agent --no-pager --lines=4 2>/dev/null || true
  else
    info "当前 init 不支持此处的 systemd Guest Agent 查询"
  fi
  ip -br link show 2>/dev/null || true
}
vm_guest_install() {
  confirm_word INSTALL "安装本机 cloud-init 和 QEMU Guest Agent（不修改网络）请输入 INSTALL > " || return 0
  pkg_install cloud-init qemu-guest-agent || return 1
  if is_systemd; then
    systemctl enable --now qemu-guest-agent >/dev/null 2>&1 || warn "Guest Agent 启动失败；检查服务与 hypervisor 通道"
  fi
  info "未启用 cloud-init 网络接管或清理状态；请由服务商控制台执行换 IP 操作"
}
vm_guest_menu() {
  local c=""
  while true; do
    menu_header "虚拟化来宾组件" "只在本机运行；不修改网卡、云元数据或引导配置"
    menu_item 1 "检查 cloud-init / QGA" "只读状态"
    menu_item 2 "安装 cloud-init / QGA" "用户确认，安装后不触发 cloud-init clean"
    menu_back_item
    read_tty c "请输入编号 > " ""
    case "$c" in
      1) vm_guest_status || true; pause_up ;;
      2) vm_guest_install || true; pause_up ;;
      0|q|Q) return 0 ;;
      *) warn "无效选项"; pause_up ;;
    esac
  done
}

ssh_pkg_install() {
  if ! have_cmd sshd && [[ -x /usr/sbin/sshd ]]; then
    export PATH="$PATH:/usr/sbin:/sbin"
  fi
  if have_cmd sshd && have_cmd ssh-keygen; then return 0; fi
  if have_cmd apk; then
    pkg_install openssh
  elif have_cmd apt-get; then
    pkg_install openssh-server openssh-client
  elif have_cmd dnf || have_cmd yum; then
    pkg_install openssh-server openssh-clients
  else
    pkg_install openssh-server
  fi
  if ! have_cmd sshd && [[ -x /usr/sbin/sshd ]]; then
    export PATH="$PATH:/usr/sbin:/sbin"
  fi
  have_cmd sshd || { warn "OpenSSH Server 安装失败，未继续修改 SSH"; return 1; }
  have_cmd ssh-keygen || { warn "OpenSSH 客户端工具安装失败，未继续修改 SSH"; return 1; }
}

ssh_backup_once() {
  [[ -d /etc/ssh && ! -L /etc/ssh ]] || { warn "/etc/ssh 不是可安全备份的普通目录"; return 1; }
  [[ ! -e /etc/ssh/sshd_config || ( -f /etc/ssh/sshd_config && ! -L /etc/ssh/sshd_config ) ]] || {
    warn "sshd_config 不是可安全备份的普通文件"
    return 1
  }
  [[ ! -e "$SSH_DROPIN_DIR" || ( -d "$SSH_DROPIN_DIR" && ! -L "$SSH_DROPIN_DIR" ) ]] || {
    warn "sshd_config.d 不是可安全管理的普通目录"
    return 1
  }
  have_cmd find || { warn "系统缺少 find，无法安全检查 SSH 配置路径"; return 1; }
  if [[ -n "$(find /etc/ssh -type l -print -quit 2>/dev/null || true)" ]]; then
    warn "/etc/ssh 中存在符号链接，为避免备份或恢复越界，已取消自动修改"
    return 1
  fi
  ensure_dir "$BACKUP_BASE"
  chmod 700 "$BACKUP_BASE" >/dev/null 2>&1 || { warn "无法保护 SSH 备份目录权限"; return 1; }
  [[ ! -e "$SSH_ORIG_TGZ" ]] || chmod 600 "$SSH_ORIG_TGZ" >/dev/null 2>&1 || return 1
  [[ ! -e "$SSH_DROPIN_ORIG_MARKER" ]] || chmod 600 "$SSH_DROPIN_ORIG_MARKER" >/dev/null 2>&1 || return 1
  if [[ -s "$SSH_ORIG_TGZ" ]] && ! ssh_backup_archive_safe "$SSH_ORIG_TGZ"; then
    warn "现有 SSH 原始备份损坏或范围异常，已取消配置修改：$SSH_ORIG_TGZ"
    return 1
  fi
  if [[ ! -s "$SSH_ORIG_TGZ" ]]; then
    ssh_validate_config || { warn "当前 SSH 配置无效，未创建原始备份，也未继续修改"; return 1; }
    info "SSH：备份原始配置 → $SSH_ORIG_TGZ"
    if [[ -e "$SSH_DROPIN_FILE" || -e "$SSH_LEGACY_DROPIN_FILE" ]]; then
      write_file "$SSH_DROPIN_ORIG_MARKER" "1"
    else
      write_file "$SSH_DROPIN_ORIG_MARKER" "0"
    fi
    chmod 600 "$SSH_DROPIN_ORIG_MARKER" >/dev/null 2>&1 || return 1
    local paths=()
    local relative_paths=() path=""
    [[ -f /etc/ssh/sshd_config ]] && paths+=(/etc/ssh/sshd_config)
    [[ -d /etc/ssh/sshd_config.d ]] && paths+=(/etc/ssh/sshd_config.d)
    if (( ${#paths[@]} == 0 )); then
      warn "未找到 SSH 配置，无法创建备份"
      return 1
    fi
    for path in "${paths[@]}"; do relative_paths+=("${path#/}"); done
    if ! ( umask 077; tar -C / -czf "$SSH_ORIG_TGZ" "${relative_paths[@]}" ) 2>/dev/null; then
      rm -f "$SSH_ORIG_TGZ" >/dev/null 2>&1 || true
      rm -f "$SSH_DROPIN_ORIG_MARKER" >/dev/null 2>&1 || true
      warn "SSH 配置备份失败，已取消后续修改"
      return 1
    fi
    if ! ssh_backup_archive_safe "$SSH_ORIG_TGZ"; then
      rm -f "$SSH_ORIG_TGZ" "$SSH_DROPIN_ORIG_MARKER" >/dev/null 2>&1 || true
      warn "SSH 配置含符号链接、特殊文件或异常路径，未保留不安全备份"
      return 1
    fi
    chmod 600 "$SSH_ORIG_TGZ" >/dev/null 2>&1 || { warn "无法保护 SSH 备份文件权限"; return 1; }
    ok "SSH 原始配置已备份"
  fi
}

ssh_backup_archive_safe() {
  local archive="${1:-}" member="" found=0
  [[ -s "$archive" ]] || return 1
  tar -tzf "$archive" >/dev/null 2>&1 || return 1
  while IFS= read -r member; do
    [[ -n "$member" ]] || continue
    found=1
    [[ "$member" != /* ]] || return 1
    [[ "/$member/" != *"/../"* ]] || return 1
    [[ "$member" == "etc/ssh" || "$member" == etc/ssh/* ]] || return 1
  done < <(tar -tzf "$archive" 2>/dev/null) || return 1
  if tar -tvzf "$archive" 2>/dev/null | awk 'substr($1,1,1) !~ /^[-d]$/ {unsafe=1} END{exit !unsafe}'; then
    return 1
  fi
  (( found == 1 ))
}

ssh_backup_archive_has_member() {
  local archive="$1" wanted="$2" member=""
  ssh_backup_archive_safe "$archive" || return 1
  while IFS= read -r member; do
    [[ "${member%/}" == "${wanted%/}" ]] && return 0
  done < <(tar -tzf "$archive" 2>/dev/null)
  return 1
}

ssh_validate_config() {
  have_cmd sshd || { warn "未找到 sshd，无法校验配置"; return 1; }
  [[ -d /run ]] && mkdir -p /run/sshd >/dev/null 2>&1 || true
  local output=""
  output="$(sshd -t 2>&1)" || {
    warn "sshd 配置校验失败"
    [[ -n "$output" ]] && printf '%s\n' "$output" | sed -n '1,12p'
    return 1
  }
}

ssh_service_name() {
  local name=""
  if is_systemd; then
    for name in ssh sshd; do
      command_with_timeout 6 systemctl is-active --quiet "${name}.service" >/dev/null 2>&1 && {
        printf '%s\n' "$name"
        return 0
      }
    done
    for name in ssh sshd; do
      command_with_timeout 6 systemctl cat "${name}.service" >/dev/null 2>&1 && {
        printf '%s\n' "$name"
        return 0
      }
    done
  elif have_cmd rc-service; then
    for name in sshd ssh; do
      [[ -x "/etc/init.d/${name}" ]] && { printf '%s\n' "$name"; return 0; }
    done
  elif have_cmd service; then
    for name in ssh sshd; do
      service "$name" status >/dev/null 2>&1 && { printf '%s\n' "$name"; return 0; }
    done
    for name in ssh sshd; do
      [[ -x "/etc/init.d/${name}" ]] && { printf '%s\n' "$name"; return 0; }
    done
  fi
  return 1
}

sshd_restart() {
  ssh_validate_config || return 1
  local service_name=""
  service_name="$(ssh_service_name 2>/dev/null || true)"
  [[ -n "$service_name" ]] || { warn "未找到可管理的 SSH 服务，配置未重新加载"; return 1; }

  if is_systemd; then
    if command_with_timeout 20 systemctl is-active --quiet "${service_name}.service" >/dev/null 2>&1; then
      command_with_timeout 25 systemctl reload "${service_name}.service" >/dev/null 2>&1 || \
        command_with_timeout 25 systemctl restart "${service_name}.service" >/dev/null 2>&1 || return 1
    else
      command_with_timeout 25 systemctl start "${service_name}.service" >/dev/null 2>&1 || return 1
    fi
    command_with_timeout 8 systemctl is-active --quiet "${service_name}.service" >/dev/null 2>&1 || return 1
  elif have_cmd rc-service; then
    command_with_timeout 25 rc-service "$service_name" reload >/dev/null 2>&1 || \
      command_with_timeout 25 rc-service "$service_name" restart >/dev/null 2>&1 || return 1
    command_with_timeout 8 rc-service "$service_name" status >/dev/null 2>&1 || return 1
  else
    command_with_timeout 25 service "$service_name" reload >/dev/null 2>&1 || \
      command_with_timeout 25 service "$service_name" restart >/dev/null 2>&1 || return 1
    command_with_timeout 8 service "$service_name" status >/dev/null 2>&1 || return 1
  fi
}

sshd_status_hint() {
  echo -e "${c_dim}--- SSH 当前生效配置（节选）---${c_reset}"
  if have_cmd sshd; then
    command_with_timeout 8 sshd -T 2>/dev/null | grep -Ei \
      '^(port|passwordauthentication|permitrootlogin|pubkeyauthentication|authenticationmethods|kbdinteractiveauthentication|challengeresponseauthentication|usepam|maxauthtries|logingracetime|permituserenvironment|x11forwarding|allowtcpforwarding|authorizedkeysfile)[[:space:]]' || true
  else
    warn "未找到 sshd 命令，改为简单 grep："
    grep -ERini 'Port|PasswordAuthentication|PubkeyAuthentication|KbdInteractiveAuthentication|ChallengeResponseAuthentication|UsePAM|PermitRootLogin|AuthenticationMethods' /etc/ssh/sshd_config /etc/ssh/sshd_config.d 2>/dev/null || true
  fi
  echo -e "${c_dim}--------------------------------${c_reset}"
}

ssh_effective_config() {
  local user="${1:-root}" port="22" addr="127.0.0.1" laddr="127.0.0.1" host_name=""
  local connection_fields=()
  have_cmd sshd || return 1
  if [[ -n "${SSH_CONNECTION:-}" ]]; then
    IFS=' ' read -r -a connection_fields <<< "$SSH_CONNECTION"
    if (( ${#connection_fields[@]} >= 4 )); then
      if [[ "${connection_fields[0]}" != */* ]] && valid_ip_or_cidr "${connection_fields[0]}"; then addr="${connection_fields[0]}"; fi
      if [[ "${connection_fields[2]}" != */* ]] && valid_ip_or_cidr "${connection_fields[2]}"; then laddr="${connection_fields[2]}"; fi
    fi
  fi
  port="$(ssh_current_ports 2>/dev/null | awk '{print $1}')"
  is_uint_in_range "$port" 1 65535 || port=22
  host_name="$addr"
  command_with_timeout 8 sshd -T -C \
    "user=${user},host=${host_name},addr=${addr},laddr=${laddr},lport=${port}" 2>/dev/null || \
    command_with_timeout 8 sshd -T 2>/dev/null
}

ssh_effective_context_source() {
  local connection_fields=()
  if [[ -n "${SSH_CONNECTION:-}" ]]; then
    IFS=' ' read -r -a connection_fields <<< "$SSH_CONNECTION"
    if (( ${#connection_fields[@]} >= 1 )) && [[ "${connection_fields[0]}" != */* ]] && valid_ip_or_cidr "${connection_fields[0]}"; then
      printf '%s\n' "${connection_fields[0]}"
      return 0
    fi
  fi
  printf '127.0.0.1（本机默认上下文）\n'
}

ssh_effective_value() {
  local key="${1,,}" user="${2:-root}"
  ssh_effective_config "$user" | awk -v wanted="$key" '$1==wanted {$1=""; sub(/^[[:space:]]+/, ""); print; exit}'
}

ssh_effective_text_value() {
  local key="${1,,}"
  awk -v wanted="$key" '$1==wanted {$1=""; sub(/^[[:space:]]+/, ""); print; exit}'
}

ssh_effective_text_value_or() {
  local key="$1" fallback="${2:-未知}" value=""
  value="$(ssh_effective_text_value "$key")"
  printf '%s\n' "${value:-$fallback}"
}

ssh_effective_value_matches() {
  local key="${1,,}" expected="${2,,}" user="${3:-root}" value=""
  value="$(ssh_effective_value "$key" "$user" 2>/dev/null || true)"
  value="${value,,}"
  if [[ "$key" == "port" ]]; then
    ssh_effective_config "$user" | awk -v wanted="$expected" '$1=="port" && $2==wanted {found=1} END {exit !found}'
    return $?
  fi
  if [[ "$key" == "permitrootlogin" && "$expected" == "prohibit-password" ]]; then
    [[ "$value" == "prohibit-password" || "$value" == "without-password" ]]
    return $?
  fi
  [[ "$value" == "$expected" ]]
}

ssh_verify_effective_setting() {
  local key="$1" expected="$2" user="${3:-root}"
  if ssh_effective_value_matches "$key" "$expected" "$user"; then return 0; fi
  warn "SSH 设置未实际生效：${key} 期望 ${expected}，当前 $(ssh_effective_value "$key" "$user" 2>/dev/null || echo N/A)"
  info "脚本没有删除其他 SSH 配置；请检查更早出现的主配置或 drop-in 冲突"
  return 1
}

ssh_verify_authentication_path() {
  local method="${1,,}" user="${2:-root}" value="" alternative=""
  local alternatives=()
  value="$(ssh_effective_value authenticationmethods "$user" 2>/dev/null || true)"
  value="${value,,}"
  [[ -n "$value" ]] || value="any"
  IFS=' ' read -r -a alternatives <<< "$value"
  for alternative in "${alternatives[@]}"; do
    [[ "$alternative" == "any" || "$alternative" == "$method" ]] && return 0
  done
  warn "当前 AuthenticationMethods=${value}，不能保证 ${user} 单独使用 ${method} 登录"
  info "脚本不会降低其他软件设置的多因素认证要求，已取消本次策略变更"
  return 1
}

# 便携删除配置行（避免 sed -i /I 在某些系统不兼容）
conf_strip_keys_in_file() {
  local f="$1"; shift
  [[ -f "$f" ]] || return 0
  local tmp=""
  tmp="$(mktemp /tmp/dmitbox-ssh-conf.XXXXXX)" || return 1
  awk -v KEYS="$(printf "%s|" "$@")" '
    BEGIN{
      n=split(KEYS,a,"|");
      for(i=1;i<=n;i++){ if(a[i]!=""){ k[tolower(a[i])]=1; } }
    }
    {
      line=$0
      # keep comments
      if(line ~ /^[[:space:]]*#/){ print line; next }
      # detect key as first token
      m=line
      sub(/^[[:space:]]+/,"",m)
      split(m,toks,/([[:space:]]+|=)/)
      key=tolower(toks[1])
      if(key in k){
        next
      }
      print line
    }
  ' "$f" > "$tmp" 2>/dev/null || { rm -f "$tmp" >/dev/null 2>&1 || true; return 1; }
  cat "$tmp" > "$f" 2>/dev/null || { rm -f "$tmp" >/dev/null 2>&1 || true; return 1; }
  rm -f "$tmp" >/dev/null 2>&1 || true
}

ssh_dropin_ensure() {
  [[ ! -e "$SSH_DROPIN_DIR" || ( -d "$SSH_DROPIN_DIR" && ! -L "$SSH_DROPIN_DIR" ) ]] || {
    warn "SSH drop-in 路径不是可安全管理的普通目录：$SSH_DROPIN_DIR"
    return 1
  }
  ensure_dir "$SSH_DROPIN_DIR" || return 1
  [[ ! -e "$SSH_DROPIN_FILE" || ( -f "$SSH_DROPIN_FILE" && ! -L "$SSH_DROPIN_FILE" ) ]] || {
    warn "SSH 管理配置不是普通文件，脚本不会接管：$SSH_DROPIN_FILE"
    return 1
  }
  [[ ! -e "$SSH_LEGACY_DROPIN_FILE" || ( -f "$SSH_LEGACY_DROPIN_FILE" && ! -L "$SSH_LEGACY_DROPIN_FILE" ) ]] || {
    warn "旧版 SSH 管理路径不是普通文件，脚本不会接管：$SSH_LEGACY_DROPIN_FILE"
    return 1
  }
  if [[ -e "$SSH_DROPIN_FILE" ]] && ! grep -Fq "managed by ${SCRIPT_NAME}" "$SSH_DROPIN_FILE" 2>/dev/null; then
    warn "SSH 管理文件路径已被其他配置占用，脚本不会覆盖：$SSH_DROPIN_FILE"
    return 1
  fi
  if [[ ! -e "$SSH_DROPIN_FILE" && -f "$SSH_LEGACY_DROPIN_FILE" ]] && \
     grep -Fq "managed by ${SCRIPT_NAME}" "$SSH_LEGACY_DROPIN_FILE" 2>/dev/null; then
    mv "$SSH_LEGACY_DROPIN_FILE" "$SSH_DROPIN_FILE" || return 1
    info "已将旧版 SSH 配置迁移为优先加载的 00-dmitbox.conf"
  fi
  if [[ ! -f "$SSH_DROPIN_FILE" ]]; then
    write_file "$SSH_DROPIN_FILE" "# managed by ${SCRIPT_NAME}"
  fi
  if [[ -f "$SSH_LEGACY_DROPIN_FILE" ]] && grep -Fq "managed by ${SCRIPT_NAME}" "$SSH_LEGACY_DROPIN_FILE" 2>/dev/null; then
    rm -f "$SSH_LEGACY_DROPIN_FILE" || { warn "无法清理本脚本旧版 SSH 管理文件"; return 1; }
    info "已清理本脚本旧版 99-dmitbox.conf，其他 SSH 配置未改动"
  fi
  chown root:root "$SSH_DROPIN_FILE" >/dev/null 2>&1 || { warn "无法设置 SSH 管理配置所有者"; return 1; }
  chmod 600 "$SSH_DROPIN_FILE" >/dev/null 2>&1 || { warn "无法设置 SSH 管理配置权限"; return 1; }
}

ssh_dropin_set_kv() {
  local key="$1" val="$2"
  [[ "$key" =~ ^[A-Za-z][A-Za-z0-9]*$ ]] || return 1
  [[ -n "$val" && "$val" != *$'\n'* && "$val" != *$'\r'* ]] || return 1
  ssh_dropin_ensure || return 1
  # 只清理本脚本管理文件中的同名旧行。
  conf_strip_keys_in_file "$SSH_DROPIN_FILE" "$key" || return 1
  printf "%s %s\n" "$key" "$val" >> "$SSH_DROPIN_FILE" || return 1
  chmod 600 "$SSH_DROPIN_FILE" >/dev/null 2>&1 || return 1
}

ssh_dropin_set_many() {
  (( $# > 0 && $# % 2 == 0 )) || return 1
  while (( $# > 0 )); do
    ssh_dropin_set_kv "$1" "$2" || return 1
    shift 2
  done
}

ssh_managed_snapshot_create() {
  local snapshot=""
  snapshot="$(mktemp -d /tmp/dmitbox-ssh-config.XXXXXX)" || return 1
  if [[ -e "$SSH_DROPIN_FILE" ]]; then
    cp -a "$SSH_DROPIN_FILE" "$snapshot/00-dmitbox.conf" || { rm -rf -- "$snapshot"; return 1; }
  else
    touch "$snapshot/.00-missing"
  fi
  if [[ -e "$SSH_LEGACY_DROPIN_FILE" ]]; then
    cp -a "$SSH_LEGACY_DROPIN_FILE" "$snapshot/99-dmitbox.conf" || { rm -rf -- "$snapshot"; return 1; }
  else
    touch "$snapshot/.99-missing"
  fi
  printf '%s\n' "$snapshot"
}

ssh_managed_snapshot_restore() {
  local snapshot="$1"
  [[ "$snapshot" == /tmp/dmitbox-ssh-config.* && -d "$snapshot" ]] || return 1
  rm -f "$SSH_DROPIN_FILE" "$SSH_LEGACY_DROPIN_FILE" || return 1
  if [[ -f "$snapshot/00-dmitbox.conf" ]]; then
    cp -a "$snapshot/00-dmitbox.conf" "$SSH_DROPIN_FILE" || return 1
  fi
  if [[ -f "$snapshot/99-dmitbox.conf" ]]; then
    cp -a "$snapshot/99-dmitbox.conf" "$SSH_LEGACY_DROPIN_FILE" || return 1
  fi
}

ssh_managed_snapshot_discard() {
  local snapshot="$1"
  [[ "$snapshot" == /tmp/dmitbox-ssh-config.* && -d "$snapshot" ]] || return 0
  rm -rf -- "$snapshot"
}

# 防止 ssh.socket 抢占 22（即便现在 inactive，也避免后续被 enable）
ssh_socket_disable_if_any() {
  is_systemd || return 0
  if command_with_timeout 8 systemctl list-unit-files 2>/dev/null | awk '{print $1}' | grep -qx "ssh.socket"; then
    if command_with_timeout 6 systemctl is-enabled --quiet ssh.socket 2>/dev/null; then
      warn "检测到 ssh.socket 已启用：将 disable（否则端口可能被固定在 22）"
      command_with_timeout 20 systemctl disable --now ssh.socket >/dev/null 2>&1 || {
        warn "无法停用 ssh.socket，已取消端口修改"
        return 1
      }
    else
      command_with_timeout 10 systemctl stop ssh.socket >/dev/null 2>&1 || true
    fi
  fi
}

ssh_random_pass() { tr -dc 'A-Za-z0-9' < /dev/urandom | head -c 18 || true; }

ssh_set_user_password() {
  local user="$1" passwd="$2"
  id "$user" >/dev/null 2>&1 || { warn "用户不存在：$user"; return 1; }
  [[ -n "$passwd" ]] || { warn "密码不能为空"; return 1; }

  if have_cmd chpasswd; then
    if ! printf '%s:%s\n' "$user" "$passwd" | chpasswd; then
      warn "设置 ${user} 密码失败"
      return 1
    fi
  elif have_cmd passwd; then
    if ! printf "%s\n%s\n" "$passwd" "$passwd" | passwd "$user" >/dev/null 2>&1; then
      warn "设置 ${user} 密码失败"
      return 1
    fi
  else
    warn "系统缺少 chpasswd/passwd，无法设置密码"
    return 1
  fi
}

ssh_group_exists() {
  local group="${1:-}"
  [[ "$group" =~ ^[a-z_][a-z0-9_-]{0,31}$ ]] || return 1
  if have_cmd getent; then
    getent group "$group" >/dev/null 2>&1
  else
    awk -F: -v wanted="$group" '$1==wanted {found=1} END{exit !found}' /etc/group 2>/dev/null
  fi
}

ssh_create_user_with_password() {
  local user="$1"
  local passwd="$2"
  local admin_group=""

  valid_username "$user" || { warn "用户名无效：仅支持小写字母/数字/_/-，且必须以字母或下划线开头（最长 32 位）"; return 1; }

  if ! id "$user" >/dev/null 2>&1; then
    info "创建用户：$user"
    if have_cmd useradd; then
      useradd -m -s /bin/bash "$user" >/dev/null 2>&1 || { warn "useradd 创建用户失败"; return 1; }
    elif have_cmd adduser; then
      adduser -D "$user" >/dev/null 2>&1 || \
        adduser --disabled-password --gecos "" "$user" >/dev/null 2>&1 || { warn "adduser 创建用户失败"; return 1; }
    else
      warn "没有 useradd/adduser，无法创建用户"
      return 1
    fi
  fi

  id "$user" >/dev/null 2>&1 || { warn "用户创建失败：$user"; return 1; }
  ssh_user_login_shell_usable "$user" || { warn "用户 ${user} 的登录 Shell 不可用，未设置 SSH 密码"; return 1; }
  ssh_set_user_password "$user" "$passwd" || return 1

  ok "已设置 ${user} 密码"
  echo -e "${c_green}${user} 密码：${passwd}${c_reset}"

  if ssh_group_exists sudo; then admin_group="sudo"; elif ssh_group_exists wheel; then admin_group="wheel"; fi
  if [[ -z "$admin_group" ]]; then
    warn "系统没有 sudo/wheel 管理组，请按系统需要手动授权"
  elif have_cmd usermod; then
    usermod -aG "$admin_group" "$user" >/dev/null 2>&1 || warn "用户已创建，但加入 ${admin_group} 组失败"
  elif have_cmd adduser; then
    adduser "$user" "$admin_group" >/dev/null 2>&1 || warn "用户已创建，但加入 ${admin_group} 组失败"
  else
    warn "未自动加入 sudo/wheel 管理组，请按系统需要手动授权"
  fi
}

ssh_apply_base_hardening() {
  # 只修改本脚本自己的 drop-in；不删除主配置或其他软件的设置。
  ssh_dropin_ensure || return 1
  ssh_dropin_set_many \
    "KbdInteractiveAuthentication" "no" \
    "ChallengeResponseAuthentication" "no" \
    "PermitEmptyPasswords" "no" \
    "MaxAuthTries" "3" \
    "LoginGraceTime" "30"
}

ssh_managed_change_abort() {
  local snapshot="$1" reason="${2:-SSH 配置未能安全应用}"
  warn "$reason；正在恢复本次修改前的 SSH 配置"
  if ssh_managed_snapshot_restore "$snapshot"; then
    ssh_validate_config >/dev/null 2>&1 || warn "恢复后的 SSH 配置仍需人工检查"
    sshd_restart >/dev/null 2>&1 || warn "旧配置已恢复，但 SSH 服务未能自动重新加载"
  else
    warn "自动恢复本次 SSH 配置失败，请保持当前会话并立即人工检查"
  fi
  ssh_managed_snapshot_discard "$snapshot"
  return 1
}

ssh_managed_change_finish() {
  local snapshot="$1" keep_snapshot="${2:-0}"
  ssh_validate_config || {
    ssh_managed_change_abort "$snapshot" "新配置未通过 sshd -t"
    return 1
  }
  if ! sshd_restart; then
    ssh_managed_change_abort "$snapshot" "SSH 服务重新加载失败"
    return 1
  fi
  [[ "$keep_snapshot" == "1" ]] || ssh_managed_snapshot_discard "$snapshot"
}

ssh_user_home() {
  local user="${1:-}" entry="" home=""
  valid_username "$user" || return 1
  if have_cmd getent; then
    entry="$(getent passwd "$user" 2>/dev/null || true)"
  else
    entry="$(awk -F: -v wanted="$user" '$1==wanted {print; exit}' /etc/passwd 2>/dev/null || true)"
  fi
  home="$(printf '%s\n' "$entry" | awk -F: 'NF>=6 {print $6; exit}')"
  [[ "$home" == /* && "$home" != "/" ]] || return 1
  printf '%s\n' "$home"
}

ssh_user_login_shell() {
  local user="${1:-}" entry="" shell=""
  valid_username "$user" || return 1
  if have_cmd getent; then
    entry="$(getent passwd "$user" 2>/dev/null || true)"
  else
    entry="$(awk -F: -v wanted="$user" '$1==wanted {print; exit}' /etc/passwd 2>/dev/null || true)"
  fi
  shell="$(printf '%s\n' "$entry" | awk -F: 'NF>=7 {print $7; exit}')"
  [[ "$shell" == /* ]] || return 1
  printf '%s\n' "$shell"
}

ssh_user_login_shell_usable() {
  local user="${1:-}" shell=""
  shell="$(ssh_user_login_shell "$user")" || return 1
  case "$shell" in */nologin|*/false) return 1 ;; esac
  [[ -x "$shell" ]]
}

ssh_normalize_path() {
  local path="${1:-}"
  [[ "$path" == /* ]] || return 1
  if have_cmd readlink && readlink -m -- / >/dev/null 2>&1; then
    readlink -m -- "$path"
  elif have_cmd realpath && realpath -m -- / >/dev/null 2>&1; then
    realpath -m -- "$path"
  else
    [[ "/$path/" != *"/../"* && "/$path/" != *"/./"* ]] || return 1
    printf '%s\n' "$path"
  fi
}

ssh_authorized_key_path_expand() {
  local user="$1" spec="$2" home="" home_real="" path="" uid="" marker="__DMITBOX_PERCENT__"
  home="$(ssh_user_home "$user")" || return 1
  home_real="$(ssh_normalize_path "$home")" || return 1
  uid="$(id -u "$user" 2>/dev/null || true)"
  [[ -n "$uid" ]] || return 1
  [[ -n "$spec" && "$spec" != "none" ]] || return 1

  spec="${spec//%%/$marker}"
  spec="${spec//%h/$home}"
  spec="${spec//%u/$user}"
  spec="${spec//%U/$uid}"
  spec="${spec//$marker/%}"
  [[ "$spec" != *%* ]] || return 1
  if [[ "$spec" == /* ]]; then path="$spec"; else path="${home}/${spec}"; fi
  path="$(ssh_normalize_path "$path")" || return 1

  # 密钥管理只触碰用户家目录内的文件，拒绝系统级共享路径。
  [[ "$path" == "$home_real"/* ]] || return 1
  printf '%s\n' "$path"
}

ssh_authorized_keys_paths() {
  local user="${1:-root}" configured="" spec="" path="" seen=$'\n'
  local specs=()
  valid_username "$user" || return 1
  id "$user" >/dev/null 2>&1 || return 1
  configured="$(ssh_effective_value "authorizedkeysfile" "$user" 2>/dev/null || true)"
  [[ -n "$configured" ]] || configured=".ssh/authorized_keys .ssh/authorized_keys2"
  IFS=' ' read -r -a specs <<< "$configured"
  for spec in "${specs[@]}"; do
    path="$(ssh_authorized_key_path_expand "$user" "$spec" 2>/dev/null || true)"
    [[ -n "$path" ]] || continue
    [[ "$seen" == *$'\n'"$path"$'\n'* ]] && continue
    printf '%s\n' "$path"
    seen+="${path}"$'\n'
  done
}

ssh_authorized_keys_path() {
  local user="${1:-root}" path=""
  path="$(ssh_authorized_keys_paths "$user" | sed -n '1p')"
  [[ -n "$path" ]] || return 1
  printf '%s\n' "$path"
}

ssh_key_line_fingerprint() {
  local line="${1:-}" tmp="" output=""
  have_cmd ssh-keygen || return 1
  [[ -n "$line" && ${#line} -le 65536 && "$line" != *$'\n'* && "$line" != *$'\r'* ]] || return 1
  tmp="$(mktemp /tmp/dmitbox-ssh-key.XXXXXX)" || return 1
  chmod 600 "$tmp" >/dev/null 2>&1 || true
  printf '%s\n' "$line" > "$tmp"
  output="$(ssh-keygen -lf "$tmp" 2>/dev/null)" || { rm -f -- "$tmp"; return 1; }
  rm -f -- "$tmp"
  [[ -n "$output" ]] || return 1
  printf '%s\n' "$output"
}

ssh_public_key_validate() {
  local key="${1:-}" type="" fingerprint=""
  [[ -n "$key" && "$key" != *$'\n'* && "$key" != *$'\r'* && ${#key} -le 16384 ]] || return 1
  type="${key%%[[:space:]]*}"
  case "$type" in
    ssh-ed25519|ssh-rsa|ecdsa-sha2-*|sk-ssh-ed25519@openssh.com|sk-ecdsa-sha2-*@openssh.com) ;;
    *) return 1 ;;
  esac
  fingerprint="$(ssh_key_line_fingerprint "$key")" || return 1
  printf '%s\n' "$fingerprint"
}

ssh_valid_key_count_path() {
  local path="${1:-}" line="" count=0
  [[ -f "$path" && ! -L "$path" ]] || { printf '0\n'; return 0; }
  while IFS= read -r line || [[ -n "$line" ]]; do
    line="${line%$'\r'}"
    ssh_key_line_fingerprint "$line" >/dev/null 2>&1 && count=$((count + 1))
  done < "$path"
  printf '%s\n' "$count"
}

ssh_user_valid_key_count() {
  local user="${1:-root}" path="" count=0 part=0
  while IFS= read -r path; do
    [[ -n "$path" ]] || continue
    part="$(ssh_valid_key_count_path "$path")"
    is_uint_in_range "$part" 0 100000 || part=0
    count=$((count + part))
  done < <(ssh_authorized_keys_paths "$user" 2>/dev/null || true)
  printf '%s\n' "$count"
}

ssh_authorized_keys_backup() {
  local user="$1" path="$2" backup=""
  [[ -f "$path" && ! -L "$path" ]] || return 0
  ensure_dir "$SSH_KEYS_BACKUP_DIR"
  chmod 700 "$SSH_KEYS_BACKUP_DIR" >/dev/null 2>&1 || true
  backup="$(mktemp "${SSH_KEYS_BACKUP_DIR}/$(ts_now)-${user}-authorized_keys.XXXXXX")" || {
    warn "无法创建登录密钥备份文件，已取消修改"
    return 1
  }
  cp -a -- "$path" "$backup" || {
    rm -f -- "$backup" >/dev/null 2>&1 || true
    warn "登录密钥备份失败，已取消修改"
    return 1
  }
  chmod 600 "$backup" >/dev/null 2>&1 || true
  printf '%s\n' "$backup"
}

ssh_authorized_keys_change_abort() {
  local user="$1" path="$2" backup="${3:-}" existed="${4:-1}"
  if [[ -n "$backup" && -f "$backup" ]]; then
    cp -a -- "$backup" "$path" >/dev/null 2>&1 || {
      warn "登录密钥修改失败，且自动恢复备份失败：$backup"
      return 1
    }
    ssh_fix_authorized_keys_permissions "$user" "$path" >/dev/null 2>&1 || true
    warn "登录密钥修改失败，已恢复修改前内容"
  elif [[ "$existed" == "0" ]]; then
    rm -f -- "$path" >/dev/null 2>&1 || true
    warn "登录密钥修改失败，已撤销本次新建文件"
  else
    warn "登录密钥修改失败，请保持当前会话并人工检查：$path"
  fi
  return 1
}

ssh_fix_authorized_keys_permissions() {
  local user="$1" path="$2" home="" home_real="" dir="" group="" component="" parent="" owner=""
  valid_username "$user" || return 1
  id "$user" >/dev/null 2>&1 || return 1
  home="$(ssh_user_home "$user")" || return 1
  home_real="$(ssh_normalize_path "$home")" || return 1
  path="$(ssh_normalize_path "$path")" || return 1
  [[ "$path" == "$home_real"/* ]] || { warn "拒绝修改用户家目录外的密钥文件：$path"; return 1; }
  [[ ! -L "$path" ]] || { warn "密钥文件是符号链接，已拒绝自动修改"; return 1; }
  [[ ! -e "$path" || -f "$path" ]] || { warn "密钥路径不是普通文件：$path"; return 1; }
  if [[ -e "$path" ]]; then
    owner="$(stat -c '%U' "$path" 2>/dev/null || true)"
    [[ "$owner" == "$user" || "$owner" == "root" ]] || { warn "密钥文件所有者异常，已拒绝接管：${owner:-未知}"; return 1; }
  fi
  dir="$(dirname "$path")"
  [[ ! -L "$dir" ]] || { warn "密钥目录是符号链接，已拒绝自动修改"; return 1; }
  group="$(id -gn "$user" 2>/dev/null || true)"
  [[ -n "$group" ]] || return 1

  ( umask 077; mkdir -p -- "$dir" ) || return 1
  component="$dir"
  while [[ "$component" == "$home_real" || "$component" == "$home_real"/* ]]; do
    owner="$(stat -c '%U' "$component" 2>/dev/null || true)"
    [[ "$owner" == "$user" || "$owner" == "root" ]] || {
      warn "密钥路径目录所有者异常，已拒绝接管：${owner:-未知} / $component"
      return 1
    }
    chmod go-w "$component" >/dev/null 2>&1 || return 1
    [[ "$component" == "$home_real" ]] && break
    parent="$(dirname "$component")"
    [[ "$parent" != "$component" ]] || break
    component="$parent"
  done
  ( umask 077; touch -- "$path" ) || return 1
  if [[ "$dir" != "$home_real" ]]; then
    chown "$user:$group" "$dir" >/dev/null 2>&1 || return 1
    chmod 700 "$dir" >/dev/null 2>&1 || return 1
  fi
  chown "$user:$group" "$path" >/dev/null 2>&1 || return 1
  chmod 600 "$path" >/dev/null 2>&1 || return 1
  if have_cmd restorecon; then restorecon -F "$dir" "$path" >/dev/null 2>&1 || true; fi
}

ssh_list_authorized_keys() {
  local user="${1:-root}" path="" line="" fingerprint="" index=0 file_count=0
  valid_username "$user" || { warn "用户名无效：$user"; return 1; }
  id "$user" >/dev/null 2>&1 || { warn "用户不存在：$user"; return 1; }
  have_cmd ssh-keygen || ssh_pkg_install || return 1

  echo -e "${c_dim}--- ${user} 的登录密钥 ---${c_reset}"
  while IFS= read -r path; do
    [[ -n "$path" ]] || continue
    file_count=$((file_count + 1))
    print_kv "密钥文件" "$path"
    if [[ ! -e "$path" ]]; then
      info "文件尚未创建"
      continue
    fi
    if [[ -L "$path" || ! -f "$path" ]]; then
      warn "不是可安全管理的普通文件"
      continue
    fi
    while IFS= read -r line || [[ -n "$line" ]]; do
      line="${line%$'\r'}"
      fingerprint="$(ssh_key_line_fingerprint "$line" 2>/dev/null || true)"
      [[ -n "$fingerprint" ]] || continue
      index=$((index + 1))
      printf '  [%2d] %s\n' "$index" "$fingerprint"
    done < "$path"
  done < <(ssh_authorized_keys_paths "$user" 2>/dev/null || true)
  (( file_count > 0 )) || { warn "当前 AuthorizedKeysFile 路径不在用户家目录内，脚本不会自动管理"; return 1; }
  if (( index == 0 )); then warn "未发现有效公钥"; else ok "共发现 ${index} 个有效公钥"; fi
}

ssh_ephemeral_key_dir_create() {
  local base="${SSH_EPHEMERAL_KEY_BASE:-/tmp}"
  [[ "$base" == /* && "$base" != "/" && -d "$base" && ! -L "$base" ]] || return 1
  ( umask 077; mktemp -d "${base%/}/dmitbox-ssh-generated.XXXXXX" )
}

ssh_ephemeral_key_cleanup() {
  local dir="${1:-}" base="${SSH_EPHEMERAL_KEY_BASE:-/tmp}" dir_real="" base_real=""
  [[ "$base" == /* && "$base" != "/" && -d "$base" && ! -L "$base" ]] || return 1
  [[ "$dir" == /* && -d "$dir" && ! -L "$dir" ]] || return 0
  base_real="$(ssh_normalize_path "$base" 2>/dev/null || true)"
  dir_real="$(ssh_normalize_path "$dir" 2>/dev/null || true)"
  [[ -n "$base_real" && -n "$dir_real" ]] || return 1
  [[ "$(dirname "$dir_real")" == "$base_real" && "$(basename "$dir_real")" == dmitbox-ssh-generated.* ]] || return 1
  rm -rf -- "$dir_real"
}

ssh_copy_private_key_to_clipboard() {
  local private_file="${1:-}" encoded=""
  [[ "${DMITBOX_DISABLE_OSC52:-0}" != "1" ]] || return 1
  [[ -f "$private_file" && ! -L "$private_file" && -r "$private_file" ]] || return 1
  have_cmd base64 || return 1
  has_tty || return 1
  encoded="$(base64 < "$private_file" 2>/dev/null | tr -d '\r\n')"
  [[ -n "$encoded" && ${#encoded} -le 32768 ]] || return 1
  printf '\033]52;c;%s\a' "$encoded" > /dev/tty 2>/dev/null || return 1
}

ssh_generate_authorized_key() {
  local user="${1:-root}" target_path=""
  valid_username "$user" || { warn "用户名无效：$user"; return 1; }
  id "$user" >/dev/null 2>&1 || { warn "用户不存在：$user"; return 1; }
  ssh_user_login_shell_usable "$user" || { warn "${user} 的登录 Shell 不可用，已取消"; return 1; }
  ssh_pkg_install || return 1
  target_path="$(ssh_authorized_keys_path "$user" 2>/dev/null || true)"
  [[ -n "$target_path" ]] || {
    warn "AuthorizedKeysFile 位于用户家目录外或被设为 none，无法安全自动安装"
    return 1
  }

  warn "将为 ${user} 自动生成一把新的 Ed25519 登录密钥"
  warn "私钥无密码，便于直接登录；保存后可在本地用 ssh-keygen -p 添加密码"
  info "私钥只在本次流程中临时生成，不会保存到服务器备份目录"
  confirm_word "GENERATE" "确认生成请输入 GENERATE > " || { warn "已取消"; return 0; }

  (
    local key_dir="" private_file="" public_file="" public_key="" fingerprint="" save_name="" copied=0 private_shown=0
    key_dir="$(ssh_ephemeral_key_dir_create)" || { warn "无法创建安全的临时密钥目录"; return 1; }
    trap 'ssh_ephemeral_key_cleanup "$key_dir" >/dev/null 2>&1 || true; (( private_shown == 0 )) || soft_clear' EXIT
    trap 'exit 129' HUP
    trap 'exit 130' INT
    trap 'exit 143' TERM

    private_file="${key_dir}/id_ed25519"
    public_file="${private_file}.pub"
    save_name="dmitbox-${user}-$(date +%Y%m%d-%H%M%S).key"
    if ! ssh-keygen -q -t ed25519 -a 100 -N "" \
      -C "dmitbox-${user}-$(date +%Y%m%d)" -f "$private_file"; then
      warn "Ed25519 密钥生成失败"
      return 1
    fi
    chmod 600 "$private_file" "$public_file" >/dev/null 2>&1 || { warn "无法保护临时密钥权限"; return 1; }
    public_key="$(<"$public_file")"
    fingerprint="$(ssh_public_key_validate "$public_key" 2>/dev/null || true)"
    [[ -n "$fingerprint" ]] || { warn "生成的公钥未通过校验"; return 1; }

    if ssh_copy_private_key_to_clipboard "$private_file"; then copied=1; fi
    menu_section "一次性私钥"
    if (( copied )); then
      ok "已尝试复制到本地剪贴板；若 SSH 客户端不支持，请复制下面完整内容"
    else
      info "请复制下面从 BEGIN 到 END 的全部内容"
    fi
    warn "不要把私钥发送给别人，也不要保存在聊天记录或公开网盘"
    info "确认保存或取消后会自动清屏，并尝试清除终端回滚区"
    private_shown=1
    printf '\n%s\n' "$(<"$private_file")"
    echo
    print_kv "建议文件名" "$save_name"
    print_kv "文件权限" "chmod 600 $save_name"
    print_kv "密钥指纹" "$fingerprint"
    info "保存完成后输入 SAVED；未确认不会安装公钥"
    if ! confirm_word "SAVED" "确认私钥已安全保存请输入 SAVED > "; then
      warn "未确认保存，公钥没有安装；临时私钥将立即清理"
      return 1
    fi
    if ! ssh_add_authorized_key "$user" "$public_key"; then
      warn "公钥安装失败；临时私钥将立即清理"
      return 1
    fi
    soft_clear
    private_shown=0
    ok "自动生成并安装完成"
    print_kv "密钥指纹" "$fingerprint"
    info "请保持当前会话，另开终端测试：ssh -i ${save_name} -p $(ssh_current_ports | awk '{print $1}') ${user}@服务器IP"
    warn "确认新密钥可以登录后，再关闭当前 SSH 会话"
  )
}

ssh_add_authorized_key() {
  local user="$1" key="$2" path="" fingerprint="" blob="" backup="" tmp="" existed=0
  valid_username "$user" || { warn "用户名无效：$user"; return 1; }
  id "$user" >/dev/null 2>&1 || { warn "用户不存在：$user"; return 1; }
  ssh_pkg_install || return 1
  key="${key#"${key%%[![:space:]]*}"}"
  key="${key%"${key##*[![:space:]]}"}"
  fingerprint="$(ssh_public_key_validate "$key" 2>/dev/null || true)"
  [[ -n "$fingerprint" ]] || { warn "公钥格式无效；请粘贴完整的一行 OpenSSH 公钥"; return 1; }
  path="$(ssh_authorized_keys_path "$user" 2>/dev/null || true)"
  [[ -n "$path" ]] || { warn "AuthorizedKeysFile 位于用户家目录外或被设为 none，已拒绝自动修改"; return 1; }
  [[ -e "$path" ]] && existed=1
  ssh_fix_authorized_keys_permissions "$user" "$path" || { warn "无法安全准备密钥文件"; return 1; }
  blob="$(awk '{print $2}' <<< "$key")"
  if awk -v wanted="$blob" '{for(i=1;i<=NF;i++) if($i==wanted) found=1} END{exit !found}' "$path" 2>/dev/null; then
    info "该公钥已存在，无需重复添加"
    printf '  %s\n' "$fingerprint"
    return 0
  fi
  if (( existed )); then
    backup="$(ssh_authorized_keys_backup "$user" "$path")" || return 1
  fi
  tmp="$(mktemp "$(dirname "$path")/.dmitbox-authorized-keys.XXXXXX")" || return 1
  { cat -- "$path" 2>/dev/null || true; [[ ! -s "$path" ]] || printf '\n'; printf '%s\n' "$key"; } > "$tmp"
  mv -f -- "$tmp" "$path" || { rm -f -- "$tmp"; return 1; }
  ssh_fix_authorized_keys_permissions "$user" "$path" || {
    ssh_authorized_keys_change_abort "$user" "$path" "$backup" "$existed"
    return 1
  }
  ok "已添加 ${user} 登录公钥"
  printf '  %s\n' "$fingerprint"
  [[ -n "$backup" ]] && info "修改前备份：$backup"
  [[ "${key%%[[:space:]]*}" == "ssh-rsa" ]] && warn "这是旧式 ssh-rsa 公钥；新部署更建议使用 Ed25519"
  return 0
}

ssh_remove_authorized_key() {
  local user="$1" requested="${2:-}" path="" line="" fingerprint="" backup="" tmp=""
  local line_no=0 index=0 selected_line=0
  local key_lines=() key_files=() key_fingerprints=() key_contents=()
  valid_username "$user" || { warn "用户名无效：$user"; return 1; }
  id "$user" >/dev/null 2>&1 || { warn "用户不存在：$user"; return 1; }
  have_cmd ssh-keygen || ssh_pkg_install || return 1

  while IFS= read -r path; do
    [[ -f "$path" && ! -L "$path" ]] || continue
    line_no=0
    while IFS= read -r line || [[ -n "$line" ]]; do
      line_no=$((line_no + 1))
      line="${line%$'\r'}"
      fingerprint="$(ssh_key_line_fingerprint "$line" 2>/dev/null || true)"
      [[ -n "$fingerprint" ]] || continue
      index=$((index + 1))
      key_lines+=("$line_no")
      key_files+=("$path")
      key_fingerprints+=("$fingerprint")
      key_contents+=("$line")
      printf '  [%2d] %s\n' "$index" "$fingerprint"
    done < "$path"
  done < <(ssh_authorized_keys_paths "$user" 2>/dev/null || true)

  if (( index == 0 )); then warn "没有可移除的有效公钥"; return 1; fi
  if (( index == 1 )); then
    warn "已阻止移除最后一个有效公钥；请先添加替代密钥，避免 SSH 锁死"
    return 1
  fi
  if [[ -z "$requested" ]]; then read_tty requested "选择要移除的密钥编号 > " ""; fi
  is_uint_in_range "$requested" 1 "$index" || { warn "密钥编号无效"; return 1; }
  requested=$((10#$requested))
  path="${key_files[$((requested - 1))]}"
  selected_line="${key_lines[$((requested - 1))]}"
  warn "将移除：${key_fingerprints[$((requested - 1))]}"
  confirm_word "REMOVE" "确认请输入 REMOVE > " || { warn "已取消"; return 0; }
  line="$(sed -n "${selected_line}p" "$path" 2>/dev/null || true)"
  line="${line%$'\r'}"
  if [[ "$line" != "${key_contents[$((requested - 1))]}" ]]; then
    warn "确认期间密钥文件发生变化，已取消移除；请重新查看后再试"
    return 1
  fi
  backup="$(ssh_authorized_keys_backup "$user" "$path")" || return 1
  tmp="$(mktemp "$(dirname "$path")/.dmitbox-authorized-keys.XXXXXX")" || return 1
  awk -v remove_line="$selected_line" 'NR!=remove_line {print}' "$path" > "$tmp" || { rm -f -- "$tmp"; return 1; }
  mv -f -- "$tmp" "$path" || { rm -f -- "$tmp"; return 1; }
  ssh_fix_authorized_keys_permissions "$user" "$path" || {
    ssh_authorized_keys_change_abort "$user" "$path" "$backup" "1"
    return 1
  }
  ok "已移除所选公钥；其他密钥未改动"
  info "修改前备份：$backup"
}

ssh_repair_authorized_keys() {
  local user="${1:-root}" path="" repaired=0
  valid_username "$user" || { warn "用户名无效：$user"; return 1; }
  id "$user" >/dev/null 2>&1 || { warn "用户不存在：$user"; return 1; }
  while IFS= read -r path; do
    [[ -n "$path" ]] || continue
    [[ -e "$path" ]] || continue
    ssh_fix_authorized_keys_permissions "$user" "$path" || return 1
    print_kv "已修复" "${path} (600)"
    repaired=$((repaired + 1))
  done < <(ssh_authorized_keys_paths "$user" 2>/dev/null || true)
  if (( repaired == 0 )); then
    path="$(ssh_authorized_keys_path "$user" 2>/dev/null || true)"
    [[ -n "$path" ]] || { warn "没有可安全管理的 AuthorizedKeysFile 路径"; return 1; }
    ssh_fix_authorized_keys_permissions "$user" "$path" || return 1
    print_kv "已创建" "${path} (600)"
  fi
  ok "密钥目录与文件权限已校正；未改动密钥内容"
}

ssh_show_effective_configuration() {
  local user="${1:-root}" effective="${2:-}" root_effective="${3:-}" service_name="" service_state="未知" ports=""
  valid_username "$user" || { warn "用户名无效：$user"; return 1; }
  id "$user" >/dev/null 2>&1 || { warn "用户不存在：$user"; return 1; }
  have_cmd sshd || { warn "未找到 OpenSSH Server"; return 1; }
  ssh_validate_config || return 1
  if [[ -z "$effective" ]]; then
    effective="$(ssh_effective_config "$user")" || { warn "无法读取 sshd 最终生效配置"; return 1; }
  fi
  [[ -n "$root_effective" ]] || root_effective="$effective"
  if [[ "$user" != "root" && -z "${3:-}" ]]; then
    root_effective="$(ssh_effective_config root 2>/dev/null || true)"
    [[ -n "$root_effective" ]] || root_effective="$effective"
  fi
  service_name="$(ssh_service_name 2>/dev/null || true)"
  if [[ -n "$service_name" ]]; then
    if is_systemd; then
      if command_with_timeout 6 systemctl is-active --quiet "${service_name}.service" >/dev/null 2>&1; then service_state="运行中"; else service_state="未运行"; fi
    elif have_cmd rc-service; then
      if command_with_timeout 6 rc-service "$service_name" status >/dev/null 2>&1; then service_state="运行中"; else service_state="未运行"; fi
    elif have_cmd service; then
      if command_with_timeout 6 service "$service_name" status >/dev/null 2>&1; then service_state="运行中"; else service_state="未运行"; fi
    fi
  fi
  ports="$(awk '$1=="port" {out=(out ? out " " : "") $2} END{print out}' <<< "$effective")"
  print_kv "检查用户" "$user"
  print_kv "匹配来源" "$(ssh_effective_context_source)"
  print_kv "SSH 服务" "${service_name:-未识别} / ${service_state}"
  print_kv "监听端口配置" "${ports:-未知}"
  print_kv "密码认证" "$(ssh_effective_text_value_or passwordauthentication 未知 <<< "$effective")"
  print_kv "交互认证" "$(ssh_effective_text_value_or kbdinteractiveauthentication 未知 <<< "$effective")"
  print_kv "公钥认证" "$(ssh_effective_text_value_or pubkeyauthentication 未知 <<< "$effective")"
  print_kv "认证组合" "$(ssh_effective_text_value_or authenticationmethods any <<< "$effective")"
  print_kv "PAM" "$(ssh_effective_text_value_or usepam 未知 <<< "$effective")"
  print_kv "登录 Shell" "$(ssh_user_login_shell "$user" 2>/dev/null || echo 未知)"
  print_kv "root 登录" "$(ssh_effective_text_value_or permitrootlogin 未知 <<< "$root_effective")"
  print_kv "空密码" "$(ssh_effective_text_value_or permitemptypasswords 未知 <<< "$effective")"
  print_kv "最大认证次数" "$(ssh_effective_text_value_or maxauthtries 未知 <<< "$effective")"
  print_kv "登录宽限时间" "$(ssh_effective_text_value_or logingracetime 未知 <<< "$effective")"
  print_kv "密钥文件" "$(ssh_effective_text_value_or authorizedkeysfile 未知 <<< "$effective")"
  print_kv "TCP 转发" "$(ssh_effective_text_value_or allowtcpforwarding 未知 <<< "$effective")"
  print_kv "Agent 转发" "$(ssh_effective_text_value_or allowagentforwarding 未知 <<< "$effective")"
  print_kv "X11 转发" "$(ssh_effective_text_value_or x11forwarding 未知 <<< "$effective")"
  print_kv "用户环境变量" "$(ssh_effective_text_value_or permituserenvironment 未知 <<< "$effective")"
  print_kv "允许用户" "$(ssh_effective_text_value_or allowusers 未限制 <<< "$effective")"
  print_kv "拒绝用户" "$(ssh_effective_text_value_or denyusers 未设置 <<< "$effective")"
  print_kv "允许用户组" "$(ssh_effective_text_value_or allowgroups 未限制 <<< "$effective")"
  print_kv "拒绝用户组" "$(ssh_effective_text_value_or denygroups 未设置 <<< "$effective")"
  if [[ -f "$SSH_DROPIN_FILE" ]]; then
    print_kv "脚本管理配置" "$SSH_DROPIN_FILE"
  else
    print_kv "脚本管理配置" "未创建"
  fi
}

ssh_mode_is_group_world_writable() {
  local path="${1:-}" mode="" value=0
  [[ -e "$path" ]] || return 1
  mode="$(stat -c '%a' "$path" 2>/dev/null || true)"
  [[ "$mode" =~ ^[0-7]{3,4}$ ]] || return 1
  value=$((8#$mode))
  (( (value & 022) != 0 ))
}

ssh_security_audit() {
  local user="${1:-root}" effective="" root_effective="" value="" path="" dir="" owner="" count=0 warnings=0 ports="" port=""
  local port_list=()
  valid_username "$user" || { warn "用户名无效：$user"; return 1; }
  id "$user" >/dev/null 2>&1 || { warn "用户不存在：$user"; return 1; }
  have_cmd sshd || { warn "未找到 OpenSSH Server，无法执行安全体检"; return 1; }
  have_cmd ssh-keygen || { warn "未找到 ssh-keygen，密钥检查将受限"; }

  menu_section "配置与服务"
  if ssh_validate_config; then
    ok "sshd 配置语法正常"
  else
    warn "配置无效时无法可靠计算最终认证策略；请先修复上方错误"
    return 1
  fi
  effective="$(ssh_effective_config "$user" 2>/dev/null || true)"
  [[ -n "$effective" ]] || { warn "无法读取 sshd 最终生效配置，体检已停止"; return 1; }
  root_effective="$effective"
  if [[ "$user" != "root" ]]; then
    root_effective="$(ssh_effective_config root 2>/dev/null || true)"
    [[ -n "$root_effective" ]] || root_effective="$effective"
  fi
  ssh_show_effective_configuration "$user" "$effective" "$root_effective" || warnings=$((warnings + 1))
  ports="$(awk '$1=="port" {out=(out ? out " " : "") $2} END{print out}' <<< "$effective")"
  IFS=' ' read -r -a port_list <<< "$ports"
  for port in "${port_list[@]}"; do
    [[ -n "$port" ]] || continue
    if port_in_use "$port"; then ok "TCP/${port} 当前存在监听"; else warn "TCP/${port} 未检测到监听"; warnings=$((warnings + 1)); fi
  done

  menu_section "认证风险"
  if ssh_user_login_shell_usable "$user"; then ok "${user} 的登录 Shell 可用"; else warn "${user} 的登录 Shell 不可用或被设为 nologin/false"; warnings=$((warnings + 1)); fi
  value="$(ssh_effective_text_value passwordauthentication <<< "$effective")"
  if [[ "$value" == "yes" ]]; then warn "密码认证已开启，建议配好密钥后使用推荐加固"; warnings=$((warnings + 1)); else ok "密码认证未开启"; fi
  value="$(ssh_effective_text_value kbdinteractiveauthentication <<< "$effective")"
  if [[ "$value" == "yes" ]]; then warn "键盘交互认证已开启，可能仍允许口令类认证"; warnings=$((warnings + 1)); else ok "键盘交互认证未开启"; fi
  value="$(ssh_effective_text_value pubkeyauthentication <<< "$effective")"
  if [[ "$value" != "yes" ]]; then warn "公钥认证未开启"; warnings=$((warnings + 1)); else ok "公钥认证已开启"; fi
  value="$(ssh_effective_text_value authenticationmethods <<< "$effective")"
  [[ -n "$value" ]] && print_kv "认证组合" "$value"
  value="$(ssh_effective_text_value permitrootlogin <<< "$root_effective")"
  case "$value" in
    no|prohibit-password|without-password|forced-commands-only) ok "root 未开放直接密码登录（${value}）" ;;
    *) warn "root 登录策略偏宽松：${value:-未知}"; warnings=$((warnings + 1)) ;;
  esac
  value="$(ssh_effective_text_value permitemptypasswords <<< "$effective")"
  if [[ "$value" == "yes" ]]; then warn "检测到允许空密码"; warnings=$((warnings + 1)); else ok "空密码登录已禁止"; fi
  value="$(ssh_effective_text_value maxauthtries <<< "$effective")"
  if is_uint_in_range "$value" 1 100 && (( 10#$value <= 4 )); then ok "最大认证尝试次数合理（${value}）"; else warn "最大认证尝试次数偏高或未知：${value:-未知}"; warnings=$((warnings + 1)); fi

  menu_section "密钥与权限"
  count="$(ssh_user_valid_key_count "$user")"
  if is_uint_in_range "$count" 1 100000; then ok "${user} 共发现 ${count} 个有效登录公钥"; else warn "${user} 未发现有效登录公钥"; warnings=$((warnings + 1)); fi
  while IFS= read -r path; do
    [[ -e "$path" ]] || continue
    if [[ -L "$path" || ! -f "$path" ]]; then
      warn "密钥路径不是普通文件：$path"
      warnings=$((warnings + 1))
    elif ssh_mode_is_group_world_writable "$path"; then
      warn "密钥文件可被组或其他用户写入：$path"
      warnings=$((warnings + 1))
    else
      print_kv "密钥权限" "$(stat -c '%a %U:%G' "$path" 2>/dev/null || echo 未知)  $path"
    fi
    owner="$(stat -c '%U' "$path" 2>/dev/null || true)"
    if [[ -n "$owner" && "$owner" != "$user" && "$owner" != "root" ]]; then
      warn "密钥文件所有者异常：${owner} / $path"
      warnings=$((warnings + 1))
    fi
    dir="$(dirname "$path")"
    if [[ "$dir" != "$(ssh_user_home "$user" 2>/dev/null || true)" ]]; then
      if ssh_mode_is_group_world_writable "$dir"; then
        warn "密钥目录可被组或其他用户写入：$dir"
        warnings=$((warnings + 1))
      fi
      owner="$(stat -c '%U' "$dir" 2>/dev/null || true)"
      if [[ -n "$owner" && "$owner" != "$user" && "$owner" != "root" ]]; then
        warn "密钥目录所有者异常：${owner} / $dir"
        warnings=$((warnings + 1))
      fi
    fi
  done < <(ssh_authorized_keys_paths "$user" 2>/dev/null || true)
  path="$(ssh_user_home "$user" 2>/dev/null || true)"
  if [[ -n "$path" ]] && ssh_mode_is_group_world_writable "$path"; then
    warn "用户家目录可被组或其他用户写入，StrictModes 可能拒绝密钥：$path"
    warnings=$((warnings + 1))
  fi
  owner="$(stat -c '%U' "$path" 2>/dev/null || true)"
  if [[ -n "$owner" && "$owner" != "$user" && "$owner" != "root" ]]; then
    warn "用户家目录所有者异常：${owner} / $path"
    warnings=$((warnings + 1))
  fi
  if [[ -f /etc/ssh/sshd_config ]] && ssh_mode_is_group_world_writable /etc/ssh/sshd_config; then
    warn "/etc/ssh/sshd_config 可被非 root 写入"
    warnings=$((warnings + 1))
  fi
  if [[ -f "$SSH_DROPIN_FILE" ]] && ssh_mode_is_group_world_writable "$SSH_DROPIN_FILE"; then
    warn "脚本管理的 SSH 配置可被非 root 写入：$SSH_DROPIN_FILE"
    warnings=$((warnings + 1))
  fi

  menu_section "体检结论"
  if (( warnings == 0 )); then
    ok "未发现常见 SSH 配置风险"
  else
    warn "共发现 ${warnings} 项需要关注；体检只读取状态，没有修改配置"
  fi
}

ssh_apply_recommended_hardening() {
  local user="${1:-root}" count=0 snapshot=""
  valid_username "$user" || { warn "用户名无效：$user"; return 1; }
  id "$user" >/dev/null 2>&1 || { warn "用户不存在：$user"; return 1; }
  ssh_pkg_install || return 1
  if ! ssh_user_login_shell_usable "$user"; then
    warn "${user} 的登录 Shell 不可用或被设为 nologin/false，已取消加固"
    return 1
  fi
  count="$(ssh_user_valid_key_count "$user")"
  if ! is_uint_in_range "$count" 1 100000; then
    warn "${user} 没有可验证的本地公钥"
    info "进入自动生成流程；私钥保存并安装成功后才能继续加固"
    ssh_generate_authorized_key "$user" || return 1
    count="$(ssh_user_valid_key_count "$user")"
    is_uint_in_range "$count" 1 100000 || { warn "仍未发现有效公钥，已取消加固"; return 1; }
  fi
  ssh_backup_once || return 1
  warn "将关闭所有用户的密码/交互认证，并保留公钥登录"
  info "同时禁止空密码、主机信任认证、用户环境变量与 X11 转发"
  warn "root 将只允许公钥；不会禁用 TCP 转发或 Agent 转发"
  warn "请保持当前会话，完成后另开窗口验证 ${user} 的密钥登录"
  if [[ "$user" != "root" ]] && ! id -nG "$user" 2>/dev/null | tr ' ' '\n' | grep -Eq '^(sudo|wheel)$'; then
    warn "${user} 未检测到 sudo/wheel 管理组；加固后可能无法执行管理操作"
  fi
  confirm_word "HARDEN" "确认加固请输入 HARDEN > " || { warn "已取消"; return 0; }
  ssh_repair_authorized_keys "$user" || { warn "密钥权限检查失败，未修改 SSH 策略"; return 1; }

  snapshot="$(ssh_managed_snapshot_create)" || { warn "无法创建本次 SSH 配置快照"; return 1; }
  ssh_apply_base_hardening || { ssh_managed_change_abort "$snapshot" "无法写入 SSH 加固配置"; return 1; }
  ssh_dropin_set_many \
    "PasswordAuthentication" "no" \
    "KbdInteractiveAuthentication" "no" \
    "ChallengeResponseAuthentication" "no" \
    "PubkeyAuthentication" "yes" \
    "PermitRootLogin" "prohibit-password" \
    "StrictModes" "yes" \
    "HostbasedAuthentication" "no" \
    "IgnoreRhosts" "yes" \
    "PermitUserEnvironment" "no" \
    "X11Forwarding" "no" || {
      ssh_managed_change_abort "$snapshot" "无法完整写入推荐加固配置"
      return 1
    }

  if ! ssh_validate_config || \
     ! ssh_verify_effective_setting "PasswordAuthentication" "no" "$user" || \
     ! ssh_verify_effective_setting "KbdInteractiveAuthentication" "no" "$user" || \
     ! ssh_verify_effective_setting "PubkeyAuthentication" "yes" "$user" || \
     ! ssh_verify_effective_setting "PermitRootLogin" "prohibit-password" "root" || \
     ! ssh_verify_authentication_path "publickey" "$user"; then
    ssh_managed_change_abort "$snapshot" "推荐加固策略未按预期生效"
    return 1
  fi
  ssh_managed_change_finish "$snapshot" || return 1
  ok "SSH 推荐加固已安全应用"
  info "请不要关闭当前会话；先另开终端验证：ssh -p $(ssh_current_ports | awk '{print $1}') ${user}@服务器IP"
}

ssh_login_activity() {
  local output=""
  menu_section "最近成功登录"
  if have_cmd last; then
    output="$(command_with_timeout 10 last -ai -n 15 2>/dev/null || command_with_timeout 10 last 2>/dev/null | sed -n '1,15p' || true)"
    [[ -n "$output" ]] && printf '%s\n' "$output" || info "没有可显示的 wtmp 登录记录"
  else
    warn "系统缺少 last 命令"
  fi

  menu_section "最近失败登录"
  if have_cmd lastb; then
    output="$(command_with_timeout 10 lastb -ai -n 15 2>/dev/null || true)"
    [[ -n "$output" ]] && printf '%s\n' "$output" || info "没有可显示的 btmp 失败记录"
  else
    info "系统没有 lastb，改查认证日志"
  fi

  menu_section "24 小时认证日志"
  output=""
  if have_cmd journalctl; then
    output="$(command_with_timeout 12 journalctl -u ssh.service -u sshd.service --since '-24 hours' --no-pager -n 200 2>/dev/null | grep -Ei 'Accepted|Failed|Invalid user|authentication failure|Disconnected from authenticating' | tail -n 40 || true)"
  fi
  if [[ -z "$output" ]]; then
    local log_file=""
    for log_file in /var/log/auth.log /var/log/secure; do
      [[ -r "$log_file" ]] || continue
      output="$(tail -n 500 "$log_file" 2>/dev/null | grep -Ei 'Accepted|Failed|Invalid user|authentication failure|Disconnected from authenticating' | tail -n 40 || true)"
      [[ -n "$output" ]] && break
    done
  fi
  [[ -n "$output" ]] && printf '%s\n' "$output" || info "最近 24 小时没有匹配的认证日志，或日志不可读"
  info "日志仅供排查；来源 IP 可能经过跳板机、NAT 或代理"
}

ssh_keys_menu() {
  while true; do
    menu_header "SSH 登录密钥" "自动生成 · 已有密钥导入 · 权限修复 · 防止误删最后密钥"
    menu_section "密钥管理"
    menu_item "1" "查看登录密钥" "列出指纹，不显示完整密钥内容"
    menu_item "2" "自动生成登录密钥" "默认方式；一次性显示私钥并自动安装公钥"
    menu_item "3" "导入已有公钥" "仅在已有本地密钥时使用，校验并自动去重"
    menu_item "4" "移除登录公钥" "按指纹选择，禁止删除最后一个密钥"
    menu_item "5" "修复密钥权限" ".ssh 目录 700、密钥文件 600，并移除路径写入风险"
    menu_back_item
    local c="" user="" key=""
    read_tty c "请输入编号 > " ""
    case "$c" in
      1)
        read_tty user "用户名（默认 root）> " "root"
        ssh_list_authorized_keys "$user" || true
        pause_up
        ;;
      2)
        read_tty user "用户名（默认 root）> " "root"
        ssh_generate_authorized_key "$user" || true
        pause_up
        ;;
      3)
        read_tty user "用户名（默认 root）> " "root"
        read_tty key "粘贴一整行 OpenSSH 公钥 > " ""
        ssh_add_authorized_key "$user" "$key" || true
        pause_up
        ;;
      4)
        read_tty user "用户名（默认 root）> " "root"
        ssh_remove_authorized_key "$user" || true
        pause_up
        ;;
      5)
        read_tty user "用户名（默认 root）> " "root"
        ssh_repair_authorized_keys "$user" || true
        pause_up
        ;;
      0|q|Q) return 0 ;;
      *) warn "无效选项"; pause_up ;;
    esac
  done
}

ssh_safe_enable_password_for_user_keep_root_key() {
  local user="${1:-dmit}" snapshot=""
  valid_username "$user" || { warn "用户名无效：$user"; return 1; }
  [[ "$user" != "root" ]] || { warn "此功能只创建普通用户；如需 root 密码，请选择“开启密码登录”"; return 1; }
  ssh_pkg_install || return 1
  ssh_backup_once || return 1

  warn "推荐模式：普通用户密码登录；root 禁止密码（仅密钥）"
  warn "密码认证属于全局开关：其他未被 SSH 策略禁止且已设置密码的普通账号也可能使用密码"
  warn "建议保持当前 SSH 会话不要断开，确认新用户可登录后再退出"

  local p; p="$(ssh_random_pass)"
  [[ -z "${p:-}" ]] && { warn "生成随机密码失败"; return 1; }

  snapshot="$(ssh_managed_snapshot_create)" || { warn "无法创建本次 SSH 配置快照"; return 1; }
  ssh_apply_base_hardening || { ssh_managed_change_abort "$snapshot" "无法写入 SSH 加固配置"; return 1; }
  ssh_dropin_set_many \
    "PasswordAuthentication" "yes" \
    "PubkeyAuthentication" "yes" \
    "PermitRootLogin" "prohibit-password" || {
      ssh_managed_change_abort "$snapshot" "无法完整写入 SSH 登录策略"
      return 1
    }

  if ! ssh_validate_config || \
     ! ssh_verify_effective_setting "PasswordAuthentication" "yes" "$user" || \
     ! ssh_verify_effective_setting "PubkeyAuthentication" "yes" "$user" || \
     ! ssh_verify_effective_setting "PermitRootLogin" "prohibit-password" "root" || \
     ! ssh_verify_authentication_path "any" "$user" || \
     ! ssh_verify_authentication_path "publickey" "root"; then
    ssh_managed_change_abort "$snapshot" "SSH 登录策略未按预期生效"
    return 1
  fi
  if ! ssh_create_user_with_password "$user" "$p"; then
    ssh_managed_change_abort "$snapshot" "用户或密码创建失败"
    return 1
  fi
  if ! ssh_managed_change_finish "$snapshot"; then
    warn "SSH 策略已回滚；已创建的 ${user} 账号及其密码不会被自动删除"
    return 1
  fi
  ok "SSH 已安全重新加载（推荐模式已生效）"
  sshd_status_hint
}

ssh_enable_password_keep_key_for_user() {
  local user="${1:-root}"
  local mode="${2:-random}" # random|custom
  local passwd="${3:-}" snapshot="" root_policy="prohibit-password"

  valid_username "$user" || { warn "用户名无效：$user"; return 1; }
  id "$user" >/dev/null 2>&1 || { warn "用户不存在：$user"; return 1; }
  ssh_user_login_shell_usable "$user" || { warn "${user} 的登录 Shell 不可用，已取消"; return 1; }
  ssh_pkg_install || return 1
  ssh_backup_once || return 1

  warn "中等模式：开启密码登录（保留密钥登录）"
  warn "密码认证属于全局开关；选择非 root 用户时，root 仍会保持禁止密码"
  warn "建议保持当前 SSH 会话不要断开，确认密码可登录后再退出"

  if [[ "$mode" == "random" ]]; then passwd="$(ssh_random_pass)"; fi
  [[ -z "${passwd:-}" ]] && { warn "密码为空：取消"; return 1; }

  snapshot="$(ssh_managed_snapshot_create)" || { warn "无法创建本次 SSH 配置快照"; return 1; }
  ssh_apply_base_hardening || { ssh_managed_change_abort "$snapshot" "无法写入 SSH 配置"; return 1; }
  if [[ "$user" == "root" ]]; then root_policy="yes"; fi
  ssh_dropin_set_many \
    "PasswordAuthentication" "yes" \
    "PubkeyAuthentication" "yes" \
    "PermitRootLogin" "$root_policy" || {
      ssh_managed_change_abort "$snapshot" "无法完整写入 SSH 登录策略"
      return 1
    }

  if ! ssh_validate_config || \
     ! ssh_verify_effective_setting "PasswordAuthentication" "yes" "$user" || \
     ! ssh_verify_effective_setting "PubkeyAuthentication" "yes" "$user" || \
     ! ssh_verify_effective_setting "PermitRootLogin" "$root_policy" "root" || \
     ! ssh_verify_authentication_path "any" "$user"; then
    ssh_managed_change_abort "$snapshot" "SSH 登录策略未按预期生效"
    return 1
  fi
  if [[ "$root_policy" != "yes" ]] && ! ssh_verify_authentication_path "publickey" "root"; then
    ssh_managed_change_abort "$snapshot" "root 公钥登录路径未按预期保留"
    return 1
  fi
  if ! ssh_set_user_password "$user" "$passwd"; then
    ssh_managed_change_abort "$snapshot" "用户密码设置失败"
    return 1
  fi
  ok "已设置用户密码：${user}"
  if [[ "$mode" == "random" ]]; then echo -e "${c_green}随机密码：${passwd}${c_reset}"; fi
  if ! ssh_managed_change_finish "$snapshot"; then
    warn "SSH 策略已回滚；${user} 的账号密码已经更新，请妥善保存"
    return 1
  fi
  if [[ "$user" == "root" ]]; then
    ok "SSH 已安全重新加载（root 密码与密钥均可）"
  else
    ok "SSH 已安全重新加载（${user} 可使用密码；root 仍仅允许密钥）"
  fi
  sshd_status_hint
}

ssh_password_only_disable_key_risky() {
  local user="${1:-root}"
  local mode="${2:-random}" # random|custom
  local passwd="${3:-}" snapshot="" root_policy="prohibit-password"

  valid_username "$user" || { warn "用户名无效：$user"; return 1; }
  id "$user" >/dev/null 2>&1 || { warn "用户不存在：$user"; return 1; }
  ssh_user_login_shell_usable "$user" || { warn "${user} 的登录 Shell 不可用，已取消"; return 1; }
  ssh_pkg_install || return 1
  ssh_backup_once || return 1

  warn "高风险模式：仅密码登录（禁用密钥）"
  warn "该设置会全局禁用 SSH 公钥认证，而不只影响所选用户"
  warn "有锁门风险：务必保持当前 SSH 会话不断开"
  local ans=""
  read_tty ans "确认继续请输入 YES > " ""
  if [[ "${ans}" != "YES" ]]; then
    warn "已取消"
    return 0
  fi

  if [[ "$mode" == "random" ]]; then passwd="$(ssh_random_pass)"; fi
  [[ -z "${passwd:-}" ]] && { warn "密码为空：取消"; return 1; }

  snapshot="$(ssh_managed_snapshot_create)" || { warn "无法创建本次 SSH 配置快照"; return 1; }
  ssh_apply_base_hardening || { ssh_managed_change_abort "$snapshot" "无法写入 SSH 配置"; return 1; }
  if [[ "$user" == "root" ]]; then root_policy="yes"; fi
  ssh_dropin_set_many \
    "PasswordAuthentication" "yes" \
    "PubkeyAuthentication" "no" \
    "PermitRootLogin" "$root_policy" || {
      ssh_managed_change_abort "$snapshot" "无法完整写入 SSH 登录策略"
      return 1
    }

  if ! ssh_validate_config || \
     ! ssh_verify_effective_setting "PasswordAuthentication" "yes" "$user" || \
     ! ssh_verify_effective_setting "PubkeyAuthentication" "no" "$user" || \
     ! ssh_verify_effective_setting "PermitRootLogin" "$root_policy" "root" || \
     ! ssh_verify_authentication_path "password" "$user"; then
    ssh_managed_change_abort "$snapshot" "SSH 登录策略未按预期生效"
    return 1
  fi
  if ! ssh_set_user_password "$user" "$passwd"; then
    ssh_managed_change_abort "$snapshot" "用户密码设置失败"
    return 1
  fi
  ok "已设置用户密码：${user}"
  if [[ "$mode" == "random" ]]; then echo -e "${c_green}随机密码：${passwd}${c_reset}"; fi
  if ! ssh_managed_change_finish "$snapshot"; then
    warn "SSH 策略已回滚；${user} 的账号密码已经更新，请妥善保存"
    return 1
  fi
  ok "SSH 已安全重新加载（仅密码登录）"
  sshd_status_hint
}

ssh_restore_snapshot_create() {
  local snapshot=""
  snapshot="$(mktemp -d /tmp/dmitbox-ssh-restore.XXXXXX)" || return 1
  mkdir -p "$snapshot/current" || { rm -rf -- "$snapshot"; return 1; }
  cp -a /etc/ssh/. "$snapshot/current/" || { rm -rf -- "$snapshot"; return 1; }
  printf '%s\n' "$snapshot"
}

ssh_restore_snapshot_discard() {
  local snapshot="${1:-}"
  [[ "$snapshot" == /tmp/dmitbox-ssh-restore.* && -d "$snapshot" ]] || return 0
  rm -rf -- "$snapshot"
}

ssh_restore_snapshot_apply() {
  local snapshot="$1" archive="$2" member="" relative="" target=""
  [[ "$snapshot" == /tmp/dmitbox-ssh-restore.* && -d "$snapshot/current" ]] || return 1
  ssh_backup_archive_safe "$archive" || return 1
  while IFS= read -r member; do
    [[ "$member" == etc/ssh/* ]] || continue
    relative="${member#etc/ssh/}"
    [[ -n "$relative" && "$relative" != */ ]] || continue
    if [[ ! -e "$snapshot/current/$relative" && ! -L "$snapshot/current/$relative" ]]; then
      target="/etc/ssh/$relative"
      [[ -f "$target" && ! -L "$target" ]] && rm -f -- "$target" || true
    fi
  done < <(tar -tzf "$archive" 2>/dev/null)
  cp -a "$snapshot/current/." /etc/ssh/
}

ssh_restore_key_login() {
  local snapshot=""
  info "SSH：恢复原来的配置（从备份还原）"
  if [[ -s "$SSH_ORIG_TGZ" ]]; then
    [[ -d /etc/ssh && ! -L /etc/ssh ]] || { warn "/etc/ssh 不是可安全恢复的普通目录"; return 1; }
    [[ ! -e "$SSH_DROPIN_DIR" || ( -d "$SSH_DROPIN_DIR" && ! -L "$SSH_DROPIN_DIR" ) ]] || {
      warn "sshd_config.d 不是可安全恢复的普通目录"
      return 1
    }
    have_cmd find || { warn "系统缺少 find，无法安全检查 SSH 恢复路径"; return 1; }
    if [[ -n "$(find /etc/ssh -type l -print -quit 2>/dev/null || true)" ]]; then
      warn "/etc/ssh 中存在符号链接，为避免恢复越界，已取消恢复"
      return 1
    fi
    if ! ssh_backup_archive_safe "$SSH_ORIG_TGZ"; then
      warn "SSH 备份损坏或包含越界路径，未执行恢复：$SSH_ORIG_TGZ"
      return 1
    fi
    snapshot="$(ssh_restore_snapshot_create)" || { warn "无法创建恢复前快照，已取消恢复"; return 1; }
    rm -f "$SSH_DROPIN_FILE" "$SSH_LEGACY_DROPIN_FILE" 2>/dev/null || true
    if ! tar -xzf "$SSH_ORIG_TGZ" -C / 2>/dev/null; then
      warn "SSH 配置解压失败，正在恢复操作前配置"
      ssh_restore_snapshot_apply "$snapshot" "$SSH_ORIG_TGZ" || warn "自动恢复操作前配置失败，请立即人工检查"
      ssh_restore_snapshot_discard "$snapshot"
      return 1
    fi
    ssh_backup_archive_has_member "$SSH_ORIG_TGZ" "${SSH_DROPIN_FILE#/}" || rm -f "$SSH_DROPIN_FILE" 2>/dev/null || true
    ssh_backup_archive_has_member "$SSH_ORIG_TGZ" "${SSH_LEGACY_DROPIN_FILE#/}" || rm -f "$SSH_LEGACY_DROPIN_FILE" 2>/dev/null || true
    if ! ssh_validate_config; then
      warn "备份配置未通过 sshd -t，正在恢复操作前配置"
      ssh_restore_snapshot_apply "$snapshot" "$SSH_ORIG_TGZ" || warn "自动恢复操作前配置失败，请立即人工检查"
      ssh_restore_snapshot_discard "$snapshot"
      return 1
    fi
    if ! sshd_restart; then
      warn "SSH 服务未能加载备份配置，正在恢复操作前配置"
      if ssh_restore_snapshot_apply "$snapshot" "$SSH_ORIG_TGZ"; then
        sshd_restart >/dev/null 2>&1 || warn "操作前配置已恢复，但 SSH 服务未能自动重新加载"
      else
        warn "自动恢复操作前配置失败，请保持当前会话并立即人工检查"
      fi
      ssh_restore_snapshot_discard "$snapshot"
      return 1
    fi
    ssh_restore_snapshot_discard "$snapshot"
    ok "已恢复 SSH 原始配置并安全重新加载"
    sshd_status_hint
  else
    warn "未找到备份：$SSH_ORIG_TGZ"
  fi
}

ssh_current_ports() {
  if have_cmd sshd; then
    sshd -T 2>/dev/null | awk '$1=="port"{print $2}' | tr '\n' ' ' | sed 's/[[:space:]]*$//'
    return 0
  fi
  local ports=""
  ports="$(grep -RihE '^[[:space:]]*Port[[:space:]]+' /etc/ssh/sshd_config /etc/ssh/sshd_config.d 2>/dev/null | awk '{print $2}' | tr '\n' ' ' | sed 's/[[:space:]]*$//')"
  echo "${ports:-22}"
}

port_in_use() {
  local p="$1"
  if have_cmd ss; then
    ss -lntp 2>/dev/null | awk '{print $4}' | grep -Eq "[:.]${p}$" && return 0
  elif have_cmd netstat; then
    netstat -lntp 2>/dev/null | awk '{print $4}' | grep -Eq "[:.]${p}$" && return 0
  fi
  return 1
}

firewall_open_port_best_effort() {
  local p="$1" backend="" rc=0 zone="-" netfilter_risk="none"
  is_uint_in_range "$p" 1 65535 || return 1
  p=$((10#$p))
  backend="$(common_firewall_active_backend)" || rc=$?
  if (( rc == 2 )); then
    warn "UFW 与 firewalld 同时启用，无法安全放行 SSH 新端口"
    return 1
  fi
  if (( rc != 0 )); then
    netfilter_risk="$(common_firewall_netfilter_risk)"
    case "$netfilter_risk" in
      input|unknown)
        warn "已有规则可能影响本机入站，脚本不会向未知链盲目插入规则"
        common_firewall_netfilter_show_inventory
        info "请先在“常用防火墙”中完成安全启用，或人工在原规则中放行 ${p}/tcp"
        return 1
        ;;
      forwarding)
        info "只检测到转发/NAT/输出规则，不负责拦截本机 INPUT"
        info "当前没有启用中的本机防火墙，无需新增 ${p}/tcp 放行规则"
        return 0
        ;;
      *)
        info "未启用本机防火墙，无需新增 ${p}/tcp 放行规则"
        return 0
        ;;
    esac
  fi
  if [[ "$backend" == "firewalld" ]]; then
    zone="$(common_firewall_firewalld_zone)" || { warn "无法确定 firewalld 入站区域"; return 1; }
  fi
  common_firewall_apply_rule "$backend" allow-port tcp "$p" - "$zone" || return 1
  ok "${backend} 已允许 SSH 新端口：${p}/tcp"
}

ssh_set_port_legacy() {
  local newp="$1" snapshot="" attempt=0

  is_uint_in_range "$newp" 1 65535 || { warn "端口必须是 1-65535 的数字"; return 1; }
  newp=$((10#$newp))
  if (( newp < 1024 )); then warn "不建议使用 1024 以下端口"; fi

  local cur_ports; cur_ports="$(ssh_current_ports || echo "22")"
  if echo " $cur_ports " | grep -q " ${newp} "; then
    warn "端口 ${newp} 已在 SSH 当前配置中"
    return 0
  fi

  if port_in_use "$newp"; then
    warn "端口 ${newp} 似乎已被占用（请换一个）"
    return 1
  fi

  ssh_pkg_install || return 1
  ssh_backup_once || return 1

  warn "更换 SSH 端口会影响新连接"
  warn "强烈建议保持当前 SSH 会话不要断开"
  warn "请先测试：ssh -p ${newp} user@你的IP"

  if ! firewall_open_port_best_effort "$newp"; then
    warn "无法确认防火墙已放行新端口，SSH 配置未修改"
    return 1
  fi
  ssh_socket_disable_if_any || return 1
  snapshot="$(ssh_managed_snapshot_create)" || { warn "无法创建本次 SSH 配置快照"; return 1; }
  ssh_dropin_ensure || { ssh_managed_change_abort "$snapshot" "无法准备 SSH drop-in"; return 1; }
  ssh_dropin_set_kv "Port" "$newp" || { ssh_managed_change_abort "$snapshot" "无法写入 SSH 新端口"; return 1; }

  if ! ssh_validate_config || ! ssh_verify_effective_setting "Port" "$newp" "root"; then
    ssh_managed_change_abort "$snapshot" "SSH 新端口未通过配置验证"
    return 1
  fi
  ssh_managed_change_finish "$snapshot" 1 || return 1
  for ((attempt=1; attempt<=8; attempt++)); do
    port_in_use "$newp" && break
    sleep 1
  done
  if ! port_in_use "$newp"; then
    ssh_managed_change_abort "$snapshot" "SSH 服务重新加载后仍未监听 ${newp}/tcp"
    return 1
  fi
  ssh_managed_snapshot_discard "$snapshot"
  ok "SSH 已切换并确认监听新端口 → ${newp}"

  echo -e "${c_dim}--- 立即验证 ---${c_reset}"
  sshd -T 2>/dev/null | grep -Ei 'port|passwordauthentication|permitrootlogin|pubkeyauthentication|authenticationmethods' || true
  ss -lntp 2>/dev/null | grep -E "sshd|:${newp}\b|:22\b" || true

  echo -e "${c_green}提示：请用新端口测试登录成功后，再退出当前会话${c_reset}"
  echo -e "${c_dim}当前端口：$(ssh_current_ports)${c_reset}"
}

ssh_menu() {
  while true; do
    menu_header "SSH 安全" "安全体检 · 密钥管理 · 防锁死加固 · 登录审计"
    menu_section "检查与记录"
    menu_item "1" "SSH 安全体检" "配置语法、认证风险、监听与密钥权限"
    menu_item "2" "查看生效配置" "按指定用户计算最终认证策略"
    menu_item "3" "最近登录记录" "成功、失败与近 24 小时认证日志"
    menu_section "密钥与加固"
    menu_item "4" "登录密钥管理" "查看、添加、移除与权限修复"
    menu_item "5" "一键推荐加固" "验证公钥后关闭密码认证，失败自动回滚"
    menu_item "6" "创建普通用户" "普通用户密码登录，root 保持仅密钥"
    menu_item "7" "开启密码登录" "保留公钥；非 root 操作不放开 root 密码"
    menu_section "连接与恢复"
    menu_item "8" "更换 SSH 端口（受保护）" "新连接确认 + 独立 systemd 回滚"
    menu_item "9" "恢复原始配置" "从首次修改前的备份还原"
    menu_section "高风险"
    menu_item "10" "仅密码登录" "禁用公钥，可能导致无法登录"
    menu_back_item
    local c=""
    read_tty c "请输入编号 > " ""
    case "$c" in
      1)
        local u=""
        read_tty u "检查用户（默认 root）> " "root"
        ssh_security_audit "$u" || true
        pause_up
        ;;
      2)
        local u=""
        read_tty u "检查用户（默认 root）> " "root"
        ssh_show_effective_configuration "$u" || true
        pause_up
        ;;
      3) ssh_login_activity || true; pause_up ;;
      4) ssh_keys_menu ;;
      5)
        if [[ "${DMITBOX_ALLOW_UNGUARDED_SSH:-0}" != "1" ]]; then
          warn "旧式全局 SSH 认证加固缺少独立超时恢复，默认禁用"; pause_up; continue
        fi
        local u=""
        read_tty u "确保可用公钥的用户（默认 root）> " "root"
        ssh_apply_recommended_hardening "$u" || true
        pause_up
        ;;
      6)
        local u=""
        read_tty u "新用户名（默认 dmit）> " "dmit"
        ssh_safe_enable_password_for_user_keep_root_key "$u" || true
        pause_up
        ;;
      7)
        if [[ "${DMITBOX_ALLOW_UNGUARDED_SSH:-0}" != "1" ]]; then
          warn "旧式认证策略修改缺少独立超时恢复，默认禁用"; pause_up; continue
        fi
        local u="" m="" p=""
        read_tty u "用户名（默认 root）> " "root"
        echo "  1) 随机密码"
        echo "  2) 自定义密码"
        read_tty m "选择> " ""
        if [[ "$m" == "1" ]]; then
          ssh_enable_password_keep_key_for_user "$u" "random" "" || true
        elif [[ "$m" == "2" ]]; then
          read_tty_secret p "设置密码（输入不回显）> "
          ssh_enable_password_keep_key_for_user "$u" "custom" "$p" || true
        else
          warn "无效选项"
        fi
        pause_up
        ;;
      8)
        echo -e "${c_dim}当前 SSH 端口：$(ssh_current_ports)${c_reset}"
        local p=""
        read_tty p "输入新端口（建议 20000-59999）> " ""
        netg_ssh_port_safe "$p" || true
        pause_up
        ;;
      9)
        if [[ "${DMITBOX_ALLOW_UNGUARDED_SSH:-0}" != "1" ]]; then
          warn "旧式 SSH 全量恢复缺少独立超时恢复，默认禁用"; pause_up; continue
        fi
        if confirm_word "RESTORE" "确认恢复首次备份请输入 RESTORE > "; then
          ssh_restore_key_login || true
        else
          warn "已取消"
        fi
        pause_up
        ;;
      10)
        if [[ "${DMITBOX_ALLOW_UNGUARDED_SSH:-0}" != "1" ]]; then
          warn "仅密码登录缺少超时自动回滚，默认禁用；需要独立审查后才能手动启用"
          pause_up
          continue
        fi
        local u="" m="" p=""
        read_tty u "用户名（默认 root）> " "root"
        echo "  1) 随机密码"
        echo "  2) 自定义密码"
        read_tty m "选择> " ""
        if [[ "$m" == "1" ]]; then
          ssh_password_only_disable_key_risky "$u" "random" "" || true
        elif [[ "$m" == "2" ]]; then
          read_tty_secret p "设置密码（输入不回显）> "
          ssh_password_only_disable_key_risky "$u" "custom" "$p" || true
        else
          warn "无效选项"
        fi
        pause_up
        ;;
      0|q|Q) return 0 ;;
      *) warn "无效选项"; pause_up ;;
    esac
  done
}

# ---------------- 测试：运行外部脚本 ----------------
# ======================================================================
# 系统状态 / 进程 / 端口
# ======================================================================


system_overview() {
  menu_header "系统状态总览" "硬件资源 · 网络接口 · 磁盘与运行状态"

  local cpu_model="" cpu_count="" virt="" uptime_text="" load_text=""
  local ifc="" rx="0" tx="0" timezone="" time_sync="N/A"
  cpu_model="$(awk -F: '/model name|Hardware|Processor/{gsub(/^[[:space:]]+/, "", $2); print $2; exit}' /proc/cpuinfo 2>/dev/null || true)"
  cpu_count="$(getconf _NPROCESSORS_ONLN 2>/dev/null || nproc 2>/dev/null || echo N/A)"
  virt="$(systemd-detect-virt 2>/dev/null || true)"
  uptime_text="$(uptime -p 2>/dev/null || awk '{printf "%.1f 天", $1/86400}' /proc/uptime 2>/dev/null || echo N/A)"
  load_text="$(awk '{print $1 " / " $2 " / " $3}' /proc/loadavg 2>/dev/null || echo N/A)"
  timezone="$(timedatectl show -p Timezone --value 2>/dev/null || cat /etc/timezone 2>/dev/null || echo N/A)"
  time_sync="$(time_sync_status_text 2>/dev/null || echo N/A)"
  ifc="$(default_iface)"
  [[ -r "/sys/class/net/${ifc}/statistics/rx_bytes" ]] && rx="$(cat "/sys/class/net/${ifc}/statistics/rx_bytes")"
  [[ -r "/sys/class/net/${ifc}/statistics/tx_bytes" ]] && tx="$(cat "/sys/class/net/${ifc}/statistics/tx_bytes")"

  menu_section "系统"
  print_kv "操作系统" "$(os_pretty_name)"
  print_kv "内核" "$(uname -r) ($(uname -m))"
  print_kv "虚拟化" "${virt:-none}"
  print_kv "CPU" "${cpu_model:-N/A}"
  print_kv "CPU 线程" "$cpu_count"
  print_kv "运行时间" "$uptime_text"
  print_kv "系统负载" "$load_text"
  print_kv "时区 / 时间同步" "${timezone} / ${time_sync}"

  menu_section "内存与 Swap"
  if have_cmd free; then
    free -h
  else
    sed -n '1,8p' /proc/meminfo 2>/dev/null || true
  fi

  menu_section "磁盘容量"
  df -hT -x tmpfs -x devtmpfs 2>/dev/null | sed -n '1,18p' || df -h 2>/dev/null || true
  echo
  echo -e "${c_dim}inode 使用率：${c_reset}"
  df -ih -x tmpfs -x devtmpfs 2>/dev/null | sed -n '1,18p' || true

  menu_section "网络"
  print_kv "默认网卡" "$ifc"
  print_kv "累计接收 / 发送" "$(human_bytes "$rx") / $(human_bytes "$tx")"
  print_kv "TCP 拥塞控制" "$(sysctl -n net.ipv4.tcp_congestion_control 2>/dev/null || echo N/A)"
  ip -br address show 2>/dev/null || true
  echo
  ip -4 route show default 2>/dev/null || true
  ip -6 route show default 2>/dev/null || true
}

processes_top_cpu() {
  menu_header "CPU 占用进程" "按当前 CPU 使用率排序，显示前 15 项"
  if have_cmd ps; then
    local output=""
    output="$(ps -eo pid,ppid,user,stat,%cpu,%mem,etime,comm --sort=-%cpu 2>/dev/null || true)"
    if [[ -n "$output" ]]; then
      sed -n '1,16p' <<< "$output"
    else
      warn "当前 ps 不支持排序参数，显示基础进程列表"
      ps 2>/dev/null | sed -n '1,16p' || true
    fi
  else
    warn "系统没有 ps 命令"
  fi
}

processes_top_memory() {
  menu_header "内存占用进程" "按当前内存使用率排序，显示前 15 项"
  if have_cmd ps; then
    local output=""
    output="$(ps -eo pid,ppid,user,stat,%mem,rss,etime,comm --sort=-%mem 2>/dev/null || true)"
    if [[ -n "$output" ]]; then
      sed -n '1,16p' <<< "$output"
    else
      warn "当前 ps 不支持排序参数，显示基础进程列表"
      ps 2>/dev/null | sed -n '1,16p' || true
    fi
  else
    warn "系统没有 ps 命令"
  fi
}

listening_ports() {
  menu_header "监听端口" "TCP / UDP 监听地址、PID 与程序"
  if have_cmd ss; then
    ss -lntup 2>/dev/null || ss -lntu 2>/dev/null || true
  elif have_cmd netstat; then
    netstat -lntup 2>/dev/null || netstat -lntu 2>/dev/null || true
  else
    warn "缺少 ss/netstat；可安装 iproute2 或 net-tools"
  fi
}

port_lookup() {
  local port="" output=""
  read_tty port "输入端口（1-65535）> " ""
  is_uint_in_range "$port" 1 65535 || { warn "端口必须在 1-65535 之间"; return 1; }
  port=$((10#$port))

  menu_header "端口 ${port} 诊断" "查看监听程序及相关连接"
  if have_cmd ss; then
    output="$(ss -H -lntup "sport = :${port}" 2>/dev/null || true)"
    if [[ -n "$output" ]]; then
      echo "$output"
    else
      warn "未发现监听 ${port} 端口的程序"
    fi
    echo
    echo -e "${c_dim}相关 TCP 连接：${c_reset}"
    ss -H -ntup 2>/dev/null | grep -E "[:.]${port}([[:space:]]|$)" | sed -n '1,80p' || true
  elif have_cmd netstat; then
    output="$(netstat -lntup 2>/dev/null | grep -E "[:.]${port}[[:space:]]" || true)"
    if [[ -n "$output" ]]; then echo "$output"; else warn "未发现监听 ${port} 端口的程序"; fi
  else
    warn "缺少 ss/netstat，无法诊断端口"
    return 1
  fi
}

port_listener_for_protocol() {
  local port="$1" protocol="$2" output=""
  [[ "$protocol" == "tcp" || "$protocol" == "udp" ]] || return 1
  if have_cmd ss; then
    if [[ "$protocol" == "tcp" ]]; then
      output="$(ss -H -ltnp "sport = :${port}" 2>/dev/null || true)"
      [[ -n "$output" ]] || output="$(ss -H -ltnp 2>/dev/null | awk -v port="$port" '$4 ~ (":" port "$")' || true)"
    else
      output="$(ss -H -lunp "sport = :${port}" 2>/dev/null || true)"
      [[ -n "$output" ]] || output="$(ss -H -lunp 2>/dev/null | awk -v port="$port" '$4 ~ (":" port "$")' || true)"
    fi
    printf '%s\n' "$output" | awk 'NF'
  elif have_cmd netstat; then
    if [[ "$protocol" == "tcp" ]]; then
      netstat -lntp 2>/dev/null | awk -v port="$port" '$4 ~ (":" port "$")'
    else
      netstat -lnup 2>/dev/null | awk -v port="$port" '$4 ~ (":" port "$")'
    fi
  fi
}

port_listener_is_loopback_only() {
  local output="$1" port="$2" line="" local_address="" found=0
  while IFS= read -r line; do
    [[ -n "$line" ]] || continue
    # ss/netstat place the local address in column 4 when one protocol is queried.
    local_address="$(awk '{print $4}' <<< "$line")"
    [[ -n "$local_address" ]] || continue
    found=1
    case "$local_address" in
      127.*:"$port"|"[::1]:${port}"|"::1:${port}"|localhost:"$port") ;;
      *) return 1 ;;
    esac
  done <<< "$output"
  (( found == 1 ))
}

firewall_allow_port_safe() {
  local port="$1" protocol="${2:-tcp}" backend="" backend_rc=0 zone="-" netfilter_risk="none"
  is_uint_in_range "$port" 1 65535 || return 1
  [[ "$protocol" == "tcp" || "$protocol" == "udp" ]] || return 1
  port=$((10#$port))

  # Script-managed hosting services were removed; do not rewrite unrelated rules.
  # Legacy UFW/firewalld writes remain blocked unless explicitly opted in.
  if [[ "${DMITBOX_ALLOW_UNGUARDED_FIREWALL:-0}" != 1 ]]; then
    warn "旧防火墙修改尚无全量事务回滚；本地版本默认拒绝写入"
    return 2
  fi

  backend="$(common_firewall_active_backend)" || backend_rc=$?
  if (( backend_rc == 2 )); then
    warn "UFW 与 firewalld 同时启用，已拒绝修改"
    return 2
  fi
  if (( backend_rc != 0 )); then
    netfilter_risk="$(common_firewall_netfilter_risk)"
    case "$netfilter_risk" in
      input|unknown)
        warn "已有规则可能影响本机入站；脚本不会向未知链盲目插入规则"
        common_firewall_netfilter_show_inventory
        return 2
        ;;
      forwarding)
        info "只检测到转发/NAT/输出规则，不会拦截本机监听端口"
        info "未发现启用中的本机防火墙；无需新增本机放行规则"
        return 0
        ;;
      *)
        info "未发现启用中的本机防火墙；无需新增本机放行规则"
        return 0
        ;;
    esac
  fi
  if [[ "$backend" == "firewalld" ]]; then
    zone="$(common_firewall_firewalld_zone)" || { warn "无法确定 firewalld 入站区域"; return 1; }
  fi
  common_firewall_apply_rule "$backend" allow-port "$protocol" "$port" - "$zone" || {
    warn "${backend} 放行失败"
    return 1
  }
  ok "${backend} 已放行 ${port}/${protocol}"
}

port_access_repair() {
  local port="" protocol="tcp" output=""
  menu_header "端口访问诊断" "只读监听检查；旧防火墙变更默认禁用"
  read_tty port "输入无法访问的端口（1-65535）> " ""
  is_uint_in_range "$port" 1 65535 || { warn "端口必须在 1-65535 之间"; return 1; }
  port=$((10#$port))
  read_tty protocol "协议 tcp/udp（默认 tcp）> " "tcp"
  protocol="${protocol,,}"
  [[ "$protocol" == "tcp" || "$protocol" == "udp" ]] || { warn "协议只能是 tcp 或 udp"; return 1; }

  info "不会自动清理历史防火墙链或 DMITBox 旧站点规则。"

  output="$(port_listener_for_protocol "$port" "$protocol")"
  menu_section "监听检查"
  if [[ -z "$output" ]]; then
    warn "没有程序监听 ${port}/${protocol}；这不是防火墙放行能修复的问题"
    info "请先启动对应服务，并确认它配置的端口与协议正确"
    return 1
  fi
  echo "$output"
  if port_listener_is_loopback_only "$output" "$port"; then
    warn "服务只监听本机回环地址，外部无法访问"
    info "请将该服务的监听地址改为 0.0.0.0、:: 或服务器公网地址，然后重启服务"
    return 1
  fi
  ok "服务正在非回环地址监听 ${port}/${protocol}"

  menu_section "防火墙"
  warn "下一步只新增 ${port}/${protocol} 的精确放行，不会清空其他规则"
  if [[ "${DMITBOX_ALLOW_UNGUARDED_FIREWALL:-0}" != 1 ]]; then
    warn "缺少超时回滚的旧防火墙写入被锁定：请使用服务商控制台或手工核对 nftables"
    return 2
  fi
  confirm_word "OPEN" "确认放行请输入 OPEN > " || { info "已取消，未修改规则"; return 0; }
  firewall_allow_port_safe "$port" "$protocol" || return $?
  info "如果外网仍无法访问，请在 VPS 服务商控制台的安全组/云防火墙中放行 ${port}/${protocol}"
  info "还应检查服务自身的访问控制、容器端口映射和上游网络策略"
}

process_inspect_and_stop() {
  local pid="" cmdline="" answer=""
  read_tty pid "输入 PID > " ""
  is_uint_in_range "$pid" 2 4194304 || { warn "PID 无效（不允许操作 PID 1）"; return 1; }
  pid=$((10#$pid))
  [[ "$pid" -ne "$$" && "$pid" -ne "$PPID" ]] || { warn "不能操作当前工具箱进程"; return 1; }
  kill -0 "$pid" >/dev/null 2>&1 || { warn "PID ${pid} 不存在或无权访问"; return 1; }

  menu_header "进程 ${pid}" "查看详情；可选择发送安全终止信号 SIGTERM"
  ps -p "$pid" -o pid,ppid,user,group,stat,%cpu,%mem,rss,vsz,lstart,etime,comm 2>/dev/null || true
  cmdline="$(tr '\0' ' ' < "/proc/${pid}/cmdline" 2>/dev/null || true)"
  print_kv "命令行" "${cmdline:-N/A}"
  print_kv "可执行文件" "$(readlink -f "/proc/${pid}/exe" 2>/dev/null || echo N/A)"
  print_kv "工作目录" "$(readlink -f "/proc/${pid}/cwd" 2>/dev/null || echo N/A)"
  echo
  read_tty answer "如需停止该进程请输入 TERM，直接回车取消 > " ""
  [[ "$answer" == "TERM" ]] || { info "已取消"; return 0; }
  if kill -TERM "$pid" 2>/dev/null; then
    sleep 1
    if kill -0 "$pid" >/dev/null 2>&1; then
      warn "已发送 SIGTERM，但进程仍在运行；未自动发送 SIGKILL"
    else
      ok "进程 ${pid} 已停止"
    fi
  else
    warn "无法向 PID ${pid} 发送 SIGTERM"
    return 1
  fi
}

failed_services_status() {
  menu_header "系统服务健康" "失败服务 · 重启提示 · 时间同步"
  info "正在检查系统服务、时间同步与重启状态，请稍候……"
  if is_systemd; then
    # 兼容旧版本：服务文件已删除后，systemd 仍可能缓存 failed 状态。
    # 仅在本脚本管理的 IPv6 地址池单元已确认为 not-found 且仍为 failed 时清理。
    local ipv6_pool_load_state=""
    ipv6_pool_load_state="$(command_with_timeout 5 systemctl show \
      --property=LoadState --value dmit-ipv6-pool.service 2>/dev/null || true)"
    if [[ "$ipv6_pool_load_state" == "not-found" ]] && \
       command_with_timeout 5 systemctl is-failed --quiet dmit-ipv6-pool.service \
         >/dev/null 2>&1; then
      if command_with_timeout 5 systemctl reset-failed dmit-ipv6-pool.service \
           >/dev/null 2>&1; then
        info "已自动清理旧版遗留的 IPv6 地址池服务失败记录"
      fi
    fi

    menu_section "失败的 systemd 服务"
    local service_rc=0
    command_with_timeout 10 systemctl --failed --no-pager --plain 2>/dev/null || service_rc=$?
    if (( service_rc == 124 )); then
      warn "读取 systemd 失败服务超时，已跳过；工具箱不会继续卡住"
    elif (( service_rc != 0 )); then
      warn "暂时无法读取 systemd 失败服务"
    fi
  else
    warn "当前系统不是 systemd，无法统一读取失败服务"
  fi

  menu_section "时间同步"
  info "正在确认时间同步状态……"
  time_sync_show_status || true

  menu_section "是否需要重启"
  if [[ -f /var/run/reboot-required ]]; then
    warn "系统提示需要重启"
    sed -n '1,20p' /var/run/reboot-required.pkgs 2>/dev/null || true
  elif have_cmd needs-restarting; then
    local rc=0
    command_with_timeout 15 needs-restarting -r >/dev/null 2>&1 || rc=$?
    if [[ "$rc" -eq 0 ]]; then
      ok "当前无需重启"
    elif [[ "$rc" -eq 124 ]]; then
      warn "重启状态检查超时，已跳过"
    else
      warn "系统可能需要重启"
    fi
  else
    info "未发现 reboot-required 标记"
  fi
  ok "系统健康检查完成"
}

system_status_menu() {
  while true; do
    menu_header "系统状态与端口" "资源总览 · 进程排行 · 端口与服务诊断"
    menu_section "系统状态"
    menu_item "1" "系统总览" "CPU、内存、磁盘、inode、网络与流量"
    menu_item "2" "服务健康" "失败服务、时间同步与重启提示"
    menu_section "进程"
    menu_item "3" "CPU 占用排行" "显示前 15 个进程"
    menu_item "4" "内存占用排行" "显示前 15 个进程"
    menu_item "5" "查看或停止进程" "显示 PID 详情，可发送 SIGTERM"
    menu_section "端口"
    menu_item "6" "查看全部监听端口" "TCP / UDP、PID 与程序"
    menu_item "7" "按端口诊断" "查询监听程序和相关连接"
    menu_item "8" "端口访问修复" "清理旧版规则并安全放行指定端口"
    menu_back_item

    local c=""
    read_tty c "请输入编号 > " ""
    case "$c" in
      1) system_overview || true; pause_up ;;
      2) failed_services_status || true; pause_up ;;
      3) processes_top_cpu || true; pause_up ;;
      4) processes_top_memory || true; pause_up ;;
      5) process_inspect_and_stop || true; pause_up ;;
      6) listening_ports || true; pause_up ;;
      7) port_lookup || true; pause_up ;;
      8) port_access_repair || true; pause_up ;;
      0|q|Q) return 0 ;;
      *) warn "无效选项"; pause_up ;;
    esac
  done
}

# ======================================================================
# Swap
# ======================================================================
swap_status() {
  menu_header "Swap 状态" "当前内存、Swap 设备、容量与优先级"
  if have_cmd free; then free -h; fi
  echo
  if have_cmd swapon; then
    swapon --show 2>/dev/null || true
  else
    cat /proc/swaps 2>/dev/null || true
  fi
  echo
  print_kv "swappiness" "$(sysctl -n vm.swappiness 2>/dev/null || echo N/A)"
  print_kv "脚本 Swap 文件" "$( [[ -f "$SWAP_CONF" ]] && echo 已管理 || echo 未管理 )"
}

fstab_swap_backup_once() {
  [[ -s "$FSTAB_SWAP_BACKUP" ]] && return 0
  [[ -f /etc/fstab ]] || { warn "未找到 /etc/fstab"; return 1; }
  ensure_dir "$BACKUP_BASE"
  cp -a /etc/fstab "$FSTAB_SWAP_BACKUP" || { warn "备份 /etc/fstab 失败"; return 1; }
  ok "已备份 /etc/fstab：$FSTAB_SWAP_BACKUP"
}

fstab_remove_managed_swap() {
  [[ -f /etc/fstab ]] || return 0
  local tmp="/etc/.fstab.dmitbox.$$"
  awk -v path="$SWAP_FILE" '!(($1 == path) && ($3 == "swap")) {print}' /etc/fstab > "$tmp" || {
    rm -f "$tmp" >/dev/null 2>&1 || true
    return 1
  }
  chmod 644 "$tmp" >/dev/null 2>&1 || true
  if ! mv -f "$tmp" /etc/fstab; then
    rm -f "$tmp" >/dev/null 2>&1 || true
    return 1
  fi
}

fstab_set_managed_swap() {
  [[ -f /etc/fstab ]] || return 1
  local tmp="/etc/.fstab.dmitbox.$$"
  if ! awk -v path="$SWAP_FILE" '!(($1 == path) && ($3 == "swap")) {print}' /etc/fstab > "$tmp"; then
    rm -f "$tmp" >/dev/null 2>&1 || true
    return 1
  fi
  if ! printf '%s none swap sw 0 0\n' "$SWAP_FILE" >> "$tmp"; then
    rm -f "$tmp" >/dev/null 2>&1 || true
    return 1
  fi
  chmod 644 "$tmp" >/dev/null 2>&1 || true
  if ! mv -f "$tmp" /etc/fstab; then
    rm -f "$tmp" >/dev/null 2>&1 || true
    return 1
  fi
}

swapfile_is_active() {
  awk -v path="$SWAP_FILE" 'NR > 1 && $1 == path {found=1} END{exit !found}' /proc/swaps 2>/dev/null
}

swapfile_rollback_new() {
  local old_file="$1"
  swapoff "$SWAP_FILE" >/dev/null 2>&1 || true
  rm -f "$SWAP_FILE" >/dev/null 2>&1 || true
  if [[ -e "$old_file" ]]; then
    mv "$old_file" "$SWAP_FILE" || true
    swapon "$SWAP_FILE" >/dev/null 2>&1 || true
  fi
}

swapfile_restore_conf_backup() {
  local conf_backup="$1"
  if [[ -n "$conf_backup" && -e "$conf_backup" ]]; then
    mv -f "$conf_backup" "$SWAP_CONF" || {
      warn "原 Swap 管理标记恢复失败：${conf_backup}"
      return 1
    }
  else
    rm -f "$SWAP_CONF" >/dev/null 2>&1 || true
  fi
}

swapfile_create() {
  local size_mb="" default_mb="1024" action="CREATE" verb="创建" tmp_file="${SWAP_FILE}.dmitbox.new"
  local old_file="${SWAP_FILE}.dmitbox.old.$$" conf_backup="" free_mb="0" fs_type=""

  if [[ -e "$SWAP_FILE" && ! -f "$SWAP_CONF" ]]; then
    warn "${SWAP_FILE} 已存在但不是本脚本创建的文件，为避免覆盖已取消"
    return 1
  fi
  if [[ -f "$SWAP_CONF" ]]; then
    action="RESIZE"
    verb="调整"
  fi

  read_tty size_mb "输入 Swap 大小（MiB，默认 ${default_mb}）> " "$default_mb"
  is_uint_in_range "$size_mb" 256 65536 || { warn "Swap 大小必须在 256-65536 MiB 之间"; return 1; }
  size_mb=$((10#$size_mb))
  free_mb="$(df -Pm / 2>/dev/null | awk 'NR==2{print $4}' || echo 0)"
  is_uint_in_range "$free_mb" 0 9999999999 || free_mb="0"
  if (( free_mb < size_mb + 128 )); then
    warn "可用空间不足：需要至少 $((size_mb + 128)) MiB，当前约 ${free_mb} MiB"
    return 1
  fi

  warn "将${verb} ${SWAP_FILE} 为 ${size_mb} MiB"
  confirm_word "$action" "确认请输入 ${action} > " || { warn "已取消"; return 0; }

  pkg_install util-linux
  if ! have_cmd mkswap || ! have_cmd swapon || ! have_cmd swapoff; then
    warn "缺少 mkswap/swapon/swapoff，无法继续"
    return 1
  fi
  fstab_swap_backup_once || return 1
  rm -f "$tmp_file" >/dev/null 2>&1 || true

  fs_type="$(findmnt -no FSTYPE / 2>/dev/null || true)"
  if [[ "$fs_type" == "btrfs" ]]; then
    touch "$tmp_file"
    if have_cmd chattr; then
      chattr +C "$tmp_file" >/dev/null 2>&1 || true
    fi
  fi
  if have_cmd fallocate; then
    fallocate -l "${size_mb}M" "$tmp_file" 2>/dev/null || true
  fi
  if [[ ! -s "$tmp_file" ]]; then
    info "fallocate 不可用，改用 dd 创建（可能需要一些时间）"
    dd if=/dev/zero of="$tmp_file" bs=1M count="$size_mb" conv=fsync 2>/dev/null || {
      rm -f "$tmp_file" >/dev/null 2>&1 || true
      warn "Swap 文件创建失败"
      return 1
    }
  fi
  chmod 600 "$tmp_file"
  mkswap "$tmp_file" >/dev/null || { rm -f "$tmp_file"; warn "mkswap 失败"; return 1; }

  if [[ -e "$SWAP_FILE" ]]; then
    if swapfile_is_active && ! swapoff "$SWAP_FILE"; then
      rm -f "$tmp_file" >/dev/null 2>&1 || true
      warn "无法停用原 Swap，已取消调整"
      return 1
    fi
    mv "$SWAP_FILE" "$old_file" || { rm -f "$tmp_file"; return 1; }
  fi
  mv "$tmp_file" "$SWAP_FILE" || {
    if [[ -e "$old_file" ]]; then
      mv "$old_file" "$SWAP_FILE" || true
      swapon "$SWAP_FILE" >/dev/null 2>&1 || true
    fi
    return 1
  }

  if ! swapon "$SWAP_FILE"; then
    warn "新 Swap 启用失败，正在恢复原文件"
    rm -f "$SWAP_FILE" >/dev/null 2>&1 || true
    if [[ -e "$old_file" ]]; then
      mv "$old_file" "$SWAP_FILE"
      swapon "$SWAP_FILE" >/dev/null 2>&1 || true
    fi
    return 1
  fi
  if [[ -f "$SWAP_CONF" ]]; then
    conf_backup="${SWAP_CONF}.dmitbox.old.$$"
    if ! cp -a "$SWAP_CONF" "$conf_backup"; then
      warn "无法备份原 Swap 管理标记，正在恢复原文件"
      swapfile_rollback_new "$old_file"
      return 1
    fi
  fi
  if ! write_file "$SWAP_CONF" "PATH=${SWAP_FILE}
SIZE_MIB=${size_mb}
CREATED=$(date -Is)"; then
    warn "Swap 管理标记写入失败，正在恢复原文件"
    swapfile_rollback_new "$old_file"
    swapfile_restore_conf_backup "$conf_backup" || true
    return 1
  fi
  if ! fstab_set_managed_swap; then
    warn "更新 /etc/fstab 失败，正在恢复原 Swap"
    swapfile_rollback_new "$old_file"
    swapfile_restore_conf_backup "$conf_backup" || true
    return 1
  fi
  rm -f "$old_file" >/dev/null 2>&1 || true
  if [[ -n "$conf_backup" ]]; then
    rm -f "$conf_backup" >/dev/null 2>&1 || true
  fi
  chmod 600 "$SWAP_CONF" >/dev/null 2>&1 || true
  ok "Swap 已启用并持久化：${SWAP_FILE} (${size_mb} MiB)"
  swapon --show 2>/dev/null || true
}

swapfile_remove() {
  [[ -f "$SWAP_CONF" ]] || { warn "脚本没有管理 ${SWAP_FILE}，拒绝删除未知 Swap 文件"; return 1; }
  [[ -e "$SWAP_FILE" ]] || { warn "${SWAP_FILE} 不存在，仅清理配置"; }
  warn "将停用并删除脚本创建的 ${SWAP_FILE}"
  confirm_word "REMOVE" "确认请输入 REMOVE > " || { warn "已取消"; return 0; }
  fstab_swap_backup_once || return 1
  if swapfile_is_active && ! swapoff "$SWAP_FILE"; then
    warn "Swap 正在使用且无法停用，未删除文件"
    return 1
  fi
  fstab_remove_managed_swap || { warn "无法更新 /etc/fstab，未删除文件"; return 1; }
  if ! rm -f "$SWAP_FILE"; then
    warn "Swap 已停用，但文件删除失败：${SWAP_FILE}"
    return 1
  fi
  rm -f "$SWAP_CONF" >/dev/null 2>&1 || true
  ok "脚本创建的 Swap 已删除"
}

swappiness_set() {
  local value=""
  read_tty value "输入 vm.swappiness（0-100，默认 10）> " "10"
  is_uint_in_range "$value" 0 100 || { warn "swappiness 必须在 0-100 之间"; return 1; }
  value=$((10#$value))
  write_file "$SWAPPINESS_FILE" "# managed by ${SCRIPT_NAME}
vm.swappiness=${value}"
  if sysctl -w "vm.swappiness=${value}" >/dev/null 2>&1; then
    ok "swappiness 已设置为 ${value}"
  else
    warn "运行时设置失败，但已写入持久化配置"
    return 1
  fi
}

swap_menu() {
  while true; do
    menu_header "Swap 管理" "Swap 文件 · 容量调整 · swappiness"
    menu_section "状态"
    menu_item "1" "查看 Swap 状态" "设备、容量、使用量和优先级"
    menu_section "管理"
    menu_item "2" "创建或调整 Swap" "仅管理 ${SWAP_FILE}，支持 256 MiB-64 GiB"
    menu_item "3" "删除脚本 Swap" "仅删除脚本创建的 ${SWAP_FILE}"
    menu_item "4" "设置 swappiness" "范围 0-100，默认建议 10"
    menu_back_item

    local c=""
    read_tty c "请输入编号 > " ""
    case "$c" in
      1) swap_status || true; pause_up ;;
      2) swapfile_create || true; pause_up ;;
      3) swapfile_remove || true; pause_up ;;
      4) swappiness_set || true; pause_up ;;
      0|q|Q) return 0 ;;
      *) warn "无效选项"; pause_up ;;
    esac
  done
}

# ======================================================================
# 安全清理 / 系统更新
# ======================================================================
cleanup_preview() {
  menu_header "安全清理预览" "只统计可安全清理的缓存与日志，不删除文件"
  menu_section "根分区"
  df -h / 2>/dev/null || true
  menu_section "软件包缓存"
  [[ -d /var/cache/apt/archives ]] && print_kv "APT 缓存" "$(du -sh /var/cache/apt/archives 2>/dev/null | awk '{print $1}' || echo N/A)"
  [[ -d /var/cache/dnf ]] && print_kv "DNF 缓存" "$(du -sh /var/cache/dnf 2>/dev/null | awk '{print $1}' || echo N/A)"
  [[ -d /var/cache/yum ]] && print_kv "YUM 缓存" "$(du -sh /var/cache/yum 2>/dev/null | awk '{print $1}' || echo N/A)"
  [[ -d /var/cache/apk ]] && print_kv "APK 缓存" "$(du -sh /var/cache/apk 2>/dev/null | awk '{print $1}' || echo N/A)"
  menu_section "systemd journal"
  if have_cmd journalctl; then
    journalctl --disk-usage 2>/dev/null || true
    echo -e "${c_dim}执行清理时保留最近 7 天，并限制归档日志约 200 MiB。${c_reset}"
  else
    info "当前系统没有 journalctl"
  fi
  menu_section "临时文件"
  if have_cmd systemd-tmpfiles; then
    info "将调用 systemd-tmpfiles --clean，仅按系统既定保留策略清理"
  else
    info "没有 systemd-tmpfiles，不会直接删除 /tmp 内容"
  fi
  echo
  info "不会执行 autoremove、删除当前内核、清理数据库或 Docker 数据"
}

cleanup_execute() {
  cleanup_preview
  echo
  confirm_word "CLEAN" "确认清理请输入 CLEAN > " || { warn "已取消"; return 0; }

  local before="0" after="0" freed_kb="0"
  before="$(df -Pk / 2>/dev/null | awk 'NR==2{print $4}' || echo 0)"
  if have_cmd apt-get; then
    info "清理 APT 下载缓存"
    apt-get clean >/dev/null 2>&1 || warn "APT 缓存清理失败"
  elif have_cmd dnf; then
    info "清理 DNF 缓存"
    dnf clean all >/dev/null 2>&1 || warn "DNF 缓存清理失败"
  elif have_cmd yum; then
    info "清理 YUM 缓存"
    yum clean all >/dev/null 2>&1 || warn "YUM 缓存清理失败"
  elif have_cmd apk; then
    info "清理 APK 缓存"
    apk cache clean >/dev/null 2>&1 || warn "APK 缓存清理失败"
  fi

  if have_cmd journalctl; then
    info "压缩并清理旧 journal（保留 7 天 / 约 200 MiB）"
    journalctl --rotate >/dev/null 2>&1 || true
    journalctl --vacuum-time=7d >/dev/null 2>&1 || warn "journal 时间清理失败"
    journalctl --vacuum-size=200M >/dev/null 2>&1 || warn "journal 容量清理失败"
  fi
  if have_cmd systemd-tmpfiles; then
    info "按系统策略清理临时文件"
    systemd-tmpfiles --clean >/dev/null 2>&1 || warn "临时文件清理未完全成功"
  fi

  after="$(df -Pk / 2>/dev/null | awk 'NR==2{print $4}' || echo 0)"
  if [[ "$before" =~ ^[0-9]+$ && "$after" =~ ^[0-9]+$ && "$after" -ge "$before" ]]; then
    freed_kb=$((after - before))
    ok "安全清理完成，根分区约释放 $(human_bytes "$((freed_kb * 1024))")"
  else
    ok "安全清理完成"
  fi
}

cleanup_menu() {
  while true; do
    menu_header "磁盘与日志清理" "清理前预览 · 限定目标 · 不做 autoremove"
    menu_item "1" "查看清理预览" "统计包缓存、journal 与根分区"
    menu_item "2" "执行安全清理" "需要输入 CLEAN 确认"
    menu_back_item
    local c=""
    read_tty c "请输入编号 > " ""
    case "$c" in
      1) cleanup_preview || true; pause_up ;;
      2) cleanup_execute || true; pause_up ;;
      0|q|Q) return 0 ;;
      *) warn "无效选项"; pause_up ;;
    esac
  done
}

system_updates_check() {
  menu_header "检查系统更新" "刷新软件源并列出可升级软件包，不安装更新"
  local rc=0
  if have_cmd apt-get; then
    run_with_spinner "刷新 APT 软件源" apt-get -o DPkg::Lock::Timeout=30 -y update || return 1
    if have_cmd apt; then
      apt list --upgradable 2>/dev/null | sed -n '1,120p' || true
    else
      apt-get -s upgrade 2>/dev/null | awk '/^Inst /{print}' | sed -n '1,120p' || true
    fi
  elif have_cmd dnf; then
    dnf -q check-update || rc=$?
    [[ "$rc" -eq 0 || "$rc" -eq 100 ]] || { warn "DNF 检查更新失败（rc=${rc}）"; return 1; }
  elif have_cmd yum; then
    yum -q check-update || rc=$?
    [[ "$rc" -eq 0 || "$rc" -eq 100 ]] || { warn "YUM 检查更新失败（rc=${rc}）"; return 1; }
  elif have_cmd apk; then
    apk update || return 1
    apk version -l '<' 2>/dev/null | sed -n '1,120p' || true
  else
    warn "未识别包管理器"
    return 1
  fi
  ok "更新检查完成"
}

system_updates_apply() {
  warn "将安装当前发行版提供的常规更新；不会执行跨版本升级"
  confirm_word "UPDATE" "确认更新请输入 UPDATE > " || { warn "已取消"; return 0; }

  export DEBIAN_FRONTEND=noninteractive
  export NEEDRESTART_MODE=a
  if have_cmd apt-get; then
    run_with_spinner "刷新 APT 软件源" apt-get -o DPkg::Lock::Timeout=30 -y update || return 1
    run_with_spinner "安装系统更新" apt-get -o DPkg::Lock::Timeout=30 -y \
      -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold upgrade || return 1
  elif have_cmd dnf; then
    run_with_spinner "安装系统更新" dnf -y upgrade || return 1
  elif have_cmd yum; then
    run_with_spinner "安装系统更新" yum -y update || return 1
  elif have_cmd apk; then
    run_with_spinner "刷新 APK 软件源" apk update || return 1
    run_with_spinner "安装系统更新" apk upgrade || return 1
  else
    warn "未识别包管理器"
    return 1
  fi
  ok "系统更新完成"
  [[ -f /var/run/reboot-required ]] && warn "部分更新需要重启后生效"
}

time_sync_systemd_unit_exists() {
  local unit="${1%.service}.service" output=""
  is_systemd || return 1
  command_with_timeout 6 systemctl cat "$unit" >/dev/null 2>&1 && return 0
  output="$(command_with_timeout 6 systemctl list-unit-files "$unit" --no-legend 2>/dev/null | sed -n '1p' || true)"
  [[ "$output" == "$unit "* || "$output" == "$unit"$'\t'* ]]
}

time_sync_service_exists() {
  local name="${1%.service}"
  if is_systemd; then
    time_sync_systemd_unit_exists "$name"
    return $?
  fi
  [[ -x "/etc/init.d/${name}" ]] && return 0
  if have_cmd rc-service; then
    rc-service -e 2>/dev/null | grep -Fxq "$name" && return 0
  fi
  return 1
}

time_sync_service_active() {
  local name="${1%.service}"
  if is_systemd; then
    command_with_timeout 6 systemctl is-active --quiet "${name}.service" >/dev/null 2>&1 && return 0
  elif have_cmd rc-service; then
    command_with_timeout 6 rc-service "$name" status >/dev/null 2>&1 && return 0
  elif have_cmd service; then
    command_with_timeout 6 service "$name" status >/dev/null 2>&1 && return 0
  fi
  case "$name" in
    chrony|chronyd) have_cmd pgrep && pgrep -x chronyd >/dev/null 2>&1 ;;
    ntp|ntpd|ntpsec|openntpd) have_cmd pgrep && pgrep -x ntpd >/dev/null 2>&1 ;;
    systemd-timesyncd) have_cmd pgrep && pgrep -f '(^|/)systemd-timesyncd([[:space:]]|$)' >/dev/null 2>&1 ;;
    *) return 1 ;;
  esac
}

time_sync_chrony_service_name() {
  local name=""
  for name in chrony chronyd; do
    time_sync_service_exists "$name" && { printf '%s\n' "$name"; return 0; }
  done
  if have_cmd apk; then printf 'chronyd\n'; else printf 'chrony\n'; fi
}

time_sync_ntpd_service_name() {
  local name=""
  for name in ntp ntpd ntpsec openntpd; do
    time_sync_service_exists "$name" && { printf '%s\n' "$name"; return 0; }
  done
  if have_cmd ntpd; then
    if have_cmd apk || have_cmd dnf || have_cmd yum; then printf 'ntpd\n'; else printf 'ntp\n'; fi
    return 0
  fi
  return 1
}

time_sync_chrony_installed() {
  have_cmd chronyc || have_cmd chronyd || \
    time_sync_service_exists chrony || time_sync_service_exists chronyd
}

time_sync_ntpd_installed() {
  have_cmd ntpd || time_sync_service_exists ntp || time_sync_service_exists ntpd || \
    time_sync_service_exists ntpsec || time_sync_service_exists openntpd
}

time_sync_detect_backend() {
  local name=""
  for name in chrony chronyd; do
    if time_sync_service_exists "$name" && time_sync_service_active "$name"; then
      printf 'chrony|%s\n' "$name"
      return 0
    fi
  done
  if is_systemd && time_sync_service_active systemd-timesyncd; then
    printf 'timesyncd|systemd-timesyncd\n'
    return 0
  fi
  for name in ntp ntpd ntpsec openntpd; do
    if time_sync_service_exists "$name" && time_sync_service_active "$name"; then
      printf 'ntpd|%s\n' "$name"
      return 0
    fi
  done
  if have_cmd pgrep && pgrep -x chronyd >/dev/null 2>&1; then
    printf 'chrony|%s\n' "$(time_sync_chrony_service_name)"
    return 0
  fi
  if have_cmd pgrep && pgrep -x ntpd >/dev/null 2>&1; then
    printf 'ntpd|%s\n' "$(time_sync_ntpd_service_name 2>/dev/null || echo ntpd)"
    return 0
  fi
  if time_sync_chrony_installed; then
    printf 'chrony|%s\n' "$(time_sync_chrony_service_name)"
    return 0
  fi
  if is_systemd && time_sync_systemd_unit_exists systemd-timesyncd; then
    printf 'timesyncd|systemd-timesyncd\n'
    return 0
  fi
  if time_sync_ntpd_installed; then
    printf 'ntpd|%s\n' "$(time_sync_ntpd_service_name)"
    return 0
  fi
  printf 'none|none\n'
  return 1
}

time_sync_backend_label() {
  case "${1%%|*}" in
    timesyncd) printf 'systemd-timesyncd\n' ;;
    chrony) printf 'chrony\n' ;;
    ntpd) printf 'ntpd\n' ;;
    *) printf '未配置\n' ;;
  esac
}

time_sync_backend_active() {
  local backend="${1%%|*}" service_name="${1#*|}"
  [[ "$backend" != "none" && -n "$service_name" && "$service_name" != "none" ]] || return 1
  time_sync_service_active "$service_name"
}

time_sync_is_synchronized() {
  local value="" tracking=""
  if have_cmd timedatectl; then
    value="$(command_with_timeout 6 timedatectl show -p NTPSynchronized --value 2>/dev/null || true)"
    [[ "${value,,}" == "yes" ]] && return 0
  fi
  if have_cmd chronyc; then
    tracking="$(LC_ALL=C command_with_timeout 6 chronyc tracking 2>/dev/null || true)"
    grep -Eq '^Leap status[[:space:]]*:[[:space:]]*Normal([[:space:]]|$)' <<< "$tracking" && return 0
  fi
  if have_cmd ntpq; then
    LC_ALL=C command_with_timeout 6 ntpq -pn 2>/dev/null | awk '$1 ~ /^\*/ {found=1} END {exit !found}' && return 0
  fi
  return 1
}

time_sync_status_text() {
  local backend_spec="" label=""
  backend_spec="$(time_sync_detect_backend 2>/dev/null || true)"
  [[ -n "$backend_spec" ]] || backend_spec="none|none"
  label="$(time_sync_backend_label "$backend_spec")"
  if time_sync_is_synchronized; then
    [[ "$label" == "未配置" ]] && label="系统 NTP"
    printf '已同步（%s）\n' "$label"
  elif time_sync_backend_active "$backend_spec"; then
    printf '运行中，等待同步（%s）\n' "$label"
  elif [[ "${backend_spec%%|*}" != "none" ]]; then
    printf '未运行（%s）\n' "$label"
  else
    printf '未配置\n'
  fi
}

time_sync_show_status() {
  local backend_spec="${1:-}" synchronized_hint="${2:-}" active_hint="${3:-}"
  local label="" synchronized=0 active=0
  [[ -n "$backend_spec" ]] || backend_spec="$(time_sync_detect_backend 2>/dev/null || true)"
  [[ -n "$backend_spec" ]] || backend_spec="none|none"
  label="$(time_sync_backend_label "$backend_spec")"
  if [[ "$synchronized_hint" == "1" ]]; then
    synchronized=1
  elif [[ "$synchronized_hint" != "0" ]] && time_sync_is_synchronized; then
    synchronized=1
  fi
  if [[ "$active_hint" == "1" ]]; then
    active=1
  elif [[ "$active_hint" != "0" && "$synchronized" -eq 0 ]] && \
       time_sync_backend_active "$backend_spec"; then
    active=1
  fi
  print_kv "同步服务" "$label"
  if (( synchronized == 1 )); then
    ok "系统时间已经同步"
  elif (( active == 1 )); then
    warn "同步服务已运行，但尚未确认与上游完成同步"
    info "首次启动、网络刚恢复或上游暂不可达时，通常需要稍等片刻"
  else
    warn "没有运行中的时间同步服务"
  fi
  (( synchronized == 1 || active == 1 ))
}

time_sync_enable_service() {
  local name="${1%.service}"
  if is_systemd; then
    command_with_timeout 15 systemctl unmask "${name}.service" >/dev/null 2>&1 || true
    command_with_timeout 15 systemctl enable "${name}.service" >/dev/null 2>&1 || true
    if ! time_sync_service_active "$name"; then
      command_with_timeout 25 systemctl start "${name}.service" >/dev/null 2>&1 || return 1
    fi
  elif have_cmd rc-service; then
    have_cmd rc-update && command_with_timeout 15 rc-update add "$name" default >/dev/null 2>&1 || true
    if ! time_sync_service_active "$name"; then
      command_with_timeout 25 rc-service "$name" start >/dev/null 2>&1 || return 1
    fi
  elif have_cmd service; then
    have_cmd update-rc.d && command_with_timeout 15 update-rc.d "$name" defaults >/dev/null 2>&1 || true
    have_cmd chkconfig && command_with_timeout 15 chkconfig "$name" on >/dev/null 2>&1 || true
    if ! time_sync_service_active "$name"; then
      command_with_timeout 25 service "$name" start >/dev/null 2>&1 || return 1
    fi
  else
    return 1
  fi
  time_sync_service_active "$name"
}

time_sync_start_backend() {
  local backend_spec="$1" backend="${1%%|*}" service_name="${1#*|}"
  case "$backend" in
    timesyncd)
      time_sync_enable_service "$service_name" || return 1
      if have_cmd timedatectl && ! command_with_timeout 10 timedatectl set-ntp true >/dev/null 2>&1; then
        info "timedatectl 接口不可用，但 systemd-timesyncd 服务已直接启用"
      fi
      ;;
    chrony)
      time_sync_enable_service "$service_name" || return 1
      have_cmd chronyc && command_with_timeout 10 chronyc online >/dev/null 2>&1 || true
      ;;
    ntpd)
      time_sync_enable_service "$service_name" || return 1
      ;;
    *) return 1 ;;
  esac
  time_sync_backend_active "$backend_spec"
}

time_sync_install_chrony() {
  info "安装 chrony 时间同步服务"
  pkg_install chrony
  time_sync_chrony_installed || { warn "chrony 安装后仍不可用"; return 1; }
}

time_sync_install_preferred() {
  if have_cmd apt-get && is_systemd; then
    info "安装轻量的 systemd-timesyncd"
    pkg_install systemd-timesyncd
    if time_sync_systemd_unit_exists systemd-timesyncd; then
      return 0
    fi
    warn "systemd-timesyncd 不可用，将改用 chrony"
  fi
  time_sync_install_chrony
}

time_sync_finish_enable() {
  local backend_spec="$1" label="" synchronized=0
  label="$(time_sync_backend_label "$backend_spec")"
  time_sync_backend_active "$backend_spec" || { warn "${label} 未能保持运行状态"; return 1; }
  if time_sync_is_synchronized; then
    synchronized=1
    ok "自动时间同步已开启并确认同步：${label}"
  else
    ok "自动时间同步服务已开启：${label}"
    info "当前尚未确认同步完成；请等待约 1-2 分钟后再次查看状态"
  fi
  time_sync_show_status "$backend_spec" "$synchronized" 1 || true
}

time_sync_enable() {
  local backend_spec="" backend="" service_name="" fallback_spec=""
  info "正在检查时间同步服务与当前同步状态，请稍候……"
  backend_spec="$(time_sync_detect_backend 2>/dev/null || true)"
  [[ -n "$backend_spec" ]] || backend_spec="none|none"

  if time_sync_is_synchronized && time_sync_backend_active "$backend_spec"; then
    ok "自动时间同步已经正常运行，无需重复设置"
    time_sync_show_status "$backend_spec" 1 1 || true
    return 0
  fi

  if [[ "${backend_spec%%|*}" == "none" ]]; then
    time_sync_install_preferred || return 1
    backend_spec="$(time_sync_detect_backend 2>/dev/null || true)"
    [[ -n "$backend_spec" && "${backend_spec%%|*}" != "none" ]] || {
      warn "安装后仍未找到可用的时间同步服务"
      return 1
    }
  fi

  backend="${backend_spec%%|*}"
  service_name="${backend_spec#*|}"
  info "正在启用：$(time_sync_backend_label "$backend_spec")（${service_name}）"
  if time_sync_start_backend "$backend_spec"; then
    time_sync_finish_enable "$backend_spec"
    return $?
  fi
  warn "首选时间同步服务启动失败：$(time_sync_backend_label "$backend_spec")"

  if [[ "$backend" != "chrony" ]] && time_sync_install_chrony; then
    service_name="$(time_sync_chrony_service_name)"
    fallback_spec="chrony|${service_name}"
    info "正在尝试备用服务：chrony（${service_name}）"
    if time_sync_start_backend "$fallback_spec"; then
      time_sync_finish_enable "$fallback_spec"
      return $?
    fi
  fi

  if [[ "$backend" != "timesyncd" ]] && is_systemd && \
     time_sync_systemd_unit_exists systemd-timesyncd; then
    fallback_spec="timesyncd|systemd-timesyncd"
    info "正在尝试备用服务：systemd-timesyncd"
    if time_sync_start_backend "$fallback_spec"; then
      time_sync_finish_enable "$fallback_spec"
      return $?
    fi
  fi

  warn "所有可用的时间同步后端均启动失败"
  info "请查看：systemctl status systemd-timesyncd chrony chronyd --no-pager"
  info "非 systemd 系统请查看：rc-service chronyd status"
  return 1
}

system_update_menu() {
  while true; do
    menu_header "系统更新与健康" "软件更新 · 失败服务 · 重启提示 · 时间同步"
    menu_section "软件更新"
    menu_item "1" "检查可用更新" "刷新软件源，仅列出更新"
    menu_item "2" "安装常规更新" "不执行发行版跨版本升级"
    menu_section "系统健康"
    menu_item "3" "服务与重启状态" "失败服务、时间同步、重启提示"
    menu_item "4" "检查并开启时间同步" "自动选择 timesyncd、chrony 或现有 ntpd"
    menu_back_item
    local c=""
    read_tty c "请输入编号 > " ""
    case "$c" in
      1) system_updates_check || true; pause_up ;;
      2) system_updates_apply || true; pause_up ;;
      3) failed_services_status || true; pause_up ;;
      4) time_sync_enable || true; pause_up ;;
      0|q|Q) return 0 ;;
      *) warn "无效选项"; pause_up ;;
    esac
  done
}

# ======================================================================
# Fail2Ban SSH 防爆破
# ======================================================================
valid_ip_or_cidr() {
  local value="${1:-}" address="" prefix="" part="" right="" colon_chars=""
  [[ "$value" =~ ^[0-9A-Fa-f:.]+(/[0-9]{1,3})?$ ]] || return 1

  if have_cmd python3; then
    if python3 - "$value" >/dev/null 2>&1 <<'PY'
import ipaddress
import sys

value = sys.argv[1]
try:
    ipaddress.ip_network(value, strict=False) if "/" in value else ipaddress.ip_address(value)
except ValueError:
    raise SystemExit(1)
PY
    then
      return 0
    fi
    return 1
  fi

  address="${value%%/*}"
  if [[ "$value" == */* ]]; then
    prefix="${value##*/}"
  fi
  if [[ "$address" == *.* && "$address" != *:* ]]; then
    local -a octets=()
    local IFS='.'
    read -r -a octets <<< "$address"
    [[ "${#octets[@]}" -eq 4 ]] || return 1
    for part in "${octets[@]}"; do
      [[ "$part" =~ ^[0-9]{1,3}$ ]] || return 1
      (( 10#$part <= 255 )) || return 1
    done
    [[ -z "$prefix" ]] || is_uint_in_range "$prefix" 0 32
    return $?
  fi

  [[ "$address" == *:* && "$address" != *.* && "$address" =~ ^[0-9A-Fa-f:]+$ ]] || return 1
  [[ "$address" != *:::* ]] || return 1
  if [[ "$address" == *::* ]]; then
    right="${address#*::}"
    [[ "$right" != *::* ]] || return 1
  fi
  colon_chars="${address//[0-9A-Fa-f]/}"
  (( ${#colon_chars} >= 2 && ${#colon_chars} <= 8 )) || return 1
  local -a hextets=()
  local IFS=':'
  read -r -a hextets <<< "$address"
  for part in "${hextets[@]}"; do
    [[ -z "$part" || ( ${#part} -le 4 && "$part" =~ ^[0-9A-Fa-f]+$ ) ]] || return 1
  done
  if [[ "$address" == *::* ]]; then
    (( ${#hextets[@]} < 8 )) || return 1
  else
    (( ${#hextets[@]} == 8 )) || return 1
  fi
  [[ -z "$prefix" ]] || is_uint_in_range "$prefix" 0 128
}

fail2ban_service_restart() {
  if is_systemd; then
    systemctl enable fail2ban >/dev/null 2>&1 || true
    systemctl restart fail2ban >/dev/null 2>&1
  elif have_cmd rc-service; then
    rc-update add fail2ban default >/dev/null 2>&1 || true
    rc-service fail2ban restart >/dev/null 2>&1
  elif have_cmd service; then
    service fail2ban restart >/dev/null 2>&1
  else
    return 1
  fi
}

fail2ban_validate() {
  have_cmd fail2ban-client || return 1
  fail2ban-client -t >/dev/null 2>&1
}

fail2ban_status() {
  menu_header "Fail2Ban 状态" "服务、sshd jail、封禁列表与最近日志"
  if ! have_cmd fail2ban-client; then
    warn "Fail2Ban 未安装"
    return 0
  fi
  if is_systemd; then
    print_kv "服务状态" "$(systemctl is-active fail2ban 2>/dev/null || echo inactive)"
  fi
  fail2ban-client ping 2>/dev/null || warn "Fail2Ban 服务未响应"
  echo
  fail2ban-client status 2>/dev/null || true
  echo
  if fail2ban-client status sshd >/dev/null 2>&1; then
    fail2ban-client status sshd 2>/dev/null || true
  else
    warn "sshd jail 未启用"
  fi
  menu_section "最近日志"
  if have_cmd journalctl && is_systemd; then
    journalctl -u fail2ban --no-pager -n 30 2>/dev/null || true
  else
    tail -n 30 /var/log/fail2ban.log 2>/dev/null || true
  fi
}

fail2ban_restore_config_on_error() {
  local had_previous="$1" rollback_file="$2"
  if [[ "$had_previous" == "1" && -e "$rollback_file" ]]; then
    cp -a "$rollback_file" "$FAIL2BAN_JAIL_FILE" >/dev/null 2>&1 || true
  else
    rm -f "$FAIL2BAN_JAIL_FILE" >/dev/null 2>&1 || true
  fi
}

fail2ban_install_configure() {
  warn "将安装 Fail2Ban，并启用 SSH 防爆破：5 次失败、10 分钟统计、封禁 1 小时"
  confirm_word "ENABLE" "确认请输入 ENABLE > " || { warn "已取消"; return 0; }
  pkg_install fail2ban
  have_cmd fail2ban-client || { warn "Fail2Ban 安装失败或软件源中无此包"; return 1; }

  ensure_dir "$BACKUP_BASE"
  ensure_dir "$(dirname "$FAIL2BAN_JAIL_FILE")"
  local had_previous="0" rollback_file=""
  local port="22" backend="auto" current_ip="" ignore="127.0.0.1/8 ::1"
  rollback_file="$(mktemp /tmp/dmitbox-fail2ban-rollback.XXXXXX)" || {
    warn "无法创建临时回滚文件"
    return 1
  }
  if [[ -f "$FAIL2BAN_JAIL_FILE" ]]; then
    had_previous="1"
    cp -a "$FAIL2BAN_JAIL_FILE" "$rollback_file"
  fi
  if [[ ! -f "$FAIL2BAN_ORIG_MARKER" ]]; then
    if [[ -f "$FAIL2BAN_JAIL_FILE" ]]; then
      write_file "$FAIL2BAN_ORIG_MARKER" "1"
      cp -a "$FAIL2BAN_JAIL_FILE" "$FAIL2BAN_BACKUP"
    else
      write_file "$FAIL2BAN_ORIG_MARKER" "0"
    fi
  fi
  port="$(ssh_current_ports 2>/dev/null | awk '{print $1}' || echo 22)"
  is_uint_in_range "$port" 1 65535 || port="22"
  current_ip="${SSH_CONNECTION:-}"
  current_ip="${current_ip%% *}"
  if valid_ip_or_cidr "$current_ip"; then
    ignore="${ignore} ${current_ip}"
    info "已自动加入当前 SSH 来源白名单：${current_ip}"
  fi

  if is_systemd && have_cmd journalctl; then
    backend="systemd"
  elif [[ -f /var/log/auth.log ]]; then
    backend="auto"
  elif [[ -f /var/log/secure ]]; then
    backend="auto"
  else
    warn "未检测到 journald、auth.log 或 secure；配置后请确认 sshd jail 能读取日志"
  fi

  write_file "$FAIL2BAN_JAIL_FILE" "# managed by ${SCRIPT_NAME}
[DEFAULT]
ignoreip = ${ignore}
bantime = 1h
findtime = 10m
maxretry = 5

[sshd]
enabled = true
port = ${port}
backend = ${backend}"

  if ! fail2ban_validate; then
    warn "Fail2Ban 配置校验失败，正在恢复原配置"
    fail2ban-client -t 2>&1 | tail -n 30 || true
    fail2ban_restore_config_on_error "$had_previous" "$rollback_file"
    rm -f "$rollback_file" >/dev/null 2>&1 || true
    return 1
  fi
  if ! fail2ban_service_restart; then
    warn "Fail2Ban 服务启动失败，正在恢复原配置"
    fail2ban_restore_config_on_error "$had_previous" "$rollback_file"
    fail2ban_service_restart >/dev/null 2>&1 || true
    rm -f "$rollback_file" >/dev/null 2>&1 || true
    return 1
  fi
  sleep 1
  if ! fail2ban-client status sshd >/dev/null 2>&1; then
    warn "Fail2Ban 已启动，但 sshd jail 未正常运行；正在恢复原配置"
    fail2ban_restore_config_on_error "$had_previous" "$rollback_file"
    fail2ban_service_restart >/dev/null 2>&1 || true
    rm -f "$rollback_file" >/dev/null 2>&1 || true
    return 1
  fi
  rm -f "$rollback_file" >/dev/null 2>&1 || true
  ok "SSH 防爆破已启用（SSH 端口：${port}，日志后端：${backend}）"
  fail2ban-client status sshd 2>/dev/null || true
}

fail2ban_add_whitelist() {
  [[ -f "$FAIL2BAN_JAIL_FILE" ]] || { warn "请先安装并配置 Fail2Ban SSH 防护"; return 1; }
  local value="" old_ignore="" new_ignore="" tmp="/tmp/dmitbox-fail2ban.$$"
  read_tty value "输入要加入白名单的 IP 或 CIDR > " ""
  valid_ip_or_cidr "$value" || { warn "IP/CIDR 格式无效"; return 1; }
  old_ignore="$(sed -n -E 's/^ignoreip[[:space:]]*=[[:space:]]*(.*)$/\1/p' "$FAIL2BAN_JAIL_FILE" | head -n1)"
  [[ -n "$old_ignore" ]] || { warn "配置中缺少 ignoreip，请先执行【安装或修复配置】"; return 1; }
  [[ " ${old_ignore} " == *" ${value} "* ]] && { info "${value} 已在白名单中"; return 0; }
  new_ignore="${old_ignore} ${value}"
  cp -a "$FAIL2BAN_JAIL_FILE" "$tmp"
  sed -i -E "s|^ignoreip[[:space:]]*=.*$|ignoreip = ${new_ignore}|" "$FAIL2BAN_JAIL_FILE"
  if ! fail2ban_validate || ! fail2ban_service_restart; then
    cp -a "$tmp" "$FAIL2BAN_JAIL_FILE"
    fail2ban_service_restart >/dev/null 2>&1 || true
    rm -f "$tmp"
    warn "白名单配置失败，已回滚"
    return 1
  fi
  rm -f "$tmp"
  ok "已加入 Fail2Ban 白名单：${value}"
}

fail2ban_unban_ip() {
  have_cmd fail2ban-client || { warn "Fail2Ban 未安装"; return 1; }
  local value=""
  read_tty value "输入要解封的 IP > " ""
  valid_ip_or_cidr "$value" || { warn "IP 格式无效"; return 1; }
  [[ "$value" != */* ]] || { warn "解封操作请输入单个 IP，不支持 CIDR"; return 1; }
  if fail2ban-client set sshd unbanip "$value" >/dev/null 2>&1; then
    ok "已请求解封：${value}"
  else
    warn "解封失败：请确认 sshd jail 正在运行且该 IP 已被封禁"
    return 1
  fi
}

fail2ban_disable_sshd() {
  [[ -f "$FAIL2BAN_JAIL_FILE" ]] || { warn "未找到脚本创建的 Fail2Ban 配置"; return 1; }
  warn "将关闭脚本配置的 sshd jail；不会卸载 Fail2Ban"
  confirm_word "DISABLE" "确认请输入 DISABLE > " || { warn "已取消"; return 0; }
  local had_original="0" rollback_file=""
  rollback_file="$(mktemp /tmp/dmitbox-fail2ban-disable.XXXXXX)" || {
    warn "无法创建临时回滚文件"
    return 1
  }
  cp -a "$FAIL2BAN_JAIL_FILE" "$rollback_file" || {
    rm -f "$rollback_file" >/dev/null 2>&1 || true
    warn "无法备份当前 Fail2Ban 配置，已取消"
    return 1
  }
  had_original="$(cat "$FAIL2BAN_ORIG_MARKER" 2>/dev/null || echo 0)"
  if [[ "$had_original" == "1" && -s "$FAIL2BAN_BACKUP" ]]; then
    cp -a "$FAIL2BAN_BACKUP" "$FAIL2BAN_JAIL_FILE"
    info "已恢复脚本接管前的同名 Fail2Ban 配置"
  else
    rm -f "$FAIL2BAN_JAIL_FILE" >/dev/null 2>&1 || true
  fi
  if have_cmd fail2ban-client; then
    if ! fail2ban_validate || ! fail2ban_service_restart; then
      warn "恢复/移除配置后 Fail2Ban 校验或重启失败，正在回滚"
      cp -a "$rollback_file" "$FAIL2BAN_JAIL_FILE" >/dev/null 2>&1 || true
      fail2ban_service_restart >/dev/null 2>&1 || true
      rm -f "$rollback_file" >/dev/null 2>&1 || true
      return 1
    fi
  else
    warn "fail2ban-client 不存在，仅完成配置恢复，无法校验服务"
  fi
  rm -f "$rollback_file" >/dev/null 2>&1 || true
  rm -f "$FAIL2BAN_ORIG_MARKER" >/dev/null 2>&1 || true
  ok "脚本配置的 SSH 防爆破已关闭"
}

fail2ban_menu() {
  while true; do
    menu_header "Fail2Ban SSH 防爆破" "自动识别 SSH 端口 · 白名单 · 封禁状态"
    menu_section "管理"
    menu_item "1" "查看状态与日志" "sshd jail、封禁 IP 和最近日志"
    menu_item "2" "安装或修复配置" "5 次失败，封禁 1 小时"
    menu_item "3" "添加白名单" "支持单个 IP 或 CIDR"
    menu_item "4" "解封 IP" "从 sshd jail 中解除封禁"
    menu_item "5" "关闭 SSH 防爆破" "不卸载 Fail2Ban"
    menu_back_item
    local c=""
    read_tty c "请输入编号 > " ""
    case "$c" in
      1) fail2ban_status || true; pause_up ;;
      2) fail2ban_install_configure || true; pause_up ;;
      3) fail2ban_add_whitelist || true; pause_up ;;
      4) fail2ban_unban_ip || true; pause_up ;;
      5) fail2ban_disable_sshd || true; pause_up ;;
      0|q|Q) return 0 ;;
      *) warn "无效选项"; pause_up ;;
    esac
  done
}

# ======================================================================
# 安全静态网站
# 公网 HTTP 负责证书验证，配置域名通过公网 HTTPS 访问；可选 CDN 自动停站。
# 不修改或输出任何第三方程序配置。
# ======================================================================
common_firewall_normalize_port_spec() {
  local value="${1:-}" start="" end=""
  value="${value//:/-}"
  if [[ "$value" =~ ^[0-9]{1,5}$ ]]; then
    is_uint_in_range "$value" 1 65535 || return 1
    printf '%s\n' "$((10#$value))"
    return 0
  fi
  [[ "$value" =~ ^([0-9]{1,5})-([0-9]{1,5})$ ]] || return 1
  start="${BASH_REMATCH[1]}"
  end="${BASH_REMATCH[2]}"
  is_uint_in_range "$start" 1 65535 || return 1
  is_uint_in_range "$end" 1 65535 || return 1
  start=$((10#$start))
  end=$((10#$end))
  (( start <= end )) || return 1
  printf '%s-%s\n' "$start" "$end"
}

common_firewall_ufw_port_spec() {
  local value=""
  value="$(common_firewall_normalize_port_spec "${1:-}")" || return 1
  printf '%s\n' "${value//-/:}"
}

common_firewall_valid_protocol() {
  [[ "${1:-}" == "tcp" || "${1:-}" == "udp" ]]
}

common_firewall_source_family() {
  local source="${1:-}"
  valid_ip_or_cidr "$source" || return 1
  if [[ "$source" == *:* ]]; then printf 'ipv6\n'; else printf 'ipv4\n'; fi
}

common_firewall_source_contains_ip() {
  local source="${1:-}" address="${2:-}"
  valid_ip_or_cidr "$source" || return 1
  valid_ip_or_cidr "$address" || return 1
  [[ "$address" != */* ]] || return 1
  if have_cmd python3; then
    if python3 - "$source" "$address" >/dev/null 2>&1 <<'PY'
import ipaddress
import sys

network = ipaddress.ip_network(sys.argv[1], strict=False)
address = ipaddress.ip_address(sys.argv[2])
raise SystemExit(0 if address.version == network.version and address in network else 1)
PY
    then
      return 0
    fi
    return 1
  fi
  [[ "$source" == "$address" ]]
}

common_firewall_ufw_active() {
  have_cmd ufw || return 1
  LC_ALL=C ufw status 2>/dev/null | grep -qi '^Status:[[:space:]]*active'
}

common_firewall_firewalld_active() {
  have_cmd firewall-cmd || return 1
  firewall-cmd --state 2>/dev/null | grep -qx 'running'
}

common_firewall_active_backend() {
  local ufw_active=0 firewalld_active=0
  common_firewall_ufw_active && ufw_active=1
  common_firewall_firewalld_active && firewalld_active=1
  if (( ufw_active == 1 && firewalld_active == 1 )); then
    printf 'conflict\n'
    return 2
  fi
  if (( ufw_active == 1 )); then printf 'ufw\n'; return 0; fi
  if (( firewalld_active == 1 )); then printf 'firewalld\n'; return 0; fi
  printf 'none\n'
  return 1
}

common_firewall_installed_managers() {
  local managers=""
  have_cmd ufw && managers="UFW"
  if have_cmd firewall-cmd; then
    if [[ -n "$managers" ]]; then
      managers="${managers} + firewalld"
    else
      managers="firewalld"
    fi
  fi
  [[ -n "$managers" ]] || return 1
  printf '%s\n' "$managers"
}

common_firewall_state_summary() {
  local backend="" rc=0 installed=""
  backend="$(common_firewall_active_backend)" || rc=$?
  case "$rc:$backend" in
    0:ufw) printf 'UFW 已启用\n' ;;
    0:firewalld) printf 'firewalld 已启用\n' ;;
    2:conflict) printf '双管理器冲突\n' ;;
    *)
      installed="$(common_firewall_installed_managers 2>/dev/null || true)"
      if [[ -n "$installed" ]]; then
        printf '%s 已安装未启用\n' "$installed"
      else
        printf '尚未安装\n'
      fi
      ;;
  esac
}

common_firewall_menu_notice() {
  local backend="" rc=0 installed="" netfilter_risk="none"
  backend="$(common_firewall_active_backend)" || rc=$?
  case "$rc:$backend" in
    0:ufw) ok "UFW 正在运行" ;;
    0:firewalld) ok "firewalld 正在运行" ;;
    2:conflict)
      warn "UFW 与 firewalld 同时运行；脚本已暂停规则修改"
      ;;
    *)
      installed="$(common_firewall_installed_managers 2>/dev/null || true)"
      if [[ -n "$installed" ]]; then
        warn "${installed} 已安装但尚未启用；选择 [2] 可安全启用"
      else
        warn "尚未安装 UFW/firewalld；选择 [2] 可自动安装并安全启用"
      fi
      netfilter_risk="$(common_firewall_netfilter_risk)"
      case "$netfilter_risk" in
        forwarding) info "已有转发/NAT/输出规则：安装时会自动备份并原样保留" ;;
        input) warn "已有规则影响本机 INPUT：安装时会显示明细并要求确认共存" ;;
        unknown) warn "存在无法完整解析的规则：安装时会显示明细并要求确认" ;;
      esac
      ;;
  esac
}

common_firewall_choose_install_backend() {
  local ufw_installed=0 firewalld_installed=0
  have_cmd ufw && ufw_installed=1
  have_cmd firewall-cmd && firewalld_installed=1

  if (( ufw_installed == 1 && firewalld_installed == 0 )); then printf 'ufw\n'; return 0; fi
  if (( firewalld_installed == 1 && ufw_installed == 0 )); then printf 'firewalld\n'; return 0; fi
  if have_cmd apt-get || have_cmd apk; then printf 'ufw\n'; return 0; fi
  if have_cmd dnf || have_cmd yum; then printf 'firewalld\n'; return 0; fi
  return 1
}

common_firewall_firewalld_zone() {
  local zone="" ifc="" connection=""
  ifc="$(default_iface)"
  if common_firewall_firewalld_active; then
    zone="$(firewall-cmd --get-zone-of-interface="$ifc" 2>/dev/null || true)"
    [[ "$zone" == "no zone" ]] && zone=""
    [[ -n "$zone" ]] || zone="$(firewall-cmd --get-default-zone 2>/dev/null || true)"
  elif have_cmd firewall-offline-cmd; then
    if have_cmd nmcli; then
      connection="$(nmcli -g GENERAL.CONNECTION device show "$ifc" 2>/dev/null | sed -n '1p' || true)"
      if [[ -n "$connection" && "$connection" != "--" ]]; then
        zone="$(nmcli -g connection.zone connection show "$connection" 2>/dev/null | sed -n '1p' || true)"
      fi
    fi
    [[ -n "$zone" && "$zone" != "--" ]] || zone=""
    [[ -n "$zone" ]] || zone="$(firewall-offline-cmd --get-default-zone 2>/dev/null || true)"
  fi
  zone="${zone:-public}"
  [[ "$zone" =~ ^[A-Za-z0-9_.-]{1,32}$ ]] || return 1
  printf '%s\n' "$zone"
}

common_firewall_netfilter_inventory() {
  local output="" result="" command_name="" line="" ipt_risk="none"
  local nft_parsed=0
  if have_cmd nft; then
    if have_cmd python3; then
      output="$(nft -j list ruleset 2>/dev/null || true)"
      if [[ -n "$output" ]]; then
        if result="$(python3 -c '
import json
import sys

try:
    objects = json.load(sys.stdin).get("nftables", [])
except (ValueError, AttributeError):
    raise SystemExit(2)

owned_tables = {("inet", "dmitbox_rand6")}
chains = {}
rules = []
table_keys = set()

for item in objects:
    if not isinstance(item, dict) or not item:
        continue
    kind, data = next(iter(item.items()))
    if kind == "metainfo" or not isinstance(data, dict):
        continue
    family = str(data.get("family", ""))
    table = str(data.get("table", ""))
    if kind == "table":
        table = str(data.get("name", ""))
    key = (family, table)
    if not family or not table or key in owned_tables:
        continue
    table_keys.add(key)
    if kind == "chain":
        name = str(data.get("name", ""))
        if name:
            chains[(family, table, name)] = data
    elif kind == "rule":
        rules.append(data)

for family, table in sorted(table_keys):
    table_chains = {
        name: data for (fam, tab, name), data in chains.items()
        if fam == family and tab == table
    }
    table_rules = [
        rule for rule in rules
        if str(rule.get("family", "")) == family and str(rule.get("table", "")) == table
    ]
    input_chains = {
        name for name, data in table_chains.items()
        if str(data.get("hook", "")).lower() == "input"
    }
    input_policy = any(
        str(table_chains[name].get("policy") or "accept").lower() != "accept"
        for name in input_chains
    )
    input_rules = any(str(rule.get("chain", "")) in input_chains for rule in table_rules)
    hooked_policy = any(
        data.get("hook") and str(data.get("policy") or "accept").lower() != "accept"
        for data in table_chains.values()
    )
    if input_policy or input_rules:
        risk = "input"
        detail = "包含 INPUT 钩子规则或非 ACCEPT 策略"
    elif table_rules or hooked_policy:
        risk = "forwarding"
        hooks = sorted({str(data.get("hook")) for data in table_chains.values() if data.get("hook")})
        detail = "仅发现转发/NAT/输出路径"
        if hooks:
            detail += "（" + ",".join(hooks) + "）"
    else:
        continue
    label = (family + " " + table).replace("|", "/")
    print("|".join((risk, "nft", label, detail)))
' <<< "$output")"; then
          nft_parsed=1
          [[ -n "$result" ]] && printf '%s\n' "$result"
        fi
      fi
    fi
    if (( nft_parsed == 0 )); then
      output="$(nft list tables 2>/dev/null || true)"
      while IFS= read -r line; do
        [[ -n "$line" ]] || continue
        [[ "$line" =~ ^[[:space:]]*table[[:space:]]+inet[[:space:]]+dmitbox_rand6[[:space:]]*$ ]] && continue
        printf 'unknown|nft|%s|无法解析该表对入站流量的影响\n' "${line//|//}"
      done <<< "$output"
    fi
  fi
  for command_name in iptables ip6tables; do
    have_cmd "$command_name" || continue
    output="$("$command_name" -S 2>/dev/null || true)"
    ipt_risk="none"
    while IFS= read -r line; do
      [[ -n "$line" ]] || continue
      case "$line" in
        "-P INPUT ACCEPT"|"-P FORWARD ACCEPT"|"-P OUTPUT ACCEPT"|-N\ *) ;;
        "-P INPUT "*|-A\ INPUT\ *) ipt_risk="input"; break ;;
        -P\ FORWARD\ *|-P\ OUTPUT\ *|-A\ *) [[ "$ipt_risk" == "none" ]] && ipt_risk="forwarding" ;;
      esac
    done <<< "$output"
    case "$ipt_risk" in
      input) printf 'input|%s|filter 规则集|包含 INPUT 规则或非 ACCEPT 策略\n' "$command_name" ;;
      forwarding) printf 'forwarding|%s|规则集|仅发现转发、输出或自定义链\n' "$command_name" ;;
    esac
  done
}

common_firewall_netfilter_risk() {
  local inventory=""
  inventory="$(common_firewall_netfilter_inventory | sort -u)"
  if grep -q '^input|' <<< "$inventory"; then printf 'input\n'; return 0; fi
  if grep -q '^unknown|' <<< "$inventory"; then printf 'unknown\n'; return 0; fi
  if grep -q '^forwarding|' <<< "$inventory"; then printf 'forwarding\n'; return 0; fi
  printf 'none\n'
}

common_firewall_netfilter_show_inventory() {
  local inventory="" risk="" source="" object="" detail="" count=0
  inventory="$(common_firewall_netfilter_inventory | sort -u)"
  [[ -n "$inventory" ]] || { info "未发现额外的 netfilter 规则"; return 0; }
  while IFS='|' read -r risk source object detail; do
    [[ -n "$risk" ]] || continue
    count=$((count + 1))
    if (( count > 16 )); then
      info "其余规则已省略，可在防火墙状态页查看"
      break
    fi
    case "$risk" in
      input) warn "影响本机入站：${source} · ${object} · ${detail}" ;;
      forwarding) info "保留共存规则：${source} · ${object} · ${detail}" ;;
      unknown) warn "影响范围未知：${source} · ${object} · ${detail}" ;;
    esac
  done <<< "$inventory"
}

common_firewall_backup_netfilter() {
  local backup_dir="" saved=0 command_name="" output_file=""
  COMMON_FIREWALL_LAST_BACKUP=""
  ensure_dir "$COMMON_FIREWALL_BACKUP_DIR" || { warn "无法创建防火墙备份目录"; return 1; }
  backup_dir="${COMMON_FIREWALL_BACKUP_DIR}/pre-enable-$(ts_now)-$$"
  ensure_dir "$backup_dir" || { warn "无法创建本次防火墙备份"; return 1; }
  chmod 700 "$COMMON_FIREWALL_BACKUP_DIR" "$backup_dir" 2>/dev/null || true

  if have_cmd nft; then
    output_file="${backup_dir}/nftables.rules"
    if (umask 077; nft list ruleset > "$output_file" 2>/dev/null) && [[ -s "$output_file" ]]; then
      saved=$((saved + 1))
    else
      rm -f "$output_file"
    fi
  fi
  for command_name in iptables-save ip6tables-save; do
    have_cmd "$command_name" || continue
    output_file="${backup_dir}/${command_name}.rules"
    if (umask 077; "$command_name" > "$output_file" 2>/dev/null) && [[ -s "$output_file" ]]; then
      saved=$((saved + 1))
    else
      rm -f "$output_file"
    fi
  done
  if (( saved == 0 )); then
    rmdir "$backup_dir" 2>/dev/null || true
    warn "已有规则无法完整读取和备份，已取消启用"
    return 1
  fi
  (umask 077; printf '%s\n' \
    "DMITBox 防火墙启用前只读备份" \
    "时间：$(date -Is 2>/dev/null || date)" \
    "说明：脚本不会自动恢复此备份；需要恢复时请从 VPS 控制台人工确认。" \
    > "${backup_dir}/README.txt") || return 1
  COMMON_FIREWALL_LAST_BACKUP="$backup_dir"
  ok "原有 netfilter 规则已备份：${backup_dir}"
}

common_firewall_listener_lines() {
  local protocol="$1"
  common_firewall_valid_protocol "$protocol" || return 1
  if have_cmd ss; then
    if [[ "$protocol" == "tcp" ]]; then
      ss -H -ltn 2>/dev/null || true
    else
      ss -H -lun 2>/dev/null || true
    fi
  elif have_cmd netstat; then
    if [[ "$protocol" == "tcp" ]]; then
      netstat -lnt 2>/dev/null | sed -n '3,$p' || true
    else
      netstat -lnu 2>/dev/null | sed -n '3,$p' || true
    fi
  fi
}

common_firewall_public_listeners() {
  local protocol="" line="" address="" host="" port=""
  for protocol in tcp udp; do
    while IFS= read -r line; do
      [[ -n "$line" ]] || continue
      address="$(awk '{print $4}' <<< "$line")"
      [[ "$address" == *:* ]] || continue
      port="${address##*:}"
      is_uint_in_range "$port" 1 65535 || continue
      port=$((10#$port))
      host="${address%:*}"
      host="${host%%%*}"
      case "$host" in
        127.*|localhost|"[::1]"|::1|::ffff:127.*) continue ;;
      esac
      printf '%s|%s\n' "$protocol" "$port"
    done < <(common_firewall_listener_lines "$protocol")
  done | sort -t '|' -k1,1 -k2,2n -u
}

common_firewall_preserved_ports() {
  local preset="${1:-listeners}" port=""
  {
    while IFS= read -r port; do
      is_uint_in_range "$port" 1 65535 || continue
      printf 'tcp|%s\n' "$((10#$port))"
    done < <(ssh_current_ports 2>/dev/null | tr '[:space:]' '\n')
    case "$preset" in
      ssh) ;;
      web)
        printf 'tcp|80\ntcp|443\n'
        ;;
      listeners)
        common_firewall_public_listeners
        ;;
      *) return 1 ;;
    esac
  } | sort -t '|' -k1,1 -k2,2n -u
}

common_firewall_registry_init() {
  local first=""
  if [[ -L "$COMMON_FIREWALL_REGISTRY" ]]; then
    warn "防火墙规则记录路径不能是符号链接"
    return 1
  fi
  if [[ -e "$COMMON_FIREWALL_REGISTRY" && ! -f "$COMMON_FIREWALL_REGISTRY" ]]; then
    warn "防火墙规则记录路径不是普通文件：${COMMON_FIREWALL_REGISTRY}"
    return 1
  fi
  if [[ -s "$COMMON_FIREWALL_REGISTRY" ]]; then
    first="$(sed -n '1p' "$COMMON_FIREWALL_REGISTRY" 2>/dev/null || true)"
    if [[ "$first" != "$COMMON_FIREWALL_MARKER" ]]; then
      warn "防火墙规则记录文件不属于本脚本，拒绝覆盖"
      return 1
    fi
  else
    ensure_dir "$(dirname "$COMMON_FIREWALL_REGISTRY")" || return 1
    (umask 077; printf '%s\n' "$COMMON_FIREWALL_MARKER" > "$COMMON_FIREWALL_REGISTRY") || return 1
  fi
  chmod 600 "$COMMON_FIREWALL_REGISTRY" 2>/dev/null || true
}

common_firewall_registry_add() {
  local backend="$1" action="$2" protocol="$3" port="$4" source="$5" zone="$6"
  local id="" created=""
  [[ "$backend" == "ufw" || "$backend" == "firewalld" ]] || return 1
  [[ "$action" == "allow-port" || "$action" == "allow-source-port" || "$action" == "deny-source" ]] || return 1
  if [[ "$protocol" != "-" ]]; then common_firewall_valid_protocol "$protocol" || return 1; fi
  if [[ "$port" != "-" ]]; then port="$(common_firewall_normalize_port_spec "$port")" || return 1; fi
  if [[ "$source" != "-" ]]; then valid_ip_or_cidr "$source" || return 1; fi
  [[ "$zone" == "-" || "$zone" =~ ^[A-Za-z0-9_.-]{1,32}$ ]] || return 1
  common_firewall_registry_init || return 1
  if awk -F '|' -v b="$backend" -v a="$action" -v p="$protocol" -v n="$port" -v s="$source" -v z="$zone" '
    NR > 1 && $2 == b && $3 == a && $4 == p && $5 == n && $6 == s && $7 == z {found=1}
    END {exit !found}
  ' "$COMMON_FIREWALL_REGISTRY"; then
    return 0
  fi
  created="$(date '+%Y-%m-%dT%H:%M:%S%z')"
  id="$(date +%s)-$$-${RANDOM}"
  printf '%s|%s|%s|%s|%s|%s|%s|%s\n' \
    "$id" "$backend" "$action" "$protocol" "$port" "$source" "$zone" "$created" \
    >> "$COMMON_FIREWALL_REGISTRY" || return 1
  chmod 600 "$COMMON_FIREWALL_REGISTRY" 2>/dev/null || true
}

common_firewall_registry_count() {
  if [[ -f "$COMMON_FIREWALL_REGISTRY" ]] && \
     [[ "$(sed -n '1p' "$COMMON_FIREWALL_REGISTRY" 2>/dev/null || true)" == "$COMMON_FIREWALL_MARKER" ]]; then
    awk 'NR > 1 && NF {count++} END {print count + 0}' "$COMMON_FIREWALL_REGISTRY"
  else
    printf '0\n'
  fi
}

common_firewall_action_text() {
  case "${1:-}" in
    allow-port) printf '放行端口\n' ;;
    allow-source-port) printf '来源放行\n' ;;
    deny-source) printf '封禁来源\n' ;;
    *) printf '未知\n' ;;
  esac
}

common_firewall_registry_list() {
  local id="" backend="" action="" protocol="" port="" source="" zone="" created=""
  if [[ ! -f "$COMMON_FIREWALL_REGISTRY" ]] || \
     [[ "$(sed -n '1p' "$COMMON_FIREWALL_REGISTRY" 2>/dev/null || true)" != "$COMMON_FIREWALL_MARKER" ]]; then
    info "本脚本尚未创建防火墙规则"
    return 0
  fi
  printf '%-24s %-10s %-10s %-5s %-11s %-25s %s\n' "ID" "管理器" "动作" "协议" "端口" "来源" "区域"
  while IFS='|' read -r id backend action protocol port source zone created; do
    [[ -n "$id" && "$id" != \#* ]] || continue
    printf '%-24s %-10s %-10s %-5s %-11s %-25s %s\n' \
      "$id" "$backend" "$(common_firewall_action_text "$action")" "$protocol" "$port" "$source" "$zone"
  done < "$COMMON_FIREWALL_REGISTRY"
}

common_firewall_ufw_apply_rule() {
  local action="$1" protocol="$2" port="$3" source="$4"
  local ufw_port="" output="" rc=0
  COMMON_FIREWALL_CREATED=0
  case "$action" in
    allow-port)
      common_firewall_valid_protocol "$protocol" || return 1
      ufw_port="$(common_firewall_ufw_port_spec "$port")" || return 1
      output="$(LC_ALL=C ufw allow "${ufw_port}/${protocol}" 2>&1)" || rc=$?
      ;;
    allow-source-port)
      common_firewall_valid_protocol "$protocol" || return 1
      valid_ip_or_cidr "$source" || return 1
      ufw_port="$(common_firewall_ufw_port_spec "$port")" || return 1
      output="$(LC_ALL=C ufw allow proto "$protocol" from "$source" to any port "$ufw_port" 2>&1)" || rc=$?
      ;;
    deny-source)
      valid_ip_or_cidr "$source" || return 1
      output="$(LC_ALL=C ufw prepend deny from "$source" 2>&1)" || rc=$?
      ;;
    *) return 1 ;;
  esac
  if (( rc != 0 )); then
    [[ -n "$output" ]] && warn "$output"
    return "$rc"
  fi
  if grep -qi 'Skipping adding existing rule' <<< "$output"; then
    COMMON_FIREWALL_CREATED=0
  else
    COMMON_FIREWALL_CREATED=1
  fi
}

common_firewall_ufw_remove_rule() {
  local action="$1" protocol="$2" port="$3" source="$4"
  local ufw_port="" output="" rc=0
  case "$action" in
    allow-port)
      common_firewall_valid_protocol "$protocol" || return 1
      ufw_port="$(common_firewall_ufw_port_spec "$port")" || return 1
      output="$(LC_ALL=C ufw --force delete allow "${ufw_port}/${protocol}" 2>&1)" || rc=$?
      ;;
    allow-source-port)
      common_firewall_valid_protocol "$protocol" || return 1
      valid_ip_or_cidr "$source" || return 1
      ufw_port="$(common_firewall_ufw_port_spec "$port")" || return 1
      output="$(LC_ALL=C ufw --force delete allow proto "$protocol" from "$source" to any port "$ufw_port" 2>&1)" || rc=$?
      ;;
    deny-source)
      valid_ip_or_cidr "$source" || return 1
      output="$(LC_ALL=C ufw --force delete deny from "$source" 2>&1)" || rc=$?
      ;;
    *) return 1 ;;
  esac
  if (( rc != 0 )) && ! grep -Eqi 'non-existent|could not find|not found' <<< "$output"; then
    [[ -n "$output" ]] && warn "$output"
    return "$rc"
  fi
}

common_firewall_firewalld_rich_rule() {
  local action="$1" protocol="$2" port="$3" source="$4" family=""
  family="$(common_firewall_source_family "$source")" || return 1
  case "$action" in
    allow-source-port)
      common_firewall_valid_protocol "$protocol" || return 1
      port="$(common_firewall_normalize_port_spec "$port")" || return 1
      printf 'rule family="%s" source address="%s" port port="%s" protocol="%s" accept\n' \
        "$family" "$source" "$port" "$protocol"
      ;;
    deny-source)
      printf 'rule family="%s" source address="%s" drop\n' "$family" "$source"
      ;;
    *) return 1 ;;
  esac
}

common_firewall_firewalld_apply_rule() {
  local action="$1" protocol="$2" port="$3" source="$4" zone="$5"
  local kind="" value="" rich_rule="" permanent_created=0
  COMMON_FIREWALL_CREATED=0
  [[ "$zone" =~ ^[A-Za-z0-9_.-]{1,32}$ ]] || return 1
  case "$action" in
    allow-port)
      common_firewall_valid_protocol "$protocol" || return 1
      port="$(common_firewall_normalize_port_spec "$port")" || return 1
      kind="port"
      value="${port}/${protocol}"
      ;;
    allow-source-port|deny-source)
      rich_rule="$(common_firewall_firewalld_rich_rule "$action" "$protocol" "$port" "$source")" || return 1
      kind="rich-rule"
      value="$rich_rule"
      ;;
    *) return 1 ;;
  esac

  if common_firewall_firewalld_active; then
    if ! firewall-cmd --permanent --zone="$zone" "--query-${kind}=${value}" >/dev/null 2>&1; then
      firewall-cmd --permanent --zone="$zone" "--add-${kind}=${value}" >/dev/null 2>&1 || return 1
      permanent_created=1
      COMMON_FIREWALL_CREATED=1
    fi
    if ! firewall-cmd --zone="$zone" "--query-${kind}=${value}" >/dev/null 2>&1; then
      if ! firewall-cmd --zone="$zone" "--add-${kind}=${value}" >/dev/null 2>&1; then
        if (( permanent_created == 1 )); then
          firewall-cmd --permanent --zone="$zone" "--remove-${kind}=${value}" >/dev/null 2>&1 || true
        fi
        return 1
      fi
    fi
    return 0
  fi

  have_cmd firewall-offline-cmd || return 1
  if firewall-offline-cmd --zone="$zone" "--query-${kind}=${value}" >/dev/null 2>&1; then
    COMMON_FIREWALL_CREATED=0
    return 0
  fi
  firewall-offline-cmd --zone="$zone" "--add-${kind}=${value}" >/dev/null 2>&1 || return 1
  COMMON_FIREWALL_CREATED=1
}

common_firewall_firewalld_remove_rule() {
  local action="$1" protocol="$2" port="$3" source="$4" zone="$5"
  local kind="" value="" rich_rule="" rc=0
  [[ "$zone" =~ ^[A-Za-z0-9_.-]{1,32}$ ]] || return 1
  case "$action" in
    allow-port)
      common_firewall_valid_protocol "$protocol" || return 1
      port="$(common_firewall_normalize_port_spec "$port")" || return 1
      kind="port"
      value="${port}/${protocol}"
      ;;
    allow-source-port|deny-source)
      rich_rule="$(common_firewall_firewalld_rich_rule "$action" "$protocol" "$port" "$source")" || return 1
      kind="rich-rule"
      value="$rich_rule"
      ;;
    *) return 1 ;;
  esac
  if common_firewall_firewalld_active; then
    if firewall-cmd --zone="$zone" "--query-${kind}=${value}" >/dev/null 2>&1; then
      firewall-cmd --zone="$zone" "--remove-${kind}=${value}" >/dev/null 2>&1 || rc=1
    fi
    if firewall-cmd --permanent --zone="$zone" "--query-${kind}=${value}" >/dev/null 2>&1; then
      firewall-cmd --permanent --zone="$zone" "--remove-${kind}=${value}" >/dev/null 2>&1 || rc=1
    fi
  elif have_cmd firewall-offline-cmd; then
    if firewall-offline-cmd --zone="$zone" "--query-${kind}=${value}" >/dev/null 2>&1; then
      firewall-offline-cmd --zone="$zone" "--remove-${kind}=${value}" >/dev/null 2>&1 || rc=1
    fi
  else
    return 1
  fi
  return "$rc"
}

common_firewall_apply_rule() {
  local backend="$1" action="$2" protocol="$3" port="$4" source="$5" zone="$6"
  if [[ "${DMITBOX_ALLOW_UNGUARDED_FIREWALL:-0}" != 1 ]] &&
     ! netg_guarded_ssh_firewall_permit "$backend" "$action" "$protocol" "$port" "$source"; then
    warn "旧防火墙规则写入未受超时恢复保护，默认拒绝"
    return 2
  fi
  COMMON_FIREWALL_CREATED=0
  case "$backend" in
    ufw)
      have_cmd ufw || return 1
      common_firewall_ufw_apply_rule "$action" "$protocol" "$port" "$source" || return 1
      ;;
    firewalld)
      common_firewall_firewalld_apply_rule "$action" "$protocol" "$port" "$source" "$zone" || return 1
      ;;
    *) return 1 ;;
  esac
  if (( COMMON_FIREWALL_CREATED == 1 )); then
    if ! common_firewall_registry_add "$backend" "$action" "$protocol" "$port" "$source" "$zone"; then
      warn "规则已添加，但写入脚本记录失败；正在回滚该规则"
      if [[ "$backend" == "ufw" ]]; then
        common_firewall_ufw_remove_rule "$action" "$protocol" "$port" "$source" || true
      else
        common_firewall_firewalld_remove_rule "$action" "$protocol" "$port" "$source" "$zone" || true
      fi
      return 1
    fi
  fi
}

common_firewall_remove_rule_by_id() {
  local id="${1:-}" line="" backend="" action="" protocol="" port="" source="" zone="" created=""
  local tmp=""
  [[ "$id" =~ ^[0-9]+-[0-9]+-[0-9]+$ ]] || { warn "规则 ID 格式无效"; return 1; }
  [[ ! -L "$COMMON_FIREWALL_REGISTRY" ]] || { warn "规则记录路径是符号链接，拒绝操作"; return 1; }
  [[ -f "$COMMON_FIREWALL_REGISTRY" ]] || { warn "没有脚本规则记录"; return 1; }
  [[ "$(sed -n '1p' "$COMMON_FIREWALL_REGISTRY" 2>/dev/null || true)" == "$COMMON_FIREWALL_MARKER" ]] || {
    warn "规则记录文件校验失败"
    return 1
  }
  line="$(awk -F '|' -v id="$id" '$1 == id {print; exit}' "$COMMON_FIREWALL_REGISTRY")"
  [[ -n "$line" ]] || { warn "没有找到规则 ID：${id}"; return 1; }
  IFS='|' read -r id backend action protocol port source zone created <<< "$line"
  case "$backend" in
    ufw)
      have_cmd ufw || { warn "UFW 不存在，无法精确删除规则"; return 1; }
      common_firewall_ufw_remove_rule "$action" "$protocol" "$port" "$source" || return 1
      ;;
    firewalld)
      common_firewall_firewalld_remove_rule "$action" "$protocol" "$port" "$source" "$zone" || return 1
      ;;
    *) warn "记录中的防火墙类型无效"; return 1 ;;
  esac
  tmp="$(mktemp "${COMMON_FIREWALL_REGISTRY}.tmp.XXXXXX")" || return 1
  if ! awk -F '|' -v id="$id" '$1 != id' "$COMMON_FIREWALL_REGISTRY" > "$tmp"; then
    rm -f "$tmp"
    return 1
  fi
  chmod 600 "$tmp" 2>/dev/null || true
  if ! mv -f "$tmp" "$COMMON_FIREWALL_REGISTRY"; then
    rm -f "$tmp"
    return 1
  fi
  ok "已删除脚本规则：${id}"
}

common_firewall_require_active_backend() {
  local backend="" rc=0 installed="" setup_choice=""
  COMMON_FIREWALL_BACKEND=""
  backend="$(common_firewall_active_backend)" || rc=$?
  case "$rc:$backend" in
    0:ufw|0:firewalld)
      COMMON_FIREWALL_BACKEND="$backend"
      return 0
      ;;
    2:conflict)
      warn "UFW 与 firewalld 同时处于启用状态，已拒绝修改"
      info "请先人工确认实际规则来源，再停用其中一个管理器"
      return 2
      ;;
    *)
      installed="$(common_firewall_installed_managers 2>/dev/null || true)"
      if [[ -n "$installed" ]]; then
        warn "${installed} 已安装，但当前尚未启用"
      else
        warn "本机尚未安装 UFW/firewalld"
      fi
      info "安全流程会先放行 SSH 与所选服务，再安装或启用防火墙"
      if [[ "${RUN_MODE:-menu}" != "menu" ]]; then
        info "请进入“常用防火墙”，选择 [2] 安装并安全启用"
        return 1
      fi
      read_tty setup_choice "是否立即进入安全安装/启用流程？[Y/n] > " "y"
      case "${setup_choice,,}" in
        y|yes|是)
          common_firewall_safe_enable || return $?
          rc=0
          backend="$(common_firewall_active_backend)" || rc=$?
          if (( rc == 0 )) && [[ "$backend" == "ufw" || "$backend" == "firewalld" ]]; then
            COMMON_FIREWALL_BACKEND="$backend"
            ok "防火墙已就绪，继续执行刚才选择的功能"
            return 0
          fi
          info "防火墙仍未启用，已取消当前操作"
          return 1
          ;;
        n|no|否)
          info "已取消；需要时可选择 [2] 安装并安全启用"
          return 1
          ;;
        *)
          warn "请输入 y 或 n"
          return 1
          ;;
      esac
      ;;
  esac
}

common_firewall_protocols_from_choice() {
  case "${1,,}" in
    1|tcp) printf 'tcp\n' ;;
    2|udp) printf 'udp\n' ;;
    3|both|all|tcp+udp) printf 'tcp\nudp\n' ;;
    *) return 1 ;;
  esac
}

common_firewall_add_port_ui() {
  local backend="" port="" choice="" protocol="" zone="-" added=0 existing=0
  common_firewall_require_active_backend || return $?
  backend="$COMMON_FIREWALL_BACKEND"
  read_tty port "端口或范围（如 443、8000-8100）> " ""
  port="$(common_firewall_normalize_port_spec "$port")" || { warn "端口必须是 1-65535，范围起点不能大于终点"; return 1; }
  echo "  1) TCP"
  echo "  2) UDP"
  echo "  3) TCP + UDP"
  read_tty choice "协议（默认 TCP）> " "1"
  common_firewall_protocols_from_choice "$choice" >/dev/null || { warn "协议选项无效"; return 1; }
  if [[ "$backend" == "firewalld" ]]; then
    zone="$(common_firewall_firewalld_zone)" || { warn "无法确定 firewalld 入站区域"; return 1; }
  fi
  while IFS= read -r protocol; do
    common_firewall_apply_rule "$backend" allow-port "$protocol" "$port" - "$zone" || return 1
    if (( COMMON_FIREWALL_CREATED == 1 )); then
      ok "已放行 ${port}/${protocol}"
      added=$((added + 1))
    else
      info "${port}/${protocol} 已存在，未接管原规则"
      existing=$((existing + 1))
    fi
  done < <(common_firewall_protocols_from_choice "$choice")
  (( added + existing > 0 ))
}

common_firewall_add_source_port_ui() {
  local backend="" source="" port="" choice="" protocol="" zone="-" added=0 existing=0
  common_firewall_require_active_backend || return $?
  backend="$COMMON_FIREWALL_BACKEND"
  read_tty source "允许的来源 IP/CIDR > " ""
  valid_ip_or_cidr "$source" || { warn "来源必须是有效 IPv4、IPv6 或 CIDR"; return 1; }
  read_tty port "端口或范围（如 22、8000-8100）> " ""
  port="$(common_firewall_normalize_port_spec "$port")" || { warn "端口格式无效"; return 1; }
  echo "  1) TCP"
  echo "  2) UDP"
  echo "  3) TCP + UDP"
  read_tty choice "协议（默认 TCP）> " "1"
  common_firewall_protocols_from_choice "$choice" >/dev/null || { warn "协议选项无效"; return 1; }
  if [[ "$backend" == "firewalld" ]]; then
    zone="$(common_firewall_firewalld_zone)" || { warn "无法确定 firewalld 入站区域"; return 1; }
  fi
  while IFS= read -r protocol; do
    common_firewall_apply_rule "$backend" allow-source-port "$protocol" "$port" "$source" "$zone" || return 1
    if (( COMMON_FIREWALL_CREATED == 1 )); then
      ok "已允许 ${source} 访问 ${port}/${protocol}"
      added=$((added + 1))
    else
      info "相同规则已存在，未接管原规则"
      existing=$((existing + 1))
    fi
  done < <(common_firewall_protocols_from_choice "$choice")
  (( added + existing > 0 ))
}

common_firewall_block_source_ui() {
  local backend="" source="" zone="-" ssh_client="" confirm="BLOCK"
  common_firewall_require_active_backend || return $?
  backend="$COMMON_FIREWALL_BACKEND"
  read_tty source "要封禁的来源 IP/CIDR > " ""
  valid_ip_or_cidr "$source" || { warn "来源必须是有效 IPv4、IPv6 或 CIDR"; return 1; }
  ssh_client="${SSH_CONNECTION:-}"
  ssh_client="${ssh_client%% *}"
  if [[ -n "$ssh_client" ]] && common_firewall_source_contains_ip "$source" "$ssh_client"; then
    warn "该范围包含当前 SSH 客户端 ${ssh_client}，为避免立即失联已拒绝"
    info "如确需封禁，请从 VPS 控制台操作"
    return 2
  fi
  if [[ "$source" == "0.0.0.0/0" || "$source" == "::/0" ]]; then
    warn "该网段代表整个地址族，将封禁几乎所有入站来源"
    confirm="BLOCK-ALL"
  fi
  warn "该规则将丢弃此来源访问本机的流量"
  confirm_word "$confirm" "确认封禁请输入 ${confirm} > " || { info "已取消"; return 0; }
  if [[ "$backend" == "firewalld" ]]; then
    zone="$(common_firewall_firewalld_zone)" || { warn "无法确定 firewalld 入站区域"; return 1; }
  fi
  common_firewall_apply_rule "$backend" deny-source - - "$source" "$zone" || return 1
  if (( COMMON_FIREWALL_CREATED == 1 )); then
    ok "已封禁：${source}"
  else
    info "相同规则已存在，未接管原规则"
  fi
}

common_firewall_delete_rules_ui() {
  local choice="" id="" rc=0 count=0
  local -a ids=()
  menu_header "脚本防火墙规则" "只删除本脚本确实创建并记录的规则"
  common_firewall_registry_list
  count="$(common_firewall_registry_count)"
  (( count > 0 )) || return 0
  echo
  echo "  1) 按 ID 删除一条"
  echo "  2) 删除全部脚本规则"
  read_tty choice "请选择 > " ""
  case "$choice" in
    1)
      read_tty id "输入规则 ID > " ""
      confirm_word "DELETE" "确认删除请输入 DELETE > " || { info "已取消"; return 0; }
      common_firewall_remove_rule_by_id "$id"
      ;;
    2)
      confirm_word "CLEAR" "确认删除全部脚本规则请输入 CLEAR > " || { info "已取消"; return 0; }
      mapfile -t ids < <(awk -F '|' 'NR > 1 && NF {print $1}' "$COMMON_FIREWALL_REGISTRY")
      for id in "${ids[@]}"; do
        [[ -n "$id" ]] || continue
        common_firewall_remove_rule_by_id "$id" || rc=1
      done
      (( rc == 0 )) && ok "全部脚本规则已删除"
      return "$rc"
      ;;
    *) warn "无效选项"; return 1 ;;
  esac
}

common_firewall_show_preserved_ports() {
  local entries="$1" protocol="" port=""
  printf '%-8s %s\n' "协议" "端口"
  while IFS='|' read -r protocol port; do
    [[ -n "$protocol" && -n "$port" ]] || continue
    printf '%-8s %s\n' "$protocol" "$port"
  done <<< "$entries"
}

common_firewall_install_backend() {
  local backend="$1"
  case "$backend" in
    ufw)
      have_cmd ufw || pkg_install ufw
      have_cmd ufw || { warn "UFW 安装失败"; return 1; }
      ;;
    firewalld)
      have_cmd firewall-cmd || pkg_install firewalld
      have_cmd firewall-cmd || { warn "firewalld 安装失败"; return 1; }
      have_cmd firewall-offline-cmd || { warn "缺少 firewall-offline-cmd，无法在启动前安全预置 SSH 规则"; return 1; }
      ;;
    *) return 1 ;;
  esac
}

common_firewall_safe_enable() {
  if [[ "${DMITBOX_ALLOW_UNGUARDED_FIREWALL:-0}" != 1 ]]; then
    warn "旧 UFW/firewalld 启用流程无独立回滚，默认拒绝"
    return 2
  fi
  local active="" active_rc=0 backend="" preset_choice="" preset="listeners"
  local entries="" protocol="" port="" zone="-" existing_config="" existing_zone=""
  local netfilter_risk="none" confirm_token="ENABLE" needs_netfilter_backup=0
  active="$(common_firewall_active_backend)" || active_rc=$?
  if (( active_rc == 0 )); then
    info "防火墙已经启用：${active}"
    return 0
  fi
  if (( active_rc == 2 )); then
    warn "UFW 与 firewalld 同时启用，拒绝继续"
    return 2
  fi
  netfilter_risk="$(common_firewall_netfilter_risk)"
  case "$netfilter_risk" in
    forwarding)
      warn "检测到已有转发/NAT/输出规则（常见于 Docker、Podman 或代理服务）"
      common_firewall_netfilter_show_inventory
      info "这些规则不会被清空或接管；确认启用后将先备份，再以共存方式继续"
      needs_netfilter_backup=1
      ;;
    input)
      warn "检测到会直接影响本机 INPUT 的既有规则"
      common_firewall_netfilter_show_inventory
      info "脚本不会清空或改写这些规则；新防火墙将与它们同时生效"
      warn "多套入站规则的最终效果取决于链优先级，请保持 SSH 会话并准备 VPS 控制台"
      confirm_token="COEXIST"
      needs_netfilter_backup=1
      ;;
    unknown)
      warn "发现无法完整解析的 netfilter 表，无法自动判断是否影响 INPUT"
      common_firewall_netfilter_show_inventory
      info "脚本不会清空或接管这些规则"
      confirm_token="COEXIST"
      needs_netfilter_backup=1
      ;;
  esac
  backend="$(common_firewall_choose_install_backend)" || {
    warn "当前发行版无法自动选择受支持的防火墙管理器"
    return 1
  }

  menu_header "安装并安全启用防火墙" "先放行 SSH 与所选服务，再启用默认入站保护"
  menu_section "启用时保留"
  menu_item "1" "SSH 端口" "最小开放范围"
  menu_item "2" "SSH + 网站端口" "额外保留 TCP/80、TCP/443"
  menu_item "3" "SSH + 当前非回环监听" "推荐，避免影响已安装服务"
  read_tty preset_choice "请选择（默认 3）> " "3"
  case "$preset_choice" in
    1) preset="ssh" ;;
    2) preset="web" ;;
    3) preset="listeners" ;;
    *) warn "无效选项"; return 1 ;;
  esac
  entries="$(common_firewall_preserved_ports "$preset")"
  [[ -n "$entries" ]] || { warn "没有识别到 SSH 端口，拒绝启用以避免失联"; return 1; }
  menu_section "将预先放行"
  common_firewall_show_preserved_ports "$entries"
  echo
  print_kv "管理器" "$backend"
  if [[ "$backend" == "ufw" ]] && have_cmd ufw; then
    existing_config="$(LC_ALL=C ufw show added 2>/dev/null || true)"
    if grep -q '^ufw ' <<< "$existing_config"; then
      warn "UFW 中已有永久规则；启用后这些规则也会生效"
      printf '%s\n' "$existing_config" | sed -n '1,60p'
    fi
  elif [[ "$backend" == "firewalld" ]] && have_cmd firewall-offline-cmd; then
    existing_zone="$(common_firewall_firewalld_zone 2>/dev/null || true)"
    if [[ -n "$existing_zone" ]]; then
      existing_config="$(firewall-offline-cmd --zone="$existing_zone" --list-all 2>/dev/null || true)"
      if [[ -n "$existing_config" ]]; then
        info "firewalld 区域 ${existing_zone} 的现有永久配置也会保留"
        printf '%s\n' "$existing_config" | sed -n '1,40p'
      fi
    fi
  fi
  warn "启用防火墙可能影响未显示的服务；请保持当前 SSH 会话并准备 VPS 控制台"
  if [[ "$confirm_token" == "COEXIST" ]]; then
    warn "输入 COEXIST 表示保留现有规则，并确认同时启用新的防火墙管理器"
    confirm_word "COEXIST" "确认共存并启用请输入 COEXIST > " || { info "已取消，原规则未改动"; return 0; }
  else
    confirm_word "ENABLE" "确认启用请输入 ENABLE > " || { info "已取消"; return 0; }
  fi
  if (( needs_netfilter_backup == 1 )); then
    common_firewall_backup_netfilter || return 1
  fi

  common_firewall_install_backend "$backend" || return 1
  if [[ "$backend" == "ufw" ]]; then
    if common_firewall_ufw_active; then
      warn "确认期间 UFW 状态已变为启用，已取消以避免覆盖并发修改"
      return 2
    fi
    LC_ALL=C ufw default deny incoming >/dev/null 2>&1 || { warn "设置 UFW 入站策略失败"; return 1; }
    LC_ALL=C ufw default allow outgoing >/dev/null 2>&1 || { warn "设置 UFW 出站策略失败"; return 1; }
  else
    zone="$(common_firewall_firewalld_zone)" || { warn "无法确定 firewalld 入站区域"; return 1; }
    local zone_target=""
    if common_firewall_firewalld_active; then
      zone_target="$(firewall-cmd --zone="$zone" --get-target 2>/dev/null || true)"
    else
      zone_target="$(firewall-offline-cmd --zone="$zone" --get-target 2>/dev/null || true)"
    fi
    if [[ "$zone" == "trusted" || "${zone_target^^}" == "ACCEPT" ]]; then
      warn "firewalld 区域 ${zone} 当前允许全部入站，拒绝以“一键安全”方式启用"
      info "请先人工调整区域目标，再重新执行"
      return 2
    fi
  fi

  while IFS='|' read -r protocol port; do
    [[ -n "$protocol" && -n "$port" ]] || continue
    if ! common_firewall_apply_rule "$backend" allow-port "$protocol" "$port" - "$zone"; then
      warn "预置 ${port}/${protocol} 失败，防火墙尚未启用"
      return 1
    fi
  done <<< "$entries"

  if [[ "$backend" == "ufw" ]]; then
    if ! LC_ALL=C ufw --force enable >/dev/null 2>&1; then
      warn "UFW 启用失败；已写入的放行规则仍保留但防火墙未启用"
      return 1
    fi
    common_firewall_ufw_active || { warn "UFW 命令完成但状态仍不是 active"; return 1; }
  else
    if is_systemd; then
      systemctl enable --now firewalld >/dev/null 2>&1 || { warn "firewalld 启动失败"; return 1; }
    elif have_cmd service; then
      service firewalld start >/dev/null 2>&1 || { warn "firewalld 启动失败"; return 1; }
    else
      warn "没有可用的服务管理器，firewalld 未启动"
      return 1
    fi
    common_firewall_firewalld_active || { warn "firewalld 启动后未进入 running 状态"; return 1; }
  fi
  ok "防火墙已安全启用：${backend}"
  info "请立即新开一个 SSH 会话验证登录；当前会话先不要退出"
}

common_firewall_container_warning() {
  if have_cmd docker || have_cmd podman || \
     ip link show docker0 >/dev/null 2>&1 || ip link show podman0 >/dev/null 2>&1; then
    warn "检测到容器环境：Docker/Podman 发布端口可能使用独立转发规则"
    info "请同时检查容器端口映射；不要仅凭 UFW/firewalld 状态判断容器端口已被拦截"
    return 0
  fi
  return 1
}

common_firewall_status() {
  local backend="" rc=0 zone="" listeners="" rule_count="" installed="" netfilter_risk="none"
  menu_header "防火墙状态" "管理器 · 入站规则 · 非回环监听 · 冲突诊断"
  backend="$(common_firewall_active_backend)" || rc=$?
  menu_section "管理状态"
  case "$rc:$backend" in
    0:ufw)
      print_kv "当前管理器" "UFW（已启用）"
      LC_ALL=C ufw status verbose 2>/dev/null || true
      echo
      LC_ALL=C ufw status numbered 2>/dev/null | sed -n '1,100p' || true
      ;;
    0:firewalld)
      zone="$(common_firewall_firewalld_zone 2>/dev/null || echo public)"
      print_kv "当前管理器" "firewalld（已启用）"
      print_kv "当前入站区域" "$zone"
      firewall-cmd --zone="$zone" --list-all 2>/dev/null || true
      ;;
    2:conflict)
      warn "UFW 与 firewalld 同时启用：规则可能互相覆盖"
      info "在明确规则来源前，脚本不会修改任何一方"
      ;;
    *)
      installed="$(common_firewall_installed_managers 2>/dev/null || true)"
      if [[ -n "$installed" ]]; then
        print_kv "当前管理器" "${installed}（已安装但未启用）"
        info "选择 [2] 可在保留 SSH 与现有服务后安全启用"
      else
        print_kv "当前管理器" "未安装"
        warn "尚未安装 UFW/firewalld"
        info "返回后选择 [2]，脚本会自动选择、安装并安全启用"
      fi
      netfilter_risk="$(common_firewall_netfilter_risk)"
      case "$netfilter_risk" in
        input)
          warn "现有规则会影响本机 INPUT；启用新管理器前需要确认共存"
          common_firewall_netfilter_show_inventory
          ;;
        forwarding)
          info "现有规则仅涉及转发/NAT/输出，可在备份后与新防火墙共存"
          common_firewall_netfilter_show_inventory
          ;;
        unknown)
          warn "存在无法完整解析的 netfilter 规则"
          common_firewall_netfilter_show_inventory
          ;;
        *) info "未发现明显的自定义 netfilter 规则" ;;
      esac
      if have_cmd nft; then
        echo -e "${c_dim}nftables 表：${c_reset}"
        nft list tables 2>/dev/null | sed -n '1,30p' || true
      fi
      ;;
  esac

  menu_section "本脚本创建的规则"
  rule_count="$(common_firewall_registry_count)"
  print_kv "记录数量" "$rule_count"
  common_firewall_registry_list

  menu_section "非回环监听端口"
  listeners="$(common_firewall_public_listeners)"
  if [[ -n "$listeners" ]]; then
    common_firewall_show_preserved_ports "$listeners"
  else
    info "未发现非回环 TCP/UDP 监听端口，或缺少 ss/netstat"
  fi

  menu_section "边界提醒"
  common_firewall_container_warning || true
  info "VPS 服务商安全组/云防火墙是另一层策略，需要单独检查"
  info "本模块不修改转发/NAT，不清空 nftables/iptables，也不删除非脚本规则"
}

common_firewall_logging_ui() {
  local backend="" choice=""
  common_firewall_require_active_backend || return $?
  backend="$COMMON_FIREWALL_BACKEND"
  echo "  1) 开启丢弃日志（UFW low / firewalld 单播）"
  echo "  2) 关闭防火墙日志"
  read_tty choice "请选择 > " ""
  case "$backend:$choice" in
    ufw:1) LC_ALL=C ufw logging low >/dev/null 2>&1 || return 1 ;;
    ufw:2) LC_ALL=C ufw logging off >/dev/null 2>&1 || return 1 ;;
    firewalld:1) firewall-cmd --set-log-denied=unicast >/dev/null 2>&1 || return 1 ;;
    firewalld:2) firewall-cmd --set-log-denied=off >/dev/null 2>&1 || return 1 ;;
    *) warn "无效选项"; return 1 ;;
  esac
  ok "防火墙日志设置已更新"
  info "日志可能写入 journal、kern.log 或系统防火墙日志，取决于发行版配置"
  if [[ "$choice" == "1" ]]; then
    warn "遭遇扫描或攻击时日志会增长，请同时关注磁盘空间"
  fi
}

common_firewall_disable_ui() {
  local backend="" rc=0 installed=""
  backend="$(common_firewall_active_backend)" || rc=$?
  if (( rc == 2 )); then
    warn "UFW 与 firewalld 同时启用，拒绝自动停用其中一方"
    return 2
  fi
  if (( rc != 0 )); then
    installed="$(common_firewall_installed_managers 2>/dev/null || true)"
    if [[ -n "$installed" ]]; then
      info "${installed} 已安装但当前未启用，无需停用"
    else
      info "本机尚未安装 UFW/firewalld，无需停用"
    fi
    return 0
  fi
  warn "停用 ${backend} 后，本机入站保护将不再生效"
  warn "永久规则和脚本记录会保留，重新启用后可继续使用"
  confirm_word "DISABLE" "确认停用请输入 DISABLE > " || { info "已取消"; return 0; }
  case "$backend" in
    ufw)
      LC_ALL=C ufw --force disable >/dev/null 2>&1 || { warn "UFW 停用失败"; return 1; }
      ;;
    firewalld)
      if is_systemd; then
        systemctl disable --now firewalld >/dev/null 2>&1 || { warn "firewalld 停用失败"; return 1; }
      elif have_cmd service; then
        service firewalld stop >/dev/null 2>&1 || { warn "firewalld 停用失败"; return 1; }
      else
        warn "没有可用的服务管理器"
        return 1
      fi
      ;;
  esac
  ok "防火墙已停用：${backend}"
}

common_firewall_menu() {
  while true; do
    local state=""
    state="$(common_firewall_state_summary)"
    menu_header "常用防火墙" "当前：${state} · 规则共存保护 · 精确规则管理"
    common_firewall_menu_notice
    menu_section "状态与启用"
    menu_item "1" "状态与安全诊断" "规则、监听端口、冲突与容器提示"
    if [[ "${DMITBOX_ALLOW_UNGUARDED_FIREWALL:-0}" != "1" ]]; then
      warn "写入操作默认锁定：该旧模块尚无独立超时恢复，请使用网络安全工作台审计规则指纹"
    fi
    menu_item "2" "安装并安全启用" "保留 SSH/现有服务，识别并备份原规则"
    menu_section "常用规则"
    menu_item "3" "放行端口" "TCP、UDP、单端口或端口范围"
    menu_item "4" "仅指定来源放行" "IPv4/IPv6/CIDR + 端口"
    menu_item "5" "封禁 IP 或网段" "丢弃该来源访问本机的流量"
    menu_item "6" "关闭端口 / 解封来源" "查看并只删除本脚本创建的精确规则"
    menu_section "运行设置"
    menu_item "7" "防火墙日志" "开启基础丢弃日志或关闭日志"
    menu_item "8" "停用防火墙" "保留永久规则，需要 DISABLE 确认"
    menu_back_item
    local c=""
    read_tty c "请输入编号 > " ""
    case "$c" in
      1) common_firewall_status || true; pause_up ;;
      2|3|4|5|6|7|8)
        if [[ "${DMITBOX_ALLOW_UNGUARDED_FIREWALL:-0}" != "1" ]]; then
          warn "旧 UFW/firewalld 写操作尚未接入独立超时回滚，默认禁用"
          info "请先使用【网络安全工作台】获取 SHA-256，再在控制台人工管理"
          info "测试隔离环境可显式设置 DMITBOX_ALLOW_UNGUARDED_FIREWALL=1"
        else
          warn "你已允许未经统一保护的旧式防火墙写操作，风险由操作方承担"
          case "$c" in
            2) common_firewall_safe_enable || true ;;
            3) common_firewall_add_port_ui || true ;;
            4) common_firewall_add_source_port_ui || true ;;
            5) common_firewall_block_source_ui || true ;;
            6) common_firewall_delete_rules_ui || true ;;
            7) common_firewall_logging_ui || true ;;
            8) common_firewall_disable_ui || true ;;
          esac
        fi
        pause_up ;;
      0|q|Q) return 0 ;;
      *) warn "无效选项"; pause_up ;;
    esac
  done
}

security_overview() {
  menu_header "安全防护状态" "SSH · Fail2Ban · 常用防火墙"
  menu_section "SSH"
  print_kv "当前端口" "$(ssh_current_ports 2>/dev/null || echo 22)"
  if have_cmd sshd; then
    sshd -T 2>/dev/null | grep -Ei '^(passwordauthentication|permitrootlogin|pubkeyauthentication|maxauthtries|logingracetime) ' || true
  else
    warn "未找到 sshd"
  fi
  menu_section "Fail2Ban"
  if have_cmd fail2ban-client; then
    fail2ban-client ping 2>/dev/null || warn "Fail2Ban 服务未响应"
    fail2ban-client status sshd 2>/dev/null || warn "sshd jail 未运行"
  else
    info "Fail2Ban 未安装"
  fi
  menu_section "防火墙"
  local firewall_backend="" firewall_rc=0 firewall_installed=""
  firewall_backend="$(common_firewall_active_backend)" || firewall_rc=$?
  case "$firewall_rc:$firewall_backend" in
    0:ufw) ok "UFW 已启用" ;;
    0:firewalld) ok "firewalld 已启用" ;;
    2:conflict) warn "UFW 与 firewalld 同时启用" ;;
    *)
      firewall_installed="$(common_firewall_installed_managers 2>/dev/null || true)"
      if [[ -n "$firewall_installed" ]]; then
        info "${firewall_installed} 已安装但未启用"
      else
        warn "尚未安装 UFW/firewalld"
      fi
      ;;
  esac
  print_kv "脚本规则" "$(common_firewall_registry_count) 条"
}

security_menu() {
  while true; do
    menu_header "安全防护" "SSH 登录 · 防爆破 · 常用防火墙"
    menu_item "1" "安全状态总览" "SSH、Fail2Ban 与防火墙状态"
    menu_item "2" "SSH 安全工具" "用户、认证方式、端口与恢复"
    menu_item "3" "Fail2Ban SSH 防爆破" "安装、白名单、封禁与解封"
    menu_item "4" "常用防火墙" "安全启用、端口、来源、封禁与日志"
    menu_back_item
    local c=""
    read_tty c "请输入编号 > " ""
    case "$c" in
      1) security_overview || true; pause_up ;;
      2) ssh_menu ;;
      3) fail2ban_menu ;;
      4) common_firewall_menu ;;
      0|q|Q) return 0 ;;
      *) warn "无效选项"; pause_up ;;
    esac
  done
}

# ---------------- 一键还原 ----------------


# ---------------- 分组菜单 ----------------
network_menu() {
  local c=""
  while true; do
    menu_header "网络与路由" "本地路由 / DNS 诊断 · 独立事务回滚"
    menu_section "诊断"
    menu_item 1 "本地网络体检" "仅读取路由、网卡与系统配置；DNS 测试会进行解析"
    menu_item 2 "主动路由测试" "Ping / tracepath：只有选择时才向外发送测试流量"
    menu_item 3 "刷新 DNS 缓存" "仅检查并刷新 resolved 缓存"
    menu_section "IPv4 / IPv6 优先级"
    menu_item 4 "优先 IPv4" "glibc 地址选择优先级，自动回滚保护"
    menu_item 5 "系统默认优先级" "恢复 glibc 默认倾向"
    menu_item 6 "首次配置前优先级" "从本机备份精确恢复"
    menu_item 7 "网络保护工作台" "网卡、MTU、DNS、SHA-256 和事务管理"
    menu_back_item
    read_tty c "请输入编号 > " ""
    case "$c" in
      1) health_check_only || true; pause_up ;;
      2) netg_route_test || true; pause_up ;;
      3) health_check_autofix || true; pause_up ;;
      4) netg_gai_set_safe v4 || true; pause_up ;;
      5) netg_gai_set_safe default || true; pause_up ;;
      6) netg_gai_set_safe restore || true; pause_up ;;
      7) netg_network_menu ;;
      0|q|Q) return 0 ;;
      *) warn "无效选项"; pause_up ;;
    esac
  done
}

ipv6_menu() {
  while true; do
    menu_header "IPv6 管理" "基础开关 · 故障修复 · /64 地址池与随机出网"
    menu_section "基础管理"
    menu_item "1" "查看 IPv6 状态" "地址、路由、/64 前缀与 /128 列表"
    menu_item "2" "开启 IPv6" "受保护 sysctl 变更，不重写引导或 RA"
    menu_item "3" "关闭 IPv6" "受保护变更；不会强制中断当前 IPv6 SSH"
    menu_section "高级工具"
    menu_item "4" "/64 地址池与随机出网" "管理 /128 地址和 nft NAT66"
    menu_item "5" "IPv6 故障排查" "只读检查内核启动参数和黑名单"
    menu_back_item

    local c=""
    read_tty c "请输入编号 > " ""
    case "$c" in
      1) ipv6_pool_status || true; pause_up ;;
      2) netg_ipv6_toggle_safe enable || true; pause_up ;;
      3) netg_ipv6_toggle_safe disable || true; pause_up ;;
      4) ipv6_tools_menu ;;
      5) _ipv6_find_disable_sources || true; pause_up ;;
      0|q|Q) return 0 ;;
      *) warn "无效选项"; pause_up ;;
    esac
  done
}

tcp_menu() {
  local c=""
  while true; do
    menu_header "TCP / BBR（本地）" "只读检测 · 固定预设 · 原始参数恢复"
    print_kv "固定调优" "$(tcp_fixed_status_text)"
    external_tcp_tuning_active && warn "存在第三方 TCP 调优：不会下载、执行或覆盖它"
    menu_item 1 "检测拥塞控制" "查看可用算法与 FQ"
    menu_item 2 "固定通用调优" "先验证内核支持、创建快照并校验"
    menu_item 3 "恢复原始配置" "使用已保存的 sysctl 快照"
    menu_back_item
    read_tty c "请输入编号 > " ""
    case "$c" in
      1) bbr_check || true; pause_up ;;
      2) tcp_fixed_apply || true; pause_up ;;
      3) tcp_restore_default || true; pause_up ;;
      0|q|Q) return 0 ;;
      *) warn "无效选项"; pause_up ;;
    esac
  done
}

dns_mtu_menu() {
  while true; do
    menu_header "DNS / MTU" "受保护 DNS 切换 · MTU 探测与临时应用"
    menu_section "DNS"
    menu_item "1" "切换 DNS" "Cloudflare、Google、Quad9"
    menu_item "2" "恢复原始 DNS" "回到脚本首次修改前的配置"
    menu_section "MTU"
    menu_item "3" "MTU 管理" "只读探测、临时调整与超时回滚"
    menu_back_item

    local c=""
    read_tty c "请输入编号 > " ""
    case "$c" in
      1) dns_switch_menu ;;
      2) netg_dns_restore_safe "$(default_iface)" || true; pause_up ;;
      3) mtu_menu ;;
      0|q|Q) return 0 ;;
      *) warn "无效选项"; pause_up ;;
    esac
  done
}

system_tools_menu() {
  local c=""
  while true; do
    menu_header "系统维护" "本机组件、清理、更新与离线配置快照"
    menu_item 1 "Swap 管理" "创建、调整与恢复"
    menu_item 2 "磁盘与日志清理" "先预览、再确认执行"
    menu_item 3 "软件包更新与时间同步" "使用发行版软件源，不执行第三方安装脚本"
    menu_item 4 "设置时区" "Asia/Shanghai（需要确认）"
    menu_item 5 "KVM 来宾组件" "检查/安装 cloud-init 与 QEMU Guest Agent，不接管网络"
    menu_item 6 "本地系统快照" "保存到 root 私有目录，不上传"
    menu_back_item
    read_tty c "请输入编号 > " ""
    case "$c" in
      1) swap_menu ;;
      2) cleanup_menu ;;
      3) system_update_menu ;;
      4) set_timezone_china || true; pause_up ;;
      5) vm_guest_menu ;;
      6) env_snapshot || true; pause_up ;;
      0|q|Q) return 0 ;;
      *) warn "无效选项"; pause_up ;;
    esac
  done
}

# ---------------- 主菜单 ----------------
menu() {
  RUN_MODE="menu"
  local choice=""
  while true; do
    banner
    menu_section "网络管理"
    menu_item 1 "网络、路由与事务" "双栈诊断、网卡、规则指纹、超时回滚"
    menu_item 2 "IPv6 与地址池" "受保护 IPv6 开关、/128 地址与 nft NAT66"
    menu_item 3 "TCP / BBR" "本地固定预设与原始参数恢复"
    menu_item 4 "DNS / MTU" "受保护 DNS 切换与路径 MTU"
    menu_section "主机与安全"
    menu_item 5 "系统资源与端口" "进程、连接、监听端口与服务状态"
    menu_item 6 "SSH、Fail2Ban 与防火墙" "配置与检查；旧防火墙写入默认禁用"
    menu_item 7 "系统维护与虚拟化" "包管理、Swap、日志、KVM 来宾及本地快照"
    echo
    menu_item 0 "退出"
    menu_rule
    read_tty choice "请输入编号 > " ""
    case "$choice" in
      1) network_menu ;;
      2) ipv6_menu ;;
      3) tcp_menu ;;
      4) dns_mtu_menu ;;
      5) system_status_menu ;;
      6) security_menu ;;
      7) system_tools_menu ;;
      0|q|Q) return 0 ;;
      *) warn "无效选项"; pause_main ;;
    esac
  done
}

main() {
  case "${1:-}" in
    --help|-h)
      cat <<'HELP'
DMITBox: 本地 VPS 配置工具（无遥测、无远端控制端）
Usage: bash dmitbox-local.sh COMMAND [ARGS]
  --help       显示帮助，无需 root
  --version    显示版本，无需 root
  --menu       打开交互菜单（默认，要求 root）
  --net-report  双栈、DNS、接口、内核与防火墙只读审计
  --net-route-test  IPv4/IPv6 路由、ICMP 与 tracepath 测试
  --net-firewall-hash  计算当前 netfilter 规则 SHA-256（只读）
  --net-list  列出网络保护事务
  --net-status ID  查看事务状态及前后 SHA-256
  --net-confirm ID  在新 SSH 会话确认并取消超时回滚（要求 root）
  --net-rollback ID  显式恢复指定未确认的事务（要求 root）
  --net-dns-set PROVIDER IFACE  事务化切换 DNS：cloudflare|google|quad9
  --net-dns-restore IFACE  恢复 DNS（先产生独立回滚点）
  --net-mtu-set IFACE MTU  临时调整 MTU，待外部连接确认
  --net-ipv6 enable|disable  事务化修改 IPv6 sysctl 开关
  --net-iface IFACE up|down  网卡状态变更（不允许关闭默认出口）
  --net-gai v4|default|restore  glibc 地址优先级
  --net-ssh-port PORT  事务化更换 SSH 端口（要求独立新连接确认）
Default safety:
  * 无遥测、无远端控制端；启动与菜单状态采集不向外联网
  * 不包含第三方在线脚本执行、DD 重装、DMITBox 站点部署和反向代理
  * 软件包安装/更新及主动网络诊断仍可能联网（显式选择后）
  * DNS/IPv6/MTU/SSH 变更采用独立超时回滚；TCP 与地址池仍使用原有局部备份
  * 旧 UFW/firewalld 写入默认禁用，不代表原生 nftables 已接管
  * 不覆盖 DNS 管理器拥有的 resolv.conf 符号链接
HELP
      return 0 ;;
    --version) printf '%s %s\n' "$SCRIPT_NAME" "$SCRIPT_VERSION"; return 0 ;;
    --net-report) netg_report; return $? ;;
    --net-route-test) netg_route_test; return $? ;;
    --net-firewall-hash) netg_firewall_fingerprint; return $? ;;
    --net-list) need_root; netg_tx_list; return $? ;;
    --net-status) [[ "$#" -eq 2 ]] || return 2; need_root; netg_tx_status "$2"; return $? ;;
    --net-confirm) [[ "$#" -eq 2 ]] || return 2; need_root; netg_tx_confirm "$2"; return $? ;;
    --net-rollback) [[ "$#" -eq 2 ]] || return 2; need_root; netg_tx_rollback_now "$2"; return $? ;;
    --net-dns-set) [[ "$#" -eq 3 ]] || return 2; need_root; netg_dns_set_safe "$2" "$3"; return $? ;;
    --net-dns-restore) [[ "$#" -eq 2 ]] || return 2; need_root; netg_dns_restore_safe "$2"; return $? ;;
    --net-mtu-set) [[ "$#" -eq 3 ]] || return 2; need_root; netg_mtu_set_safe "$2" "$3"; return $? ;;
    --net-ipv6) [[ "$#" -eq 2 ]] || return 2; need_root; netg_ipv6_toggle_safe "$2"; return $? ;;
    --net-iface) [[ "$#" -eq 3 ]] || return 2; need_root; netg_iface_set_safe "$2" "$3"; return $? ;;
    --net-gai) [[ "$#" -eq 2 ]] || return 2; need_root; netg_gai_set_safe "$2"; return $? ;;
    --net-ssh-port) [[ "$#" -eq 2 ]] || return 2; need_root; netg_ssh_port_safe "$2"; return $? ;;
    --menu|'') ;;
    *) warn "不支持的参数：$1（运行 --help 查看选项）"; return 2 ;;
  esac
  need_root
  if ! has_tty && [[ ! -t 0 ]]; then
    warn "当前没有可交互终端，无法显示菜单；请直接在 SSH/控制台运行：bash ${SCRIPT_NAME}"
    return 1
  fi
  menu
}
if [[ "${DMITBOX_SOURCE_ONLY:-0}" != "1" ]]; then
  main "$@"
fi