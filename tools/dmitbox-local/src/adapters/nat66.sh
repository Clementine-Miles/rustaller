# shellcheck shell=bash
# Narrow runtime NAT66 for explicitly enrolled permanent pool members. The
# original/reply direction match leaves established connections and inbound SSH
# replies outside the new-flow selector. No connection/packet marks are written.
dmb_nat66_error() { dmb_error "nat66: $*"; return 69; }
dmb_nat66_semantic() {
  dmb_nft_semantic | jq -Sc 'walk(if type=="object" and (.numgen?|type)=="object" then
    .numgen.offset=(.numgen.offset // 0) else . end)'
}
dmb_nat66_state() {
  local pool=$1 members=$2 owner iface anchor prefix
  owner=$(jq -er .owner_id <<< "$pool"); iface=$(jq -er .iface <<< "$pool")
  anchor=$(jq -er .anchor <<< "$pool"); prefix=$(jq -er .prefix <<< "$pool")
  jq -cnS --arg owner "$owner" --arg iface "$iface" --arg anchor "$anchor" --arg prefix "${prefix%/*}" --argjson members "$members" '
    ("dmitbox-local:nat66:"+$owner) as $tag |
    {present:true,nftables:[
      {table:{family:"ip6",name:"dmitbox_nat66",comment:$tag}},
      {chain:{family:"ip6",table:"dmitbox_nat66",name:"postrouting",type:"nat",hook:"postrouting",prio:100,policy:"accept",comment:$tag}},
      {rule:{family:"ip6",table:"dmitbox_nat66",chain:"postrouting",comment:$tag,expr:[
        {match:{op:"==",left:{meta:{key:"oifname"}},right:$iface}},
        {match:{op:"==",left:{ct:{key:"direction"}},right:"original"}},
        {match:{op:"in",left:{ct:{key:"state"}},right:"new"}},
        {match:{op:"==",left:{payload:{protocol:"ip6",field:"saddr"}},right:$anchor}},
        {match:{op:"!=",left:{payload:{protocol:"ip6",field:"daddr"}},right:{prefix:{addr:$prefix,len:64}}}},
        {snat:{addr:{map:{key:{numgen:{mode:"random",mod:($members|length)}},data:{set:[$members|to_entries[]|[.key,.value]]}}}}}
      ]}}
    ]}' | dmb_nat66_semantic
}
dmb_nat66_step() {
  local step pool members address expected before after operation
  step=$(jq -ce --argjson i "$2" '.steps[$i]|select(.adapter=="nat66")' "$1") || return
  jq -e '.operation=="enable" or .operation=="disable"' <<< "$step" >/dev/null || return 64
  jq -e '.nft=={family:"ip6",table:"dmitbox_nat66"} and (.pool|type)=="object" and
    (.members|type)=="array" and (.members|length)>=1 and (.members|length)<=4 and
    (.members|unique|length)==(.members|length) and
    (.pool_enrollment_sha256|test("^[a-f0-9]{64}$")) and .external_probes==true' <<< "$step" >/dev/null || return 64
  pool=$(jq -ce .pool <<< "$step"); members=$(jq -ce .members <<< "$step")
  dmb_pool_id "$(jq -er .id <<< "$pool")" || return 64
  [[ $(jq -er .owner_id <<< "$pool") =~ ^[a-f0-9]{32}$ ]] || return 64
  dmb_route_iface_valid "$(jq -er .iface <<< "$pool")" || return 64
  [[ $(jq -er .prefix <<< "$pool") == */64 && $(dmb_cidr_parse "$(jq -er .prefix <<< "$pool")" 6) == "$(jq -er .prefix <<< "$pool")" ]] || return 64
  dmb_pool_global "$(jq -er .anchor <<< "$pool")" && dmb_ip_in_cidr "$(jq -er .anchor <<< "$pool")" "$(jq -er .prefix <<< "$pool")" || return 64
  while IFS= read -r address; do
    [[ $(dmb_ip_parse "$address" 6) == "$address" ]] && dmb_pool_global "$address" &&
      dmb_ip_in_cidr "$address" "$(jq -er .prefix <<< "$pool")" && [[ $address != "$(jq -er .anchor <<< "$pool")" ]] || return 64
  done < <(jq -r '.[]' <<< "$members")
  expected=$(dmb_nat66_state "$pool" "$members") || return
  before=$(jq -cS .before <<< "$step"); after=$(jq -cS .after <<< "$step"); operation=$(jq -er .operation <<< "$step")
  if [[ $operation == enable ]]; then
    [[ $after == "$expected" && ( $before == '{"nftables":[],"present":false}' || $before == "$expected" ) ]] || return 64
  else [[ $before == "$expected" && $after == '{"nftables":[],"present":false}' ]] || return 64; fi
  printf '%s\n' "$step"
}
dmb_nat66_foreign_guard() {
  local ruleset=$1
  dmb_nft_manager_guard || return
  if [[ -r /proc/net/ip6_tables_names ]] && grep -qx nat /proc/net/ip6_tables_names; then
    dmb_nat66_error 'legacy IPv6 NAT owns a competing pipeline'; return
  fi
  jq -e '[.nftables[]|.chain? // empty|select((.family=="inet" or .family=="ip6") and .type=="nat")|
    select(.family!="ip6" or .table!="dmitbox_nat66")]|length==0' <<< "$ruleset" >/dev/null ||
      dmb_nat66_error 'another IPv6 NAT hook owns the translation pipeline'
}
dmb_nat66_from_ruleset() {
  local pool=$1 ruleset=$2 tag
  tag=dmitbox-local:nat66:$(jq -er .owner_id <<< "$pool")
  jq -ce --arg tag "$tag" '
    [.nftables[]|select((.table?.family=="ip6" and .table?.name=="dmitbox_nat66") or
       (to_entries[0].value.family=="ip6" and to_entries[0].value.table=="dmitbox_nat66"))] as $items |
    if ($items|length)==0 then {present:false,nftables:[]}
    elif [$items[]|.table? // empty|select(.comment==$tag)]|length!=1 then error("foreign NAT66 table name collision")
    else {present:true,nftables:$items} end' <<< "$ruleset" | dmb_nat66_semantic
}
dmb_nat66_capture() {
  local step ruleset
  step=$(dmb_nat66_step "$1" "$2") || return
  ruleset=$(dmb_nft_ruleset) || return
  # Recovery reads only its owned table; unrelated managers/prefix disappearance
  # must not prevent an independently executable inverse of that table.
  dmb_nat66_from_ruleset "$(jq -ce .pool <<< "$step")" "$ruleset"
}
dmb_nat66_pool_context() {
  local pool=$1 iface addresses anchor member members=$2 actual expected routes all_members
  iface=$(jq -er .iface <<< "$pool"); anchor=$(jq -er .anchor <<< "$pool")
  dmb_ipv6_unmanaged "$iface" || return
  actual=$(dmb_route_link "$iface" | jq -cS '{ifindex,ifname,address,master:(.master//null),kind:(.linkinfo.info_kind//"ether")}') || return
  expected=$(jq -cS .link <<< "$pool") || return
  [[ $actual == "$expected" && $(stat -Lc %i /proc/self/ns/net) == "$(jq -er .netns_inode <<< "$pool")" ]] || return 76
  addresses=$(dmb_run 5 ip -N -j -6 address show dev "$iface") || return
  jq -e --arg anchor "$anchor" 'any(.[].addr_info[]?;.local==$anchor and .prefixlen==64 and (.scope=="global" or .scope==0) and
    .valid_life_time==4294967295 and .preferred_life_time==4294967295 and (.tentative//false|not) and (.dadfailed//false|not) and (.deprecated//false|not))' <<< "$addresses" >/dev/null || return 76
  while IFS= read -r member; do
    jq -e --arg member "$member" 'any(.[].addr_info[]?;.local==$member and .prefixlen==128 and (.scope=="global" or .scope==0) and
      (.protocol==186 or .protocol=="186" or .protocol=="0xba") and .noprefixroute==true and
      .valid_life_time==4294967295 and .preferred_life_time==4294967295 and (.tentative//false|not) and (.dadfailed//false|not) and (.deprecated//false|not))' <<< "$addresses" >/dev/null || return 76
  done < <(jq -r '.[]' <<< "$members")
  all_members=$(jq -c '[.[].addr_info[]?|select(.prefixlen==128 and (.protocol==186 or .protocol=="186" or .protocol=="0xba"))|.local]' <<< "$addresses") || return
  routes=$(dmb_run 5 ip -j -6 route show table all dev "$iface" | jq -cS --argjson members "$all_members" 'map(select((.type=="local" and (.dst|rtrimstr("/128")) as $a|$members|index($a)!=null)|not)|del(.expires,.cache,.lastuse,.used))') || return
  [[ $routes == "$(jq -cS .routing_snapshot <<< "$pool")" ]] || return 76
}
dmb_nat66_members_committed() {
  local pool=$1 members=$2 id member tx i actual expected matches
  id=$(jq -er .id <<< "$pool") || return
  while IFS= read -r member; do
    matches=0
    for tx in "$DMB_STATE_DIR"/transactions/tx-*; do
      [[ -d $tx && -f $tx/state.json && -f $tx/plan.json ]] || continue
      dmb_trusted_path "$tx" && dmb_trusted_path "$tx/state.json" || return
      [[ $(jq -er .state "$tx/state.json") == COMMITTED ]] || continue
      while IFS= read -r i; do
        dmb_tx_integrity "$tx" || return 76
        actual=$(dmb_pool_capture "$tx/plan.json" "$i" | jq -cS .) || return
        expected=$(jq -cS --argjson i "$i" '.steps[$i].after' "$tx/plan.json") || return
        [[ $actual != "$expected" ]] || matches=$((matches+1))
      done < <(jq -r --arg id "$id" --arg member "$member" '.steps|to_entries[]|select(.value.adapter=="pool" and .value.enrollment==$id and .value.address==$member and .value.after.exists==true)|.key' "$tx/plan.json")
    done
    ((matches==1)) || { dmb_nat66_error 'each NAT member requires one still-current committed pool ownership record'; return; }
  done < <(jq -r '.[]' <<< "$members")
}
dmb_nat66_plan() {
  local config=$1 operation id path pool hash members address step before after ruleset
  dmb_need nft conntrack jq ip || return
  jq -e '.schema==1 and (.operation=="enable" or .operation=="disable") and .external_probes==true and
    (.enrollment|type)=="string" and (.members|type)=="array" and (.members|length)>=1 and (.members|length)<=4' <<< "$config" >/dev/null || return 64
  operation=$(jq -er .operation <<< "$config"); id=$(jq -er .enrollment <<< "$config")
  path=$(dmb_pool_manifest_path "$id") || return; dmb_trusted_path "$path" || return
  pool=$(cat -- "$path") || return; [[ $(jq -er .id <<< "$pool") == "$id" ]] || return 76
  hash=$(sha256sum -- "$path"); hash=${hash%% *}
  [[ $(printf '%s\n' "$pool" | sha256sum | cut -d' ' -f1) == "$hash" ]] || return 76
  members='[]'
  while IFS= read -r address; do
    address=$(dmb_ip_parse "$address" 6) || return
    members=$(jq -c --arg a "$address" '.+[$a]' <<< "$members") || return
  done < <(jq -r '.members[]' <<< "$config")
  ruleset=$(dmb_nft_ruleset) || return
  dmb_nat66_foreign_guard "$ruleset" || return
  before=$(dmb_nat66_from_ruleset "$pool" "$ruleset") || return
  if [[ $operation == enable ]]; then
    dmb_pool_verifier_valid "$pool" && dmb_nat66_pool_context "$pool" "$members" && dmb_nat66_members_committed "$pool" "$members" || return
    after=$(dmb_nat66_state "$pool" "$members") || return
  else after='{"nftables":[],"present":false}'; fi
  step=$(jq -cnS --arg operation "$operation" --arg iface "$(jq -er .iface <<< "$pool")" --arg hash "$hash" --argjson pool "$pool" --argjson members "$members" --argjson before "$before" --argjson after "$after" '{id:"nat66-policy",adapter:"nat66",resource:"nft:ip6:dmitbox_nat66",nft:{family:"ip6",table:"dmitbox_nat66"},iface:$iface,operation:$operation,pool:$pool,pool_enrollment_sha256:$hash,members:$members,external_probes:true,before:$before,after:$after}') || return
  dmb_nat66_step <(jq -cn --argjson step "$step" '{steps:[$step]}') 0 >/dev/null || return
  jq -cnS --arg id "$id" --argjson step "$step" '{schema:1,adapter:"nat66",timeout_seconds:180,resources:["nft:ip6:dmitbox_nat66",("pool:"+$id),("link:"+$step.iface)],steps:[$step],notes:["Runtime NAT66 only; only new original-direction flows from the enrolled anchor are selected. Existing conntracks and all pool addresses remain. Disable waits bounded for translated flows to drain; a timeout is a failed verification and restores policy. Explicit external probes run only during apply/verify."]}'
}
dmb_nat66_snapshot() {
  local step path hash raw
  step=$(dmb_nat66_step "$1" "$2") || return
  path=$(dmb_pool_manifest_path "$(jq -er .pool.id <<< "$step")") || return
  dmb_trusted_path "$path" || return
  hash=$(sha256sum -- "$path"); hash=${hash%% *}
  [[ $hash == "$(jq -er .pool_enrollment_sha256 <<< "$step")" ]] || return 76
  [[ $(jq -cS . "$path") == "$(jq -cS .pool <<< "$step")" ]] || return 76
  dmb_atomic_write "$3/snapshots/nat66-$2-pool.json" 0600 "$hash" < "$path" || return
  raw=$(dmb_nft_ruleset) || return
  printf '%s\n' "$raw" | dmb_atomic_write "$3/snapshots/nat66-$2-raw.json" 0600
}
dmb_nat66_verifier() {
  local step pool phase=$4 request nonce digest response program proof actual
  local -a verifier_args
  step=$(dmb_nat66_step "$1" "$2") || return; pool=$(jq -ce .pool <<< "$step") || return
  dmb_pool_verifier_valid "$pool" || return
  nonce=$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n') || return
  request=$(jq -cnS --arg nonce "$nonce" --arg phase "$phase" --arg transaction "${3##*/}" --argjson step "$step" '{schema:1,nonce:$nonce,transaction_id:$transaction,phase:$phase,prefix:$step.pool.prefix,anchor:$step.pool.anchor,members:$step.members,verifier_id:$step.pool.verifier.identity,expected_source:$step.pool.verifier.source_address}') || return
  digest=$(printf '%s\n' "$request" | sha256sum); digest=${digest%% *}
  program=$(jq -er .verifier.path <<< "$pool") || return
  mapfile -d '' -t verifier_args < <(jq -jr '.verifier.args[]|.,"\u0000"' <<< "$pool")
  response=$(dmb_run 20 "$program" "${verifier_args[@]}" <<< "$request" | head -c 65537) || { dmb_nat66_error 'external NAT probe failed or was blocked; evidence inconclusive'; return; }
  ((${#response}<=65536)) || return 74
  proof=$(jq -ceS --argjson request "$request" --arg digest "$digest" '
    . as $proof | select(.schema==1 and .request_sha256==$digest and .nonce==$request.nonce and .phase==$request.phase and
      .verifier_id==$request.verifier_id and .prefix==$request.prefix and .source_address==$request.expected_source and
      .location=="external" and .result=="reachable" and
      (.observations|type)=="array" and (.observations|length)==($request.members|length) and
      all($request.members[];. as $member|any($proof.observations[];.address==$member and .reachable==true and
        (if $request.phase=="nat66-outbound" then .direction=="outbound" and .observed_source==$member else .direction=="inbound" end))))' <<< "$response") || { dmb_nat66_error 'external evidence did not prove every selected member'; return; }
  actual=$(printf '%s\n' "$proof" | sha256sum); actual=${actual%% *}
  printf '%s\n' "$proof" | dmb_atomic_write "$3/snapshots/nat66-$2-$phase-proof.json" 0600 "$actual"
}
dmb_nat66_transition() {
  local plan=$1 index=$2 tx=$3 target=$4 step current desired expected ruleset live batch pool raw hash
  step=$(dmb_nat66_step "$plan" "$index") || return
  pool=$(jq -ce .pool <<< "$step") || return
  current=$(dmb_nat66_capture "$plan" "$index") || return
  desired=$(jq -cS --arg target "$target" '.[$target]' <<< "$step") || return
  [[ $current != "$desired" ]] || return 0
  if [[ $target == after ]]; then expected=$(jq -cS .before <<< "$step"); else expected=$(jq -cS .after <<< "$step"); fi
  [[ $current == "$expected" ]] || return 76
  ruleset=$(dmb_nft_ruleset) || return
  # Apply refuses mixed ownership. An inverse does not globally adopt any of
  # those resources; it changes only its own known generation.
  [[ $target != after ]] || dmb_nat66_foreign_guard "$ruleset" || return
  live=$(dmb_nat66_from_ruleset "$pool" "$ruleset") || return
  [[ $live == "$current" ]] || return 76
  batch=$(jq -cn --argjson current "$current" --argjson desired "$desired" '{nftables:([
    if $current.present then {delete:{table:{family:"ip6",name:"dmitbox_nat66"}}} else empty end] +
    [if $desired.present then $desired.nftables[]|if has("table") then {create:.} else {add:.} end else empty end])}') || return
  raw=$tx/nat66-$index-batch.json
  hash=$(printf '%s\n' "$batch" | sha256sum); hash=${hash%% *}
  printf '%s\n' "$batch" | dmb_atomic_write "$raw" 0600 "$hash" || return
  dmb_run 8 nft -j -c -f "$raw" && dmb_run 8 nft -j -f "$raw" || return
  [[ $(dmb_nat66_capture "$plan" "$index") == "$desired" ]]
}
dmb_nat66_apply() {
  local step pool path hash marker marker_data
  step=$(dmb_nat66_step "$1" "$2") || return
  pool=$(jq -ce .pool <<< "$step") || return
  if [[ $(jq -er .operation <<< "$step") == enable ]]; then
    path=$(dmb_pool_manifest_path "$(jq -er .id <<< "$pool")") || return
    hash=$(sha256sum -- "$path"); hash=${hash%% *}
    [[ $hash == "$(jq -er .pool_enrollment_sha256 <<< "$step")" ]] || return 76
    [[ $(jq -cS . "$path") == "$(jq -cS .pool <<< "$step")" ]] || return 76
    dmb_nat66_pool_context "$pool" "$(jq -ce .members <<< "$step")" || return
    dmb_nat66_members_committed "$pool" "$(jq -ce .members <<< "$step")" || return
    dmb_nat66_verifier "$1" "$2" "$3" nat66-preflight || return
    # Retained before publication, even if a later probe fails. Conntrack NAT
    # mappings outlive policy deletion; address removal must know this history.
    dmb_trusted_path "$DMB_STATE_DIR/nat66-used" allow_missing || return
    mkdir -p -- "$DMB_STATE_DIR/nat66-used" && chmod 0700 "$DMB_STATE_DIR/nat66-used" || return
    marker=$DMB_STATE_DIR/nat66-used/${3##*/}-$2.json
    marker_data=$(jq -cS '{schema:1,iface:.pool.iface,anchor:.pool.anchor,members:.members}' <<< "$step") || return
    hash=$(printf '%s\n' "$marker_data" | sha256sum); hash=${hash%% *}
    jq -cS --arg hash "$hash" '.+{sha256:$hash}' <<< "$marker_data" | dmb_atomic_write "$marker" 0600 || return
  fi
  dmb_nat66_transition "$1" "$2" "$3" after
}
# Parse two strictly validated direction tuples, never substring-match an IP.
dmb_nat66_flow_count() {
  local anchor=$1 members=$2 text line token src dst original_source reply_destination total=0 src_count dst_count
  local nat66_reported_count='' nat66_parsed_count=0
  text=$(dmb_run 5 conntrack -L -f ipv6 -o extended 2>&1) || return 69
  while IFS= read -r line; do
    [[ -n $line ]] || continue
    if [[ $line =~ ^conntrack\ v[0-9][^:]*:\ ([0-9]+)\ flow\ entries\ have\ been\ shown\.$ ]]; then
      [[ -z $nat66_reported_count ]] || return 69
      nat66_reported_count=${BASH_REMATCH[1]}
      continue
    fi
    src_count=0; dst_count=0; original_source=''; reply_destination=''
    local -a tokens; IFS=' ' read -r -a tokens <<< "$line"
    for token in "${tokens[@]}"; do
      case $token in
        src=*) src=$(dmb_ip_parse "${token#src=}" 6) || return 69; ((src_count+=1)); ((src_count!=1)) || original_source=$src;;
        dst=*) dst=$(dmb_ip_parse "${token#dst=}" 6) || return 69; ((dst_count+=1)); ((dst_count!=2)) || reply_destination=$dst;;
      esac
    done
    ((src_count==2 && dst_count==2)) || return 69
    nat66_parsed_count=$((nat66_parsed_count+1))
    if [[ $original_source == "$anchor" ]] && jq -e --arg address "$reply_destination" 'index($address)!=null' <<< "$members" >/dev/null; then total=$((total+1)); fi
  done <<< "$text"
  [[ -n $nat66_reported_count && $nat66_reported_count == "$nat66_parsed_count" ]] || return 69
  printf '%s\n' "$total"
}
dmb_nat66_verify() {
  local step actual desired pool count start now
  step=$(dmb_nat66_step "$1" "$2") || return
  actual=$(dmb_nat66_capture "$1" "$2") || return; desired=$(jq -cS .after <<< "$step") || return
  [[ $actual == "$desired" ]] || return 76
  pool=$(jq -ce .pool <<< "$step") || return
  if [[ $(jq -er .operation <<< "$step") == enable ]]; then
    dmb_nat66_pool_context "$pool" "$(jq -ce .members <<< "$step")" || return
    dmb_nat66_verifier "$1" "$2" "$3" nat66-outbound
  else
    start=$(dmb_tx_uptime) || return
    while :; do
      count=$(dmb_nat66_flow_count "$(jq -er .anchor <<< "$pool")" "$(jq -ce .members <<< "$step")") || return
      ((count!=0)) || return 0
      now=$(dmb_tx_uptime) || return
      ((now-start<12)) || { dmb_nat66_error 'translated connections remain; keep pool addresses and retry after natural drain'; return; }
      sleep 1
    done
  fi
}
dmb_nat66_restore() { dmb_nat66_transition "$1" "$2" "$3" before; }
dmb_nat66_verify_restore() {
  local current expected
  current=$(dmb_nat66_capture "$1" "$2") || return
  expected=$(jq -cS --argjson i "$2" '.steps[$i].before' "$1") || return
  [[ $current == "$expected" ]]
}
dmb_nat66_pool_removal_check() {
  local iface=$1 address=$2 history count ruleset expected actual
  for history in "$DMB_STATE_DIR"/nat66-used/*.json; do
    [[ -f $history ]] || continue
    dmb_trusted_path "$history" || return
    jq -e '.schema==1 and (.iface|type)=="string" and (.anchor|type)=="string" and (.members|type)=="array" and all(.members[];type=="string") and (.sha256|test("^[a-f0-9]{64}$"))' "$history" >/dev/null || return 76
    expected=$(jq -er .sha256 "$history") || return
    actual=$(jq -cS 'del(.sha256)' "$history" | sha256sum); actual=${actual%% *}
    [[ $actual == "$expected" ]] || return 76
    jq -e --arg iface "$iface" --arg address "$address" '.iface==$iface and (.members|index($address)!=null)' "$history" >/dev/null || continue
    dmb_need nft conntrack || return
    ruleset=$(dmb_nft_ruleset) || return
    if jq -e '.nftables[]|.table? // empty|select(.family=="ip6" and .name=="dmitbox_nat66")' <<< "$ruleset" >/dev/null; then dmb_nat66_error 'disable and drain NAT66 before removing a pool address'; return; fi
    count=$(dmb_nat66_flow_count "$(jq -er .anchor "$history")" "$(jq -cn --arg address "$address" '[$address]')") || return
    ((count==0)) || { dmb_nat66_error 'pool address is retained by translated connections'; return; }
  done
}
