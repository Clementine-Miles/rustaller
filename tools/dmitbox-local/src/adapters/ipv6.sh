# Static IPv6 primitives use separate address, route and sysctl steps. Dynamic
# DHCP/RA and unverified native configuration owners are refused before mutation.
dmb_ipv6_error() { printf 'ipv6: %s\n' "$*" >&2; return 2; }
dmb_ipv6_inventory() {
  local iface=${1:-} link addresses routes ipv6_values='{}' key value
  dmb_route_iface_valid "$iface" && [[ $iface != all && $iface != default && $iface != lo ]] || return 2
  link=$(dmb_route_link "$iface" | jq -c '{ifindex,ifname,address,kind:(.linkinfo.info_kind//"ether"),master:(.master//null)}') || return
  addresses=$(dmb_run 5 ip -j -6 address show dev "$iface" | jq -c '[.[].addr_info[]?|select(.family=="inet6")|del(.valid_life_time,.preferred_life_time)]') || return
  routes=$(dmb_run 5 ip -j -6 route show table all dev "$iface" | jq -c 'map(del(.expires,.lastuse,.used,.cache))|sort_by([(.table//254|tostring),.dst,(.metric//0)])') || return
  for key in disable_ipv6 autoconf accept_ra addr_gen_mode;do
    [[ -f /proc/sys/net/ipv6/conf/$iface/$key && ! -L /proc/sys/net/ipv6/conf/$iface/$key ]] || return 2
    IFS= read -r value < "/proc/sys/net/ipv6/conf/$iface/$key" || return
    [[ $value =~ ^[0-9]+$ ]] || return 2
    ipv6_values=$(jq -c --arg key "$key" --argjson value "$value" '.[$key]=$value' <<< "$ipv6_values") || return
  done
  jq -cnS --argjson link "$link" --argjson addresses "$addresses" --argjson routes "$routes" --argjson sysctl "$ipv6_values" '{link:$link,addresses:$addresses,routes:$routes,sysctl:$sysctl}'
}
dmb_ipv6_unmanaged() {
  local iface=$1 json file managed comm path
  dmb_route_iface_valid "$iface" && [[ $iface != all && $iface != default && $iface != lo ]] || return 2
  if command -v nmcli >/dev/null 2>&1;then
    managed=$(dmb_run 5 nmcli -g GENERAL.NM-MANAGED device show "$iface" 2>/dev/null) || return 2
    [[ $managed == no ]] || { dmb_ipv6_error 'NetworkManager owns or ambiguously manages this link';return 2; }
  fi
  if command -v networkctl >/dev/null 2>&1;then
    json=$(dmb_run 5 networkctl --json=short status "$iface" 2>/dev/null) || return 2
    file=$(jq -r '.NetworkFile//.NetworkFilePath//empty' <<< "$json") || return
    [[ -z $file ]] || { dmb_ipv6_error 'networkd owns this link';return 2; }
  fi
  if [[ -e /etc/network/interfaces ]] && grep -Eq "^[[:space:]]*iface[[:space:]]+${iface}[[:space:]]" /etc/network/interfaces;then
    dmb_ipv6_error 'ifupdown link requires its native recovery adapter';return 2
  fi
  # Unknown DHCP clients are not treated as static ownership. A client on a
  # different link may cause a conservative refusal; no interface is guessed.
  for path in /proc/[0-9]*/comm;do
    [[ -r $path ]] || continue
    IFS= read -r comm < "$path" || continue
    case $comm in dhclient|dhcpcd|udhcpc|odhcp6c|connmand|wickedd*) dmb_ipv6_error 'active DHCP/connection manager prevents unmanaged ownership proof';return 2;;esac
  done
}
dmb_ipv6_capture() {
  local iface
  iface=$(jq -er --argjson i "$2" '.steps[$i].iface' "$1") || return
  dmb_ipv6_unmanaged "$iface" || return
  dmb_ipv6_inventory "$iface"
}
dmb_ipv6_plan() {
  local mode=${1:-} context=${2:-'{}'} iface before after value selected
  [[ $mode == enable || $mode == disable ]] || return 2
  jq -e 'type=="object" and ((keys-["iface","transaction_id"]|length)==0) and (.iface|type=="string") and (.transaction_id==null or (.transaction_id|type=="string"))' <<< "$context" >/dev/null || { dmb_ipv6_error 'select one explicit interface with {"iface":"name"}; global/default-route selection is forbidden';return 2; }
  iface=$(jq -er .iface <<< "$context") || return
  dmb_ipv6_unmanaged "$iface" || return
  if [[ -f $DMB_STATE_DIR/enrollments/ipv6-$iface.json ]];then
    selected=$(jq -r '.transaction_id//empty' <<< "$context") || return
    if [[ $mode == enable && $(cat "/proc/sys/net/ipv6/conf/$iface/disable_ipv6") == 1 ]];then dmb_ipv6_static_restore_plan "$iface" "$selected";else dmb_ipv6_static_plan "$mode" "$iface";fi;return
  fi
  before=$(dmb_ipv6_inventory "$iface") || return
  # Disabling IPv6 destroys addresses/routes. Only an already-empty unmanaged
  # link with all automatic address mechanisms disabled has a bounded inverse.
  jq -e '.addresses==[] and .routes==[] and .sysctl.autoconf==0 and .sysctl.accept_ra==0 and .sysctl.addr_gen_mode==1' <<< "$before" >/dev/null || { dmb_ipv6_error 'addressed, routed, RA, DHCP or automatic link-local interfaces are unsupported; sysctl-only restoration would lose state';return 2; }
  [[ $mode == disable ]] && value=1 || value=0
  after=$(jq -c --argjson value "$value" '.sysctl.disable_ipv6=$value' <<< "$before") || return
  jq -cn --arg iface "$iface" --argjson before "$before" --argjson after "$after" '{schema:1,adapter:"ipv6",resources:["link:"+$iface,"ipv6:"+$iface],steps:[{id:"ipv6-toggle",resource:("ipv6:"+$iface),iface:$iface,before:$before,after:$after}],notes:["Only an empty explicitly selected unmanaged interface is supported. Addressed and dynamically configured interfaces are refused before mutation."]}'
}
dmb_ipv6_set() {
  local plan=$1 index=$2 key=$3 iface value expected current
  iface=$(jq -er --argjson i "$index" '.steps[$i].iface' "$plan") || return
  expected=$(jq -cS --argjson i "$index" --arg key "$key" 'if $key=="after" then .steps[$i].before else .steps[$i].after end' "$plan") || return
  current=$(dmb_ipv6_capture "$plan" "$index" | jq -cS .) || return
  [[ $current == "$expected" ]] || return 76
  value=$(jq -er --argjson i "$index" --arg key "$key" '.steps[$i][$key].sysctl.disable_ipv6' "$plan") || return
  [[ $value == 0 || $value == 1 ]] || return 2
  dmb_trusted_path "/proc/sys/net/ipv6/conf/$iface/disable_ipv6" || return
  printf '%s\n' "$value" > "/proc/sys/net/ipv6/conf/$iface/disable_ipv6"
}
dmb_ipv6_apply() { dmb_ipv6_set "$1" "$2" after; }
dmb_ipv6_restore() {
  local actual before
  actual=$(dmb_ipv6_capture "$1" "$2" | jq -cS .) || return
  before=$(jq -cS --argjson i "$2" '.steps[$i].before' "$1") || return
  [[ $actual != "$before" ]] || return 0
  dmb_ipv6_set "$1" "$2" before
}
dmb_ipv6_state_equal() {
  local actual expected
  actual=$(dmb_ipv6_capture "$1" "$2" | jq -cS .) || return
  expected=$(jq -cS --argjson i "$2" --arg key "$3" '.steps[$i][$key]' "$1") || return
  [[ $actual == "$expected" ]]
}
dmb_ipv6_verify() { dmb_ipv6_state_equal "$1" "$2" after; }
dmb_ipv6_verify_restore() { dmb_ipv6_state_equal "$1" "$2" before; }

