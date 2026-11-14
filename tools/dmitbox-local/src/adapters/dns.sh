# DNS ownership adapters. Native commands only; source this module after core.
dmb_dns_error() { printf 'dns: %s\n' "$*" >&2; return 2; }
dmb_dns_iface_valid() { [[ ${1:-} =~ ^[[:alnum:]_.:-]{1,15}$ && $1 != -* ]]; }
dmb_dns_etc() { printf '%s\n' "${DMB_ETC_ROOT:-/etc}"; }
dmb_dns_lines_json() { jq -Rsc 'split("\n") | map(select(length>0))'; }
dmb_dns_provider() {
  case $1 in
    cloudflare) printf '%s\n' 1.1.1.1 1.0.0.1 2606:4700:4700::1111 2606:4700:4700::1001 ;;
    google) printf '%s\n' 8.8.8.8 8.8.4.4 2001:4860:4860::8888 2001:4860:4860::8844 ;;
    quad9) printf '%s\n' 9.9.9.9 149.112.112.112 2620:fe::fe 2620:fe::9 ;;
    *) return 2 ;;
  esac
}
dmb_dns_servers() {
  local value canonical family count=0 dns_seen='|' 
  for value in "$@"; do
    canonical=$(dmb_ip_parse "$value") || return 2
    [[ $canonical != 0.0.0.0 && $canonical != 127.* && $canonical != :: && $canonical != ::1 && $canonical != 255.255.255.255 ]] || return 2
    if [[ $canonical == *:* ]]; then
      [[ $canonical != ff* && $canonical != fe[89ab]* ]] || return 2
    else
      family=${canonical%%.*}; (( family < 224 || family > 239 )) || return 2
    fi
    [[ $dns_seen != *"|$canonical|"* ]] || return 2
    dns_seen+="$canonical|"; count=$((count+1)); ((count<=8)) || return 2
    printf '%s\n' "$canonical"
  done
  ((count>0))
}
dmb_dns_nm_list() { tr ',;' '\n\n' | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//' -e '/^$/d' | dmb_dns_lines_json; }
dmb_dns_numeric_list() {
  local list=$1 family=${2:-} value
  jq -e 'type=="array" and length<=64 and all(.[]; type=="string" and length>0 and length<=64)' <<< "$list" >/dev/null || return 69
  while IFS= read -r value; do
    dmb_ip_parse "$value" "$family" >/dev/null || { dmb_dns_error 'DNS endpoint names, URLs, options or scopes require a dedicated owner adapter'; return 69; }
  done < <(jq -r '.[]' <<< "$list")
}
dmb_dns_resolved_get() {
  local iface=$1 line words
  line=$(dmb_run 8 resolvectl dns "$iface") || return
  # Strip the interface label only, never the ':' within an IPv6 address.
  words=$(printf '%s\n' "$line" | sed -E '1s/^Link[[:space:]]+[0-9]+[[:space:]]+\([^)]*\):[[:space:]]*//')
  local list
  list=$(printf '%s\n' "$words" | tr ' \t' '\n\n' | sed '/^$/d' | dmb_dns_lines_json) || return
  dmb_dns_numeric_list "$list" || return
  printf '%s\n' "$list"
}
dmb_dns_owner() {
  local iface=$1 mode=${2:-owned} etc uuid path json backend manifest process comm
  etc=$(dmb_dns_etc)
  dmb_dns_iface_valid "$iface" || return 2
  dmb_run 5 ip -j link show dev "$iface" >/dev/null || return 2
  if command -v nmcli >/dev/null 2>&1; then
    uuid=$(dmb_run 8 nmcli -g GENERAL.CON-UUID device show "$iface" 2>/dev/null) || uuid=''
    if [[ $uuid =~ ^[[:xdigit:]]{8}-[[:xdigit:]]{4}-[[:xdigit:]]{4}-[[:xdigit:]]{4}-[[:xdigit:]]{12}$ ]]; then
      # Imported generator output must be changed at its source, not adopted.
      if compgen -G "$etc/netplan/*.yaml" >/dev/null || [[ -s ${DMB_RUN_ROOT:-/run}/cloud-init/network-config.json ]]; then
        dmb_dns_error 'unsupported-owner: netplan/cloud-init generated connection'; return 2
      fi
      jq -cn --arg uuid "$uuid" '{owner:"nm",identity:$uuid}'; return
    fi
  fi
  if command -v networkctl >/dev/null 2>&1; then
    json=$(dmb_run 8 networkctl --json=short status "$iface" 2>/dev/null) || json='{}'
    path=$(jq -r '.NetworkFile // .NetworkFilePath // empty' <<< "$json" 2>/dev/null) || path=''
    if [[ $path == /*.network ]]; then
      case $path in *netplan*|*cloud-init*) dmb_dns_error 'unsupported-owner: generated networkd configuration'; return 2;; esac
      [[ $path == "$etc/systemd/network/"* || $path == /usr/lib/systemd/network/* || $path == /lib/systemd/network/* ]] || { dmb_dns_error 'unsupported-owner: transient/unknown networkd file'; return 2; }
      jq -cn --arg file "$path" '{owner:"networkd",identity:$file}'; return
    fi
  fi
  if command -v resolvconf >/dev/null 2>&1 || [[ -d ${DMB_RUN_ROOT:-/run}/resolvconf ]]; then dmb_dns_error 'unsupported-owner: resolvconf requires its native adapter'; return 2; fi
  if [[ -f $etc/alpine-release && -f $etc/network/interfaces ]] && grep -Eq "^[[:space:]]*iface[[:space:]]+${iface}[[:space:]]+inet6?[[:space:]]+dhcp([[:space:]]|$)" "$etc/network/interfaces"; then
    if command -v dhcpcd >/dev/null 2>&1 && [[ -f $etc/dhcpcd.conf ]]; then
      # dhcpcd writes a shared resolver. Refuse multi-interface ambiguity.
      backend=$(dmb_run 5 ip -j link show | jq '[.[]|select(.ifname!="lo" and (.flags|index("UP")))]|length') || return
      [[ $backend == 1 ]] || { dmb_dns_error 'unsupported-owner: shared dhcpcd resolver on multiple active links'; return 2; }
      jq -cn --arg file "$etc/dhcpcd.conf" '{owner:"dhcpcd",identity:$file}'; return
    fi
    dmb_dns_error 'unsupported-owner: udhcpc or unknown Alpine DHCP hook'; return 2
  fi
  # A running unknown manager is a conflict even when its control socket is
  # unavailable. Do not infer static ownership from an unavailable dependency.
  for process in /proc/[0-9]*/comm;do
    [[ -r $process ]] || continue
    IFS= read -r comm < "$process" || continue
    case $comm in NetworkManager|systemd-network|dhclient|dhcpcd|udhcpc|connmand|wickedd*) dmb_dns_error 'unsupported-owner: active manager could not be identified for this resolver';return 2;;esac
  done
  if [[ -f $etc/resolv.conf && ! -L $etc/resolv.conf ]]; then
    # A regular file alone does not prove ownership: known generators reject.
    if command -v dhclient >/dev/null 2>&1 || command -v dhcpcd >/dev/null 2>&1 || compgen -G "$etc/netplan/*.yaml" >/dev/null || [[ -s ${DMB_RUN_ROOT:-/run}/cloud-init/network-config.json ]]; then dmb_dns_error 'unsupported-owner: generated resolver'; return 2; fi
    if grep -Eiq 'generated|managed by|do not edit' "$etc/resolv.conf"; then dmb_dns_error 'unsupported-owner: resolver declares external ownership'; return 2; fi
    if [[ $mode != candidate ]]; then
      manifest="$DMB_STATE_DIR/enrollments/dns-$iface.json"
      dmb_trusted_path "$manifest" || { dmb_dns_error 'static resolver requires explicit local DNS enrollment';return 2; }
      jq -e --arg iface "$iface" --arg path "$etc/resolv.conf" '.schema==1 and .iface==$iface and .owner=="static" and .identity==$path' "$manifest" >/dev/null || return 2
    fi
    jq -cn --arg file "$etc/resolv.conf" '{owner:"static",identity:$file}'; return
  fi
  dmb_dns_error 'unsupported-owner: resolver symlink or unidentified manager'
}
dmb_dns_content() {
  local path=$1 owner=$2 iface=$3 servers=$4 existing=''
  [[ ! -e $path ]] || existing=$(cat -- "$path")
  case $owner in
    static)
      # Preserve search/domain/options and all non-nameserver lines.
      printf '%s\n' "$existing" | sed '/^[[:space:]]*nameserver[[:space:]]/d'
      jq -r '.[]|"nameserver "+.' <<< "$servers" ;;
    networkd)
      printf '[Network]\nDNS=\n'; jq -r '.[]|"DNS="+.' <<< "$servers"
      printf '\n[DHCPv4]\nUseDNS=no\n[DHCPv6]\nUseDNS=no\n[IPv6AcceptRA]\nUseDNS=no\n' ;;
    dhcpcd)
      printf '%s\n' "$existing" | sed "/^# BEGIN dmitbox DNS $iface$/,/^# END dmitbox DNS $iface$/d"
      printf '# BEGIN dmitbox DNS %s\ninterface %s\nstatic domain_name_servers=%s\n# END dmitbox DNS %s\n' "$iface" "$iface" "$(jq -r 'join(" ")' <<< "$servers")" "$iface" ;;
    *) return 2;;
  esac
}
# Each step changes exactly one independently observable resource. Profile
# persistence and runtime overrides are never conflated by a broad reapply.
dmb_dns_step() { jq -ce --argjson i "$2" '.steps[$i]' "$1"; }
dmb_dns_owner_check() {
  local step=$1 actual iface
  iface=$(jq -er .iface <<< "$step") || return
  actual=$(dmb_dns_owner "$iface") || return
  [[ $(jq -Sc '{owner,identity}' <<< "$actual") == "$(jq -Sc '{owner,identity}' <<< "$step")" ]] || { dmb_dns_error 'configuration owner changed';return 2; }
}
dmb_dns_nm_capture() {
  local step iface identity dns4 dns6 auto4 auto6
  step=$(dmb_dns_step "$1" "$2") || return;dmb_dns_owner_check "$step" || return
  iface=$(jq -r .iface <<< "$step");identity=$(jq -r .identity <<< "$step")
  dns4=$(dmb_run 8 nmcli -g ipv4.dns connection show uuid "$identity" | dmb_dns_nm_list) || return
  dns6=$(dmb_run 8 nmcli -g ipv6.dns connection show uuid "$identity" | dmb_dns_nm_list) || return
  dmb_dns_numeric_list "$dns4" 4 && dmb_dns_numeric_list "$dns6" 6 || return
  auto4=$(dmb_run 8 nmcli -g ipv4.ignore-auto-dns connection show uuid "$identity") || return
  auto6=$(dmb_run 8 nmcli -g ipv6.ignore-auto-dns connection show uuid "$identity") || return
  [[ $auto4 == yes || $auto4 == no ]] && [[ $auto6 == yes || $auto6 == no ]] || return 2
  jq -cnS --arg identity "$identity" --argjson dns4 "$dns4" --argjson dns6 "$dns6" --arg auto4 "$auto4" --arg auto6 "$auto6" '{identity:$identity,dns4:$dns4,dns6:$dns6,auto4:$auto4,auto6:$auto6}'
}
dmb_dns_nm_set() {
  local step state identity
  step=$(dmb_dns_step "$1" "$2") || return;dmb_dns_owner_check "$step" || return
  state=$(jq -ce --arg key "$3" '.[$key]' <<< "$step") || return;identity=$(jq -r .identity <<< "$step")
  dmb_dns_numeric_list "$(jq -ce .dns4 <<< "$state")" 4 && dmb_dns_numeric_list "$(jq -ce .dns6 <<< "$state")" 6 || return
  jq -e '(.auto4=="yes" or .auto4=="no") and (.auto6=="yes" or .auto6=="no")' <<< "$state" >/dev/null || return 69
  # nmcli modifies only these four persistent properties. Never device reapply:
  # it could activate unrelated edits staged by another administrator.
  dmb_run 15 nmcli connection modify uuid "$identity" ipv4.dns "$(jq -r '.dns4|join(",")' <<< "$state")" ipv6.dns "$(jq -r '.dns6|join(",")' <<< "$state")" ipv4.ignore-auto-dns "$(jq -r .auto4 <<< "$state")" ipv6.ignore-auto-dns "$(jq -r .auto6 <<< "$state")"
}
dmb_dns_nm_apply() { dmb_dns_nm_set "$1" "$2" after; }
dmb_dns_nm_restore() { dmb_dns_nm_set "$1" "$2" before; }
dmb_dns_nm_verify() { dmb_dns_state_equal dns_nm "$1" "$2" after; }
dmb_dns_nm_verify_restore() { dmb_dns_state_equal dns_nm "$1" "$2" before; }
dmb_dns_resolved_policy() {
  local iface=$1 kind value domains=''
  for kind in domain default-route dnssec dnsovertls; do
    value=$(dmb_run 8 resolvectl "$kind" "$iface") || return
    domains+="$kind=$value"$'\n'
  done
  printf '%s' "$domains" | sha256sum | awk '{print $1}'
}
dmb_dns_resolved_capture() {
  local step iface identity dns policy link
  step=$(dmb_dns_step "$1" "$2") || return;dmb_dns_owner_check "$step" || return
  iface=$(jq -r .iface <<< "$step");identity=$(jq -r .identity <<< "$step")
  link=$(dmb_run 5 ip -j link show dev "$iface" | jq -ce '.[0]|{ifindex,ifname,address}') || return
  dns=$(dmb_dns_resolved_get "$iface") || return
  policy=$(dmb_dns_resolved_policy "$iface") || return
  jq -cnS --arg identity "$identity" --argjson link "$link" --argjson dns "$dns" --arg policy "$policy" '{identity:$identity,link:$link,dns:$dns,other_policy_sha256:$policy}'
}
dmb_dns_resolved_set() {
  local step iface state
  step=$(dmb_dns_step "$1" "$2") || return;dmb_dns_owner_check "$step" || return
  iface=$(jq -r .iface <<< "$step");state=$(jq -ce --arg key "$3" '.[$key]' <<< "$step") || return
  dmb_dns_numeric_list "$(jq -ce .dns <<< "$state")" || return
  local -a values=()
  mapfile -t values < <(jq -r '.dns[]' <<< "$state")
  if ((${#values[@]})); then dmb_run 10 resolvectl dns "$iface" "${values[@]}"
  else dmb_run 10 resolvectl dns "$iface" ''; fi
}
dmb_dns_resolved_apply() { dmb_dns_resolved_set "$1" "$2" after; }
dmb_dns_resolved_restore() { dmb_dns_resolved_set "$1" "$2" before; }
dmb_dns_resolved_verify() { dmb_dns_state_equal dns_resolved "$1" "$2" after; }
dmb_dns_resolved_verify_restore() { dmb_dns_state_equal dns_resolved "$1" "$2" before; }
dmb_dns_state_equal() {
  local adapter=$1 plan=$2 index=$3 key=$4 actual expected
  actual=$("dmb_${adapter}_capture" "$plan" "$index" | jq -ceS .) || return
  expected=$(jq -ceS --argjson i "$index" --arg key "$key" '.steps[$i][$key]' "$plan") || return
  [[ $actual == "$expected" ]]
}
dmb_dns_file_capture() {
  local step path state
  step=$(dmb_dns_step "$1" "$2") || return;dmb_dns_owner_check "$step" || return
  path=$(dmb_dns_file_path "$1" "$2") || return
  state=$(dmb_file_state "$path") || return
  jq -cnS --argjson state "$state" '{file:$state}'
}
dmb_dns_file_path() {
  local step path owner identity expected
  step=$(dmb_dns_step "$1" "$2") || return
  owner=$(jq -er .owner <<< "$step") || return
  identity=$(jq -er .identity <<< "$step") || return
  path=$(jq -er .file <<< "$step") || return
  case $owner in
    static) expected=$identity; [[ $expected == "$(dmb_dns_etc)/resolv.conf" ]] || return 78;;
    networkd)
      [[ $identity == /*.network && $identity != *'/../'* ]] || return 78
      expected="$(dmb_dns_etc)/systemd/network/$(basename "$identity").d/90-dmitbox-dns.conf"
      [[ -f $expected && ! -L $expected ]] && grep -qx '# Managed by dmitbox-local DNS' "$expected" || return 78;;
    *) return 78;;
  esac
  [[ $path == "$expected" ]] || { dmb_dns_error 'file target is outside the enrolled DNS owner'; return 78; }
  dmb_trusted_path "$path" || return
  printf '%s\n' "$path"
}
dmb_dns_file_snapshot() {
  local path
  path=$(dmb_dns_file_path "$1" "$2") || return
  dmb_file_capture "$path" "$3/snapshots" "dns-$2"
}
dmb_dns_file_apply() {
  local path mode content expected
  path=$(dmb_dns_file_path "$1" "$2") || return
  mode=$(stat -c '%a' -- "$path") || return
  content=$(jq -er --argjson i "$2" '.steps[$i].content_b64' "$1") || return
  expected=$(jq -er --argjson i "$2" '.steps[$i].after.file.sha256' "$1") || return
  printf '%s' "$content" | base64 -d | dmb_atomic_write "$path" "$mode" "$expected" "$(jq -c --argjson i "$2" '.steps[$i].after.file' "$1")"
}
dmb_dns_file_restore() {
  local path hash
  path=$(dmb_dns_file_path "$1" "$2") || return
  hash=$(jq -cS --argjson i "$2" '.steps[$i].after.file' "$1" | sha256sum | awk '{print $1}') || return
  dmb_file_restore "$path" "$3/snapshots" "dns-$2" "$hash"
}
dmb_dns_file_verify() { dmb_dns_state_equal dns_file "$1" "$2" after; }
dmb_dns_file_verify_restore() { dmb_dns_state_equal dns_file "$1" "$2" before; }
dmb_dns_append_step() {
  local plan=$1 step=$2 before after adapter file content
  adapter=$(jq -er .adapter <<< "$step") || return
  plan=$(jq -c --argjson step "$step" '.steps=[$step]' <<< "$plan") || return
  before=$("dmb_${adapter}_capture" <(printf '%s\n' "$plan") 0) || return
  case $adapter in
    dns_nm) after=$(jq -c --argjson servers "$(jq -c .servers <<< "$step")" '.dns4=[$servers[]|select(contains(":")|not)]|.dns6=[$servers[]|select(contains(":"))]|.auto4="yes"|.auto6="yes"' <<< "$before") || return;;
    dns_resolved) after=$(jq -c --argjson servers "$(jq -c .servers <<< "$step")" '.dns=$servers' <<< "$before") || return;;
    dns_file)
      content=$(jq -er .content_b64 <<< "$step") || return
      after=$(jq -c --arg hash "$(printf '%s' "$content" | base64 -d | sha256sum | awk '{print $1}')" '.file.sha256=$hash' <<< "$before") || return;;
    *) return 2;;
  esac
  jq -c --argjson before "$before" --argjson after "$after" '.before=$before|.after=$after' <<< "$step"
}
dmb_dns_plan() {
  local iface=${1:-} provider=${2:-} scope=${3:-persistent} raw servers owner identity base step steps='[]' file content
  local -a dmb_dns_provider_values=()
  shift 3 || return 2
  dmb_need jq ip sha256sum stat base64 || return
  [[ $scope == persistent || $scope == runtime ]] || return 2
  if [[ $provider == custom ]]; then raw=$(dmb_dns_servers "$@") || return
  else (($#==0)) || return 2;raw=$(dmb_dns_provider "$provider") || return;mapfile -t dmb_dns_provider_values <<< "$raw";raw=$(dmb_dns_servers "${dmb_dns_provider_values[@]}") || return;fi
  servers=$(dmb_dns_lines_json <<< "$raw") || return
  owner=$(dmb_dns_owner "$iface") || return;identity=$(jq -r .identity <<< "$owner");owner=$(jq -r .owner <<< "$owner")
  base=$(jq -cn --arg iface "$iface" '{schema:1,adapter:"dns",resources:["dns:"+$iface],steps:[]}') || return
  if [[ $scope == persistent && $owner == nm ]]; then
    step=$(jq -cn --arg iface "$iface" --arg identity "$identity" --argjson servers "$servers" '{id:"dns-profile",adapter:"dns_nm",resource:("dns:"+$iface+":profile"),iface:$iface,owner:"nm",identity:$identity,servers:$servers}') || return
    step=$(dmb_dns_append_step "$base" "$step") || return;steps=$(jq -c --argjson step "$step" '.+[$step]' <<< "$steps") || return
  elif [[ $scope == persistent && $owner == static ]]; then
    file=$identity
    content=$(dmb_dns_content "$file" static "$iface" "$servers" | base64 -w0) || return
    step=$(jq -cn --arg iface "$iface" --arg identity "$identity" --arg file "$file" --arg content "$content" '{id:"dns-file",adapter:"dns_file",resource:("dns:"+$iface+":file"),iface:$iface,owner:"static",identity:$identity,file:$file,content_b64:$content}') || return
    step=$(dmb_dns_append_step "$base" "$step") || return;steps=$(jq -c --argjson step "$step" '.+[$step]' <<< "$steps") || return
    jq -c --argjson steps "$steps" '.steps=$steps|.resources=((.resources+[$steps[].resource])|unique)|.notes=["Persistent static resolver change. DNS search/domain/options preserved. No resolution probe has been run."]' <<< "$base";return
  elif [[ $scope == persistent && $owner == networkd ]]; then
    # Only an explicitly created DMITBox dropin is adopted. The tool does not
    # take ownership of a vendor or administrator .network file implicitly.
    file="$(dmb_dns_etc)/systemd/network/$(basename "$identity").d/90-dmitbox-dns.conf"
    [[ -f $file && ! -L $file ]] && grep -qx '# Managed by dmitbox-local DNS' "$file" || { dmb_dns_error 'create/enroll a DMITBox-owned networkd DNS dropin before persistent use';return 2; }
    content=$({ printf '# Managed by dmitbox-local DNS\n';dmb_dns_content "$file" networkd "$iface" "$servers"; } | base64 -w0) || return
    step=$(jq -cn --arg iface "$iface" --arg identity "$identity" --arg file "$file" --arg content "$content" '{id:"dns-file",adapter:"dns_file",resource:("dns:"+$iface+":file"),iface:$iface,owner:"networkd",identity:$identity,file:$file,content_b64:$content}') || return
    step=$(dmb_dns_append_step "$base" "$step") || return;steps=$(jq -c --argjson step "$step" '.+[$step]' <<< "$steps") || return
  elif [[ $scope == persistent ]]; then
    dmb_dns_error 'persistent DHCP DNS requires a tested native lease/configuration adapter; shared resolver is not overwritten';return 2
  fi
  [[ $owner == nm || $owner == networkd ]] || { dmb_dns_error 'runtime override requires an authoritative managed resolved link';return 2; }
  dmb_need resolvectl || return
  dmb_run 8 resolvectl status "$iface" >/dev/null || return
  # DNS-over-TLS strict mode can depend on authenticated server names. Plain
  # numeric replacements cannot preserve that contract across all NM versions.
  local tls_policy
  tls_policy=$(dmb_run 8 resolvectl dnsovertls "$iface") || return
  if [[ $tls_policy =~ :[[:space:]]+yes[[:space:]]*$ ]];then
    dmb_dns_error 'strict DoT server-name enrollment is required; numeric DNS replacement is refused rather than weakening TLS policy';return 2
  fi
  step=$(jq -cn --arg iface "$iface" --arg owner "$owner" --arg identity "$identity" --argjson servers "$servers" '{id:"dns-runtime",adapter:"dns_resolved",resource:("dns:"+$iface+":runtime"),iface:$iface,owner:$owner,identity:$identity,servers:$servers}') || return
  step=$(dmb_dns_append_step "$base" "$step") || return;steps=$(jq -c --argjson step "$step" '.+[$step]' <<< "$steps") || return
  jq -c --argjson steps "$steps" '.steps=$steps|.resources=((.resources+[$steps[].resource])|unique)|.notes=["DNS-only per-link override; search/routing domains, DNSSEC, DoT and default-route policy are preserved. Persistence is applied on the next native connection activation/reboot; no broad reapply or networkctl reload is issued. Resolution probes require an explicit command."]' <<< "$base"
}
dmb_dns_probe() {
  local name=${1:-} server=${2:-} type=${3:-A} result rc status answers verdict
  [[ $name =~ ^[A-Za-z0-9]([A-Za-z0-9.-]{0,251}[A-Za-z0-9])?$ && $name != *..* ]] || return 2
  [[ $type == A || $type == AAAA ]] || return 2
  [[ -z $server ]] || server=$(dmb_ip_parse "$server") || return 2
  dmb_need dig || return
  local -a args=(-r +time=4 +tries=1 +noall +comments +answer)
  [[ -z $server ]] || args+=("@$server")
  # Explicitly requested DNS query. -r ignores a user's .digrc; a blocked
  # response/timeout is distinguished from a received DNS error response.
  result=$(dmb_run 12 dig "${args[@]}" "$name" "$type");rc=$?
  status=$(sed -nE 's/.*status: ([A-Z0-9]+),.*/\1/p' <<< "$result")
  [[ $status != *$'\n'* ]] || return 3
  answers=$(awk '!/^;/ && NF>0 {print}' <<< "$result")
  case $status in
    NOERROR) if [[ -n $answers ]];then verdict=resolved;rc=0;else verdict=no-data;rc=1;fi;;
    NXDOMAIN) verdict=negative-answer;rc=1;;
    SERVFAIL|REFUSED|FORMERR|NOTIMP) verdict="dns-error-response";rc=1;;
    *) status=UNOBSERVED;verdict=inconclusive;rc=3;;
  esac
  jq -cn --arg name "$name" --arg server "$server" --arg type "$type" --arg status "$status" --arg verdict "$verdict" --arg answers "$answers" '{schema:1,name:$name,server:(if $server=="" then "system resolver" else $server end),query_type:$type,dns_status:$status,verdict:$verdict,answers:$answers,scope:"Explicit probe only; this does not prove application resolution or external SSH accessibility"}'
  return "$rc"
}
dmb_dns_enroll() {
  local iface=${1:-} owner identity state='null'
  dmb_dns_iface_valid "$iface" || return 2
  owner=$(dmb_dns_owner "$iface" candidate) || return
  identity=$(jq -er .identity <<< "$owner") || return
  if [[ $(jq -r .owner <<< "$owner") == static ]];then state=$(dmb_file_state "$identity") || return;fi
  jq -cnS --arg iface "$iface" --argjson owner "$owner" --argjson state "$state" '$owner+{schema:1,iface:$iface,enrolled_initial_state:$state,scope:"DNS fields only; no implied ownership of routes, search domains or resolver security policy"}'
}
dmb_dns_restore_plan() {
  local iface=${1:-} root tx match='' candidate state count=0 plan step actual after old newsteps='[]' i adapter content
  dmb_dns_iface_valid "$iface" || return 2
  root=$(dmb_state_root) || return
  # No timestamp ordering: clock changes must not select a historical owner.
  # Require exactly one still-current committed transaction for this interface.
  for tx in "$root/transactions/"tx-*;do
    [[ -d $tx && ! -L $tx && -f $tx/state.json && -f $tx/plan.json ]] || continue
    dmb_trusted_path "$tx" || return
    state=$(jq -r '.state' "$tx/state.json") || return
    [[ $state == COMMITTED ]] || continue
    jq -e --arg iface "$iface" '.steps|any(.[]; .iface==$iface and (.adapter=="dns_nm" or .adapter=="dns_resolved" or .adapter=="dns_file"))' "$tx/plan.json" >/dev/null || continue
    plan=$(cat "$tx/plan.json") || return
    candidate=true
    while read -r i;do
      adapter=$(jq -r --argjson i "$i" '.steps[$i].adapter' <<< "$plan") || return
      actual=$("dmb_${adapter}_capture" "$tx/plan.json" "$i" | jq -cS .) || { candidate=false;break; }
      after=$(jq -cS --argjson i "$i" '.steps[$i].after' <<< "$plan") || return
      [[ $actual == "$after" ]] || { candidate=false;break; }
    done < <(jq -r --arg iface "$iface" '.steps|to_entries[]|select(.value.iface==$iface)|.key' <<< "$plan")
    [[ $candidate == true ]] || continue
    count=$((count+1));match=$tx
  done
  ((count==1)) || { dmb_dns_error 'restore requires exactly one committed DNS transaction whose owned state is still current; select an explicit historical transaction after review';return 2; }
  plan=$(cat "$match/plan.json") || return
  while read -r i;do
    step=$(jq -ce --argjson i "$i" '.steps[$i]' <<< "$plan") || return
    adapter=$(jq -er .adapter <<< "$step") || return
    old=$(jq -c .before <<< "$step") || return
    if [[ $adapter == dns_file ]];then
      [[ -f $match/snapshots/dns-$i.raw && ! -L $match/snapshots/dns-$i.raw ]] || return 2
      # Validate raw metadata, not just its bytes, before constructing inverse.
      [[ $(dmb_file_state "$match/snapshots/dns-$i.raw" | jq -cS .) == "$(jq -cS .file <<< "$old")" ]] || return 2
      content=$(base64 -w0 "$match/snapshots/dns-$i.raw") || return
      step=$(jq -c --arg content "$content" '.content_b64=$content' <<< "$step") || return
    fi
    step=$(jq -c --argjson old "$old" '.id=("restore-"+.id)|.before=.after|.after=$old' <<< "$step") || return
    newsteps=$(jq -c --argjson step "$step" '.+[$step]' <<< "$newsteps") || return
  done < <(jq -r --arg iface "$iface" '.steps|to_entries|reverse[]|select(.value.iface==$iface)|.key' <<< "$plan")
  jq -cn --arg iface "$iface" --arg source "${match##*/}" --argjson steps "$newsteps" '{schema:1,adapter:"dns",resources:((["dns:"+$iface]+[$steps[].resource])|unique),steps:$steps,restores_transaction:$source}'
}
