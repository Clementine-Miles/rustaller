# Public CLI. Report/plan commands do not initialize state, install dependencies,
# make connectivity probes, or load kernel modules.
dmb_help() {
  cat <<'EOF'
DMITBox local 4 — standalone Bash, local administration
  --help | --version | --menu | --check
  --net-report | --net-route-test [CONTEXT_JSON] | --net-firewall-hash
  --net-list | --net-status TXID | --net-confirm TXID [ssh|console]
  --net-rollback TXID
  --enroll dns IFACE | --enroll ipv6 IFACE
  --enroll ssh CONFIG_JSON | --enroll nft CONFIG_JSON
  --enroll pool CONFIG_JSON
  --plan dns IFACE PROVIDER [persistent|runtime] [DNS ...]
  --plan mtu IFACE MTU [runtime] [CONTEXT_JSON]
  --plan ipv6 enable|disable [CONTEXT_JSON]
  --plan ssh-port PORT | --plan nft CONFIG_JSON
  --plan pool CONFIG_JSON | --plan nat66 CONFIG_JSON
  --plan tcp PROFILE MEASUREMENT_REQUEST_JSON [runtime|persistent]
  --plan swap PATH SIZE_MIB PRIORITY
  --plan gai v4|default|restore
  --diff PLAN_JSON | --apply-plan PLAN_JSON
  --net-dns-set PROVIDER IFACE [persistent|runtime] [DNS ...]
  --net-dns-restore IFACE | --net-mtu-set IFACE MTU [CONTEXT_JSON]
  --net-ipv6 enable|disable [CONTEXT_JSON]
  --net-iface IFACE up|down [CONTEXT_JSON]
  --net-gai v4|default|restore | --net-ssh-port PORT
  --dns-probe NAME | --pmtu-probe ADDRESS 4|6 [IFACE]
  --tcp-report | --swap-report | --maintenance-report [FAMILY]
  --maintenance ACTION [ARG]

Plans are JSON files. Apply requires root, verified resource ownership, an armed
independent recovery worker, and an enrolled fresh SSH login or local console.
Changing SSH ports requires two fresh authentications before the same deadline.
No status command installs packages or probes the Internet. Probe commands are
explicit. Exit codes: 0 success; 2/64 invalid input; 69 dependency/unsupported;
75 lock contention; 76 conflict; 77 privilege; 78 unsafe filesystem/ownership.
EOF
}
dmb_check() {
  local cmd missing=0
  for cmd in bash jq flock timeout setsid sha256sum sync stat mktemp ip ss; do
    if command -v "$cmd" >/dev/null 2>&1; then printf 'available %s\n' "$cmd"; else printf 'missing %s\n' "$cmd"; missing=1; fi
  done
  for cmd in nft sshd getfacl getfattr setfacl setfattr resolvectl nmcli networkctl rc-service; do
    if command -v "$cmd" >/dev/null 2>&1; then printf 'optional-available %s\n' "$cmd"; else printf 'optional-missing %s\n' "$cmd"; fi
  done
  printf 'Recovery and external reachability are not established by this check.\n'
  ((missing==0))
}
dmb_report() {
  dmb_need jq ip || return
  local links addresses routes4 routes6 rules4 rules6
  links=$(dmb_run 5 ip -j -d link show) || return
  addresses=$(dmb_run 5 ip -j address show) || return
  routes4=$(dmb_run 5 ip -4 -j route show table all) || return
  routes6=$(dmb_run 5 ip -6 -j route show table all) || return
  rules4=$(dmb_run 5 ip -4 -j rule show) || return
  rules6=$(dmb_run 5 ip -6 -j rule show) || return
  jq -cn --argjson links "$links" --argjson addresses "$addresses" --argjson v4 "$routes4" --argjson v6 "$routes6" --argjson r4 "$rules4" --argjson r6 "$rules6" '{schema:1,links:$links,addresses:$addresses,routes:{ipv4:$v4,ipv6:$v6},rules:{ipv4:$r4,ipv6:$r6},external_access:"not_tested"}'
}
dmb_plan() {
  local kind=${1-}; (($#>0)) && shift
  case $kind in
    dns) (($#>=2)) || return 64; local iface=$1 provider=$2 scope=${3:-persistent}; (($#>=3)) && shift 3 || shift 2; dmb_dns_plan "$iface" "$provider" "$scope" "$@";;
    mtu) (($#>=2 && $#<=4)) || return 64; dmb_mtu_plan "$1" "$2" "${3:-runtime}" "${4:-'{}'}";;
    ipv6) (($#>=1 && $#<=2)) || return 64; dmb_ipv6_plan "$1" "${2:-'{}'}";;
    ssh-port) (($#==1)) || return 64; dmb_ssh_plan_port "$1";;
    nft) (($#==1)) || return 64; dmb_nft_plan "$1";;
    pool) (($#==1)) || return 64; local config; config=$(jq -ce . "$1") || return 64; dmb_pool_plan "$config";;
    nat66) (($#==1)) || return 64; local config; config=$(jq -ce . "$1") || return 64; dmb_nat66_plan "$config";;
    tcp) (($#>=2 && $#<=3)) || return 64; dmb_tcp_plan "$1" "$2" "${3:-runtime}";;
    swap) (($#==3)) || return 64; dmb_swap_plan_create "$1" "$2" "$3";;
    gai) (($#==1)) || return 64; dmb_gai_plan "$1";;
    *) dmb_error 'Unknown plan type'; return 64;;
  esac
}
dmb_enroll() {
  dmb_require_root && dmb_need jq || return
  local kind=${1-} manifest path
  case $kind in
    dns) (($#==2)) || return 64; manifest=$(dmb_dns_enroll "$2") || return; dmb_dns_iface_valid "$2" || return 64; path=$DMB_STATE_DIR/enrollments/dns-$2.json;;
    ipv6) (($#==2)) || return 64; manifest=$(dmb_ipv6_enroll "$2") || return; dmb_route_iface_valid "$2" || return 64; path=$DMB_STATE_DIR/enrollments/ipv6-$2.json;;
    ssh) (($#==2)) || return 64; manifest=$(dmb_ssh_enroll "$2") || return; path=$DMB_STATE_DIR/enrollments/ssh.json;;
    nft) (($#==2)) || return 64; manifest=$(dmb_nft_enroll "$2") || return; path=$DMB_STATE_DIR/enrollments/nft.json;;
    pool) (($#==2)) || return 64; local config; config=$(jq -ce . "$2") || return 64; manifest=$(dmb_pool_enroll "$config") || return; local pool_id; pool_id=$(jq -er '.id' <<< "$manifest") || return; dmb_pool_id "$pool_id" || return 64; path=$DMB_STATE_DIR/enrollments/pool-$pool_id.json;;
    *) return 64;;
  esac
  dmb_trusted_path "${path%/*}" allow_missing || return
  mkdir -p -- "${path%/*}" && chmod 0700 -- "${path%/*}" || return
  local fd
  dmb_tx_lock_path "${path%/*}/.enroll.lock" || return
  exec {fd}>"${path%/*}/.enroll.lock" || return
  flock -w 2 -x "$fd" || { exec {fd}>&-; return 75; }
  if [[ -e $path ]]; then dmb_error 'Enrollment exists; inspect and explicitly retire it before replacing'; exec {fd}>&-; return 76; fi
  printf '%s\n' "$manifest" | dmb_atomic_write "$path" 0600
  local rc=$?; exec {fd}>&-; ((rc==0)) || return "$rc"
  dmb_audit enrollment "$(jq -cn --arg kind "$kind" --arg path "$path" '{kind:$kind,path:$path}')" || return
  printf 'Enrolled %s\n' "$path"
}
dmb_apply_file() (
  dmb_require_root && dmb_need jq || return
  local source=$1 tmp plan origin=null manifest console_allowed intended
  [[ -f $source && ! -L $source ]] || return 78
  # Copy input once into private trusted storage. The immutable prepared plan is
  # independently hashed and is never sourced or interpreted as shell code.
  tmp=$(dmb_private_tmp) || return
  trap 'rm -rf -- "$tmp"' EXIT
  plan=$(jq -ceS . "$source") || return 64
  # Baselines supplied in a plan are data, never confirmation authority.
  origin=$(dmb_confirmation_origin 2>/dev/null) || { dmb_error 'Cannot identify the initiating SSH transport or explicitly authorized enrolled console'; return 77; }
  manifest=$(_dmb_confirmation_manifest) || return
  console_allowed=$(jq -c '.confirmation.allow_console_confirmation==true' "$manifest") || return
  intended=$(jq -r '[(.contexts//[])[]|.user]|unique|if length==1 then .[0] else empty end' "$manifest") || return
  plan=$(jq -c --argjson origin "$origin" --argjson console "$console_allowed" --arg intended "$intended" '
    .origin_transport=$origin |
    .allow_console_confirmation=((.allow_console_confirmation // $console) and $console) |
    if (.confirm_user==null and $origin.kind=="console" and $intended!="") then .confirm_user=$intended else . end' <<< "$plan") || return
  printf '%s\n' "$plan" | dmb_atomic_write "$tmp/input.json" 0600 || return
  dmb_tx_execute "$tmp/input.json"
)
dmb_execute_plan() (
  dmb_require_root || return
  local tmp
  tmp=$(dmb_private_tmp) || return
  trap 'rm -rf -- "$tmp"' EXIT
  dmb_plan "$@" > "$tmp/plan.json" || return
  dmb_apply_file "$tmp/plan.json"
)
dmb_diff() {
  local plan=$1 count i before current after adapter
  dmb_need jq || return
  dmb_tx_plan_validate "$plan" || return 64
  count=$(jq '.steps|length' "$plan") || return
  for ((i=0;i<count;i++)); do
    adapter=$(jq -r --argjson i "$i" '.steps[$i].adapter // .adapter' "$plan") || return
    current=$(dmb_tx_adapter_capture "$plan" "$i") || return
    before=$(jq -c --argjson i "$i" '.steps[$i].before // null' "$plan") || return
    after=$(jq -c --argjson i "$i" '.steps[$i].after' "$plan") || return
    jq -cn --argjson i "$i" --arg adapter "$adapter" --argjson current "$current" --argjson expected "$before" --argjson desired "$after" '{step:$i,adapter:$adapter,current:$current,expected_before:$expected,desired:$desired,change:($current!=$desired)}'
  done
}
dmb_menu_dns() {
  local iface provider scope address; local -a servers=()
  printf 'Interface: '; IFS= read -r iface || return
  printf 'Provider (cloudflare/google/quad9/custom): '; IFS= read -r provider || return
  printf 'Scope (persistent/runtime; empty=persistent): '; IFS= read -r scope || return
  if [[ $provider == custom ]]; then
    printf 'Enter one numeric DNS address per line; empty line finishes.\n'
    while :; do
      IFS= read -r address || return
      [[ -n $address ]] || break
      ((${#servers[@]}<8)) || { dmb_error 'At most eight addresses'; return 64; }
      servers+=("$address")
    done
  fi
  dmb_plan dns "$iface" "$provider" "${scope:-persistent}" "${servers[@]}"
}
dmb_menu_family() {
  local family=$1 action iface value second config context
  case $family in
    1)
      printf '1 Route/interface report  2 Transactions  3 Status  4 Confirm  5 Rollback\nChoice: '
      IFS= read -r action || return
      case $action in
        1) dmb_report;; 2) dmb_tx_list;;
        3|4|5) printf 'Transaction ID: '; IFS= read -r value || return
          case $action in 3) dmb_tx_status "$value";; 4) printf 'Confirmation (ssh/console): '; IFS= read -r second || return; dmb_tx_confirm "$value" "$second";; 5) dmb_require_root && dmb_recovery_attempt_rollback "$value";; esac;;
        *) return 64;;
      esac;;
    2)
      printf '1 IPv6 report  2 Toggle plan  3 Pool plan  4 NAT66 plan\nChoice: '
      IFS= read -r action || return
      case $action in
        1) dmb_route_report;;
        2) printf 'Interface: '; IFS= read -r iface || return; printf 'Mode (enable/disable): '; IFS= read -r value || return; context=$(jq -cn --arg iface "$iface" '{iface:$iface}') || return; dmb_plan ipv6 "$value" "$context";;
        3|4) printf 'Configuration JSON file: '; IFS= read -r config || return
          if [[ $action == 3 ]]; then dmb_plan pool "$config"; else dmb_plan nat66 "$config"; fi;;
        *) return 64;;
      esac;;
    3)
      printf '1 TCP report  2 Measured profile plan\nChoice: '; IFS= read -r action || return
      case $action in
        1) dmb_tcp_report;;
        2) printf 'Profile (bbr/cubic): '; IFS= read -r value || return; printf 'Explicit measurement request JSON file: '; IFS= read -r config || return; printf 'Scope (runtime/persistent): '; IFS= read -r second || return; dmb_plan tcp "$value" "$config" "${second:-runtime}";;
        *) return 64;;
      esac;;
    4)
      printf '1 DNS plan  2 MTU plan  3 Address selection plan\nChoice: '; IFS= read -r action || return
      case $action in
        1) dmb_menu_dns;;
        2) printf 'Interface: '; IFS= read -r iface || return; printf 'MTU: '; IFS= read -r value || return; dmb_plan mtu "$iface" "$value";;
        3) printf 'Policy (v4/default/restore): '; IFS= read -r value || return; dmb_plan gai "$value";;
        *) return 64;;
      esac;;
    5)
      printf '1 Resources  2 Ports/connections  3 System\nChoice: '; IFS= read -r action || return
      case $action in 1) dmb_maintenance_report resources;; 2) dmb_maintenance_report ports;; 3) dmb_maintenance_report system;; *) return 64;; esac;;
    6)
      printf '1 SSH/firewall/Fail2Ban report  2 SSH port plan  3 nft policy plan\nChoice: '; IFS= read -r action || return
      case $action in
        1) dmb_maintenance_report security;;
        2) printf 'New SSH port: '; IFS= read -r value || return; dmb_plan ssh-port "$value";;
        3) printf 'Configuration JSON file: '; IFS= read -r config || return; dmb_plan nft "$config";;
        *) return 64;;
      esac;;
    7)
      printf '1 Swap report  2 Swap creation plan  3 Guest components report  4 Maintenance report  5 Explicit maintenance action\nChoice: '; IFS= read -r action || return
      case $action in
        1) dmb_swap_report;;
        2) printf 'New swap path: '; IFS= read -r config || return; printf 'Size MiB: '; IFS= read -r value || return; printf 'Priority: '; IFS= read -r second || return; dmb_plan swap "$config" "$value" "$second";;
        3) dmb_maintenance_report guest;; 4) dmb_maintenance_report;;
        5) printf 'Action (update-packages/clean-package-cache/snapshot/set-timezone/enable-time-sync/clean-journal): '; IFS= read -r value || return
          case $value in set-timezone|clean-journal) printf 'Argument: '; IFS= read -r second || return; dmb_maintenance_run "$value" "$second";; *) dmb_maintenance_run "$value";; esac;;
        *) return 64;;
      esac;;
    *) return 64;;
  esac
}
dmb_menu() {
  [[ -t 0 && -t 1 ]] || return 64
  local choice plan_file code
  while :; do
    printf '\nDMITBox local\n1 Network, routing and transactions\n2 IPv6 and address pools\n3 TCP / BBR\n4 DNS / MTU\n5 System resources and ports\n6 SSH, Fail2Ban and firewall\n7 Maintenance and virtualization\nA Apply a reviewed JSON plan\n0 Exit\nChoice: '
    IFS= read -r choice || return
    code=0
    case $choice in
      0|q|Q) return;;
      [1-7]) dmb_menu_family "$choice" || code=$?;;
      a|A) printf 'Reviewed plan JSON file: '; IFS= read -r plan_file || return; dmb_apply_file "$plan_file" || code=$?;;
      *) dmb_error 'Invalid choice'; code=64;;
    esac
    ((code==0)) || printf 'Operation incomplete (exit %s). Inspect the error and transaction evidence.\n' "$code" >&2
  done
}