# Granular static-address primitives. A pending transaction owns every affected
# address/route individually; an interrupted inverse retries only missing items.
dmb_ipv6_static_validate() {
  local state=$1
  jq -e '
    .sysctl.autoconf==0 and .sysctl.accept_ra==0 and
    .link.master==null and
    all(.addresses[]; .valid_life_time==4294967295 and .preferred_life_time==4294967295 and
      (.scope=="global" or .scope=="link") and
      ((keys-["family","local","prefixlen","scope","valid_life_time","preferred_life_time","flags","noprefixroute","protocol"]|length)==0) and
      ((.flags//[])|all(.=="permanent" or .=="noprefixroute")) and
      (.protocol==null or .protocol=="kernel_ll" or .protocol=="static")) and
    all(.routes[]; ((keys-["dst","dev","gateway","table","protocol","metric","flags","pref","type","prefsrc"]|length)==0) and
      ((.flags//[])|length==0) and (.pref//"medium")=="medium" and
      (if .protocol=="kernel" then
        ((.type=="local" and .table=="local" and .metric==0) or
         (.type=="multicast" and .dst=="ff00::/8" and .table=="local" and .metric==256) or
         (.type==null and .table==null and .metric==256 and .gateway==null))
       else (.protocol=="static" or .protocol=="boot") and (.type==null or .type=="unicast") end))
  ' <<< "$state" >/dev/null
}
dmb_ipv6_inventory_full() {
  local iface=$1 base addresses
  base=$(dmb_ipv6_inventory "$iface") || return
  addresses=$(dmb_run 5 ip -j -6 address show dev "$iface" | jq -c '[.[].addr_info[]?|select(.family=="inet6")]') || return
  jq -c --argjson addresses "$addresses" '.addresses=$addresses' <<< "$base"
}
dmb_ipv6_enroll() {
  local iface=${1:-} state boot
  dmb_ipv6_unmanaged "$iface" || return
  state=$(dmb_ipv6_inventory_full "$iface") || return
  dmb_ipv6_static_validate "$state" || { dmb_ipv6_error 'only forever static addresses, ordinary kernel-derived routes and simple static routes can be enrolled';return 2; }
  boot=$(cat /proc/sys/kernel/random/boot_id) || return
  jq -cnS --arg iface "$iface" --arg boot "$boot" --argjson state "$state" '{schema:1,iface:$iface,owner:"unmanaged-static",enrolled_boot_id:$boot,identity:$state.link,initial_state:$state}'
}
dmb_ipv6_enrollment() {
  local iface=$1 manifest link expected
  manifest="$DMB_STATE_DIR/enrollments/ipv6-$iface.json"
  dmb_trusted_path "$manifest" || return
  jq -e --arg iface "$iface" '.schema==1 and .iface==$iface and .owner=="unmanaged-static"' "$manifest" >/dev/null || return
  link=$(dmb_route_link "$iface" | jq -cS '{ifindex,ifname,address,kind:(.linkinfo.info_kind//"ether"),master:(.master//null)}') || return
  expected=$(jq -cS '.identity' "$manifest") || return
  [[ $link == "$expected" ]] || { dmb_ipv6_error 'enrolled interface identity changed';return 76; }
}
dmb_ipv6_static_capture() {
  local plan=$1 index=$2 step iface kind state value selector link
  step=$(jq -ce --argjson i "$index" '.steps[$i]' "$plan") || return
  iface=$(jq -er .iface <<< "$step");kind=$(jq -er .kind <<< "$step") || return
  dmb_ipv6_unmanaged "$iface" && dmb_ipv6_enrollment "$iface" || return
  state=$(dmb_ipv6_inventory_full "$iface") || return
  link=$(jq -c .link <<< "$state") || return
  case $kind in
    sysctl)
      selector=$(jq -er .key <<< "$step") || return
      case $selector in addr_gen_mode|disable_ipv6) ;;*)return 2;;esac
      value=$(jq -ce --arg key "$selector" '.sysctl[$key]' <<< "$state") || return;;
    address)
      selector=$(jq -ce .selector <<< "$step") || return
      value=$(jq -c --argjson s "$selector" '[.addresses[]|select(.local==$s.local and .prefixlen==$s.prefixlen)|del(.tentative,.dadfailed)]|if length<=1 then . else error("duplicate address") end' <<< "$state") || return;;
    route)
      selector=$(jq -ce .selector <<< "$step") || return
      value=$(jq -c --argjson s "$selector" '[.routes[]|select(.dst==$s.dst and (.table//254)==($s.table//254) and (.metric//0)==($s.metric//0))]|if length<=1 then . else error("ambiguous route selector") end' <<< "$state") || return;;
    *) return 2;;
  esac
  jq -cnS --argjson link "$link" --argjson value "$value" '{link:$link,value:$value}'
}
dmb_ipv6_static_ip_address() {
  local iface=$1 operation=$2 address=$3 localip prefix scope canonical flag
  localip=$(jq -er .local <<< "$address");prefix=$(jq -er .prefixlen <<< "$address");scope=$(jq -er .scope <<< "$address") || return
  canonical=$(dmb_ip_parse "$localip" 6) || return
  [[ $prefix =~ ^[0-9]{1,3}$ ]] && ((prefix<=128)) && [[ $scope == link || $scope == global ]] || return 2
  local -a args=(-6 address "$operation" "$canonical/$prefix" dev "$iface")
  if [[ $operation == add ]];then
    args+=(scope "$scope" valid_lft forever preferred_lft forever)
    flag=$(jq -r '.protocol//empty' <<< "$address");[[ -z $flag ]] || args+=(proto "$flag")
    if jq -e '.noprefixroute==true or ((.flags//[])|index("noprefixroute")!=null)' <<< "$address" >/dev/null;then args+=(noprefixroute);fi
  fi
  dmb_run 10 ip "${args[@]}"
}
dmb_ipv6_static_ip_route() {
  local iface=$1 operation=$2 route=$3 value key dst table
  dst=$(jq -er .dst <<< "$route") || return
  [[ $dst == default ]] || dmb_cidr_parse "$dst" 6 >/dev/null || return 2
  local -a args=(-6 route "$operation" "$dst")
  table=$(jq -r '.table//254' <<< "$route")
  [[ $table =~ ^[1-9][0-9]{0,9}$ || $table == main ]] || return 2
  args+=(table "$table")
  value=$(jq -r '.gateway//empty' <<< "$route")
  if [[ -n $value ]];then value=$(dmb_ip_parse "$value" 6) || return;args+=(via "$value");fi
  args+=(dev "$iface")
  for key in metric protocol prefsrc;do
    value=$(jq -r --arg key "$key" '.[$key]//empty' <<< "$route")
    [[ -n $value ]] || continue
    case $key in
      metric) [[ $value =~ ^[0-9]{1,10}$ ]] || return 2;args+=(metric "$value");;
      protocol) [[ $value == static || $value == boot ]] || return 2;args+=(proto "$value");;
      prefsrc) value=$(dmb_ip_parse "$value" 6) || return;args+=(src "$value");;
    esac
  done
  dmb_run 10 ip "${args[@]}"
}
dmb_ipv6_static_set() {
  local plan=$1 index=$2 target=$3 step iface kind current expected value opposite key item
  step=$(jq -ce --argjson i "$index" '.steps[$i]' "$plan") || return
  iface=$(jq -er .iface <<< "$step");kind=$(jq -er .kind <<< "$step") || return
  current=$(dmb_ipv6_static_capture "$plan" "$index" | jq -cS .) || return
  value=$(jq -ce --arg key "$target" '.[$key].value' <<< "$step") || return
  expected=$(jq -cS --arg key "$target" '.[$key]' <<< "$step") || return
  [[ $current != "$expected" ]] || return 0
  [[ $target == after ]] && opposite=before || opposite=after
  [[ $current == "$(jq -cS --arg key "$opposite" '.[$key]' <<< "$step")" ]] || return 76
  case $kind in
    sysctl)
      key=$(jq -er .key <<< "$step") || return
      case $key in addr_gen_mode) [[ $value =~ ^[0-3]$ ]] || return 2;;disable_ipv6) [[ $value == 0 || $value == 1 ]] || return 2;;*)return 2;;esac
      dmb_trusted_path "/proc/sys/net/ipv6/conf/$iface/$key" || return
      # disable_ipv6 is last after all addressed resources were withdrawn.
      if [[ $key == disable_ipv6 && $value == 1 ]];then
        dmb_ipv6_inventory_full "$iface" | jq -e '.addresses==[] and all(.routes[];.protocol=="kernel" and .type=="multicast")' >/dev/null || return 76
      fi
      printf '%s\n' "$value" > "/proc/sys/net/ipv6/conf/$iface/$key";;
    address)
      if [[ $value == '[]' ]];then
        item=$(jq -c --arg key "$opposite" '.[$key].value[0]' <<< "$step") || return
        dmb_ipv6_static_ip_address "$iface" del "$item"
      else
        item=$(jq -c '.[0]' <<< "$value") || return
        dmb_ipv6_static_ip_address "$iface" add "$item"
      fi;;
    route)
      if [[ $value == '[]' ]];then
        item=$(jq -c --arg key "$opposite" '.[$key].value[0]' <<< "$step") || return
        dmb_ipv6_static_ip_route "$iface" del "$item"
      else item=$(jq -c '.[0]' <<< "$value") || return;dmb_ipv6_static_ip_route "$iface" add "$item";fi;;
    *)return 2;;
  esac
}
dmb_ipv6_static_apply() { dmb_ipv6_static_set "$1" "$2" after && dmb_ipv6_static_equal "$1" "$2" after; }
dmb_ipv6_static_restore() { dmb_ipv6_static_set "$1" "$2" before && dmb_ipv6_static_equal "$1" "$2" before; }
dmb_ipv6_static_equal() {
  local actual expected kind iface i tries=0 target=$3
  expected=$(jq -cS --argjson i "$2" --arg key "$target" '.steps[$i][$key]' "$1") || return
  # DAD can take time. A DAD failure or unexpected generated state never counts
  # as restoration; no nodad bypass is applied.
  while ((tries<12));do
    actual=$(dmb_ipv6_static_capture "$1" "$2" | jq -cS .) || return
    [[ $actual != "$expected" ]] || break
    tries=$((tries+1));sleep 0.2
  done
  [[ $actual == "$expected" ]] || return 1
  kind=$(jq -r --argjson i "$2" '.steps[$i].kind' "$1") || return
  if [[ $kind == address && $(jq -c .value <<< "$expected") != '[]' ]];then
    iface=$(jq -r --argjson i "$2" '.steps[$i].iface' "$1") || return
    local selector observed tentative
    selector=$(jq -c --argjson i "$2" '.steps[$i].selector' "$1") || return
    tries=0
    while ((tries<12));do
      observed=$(dmb_ipv6_inventory_full "$iface" | jq -ce --argjson s "$selector" '.addresses[]|select(.local==$s.local and .prefixlen==$s.prefixlen)') || return
      jq -e '.dadfailed!=true' <<< "$observed" >/dev/null || return 1
      tentative=$(jq -r '.tentative//false' <<< "$observed") || return
      [[ $tentative == true ]] || break
      tries=$((tries+1));sleep 0.2
    done
    [[ $tentative != true ]] || return 1
  fi
  i=$(jq -r --argjson i "$2" '.steps[$i].id' "$1") || return
  if [[ $target == after && $i == restore-ipv6-autogen ]];then
    iface=$(jq -r --argjson i "$2" '.steps[$i].iface' "$1") || return
    actual=$(dmb_ipv6_inventory_full "$iface" | jq -cS .) || return
    expected=$(jq -cS '.restore_ipv6_state' "$1") || return
    [[ $actual == "$expected" ]] || return 1
  fi
  if [[ $target == before && $i == restore-ipv6-disable ]];then
    iface=$(jq -r --argjson i "$2" '.steps[$i].iface' "$1") || return
    actual=$(dmb_ipv6_inventory_full "$iface" | jq -cS .) || return
    expected=$(jq -cS '.initial_ipv6_state' "$1") || return
    [[ $actual == "$expected" ]] || return 1
  fi
  if [[ $target == before && $i == ipv6-autogen ]];then
    iface=$(jq -r --argjson i "$2" '.steps[$i].iface' "$1") || return
    actual=$(dmb_ipv6_inventory_full "$iface" | jq -cS .) || return
    expected=$(jq -cS '.initial_ipv6_state' "$1") || return
    [[ $actual == "$expected" ]] || return 1
  fi
}
dmb_ipv6_static_verify() { dmb_ipv6_static_equal "$1" "$2" after; }
dmb_ipv6_static_verify_restore() { dmb_ipv6_static_equal "$1" "$2" before; }
dmb_ipv6_static_step() {
  local iface=$1 id=$2 kind=$3 key=$4 value=$5 desired=$6 link=$7 selector=${8:-null}
  jq -cn --arg iface "$iface" --arg id "$id" --arg kind "$kind" --arg key "$key" --argjson before "$value" --argjson after "$desired" --argjson link "$link" --argjson selector "$selector" '{id:$id,adapter:"ipv6_static",resource:("ipv6:"+$iface+":"+$id),iface:$iface,kind:$kind,key:$key,selector:$selector,before:{link:$link,value:$before},after:{link:$link,value:$after}}'
}
dmb_ipv6_static_plan() {
  local mode=$1 iface=$2 state link steps='[]' step item id count=0 value
  dmb_ipv6_unmanaged "$iface" && dmb_ipv6_enrollment "$iface" || return
  state=$(dmb_ipv6_inventory_full "$iface") || return
  dmb_ipv6_static_validate "$state" || { dmb_ipv6_error 'static interface state changed to unsupported ownership, lifetime or route shape';return 2; }
  if [[ $mode == enable || $(jq -r .sysctl.disable_ipv6 <<< "$state") == 1 ]];then
    link=$(jq -c .link <<< "$state") || return
    value=$(jq -r .sysctl.disable_ipv6 <<< "$state") || return
    step=$(dmb_ipv6_static_step "$iface" ipv6-noop sysctl disable_ipv6 "$value" "$value" "$link") || return
    jq -cn --arg iface "$iface" --argjson step "$step" --argjson initial "$state" '{schema:1,adapter:"ipv6_static",operation:"ipv6-idempotent",resources:["link:"+$iface,"ipv6:"+$iface,$step.resource],steps:[$step],initial_ipv6_state:$initial}'
    return
  fi
  link=$(jq -c .link <<< "$state") || return
  value=$(jq -r .sysctl.addr_gen_mode <<< "$state") || return
  step=$(dmb_ipv6_static_step "$iface" ipv6-autogen sysctl addr_gen_mode "$value" 1 "$link") || return
  steps=$(jq -c --argjson step "$step" '.+[$step]' <<< "$steps") || return
  while IFS= read -r item;do
    count=$((count+1));id="route-$count"
    step=$(dmb_ipv6_static_step "$iface" "$id" route '' "[$item]" '[]' "$link" "$item") || return
    steps=$(jq -c --argjson step "$step" '.+[$step]' <<< "$steps") || return
  done < <(jq -c '.routes|reverse[]|select(.protocol!="kernel")' <<< "$state")
  while IFS= read -r item;do
    count=$((count+1));id="address-$count"
    step=$(dmb_ipv6_static_step "$iface" "$id" address '' "[$item]" '[]' "$link" "$item") || return
    steps=$(jq -c --argjson step "$step" '.+[$step]' <<< "$steps") || return
  done < <(jq -c '.addresses[]' <<< "$state")
  step=$(dmb_ipv6_static_step "$iface" ipv6-disable sysctl disable_ipv6 0 1 "$link") || return
  steps=$(jq -c --argjson step "$step" '.+[$step]' <<< "$steps") || return
  ((count<=55)) || { dmb_ipv6_error 'static interface has too many independently owned resources for one bounded transaction';return 2; }
  jq -cn --arg iface "$iface" --argjson steps "$steps" --argjson initial "$state" '{schema:1,adapter:"ipv6_static",operation:"ipv6-disable",resources:((["link:"+$iface,"ipv6:"+$iface]+[$steps[].resource])|unique),steps:$steps,initial_ipv6_state:$initial,notes:["Explicit enrolled unmanaged static addresses/routes. Automatic configuration is refused. Rollback waits for DAD and compares the full original inventory; reboot interface identity changes require conflict review."]}'
}
dmb_ipv6_static_restore_plan() {
  local iface=$1 selected=${2:-} root tx candidate match='' count=0 i current expected plan steps initial
  root=$(dmb_state_root) || return
  [[ -z $selected || $selected =~ ^tx-[a-f0-9]{32}$ ]] || return 2
  for tx in "$root/transactions/"tx-*;do
    [[ -z $selected || ${tx##*/} == "$selected" ]] || continue
    [[ -d $tx && ! -L $tx && -f $tx/plan.json && -f $tx/state.json ]] || continue
    dmb_trusted_path "$tx" || return
    jq -e '.state=="COMMITTED"' "$tx/state.json" >/dev/null || continue
    jq -e --arg iface "$iface" '.operation=="ipv6-disable" and all(.steps[];.iface==$iface and .adapter=="ipv6_static")' "$tx/plan.json" >/dev/null || continue
    candidate=true
    while IFS= read -r i;do
      current=$(dmb_ipv6_static_capture "$tx/plan.json" "$i" | jq -cS .) || { candidate=false;break; }
      expected=$(jq -cS --argjson i "$i" '.steps[$i].after' "$tx/plan.json") || return
      [[ $current == "$expected" ]] || { candidate=false;break; }
    done < <(jq -r '.steps|keys[]' "$tx/plan.json")
    [[ $candidate == true ]] || continue
    count=$((count+1));match=$tx
  done
  ((count==1)) || { dmb_ipv6_error 'enable requires one still-current committed static-disable snapshot; specify transaction_id when historical ownership is ambiguous';return 2; }
  plan=$(cat "$match/plan.json") || return
  initial=$(dmb_ipv6_inventory_full "$iface") || return
  steps=$(jq -c '.steps|reverse|map(. as $s|.id=("restore-"+.id)|.before=$s.after|.after=$s.before)' <<< "$plan") || return
  jq -c --argjson steps "$steps" --argjson initial "$initial" --arg tx "${match##*/}" '.steps=$steps|.operation="ipv6-enable"|.restore_ipv6_state=.initial_ipv6_state|.initial_ipv6_state=$initial|.restores_transaction=$tx' <<< "$plan"
}
