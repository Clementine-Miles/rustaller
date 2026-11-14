# Local routing inventory and narrowly scoped runtime link mutations.
# No route is selected using an unsolicited public probe.
dmb_route_error() { printf 'routing: %s\n' "$*" >&2; return 2; }
dmb_route_iface_valid() { [[ ${1:-} =~ ^[[:alnum:]_.:-]{1,15}$ && $1 != -* ]]; }
dmb_route_link() {
  dmb_route_iface_valid "$1" || return 2
  dmb_run 5 ip -j -d link show dev "$1" | jq -ce 'if length==1 then .[0] else error("ambiguous interface") end'
}
dmb_route_report() {
  local context=${1:-'{}'} links addresses r4 r6 u4 u6
  jq -e 'type=="object"' <<< "$context" >/dev/null || return 2
  dmb_need ip jq || return
  links=$(dmb_run 8 ip -j -d link show) || return
  addresses=$(dmb_run 8 ip -j address show) || return
  r4=$(dmb_run 8 ip -j -4 route show table all) || return
  r6=$(dmb_run 8 ip -j -6 route show table all) || return
  u4=$(dmb_run 8 ip -j -4 rule show) || return
  u6=$(dmb_run 8 ip -j -6 rule show) || return
  jq -cn --argjson links "$links" --argjson addresses "$addresses" --argjson r4 "$r4" --argjson r6 "$r6" --argjson u4 "$u4" --argjson u6 "$u6" --argjson context "$context" '{schema:1,context:$context,links:$links,addresses:$addresses,routes:{ipv4:$r4,ipv6:$r6},rules:{ipv4:$u4,ipv6:$u6},network_probes:false}'
}
dmb_route_context_valid() {
  local context=$1 value family
  jq -e 'type=="object" and ((keys - ["family","source","peer","table","vrf","mark","encapsulation_overhead","underlay_mtu"])|length==0) and ((.family//4)==4 or .family==6) and ((.table//254)|type=="number" and floor==. and .>=1 and .<=4294967295) and ((.mark//0)|type=="number" and floor==. and .>=0 and .<=4294967295) and ((.encapsulation_overhead//0)|type=="number" and floor==. and .>=0 and .<=65535) and (.underlay_mtu==null or (.underlay_mtu|type=="number" and floor==. and .>=68 and .<=65535))' <<< "$context" >/dev/null || return 2
  family=$(jq -r '.family//4' <<< "$context")
  for value in source peer; do
    value=$(jq -r --arg key "$value" '.[$key]//empty' <<< "$context")
    [[ -z $value ]] || dmb_ip_parse "$value" "$family" >/dev/null || return 2
  done
  value=$(jq -r '.vrf//empty' <<< "$context")
  [[ -z $value ]] || dmb_route_iface_valid "$value"
}
dmb_mtu_capture() {
  local plan=$1 index=$2 iface link enabled ns
  iface=$(jq -er --argjson i "$index" '.steps[$i].iface' "$plan") || return
  link=$(dmb_route_link "$iface") || return
  enabled=$(cat "/proc/sys/net/ipv6/conf/$iface/disable_ipv6" 2>/dev/null) || enabled=1
  ns=$(stat -Lc '%i' /proc/self/ns/net) || return
  jq -cS --arg ns "$ns" --arg disabled "$enabled" '{ifindex,ifname,mtu,address,kind:(.linkinfo.info_kind//"ether"),master:(.master//null),link_index:(.link_index//null),netns_inode:$ns,ipv6_disabled:$disabled}' <<< "$link"
}
dmb_mtu_plan() {
  local iface=${1:-} mtu=${2:-} scope=${3:-runtime} context=${4:-'{}'} before after plan kind overhead max enabled underlay parent_index parent_mtu
  dmb_need jq ip stat || return
  dmb_route_iface_valid "$iface" && [[ $mtu =~ ^[0-9]{2,5}$ ]] && ((10#$mtu>=68 && 10#$mtu<=65535)) || return 2
  [[ $scope == runtime ]] || { dmb_route_error 'persistent MTU requires a dedicated configuration-owner adapter';return 2; }
  dmb_route_context_valid "$context" || return 2
  plan=$(jq -cn --arg iface "$iface" --argjson mtu "$((10#$mtu))" --argjson context "$context" '{schema:1,adapter:"mtu",resources:["link:"+$iface],steps:[{id:"mtu",resource:("link:"+$iface),iface:$iface,mtu:$mtu,context:$context,after:{}}]}') || return
  before=$(dmb_mtu_capture <(printf '%s\n' "$plan") 0) || return
  kind=$(jq -r .kind <<< "$before");enabled=$(jq -r .ipv6_disabled <<< "$before")
  [[ $enabled == 1 ]] || ((10#$mtu>=1280)) || { dmb_route_error 'IPv6 enabled: MTU must be at least 1280';return 2; }
  # Unknown tunnel layering is not guessed from a single route.
  case $kind in ether|veth|bridge|bond|dummy) ;;
    *) overhead=$(jq -r '.encapsulation_overhead//0' <<< "$context");((overhead>0)) || { dmb_route_error 'explicit encapsulation_overhead required for this link kind';return 2; };;
  esac
  max=$(dmb_route_link "$iface" | jq -r 'if (.max_mtu//0)>0 then .max_mtu else 65535 end') || return
  overhead=$(jq -r '.encapsulation_overhead//0' <<< "$context") || return
  if ((overhead>0));then
    underlay=$(jq -r '.underlay_mtu//0' <<< "$context") || return
    parent_index=$(jq -r '.link_index//0' <<< "$before") || return
    if ((parent_index>0)) && [[ $kind != veth ]];then
      parent_mtu=$(dmb_run 5 ip -j link show | jq -er --argjson i "$parent_index" '.[]|select(.ifindex==$i)|.mtu') || return
      ((underlay>0 && underlay<parent_mtu)) || underlay=$parent_mtu
    fi
    ((underlay>overhead)) || { dmb_route_error 'provide the verified underlay_mtu for this encapsulation';return 2; }
    ((max<underlay-overhead)) || max=$((underlay-overhead))
  fi
  ((10#$mtu<=max)) || { dmb_route_error 'MTU exceeds the device/underlay limit after encapsulation overhead';return 2; }
  after=$(jq -c --argjson mtu "$((10#$mtu))" '.mtu=$mtu' <<< "$before") || return
  jq -c --argjson before "$before" --argjson after "$after" '.steps[0].before=$before|.steps[0].after=$after|.notes=["Runtime MTU only. Local readback cannot establish end-to-end PMTU; request explicit PMTU probes and confirm through a new connection."]' <<< "$plan"
}
dmb_mtu_apply() {
  local iface mtu context checked expected
  iface=$(jq -er --argjson i "$2" '.steps[$i].iface' "$1") || return
  mtu=$(jq -er --argjson i "$2" '.steps[$i].after.mtu' "$1") || return
  dmb_route_iface_valid "$iface" && [[ $mtu =~ ^[0-9]+$ ]] || return 2
  context=$(jq -ce --argjson i "$2" '.steps[$i].context // {}' "$1") || return
  # The public plan file is untrusted input. Repeat the same capability and
  # IPv6/encapsulation checks immediately before the netlink mutation.
  checked=$(dmb_mtu_plan "$iface" "$mtu" runtime "$context") || return
  expected=$(jq -cS --argjson i "$2" '.steps[$i].after' "$1") || return
  [[ $(jq -cS '.steps[0].after' <<< "$checked") == "$expected" ]] || return 76
  dmb_run 10 ip link set dev "$iface" mtu "$mtu"
}
dmb_mtu_restore() {
  local iface mtu
  iface=$(jq -er --argjson i "$2" '.steps[$i].iface' "$1") || return
  mtu=$(jq -er --argjson i "$2" '.steps[$i].before.mtu' "$1") || return
  dmb_route_iface_valid "$iface" && [[ $mtu =~ ^[0-9]+$ ]] || return 2
  dmb_run 10 ip link set dev "$iface" mtu "$mtu"
}
dmb_mtu_verify() { [[ $(dmb_mtu_capture "$1" "$2" | jq -Sc .) == "$(jq -Sc --argjson i "$2" '.steps[$i].after' "$1")" ]]; }
dmb_mtu_verify_restore() { [[ $(dmb_mtu_capture "$1" "$2" | jq -Sc .) == "$(jq -Sc --argjson i "$2" '.steps[$i].before' "$1")" ]]; }
dmb_iface_plan() {
  dmb_route_error 'link up/down can trigger DHCP/RA, remove routes, or destroy an SSH return path; no tested owner-specific recovery adapter is available'
}
dmb_pmtu_probe() {
  local target=${1:-} family=${2:-4} iface=${3:-} canonical rc
  [[ $family == 4 || $family == 6 ]] || return 2
  canonical=$(dmb_ip_parse "$target" "$family") || return 2
  [[ -z $iface ]] || dmb_route_iface_valid "$iface" || return 2
  dmb_need tracepath || return
  # This is an explicitly requested network probe. An ICMP filtering timeout is
  # inconclusive and never modifies MTU or commits a transaction.
  local -a args=("-$family" -n)
  [[ -z $iface ]] || { dmb_route_error 'tracepath cannot reliably bind this interface; use an explicit namespace/VRF tool';return 2; }
  dmb_run 25 tracepath "${args[@]}" "$canonical"; rc=$?
  ((rc==0)) || printf 'PMTU probe inconclusive (blocked responses or probe failure), exit=%s\n' "$rc" >&2
  return "$rc"
}