dmb_main() {
  local command=${1:---menu}; (($#>0)) && shift
  case $command in
    --help|-h) (($#==0)) || return 64; dmb_help;;
    --version) printf '%s\n' "$DMB_VERSION";;
    --check) dmb_check;; --menu) dmb_menu;;
    --net-report) (($#==0)) || return 64; dmb_report;;
    --net-route-test) (($#<=1)) || return 64; dmb_route_report "${1:-'{}'}";;
    --net-firewall-hash) (($#==0)) || return 64; dmb_need nft jq || return; dmb_nft_ruleset | dmb_nft_semantic | sha256sum;;
    --net-list) (($#==0)) || return 64; dmb_tx_list;;
    --net-status) (($#==1)) || return 64; dmb_tx_status "$1";;
    --net-confirm) (($#>=1 && $#<=2)) || return 64; dmb_tx_confirm "$1" "${2:-ssh}";;
    --net-rollback) (($#==1)) || return 64; dmb_require_root && dmb_recovery_attempt_rollback "$1";;
    --enroll) dmb_enroll "$@";;
    --plan) dmb_plan "$@";;
    --diff) (($#==1)) || return 64; dmb_diff "$1";;
    --apply-plan) (($#==1)) || return 64; dmb_apply_file "$1";;
    --net-dns-set)
      (($#>=2)) || return 64; local provider=$1 iface=$2 scope=${3:-persistent}; (($#>=3)) && shift 3 || shift 2
      dmb_execute_plan dns "$iface" "$provider" "$scope" "$@";;
    --net-dns-restore)
      (($#==1)) || return 64; local tmp; tmp=$(dmb_private_tmp) || return
      if dmb_dns_restore_plan "$1" > "$tmp/plan.json"; then dmb_apply_file "$tmp/plan.json"; local rc=$?; rm -rf -- "$tmp"; return "$rc"; else local rc=$?; rm -rf -- "$tmp"; return "$rc"; fi;;
    --net-mtu-set) (($#>=2 && $#<=3)) || return 64; dmb_execute_plan mtu "$1" "$2" runtime "${3:-'{}'}";;
    --net-ipv6) dmb_execute_plan ipv6 "$@";;
    --net-iface)
      (($#>=2 && $#<=3)) || return 64; dmb_iface_plan "$1" "$2" runtime "${3:-'{}'}";;
    --net-gai) (($#==1)) || return 64; dmb_gai "$1";;
    --net-ssh-port) dmb_execute_plan ssh-port "$@";;
    --dns-probe) (($#==1)) || return 64; dmb_dns_probe "$1";;
    --pmtu-probe) (($#>=2 && $#<=3)) || return 64; dmb_pmtu_probe "$@";;
    --tcp-report) dmb_tcp_report;; --swap-report) dmb_swap_report;;
    --maintenance-report) (($#<=1)) || return 64; dmb_maintenance_report "${1:-system}";;
    --maintenance) (($#>=1)) || return 64; dmb_maintenance_run "$@";;
    --recover-all) (($#==0)) || return 64; dmb_require_root && dmb_recovery_boot;;
    --internal-recover-watch) (($#==2)) || return 64; dmb_require_root && dmb_recovery_watch "$@";;
    --internal-recover-once) (($#==1)) || return 64; dmb_require_root && dmb_recovery_attempt_rollback "$1";;
    --internal-adapter-guarded) (($#==4)) || return 64; dmb_require_root && dmb_tx_adapter_guarded "$@";;
    *) dmb_error "Unknown command: $command"; return 64;;
  esac
}
