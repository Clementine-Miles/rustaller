# Explicitly enrolled, permanent /128 members of a routed IPv6 /64. Runtime
# addresses only: no network-manager configuration is guessed or overwritten.
# External evidence is provided by the administrator's pinned verifier program;
# its honesty and separate-host placement are part of the enrollment trust model.
dmb_pool_error() { dmb_error "pool: $*"; return 69; }
dmb_pool_global() {
  local canonical words; local -a address_words
  canonical=$(dmb_ip_parse "$1" 6) || return
  words=$(dmb_ipv6_words "$canonical") || return
  IFS=' ' read -r -a address_words <<< "$words"
  ((address_words[0]>=0x2000 && address_words[0]<=0x3fff)) || return 64
  # Documentation, benchmarking and ORCHID prefixes are not delegated pools.
  ! dmb_ip_in_cidr "$canonical" 2001:db8::/32 || return 64
  ! dmb_ip_in_cidr "$canonical" 2001:2::/48 || return 64
  ! dmb_ip_in_cidr "$canonical" 2001:10::/28 || return 64
  ! dmb_ip_in_cidr "$canonical" 2001:20::/28 || return 64
}
dmb_pool_id() { [[ ${1-} =~ ^[A-Za-z0-9][A-Za-z0-9_-]{0,47}$ ]]; }
dmb_pool_manifest_path() { dmb_pool_id "$1" || return 64; printf '%s/enrollments/pool-%s.json\n' "$DMB_STATE_DIR" "$1"; }
dmb_pool_verifier_valid() {
  local config=$1 path expected actual source prefix
  jq -e '.verifier|type=="object" and (.path|type=="string" and test("^/[A-Za-z0-9_./-]+$")) and (.sha256|test("^[a-f0-9]{64}$")) and (.identity|type=="string" and test("^[A-Za-z0-9_.:-]{1,100}$")) and (.args|type=="array" and length<=32 and all(.[];type=="string" and length<=1024 and (explode|all(.[];.>=32 and .!=127)))) and (.source_address|type=="string")' <<< "$config" >/dev/null || return 64
  path=$(jq -er .verifier.path <<< "$config") || return
  expected=$(jq -er .verifier.sha256 <<< "$config") || return
  dmb_trusted_path "$path" || return
  [[ -f $path && -x $path && ! -L $path && $(stat -c %h -- "$path") == 1 ]] || return 78
  actual=$(sha256sum -- "$path") || return
  [[ ${actual%% *} == "$expected" ]] || return 76
  source=$(jq -er .verifier.source_address <<< "$config") || return
  source=$(dmb_ip_parse "$source" 6) || return
  dmb_pool_global "$source" || return
  prefix=$(jq -er .prefix <<< "$config") || return
  ! dmb_ip_in_cidr "$source" "$prefix" || return 64
}
dmb_pool_enroll() {
  local config=$1 prefix iface anchor source link addresses routes
  dmb_need jq ip sha256sum stat || return
  jq -e '.schema==1 and .prefix_authorized==true and (.owner_id|test("^[a-f0-9]{32}$")) and (.iface|type=="string") and (.prefix|type=="string") and (.anchor|type=="string")' <<< "$config" >/dev/null || return 64
  dmb_pool_id "$(jq -er .id <<< "$config")" || return 64
  iface=$(jq -er .iface <<< "$config") || return
  dmb_route_iface_valid "$iface" && [[ $iface != lo ]] || return 64
  prefix=$(dmb_cidr_parse "$(jq -er .prefix <<< "$config")" 6) || return
  [[ $prefix == */64 ]] && dmb_pool_global "${prefix%/*}" || return 64
  anchor=$(dmb_ip_parse "$(jq -er .anchor <<< "$config")" 6) || return
  dmb_pool_global "$anchor" && dmb_ip_in_cidr "$anchor" "$prefix" || return 64
  config=$(jq -cS --arg prefix "$prefix" --arg anchor "$anchor" '.prefix=$prefix|.anchor=$anchor' <<< "$config") || return
  dmb_pool_verifier_valid "$config" || return
  source=$(dmb_ip_parse "$(jq -er .verifier.source_address <<< "$config")" 6) || return
  addresses=$(dmb_run 5 ip -j -6 address show) || return
  jq -e --arg source "$source" 'all(.[].addr_info[]?;.local!=$source)' <<< "$addresses" >/dev/null || { dmb_pool_error 'external verifier source must not be an address on this host'; return; }
  config=$(jq -cS --arg source "$source" '.verifier.source_address=$source' <<< "$config") || return
  dmb_ipv6_unmanaged "$iface" || return
  link=$(dmb_route_link "$iface" | jq -ce '{ifindex,ifname,address,master:(.master//null),kind:(.linkinfo.info_kind//"ether")}') || return
  jq -e '.kind=="ether" or .kind=="veth"' <<< "$link" >/dev/null || { dmb_pool_error 'only an explicitly unmanaged Ethernet/veth link is supported'; return; }
  addresses=$(dmb_run 5 ip -j -6 address show dev "$iface") || return
  jq -e --arg anchor "$anchor" 'any(.[].addr_info[]?;.local==$anchor and .prefixlen==64 and .scope=="global" and .valid_life_time==4294967295 and .preferred_life_time==4294967295 and (.tentative//false|not) and (.dadfailed//false|not) and (.dynamic//false|not) and (.temporary//false|not))' <<< "$addresses" >/dev/null || { dmb_pool_error 'anchor must be a preferred permanent /64 address'; return; }
  routes=$(dmb_run 5 ip -j -6 route show table all dev "$iface") || return
  # Link-local dynamic routes are irrelevant to prefix delegation; nonlocal
  # dynamic/default policy must remain stable throughout this narrow backend.
  jq -cS --argjson link "$link" --arg ns "$(stat -Lc %i /proc/self/ns/net)" --argjson routes "$routes" '.+{link:$link,netns_inode:$ns,routing_snapshot:($routes|map(del(.expires,.cache,.lastuse,.used)))}' <<< "$config"
}
dmb_pool_step() {
  local step address prefix iface
  step=$(jq -ce --argjson i "$2" '.steps[$i]|select(.adapter=="pool")' "$1") || return
  iface=$(jq -er .iface <<< "$step") || return
  dmb_route_iface_valid "$iface" || return 64
  address=$(dmb_ip_parse "$(jq -er .address <<< "$step")" 6) || return
  prefix=$(dmb_cidr_parse "$(jq -er .prefix <<< "$step")" 6) || return
  [[ $prefix == */64 ]] && dmb_pool_global "$address" && dmb_ip_in_cidr "$address" "$prefix" || return 64
  dmb_pool_id "$(jq -er .enrollment <<< "$step")" || return 64
  jq -e '(.enrollment_sha256|test("^[a-f0-9]{64}$")) and (.external_probes==true) and (.link.ifindex|type=="number") and (.netns_inode|type=="string")' <<< "$step" >/dev/null || return 64
  printf '%s\n' "$step"
}
dmb_pool_capture() {
  local step iface address link ns matches observed identity protocol
  step=$(dmb_pool_step "$1" "$2") || return
  iface=$(jq -er .iface <<< "$step"); address=$(jq -er .address <<< "$step")
  link=$(dmb_route_link "$iface" | jq -ce '{ifindex,ifname,address,master:(.master//null),kind:(.linkinfo.info_kind//"ether")}') || return
  ns=$(stat -Lc %i /proc/self/ns/net) || return
  identity=$(jq -cnS --argjson link "$link" --arg ns "$ns" '{link:$link,netns_inode:$ns}') || return
  matches=$(dmb_run 5 ip -N -j -6 address show dev "$iface" | jq -c --arg address "$address" '[.[].addr_info[]?|select(.local==$address)]') || return
  [[ $(jq length <<< "$matches") == 0 ]] && { jq -cS '.+{exists:false}' <<< "$identity"; return; }
  [[ $(jq length <<< "$matches") == 1 ]] || return 76
  # DAD flags are observed separately in verify. Keeping them outside logical
  # configuration permits rollback of a failed/tentative address after SIGKILL.
  protocol=$(jq -r '.[0].protocol//"unspecified"|tostring' <<< "$matches") || return
  if [[ $protocol =~ ^0x[0-9a-fA-F]{1,2}$ ]]; then protocol=$((protocol)); fi
  observed=$(jq -cS --arg protocol "$protocol" '.[0]|{address:.local,prefixlen,scope:(if (.scope|tostring)=="0" then "global" else .scope end),protocol:$protocol,noprefixroute:(.noprefixroute//false),permanent:(.valid_life_time==4294967295 and .preferred_life_time==4294967295)}' <<< "$matches") || return
  jq -cnS --argjson identity "$identity" --argjson observed "$observed" '$identity+$observed+{exists:true}'
}
dmb_pool_generate() {
  local prefix=$1 words iid address; local -a p r
  words=$(dmb_ipv6_words "${prefix%/*}") || return
  IFS=' ' read -r -a p <<< "$words"
  iid=$(od -An -N8 -tx2 /dev/urandom) || return
  IFS=' ' read -r -a r <<< "$iid"
  ((${#r[@]}==4)) || return 74
  # Avoid subnet-router/reserved anycast IIDs; no MAC-derived identifier.
  r[0]=$((0x6000+(16#${r[0]} & 0x1fff)))
  address=$(dmb_ipv6_render "${p[0]}" "${p[1]}" "${p[2]}" "${p[3]}" "${r[0]}" "$((16#${r[1]}))" "$((16#${r[2]}))" "$((16#${r[3]}))") || return
  dmb_ip_in_cidr "$address" "$prefix" || return 64
  printf '%s\n' "$address"
}
dmb_pool_plan() {
  local config=$1 id count manifest path current hash iface prefix plan='[]' resources='[]' address before after i tries=0 pool_seen='|'
  jq -e 'type=="object" and .external_probes==true and (.count|type=="number" and floor==. and .>=1 and .<=16) and (.enrollment|type=="string")' <<< "$config" >/dev/null || { dmb_pool_error 'explicit external_probes:true and count 1..16 are required'; return; }
  id=$(jq -er .enrollment <<< "$config"); count=$(jq -er .count <<< "$config")
  path=$(dmb_pool_manifest_path "$id") || return
  dmb_trusted_path "$path" || return
  manifest=$(cat -- "$path") || return
  [[ $(jq -er .id <<< "$manifest") == "$id" ]] || return 76
  current=$(dmb_pool_enroll "$manifest") || return
  [[ $(jq -cS . <<< "$manifest") == "$(jq -cS . <<< "$current")" ]] || return 76
  hash=$(sha256sum -- "$path"); hash=${hash%% *}
  iface=$(jq -er .iface <<< "$manifest"); prefix=$(jq -er .prefix <<< "$manifest")
  resources=$(jq -cn --arg iface "$iface" --arg id "$id" '["link:"+$iface,"pool:"+$id]') || return
  for ((i=0;i<count;i++)); do
    while :; do
      tries=$((tries+1)); ((tries<=128)) || return 74
      address=$(dmb_pool_generate "$prefix") || return
      [[ $pool_seen != *"|$address|"* ]] || continue
      pool_seen+="$address|"; break
    done
    local step scratch
    step=$(jq -cn --arg id "$id" --arg index "$i" --arg address "$address" --arg iface "$iface" --arg prefix "$prefix" --arg hash "$hash" --argjson manifest "$manifest" '{id:("pool-address-"+$index),adapter:"pool",resource:("pool-address:"+$iface+":"+$address),enrollment:$id,enrollment_sha256:$hash,iface:$iface,prefix:$prefix,address:$address,link:$manifest.link,netns_inode:$manifest.netns_inode,external_probes:true,after:{}}') || return
    scratch=$(jq -cn --argjson step "$step" '{steps:[$step]}') || return
    before=$(dmb_pool_capture <(printf '%s\n' "$scratch") 0) || return
    [[ $(jq -r .exists <<< "$before") == false ]] || return 76
    after=$(jq -cS --arg address "$address" '.+{exists:true,address:$address,prefixlen:128,scope:"global",protocol:"186",noprefixroute:true,permanent:true}' <<< "$before") || return
    step=$(jq -cS --argjson before "$before" --argjson after "$after" '.before=$before|.after=$after' <<< "$step") || return
    plan=$(jq -c --argjson step "$step" '.+[$step]' <<< "$plan") || return
    resources=$(jq -c --arg resource "pool-address:$iface:$address" '.+[$resource]' <<< "$resources") || return
  done
  jq -cn --argjson steps "$plan" --argjson resources "$resources" '{schema:1,adapter:"pool",resources:$resources,steps:$steps,notes:["Runtime permanent /128 addresses only. Planning performs no network probes. Apply explicitly invokes the pinned separate-host verifier before/after mutation; verifier honesty and placement are trusted. No persistence or NAT66 is installed."]}'
}
dmb_pool_snapshot() {
  local step id path destination actual expected
  step=$(dmb_pool_step "$1" "$2") || return
  id=$(jq -er .enrollment <<< "$step"); path=$(dmb_pool_manifest_path "$id") || return
  dmb_trusted_path "$path" || return
  actual=$(sha256sum -- "$path"); actual=${actual%% *}; expected=$(jq -er .enrollment_sha256 <<< "$step")
  [[ $actual == "$expected" ]] || return 76
  destination=$3/snapshots/pool-enrollment-$id.json
  if [[ -e $destination ]]; then actual=$(sha256sum -- "$destination"); [[ ${actual%% *} == "$expected" ]] || return 76
  else dmb_atomic_write "$destination" 0600 < "$path" || return; fi
  dmb_run 5 ip -j -6 address show dev "$(jq -er .iface <<< "$step")" | dmb_atomic_write "$3/snapshots/pool-address-$2-raw.json" 0600
}
dmb_pool_verifier() {
  local plan=$1 index=$2 tx=$3 phase=$4 address=$5 step manifest id path request nonce digest response verifier proof
  local -a args
  step=$(dmb_pool_step "$plan" "$index") || return
  id=$(jq -er .enrollment <<< "$step")
  path=$tx/snapshots/pool-enrollment-$id.json
  dmb_trusted_path "$path" || return
  digest=$(sha256sum -- "$path") || return
  [[ ${digest%% *} == "$(jq -er .enrollment_sha256 <<< "$step")" ]] || return 76
  manifest=$(cat -- "$path") || return
  dmb_pool_verifier_valid "$manifest" || return
  nonce=$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n') || return
  [[ $nonce =~ ^[a-f0-9]{32}$ ]] || return 74
  request=$(jq -cnS --arg nonce "$nonce" --arg phase "$phase" --arg transaction "${tx##*/}" --arg address "$address" --argjson manifest "$manifest" '{schema:1,nonce:$nonce,transaction_id:$transaction,phase:$phase,prefix:$manifest.prefix,addresses:[$address],verifier_id:$manifest.verifier.identity,expected_source:$manifest.verifier.source_address}') || return
  digest=$(printf '%s\n' "$request" | sha256sum); digest=${digest%% *}
  verifier=$(jq -er .verifier.path <<< "$manifest") || return
  mapfile -d '' -t args < <(jq -jr '.verifier.args[]|.,"\u0000"' <<< "$manifest")
  response=$(dmb_run 25 "$verifier" "${args[@]}" <<< "$request" | head -c 65537) || { dmb_pool_error 'external probe inconclusive or failed; address verification was not established'; return; }
  ((${#response}<=65536)) || return 74
  proof=$(jq -ceS --arg digest "$digest" --argjson request "$request" '. as $p|select(.schema==1 and .request_sha256==$digest and .nonce==$request.nonce and .phase==$request.phase and .verifier_id==$request.verifier_id and .prefix==$request.prefix and .location=="external" and .result=="reachable" and .source_address==$request.expected_source and (.observations|length)==($request.addresses|length) and all($request.addresses[];. as $a|any($p.observations[];.address==$a and .reachable==true and .direction=="inbound")))' <<< "$response") || { dmb_pool_error 'external verifier did not return complete matching reachability evidence'; return; }
  printf '%s\n' "$proof" | dmb_atomic_write "$tx/snapshots/pool-address-$index-$phase-proof.json" 0600
}
dmb_pool_preflight() {
  local step manifest current expected iface value key routes addresses anchor
  step=$(dmb_pool_step "$1" "$2") || return
  manifest=$(cat -- "$3/snapshots/pool-enrollment-$(jq -er .enrollment <<< "$step").json") || return
  iface=$(jq -er .iface <<< "$step")
  dmb_ipv6_unmanaged "$iface" || return
  current=$(dmb_route_link "$iface" | jq -cS '{ifindex,ifname,address,master:(.master//null),kind:(.linkinfo.info_kind//"ether")}') || return
  expected=$(jq -cS .link <<< "$manifest") || return
  [[ $current == "$expected" && $(stat -Lc %i /proc/self/ns/net) == "$(jq -er .netns_inode <<< "$manifest")" ]] || return 76
  for key in disable_ipv6 accept_dad dad_transmits; do
    read -r value < "/proc/sys/net/ipv6/conf/$iface/$key" || return
    [[ $value =~ ^[0-9]+$ ]] || return 69
    if [[ $key == disable_ipv6 ]]; then [[ $value == 0 ]] || return 69; else ((value>=1 && value<=3)) || return 69; fi
  done
  anchor=$(jq -er .anchor <<< "$manifest") || return
  addresses=$(dmb_run 5 ip -j -6 address show dev "$iface") || return
  jq -e --arg anchor "$anchor" 'any(.[].addr_info[]?;.local==$anchor and .prefixlen==64 and .scope=="global" and .valid_life_time==4294967295 and .preferred_life_time==4294967295 and (.tentative//false|not) and (.dadfailed//false|not) and (.deprecated//false|not))' <<< "$addresses" >/dev/null || { dmb_pool_error 'the enrolled prefix anchor disappeared, expired or changed during application'; return; }
  local members
  members=$(jq -c '[.steps[]|select(.adapter=="pool")|.address]' "$1") || return
  routes=$(dmb_run 5 ip -j -6 route show table all dev "$iface" | jq -cS --argjson members "$members" 'map(select((.type=="local" and (.dst|rtrimstr("/128")) as $a|$members|index($a)!=null)|not)|del(.expires,.cache,.lastuse,.used))') || return
  [[ $routes == "$(jq -cS .routing_snapshot <<< "$manifest")" ]] || return 76
  if [[ ${4:-anchor} != context ]]; then dmb_pool_verifier "$1" "$2" "$3" anchor "$anchor"; fi
}
dmb_pool_apply() {
  local step iface address expected current n data
  step=$(dmb_pool_step "$1" "$2") || return
  iface=$(jq -er .iface <<< "$step"); address=$(jq -er .address <<< "$step")
  current=$(dmb_pool_capture "$1" "$2" | jq -cS .) || return
  expected=$(jq -cS .before <<< "$step") || return
  [[ $current == "$expected" ]] || return 76
  dmb_pool_preflight "$1" "$2" "$3" || return
  dmb_run 8 ip -6 address add "$address/128" dev "$iface" proto 186 noprefixroute || return
  for ((n=0;n<12;n++)); do
    data=$(dmb_run 5 ip -j -6 address show dev "$iface") || return
    jq -e --arg address "$address" 'any(.[].addr_info[]?;.local==$address and (.dadfailed//false))' <<< "$data" >/dev/null && { dmb_pool_error 'duplicate-address detection failed'; return; }
    if jq -e --arg address "$address" 'any(.[].addr_info[]?;.local==$address and (.tentative//false|not))' <<< "$data" >/dev/null; then return 0; fi
    sleep 1
  done
  dmb_pool_error 'DAD did not reach a preferred state within the bound'
}
dmb_pool_verify() {
  local step actual expected address iface data
  step=$(dmb_pool_step "$1" "$2") || return
  actual=$(dmb_pool_capture "$1" "$2" | jq -cS .) || return
  expected=$(jq -cS .after <<< "$step") || return
  [[ $actual == "$expected" ]] || return 76
  address=$(jq -er .address <<< "$step"); iface=$(jq -er .iface <<< "$step")
  data=$(dmb_run 5 ip -j -6 address show dev "$iface") || return
  jq -e --arg address "$address" 'any(.[].addr_info[]?;.local==$address and (.tentative//false|not) and (.dadfailed//false|not) and (.deprecated//false|not))' <<< "$data" >/dev/null || return 69
  dmb_pool_preflight "$1" "$2" "$3" context || return
  dmb_pool_verifier "$1" "$2" "$3" member "$address"
}
dmb_pool_restore() {
  local step actual before after iface address
  step=$(dmb_pool_step "$1" "$2") || return
  actual=$(dmb_pool_capture "$1" "$2" | jq -cS .) || return
  before=$(jq -cS .before <<< "$step"); after=$(jq -cS .after <<< "$step")
  [[ $actual != "$before" ]] || return 0
  [[ $actual == "$after" && $(jq -r .exists <<< "$before") == false ]] || return 76
  iface=$(jq -er .iface <<< "$step"); address=$(jq -er .address <<< "$step")
  if declare -F dmb_nat66_pool_removal_check >/dev/null; then dmb_nat66_pool_removal_check "$iface" "$address" || return; fi
  dmb_run 8 ip -6 address del "$address/128" dev "$iface" || return
  dmb_pool_verify_restore "$1" "$2" "$3"
}
dmb_pool_verify_restore() {
  local actual expected
  actual=$(dmb_pool_capture "$1" "$2" | jq -cS .) || return
  expected=$(jq -cS --argjson i "$2" '.steps[$i].before' "$1") || return
  [[ $actual == "$expected" ]]
}
