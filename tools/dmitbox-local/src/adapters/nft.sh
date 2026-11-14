# shellcheck shell=bash
# Native nftables ownership adapter. Sourcing this file performs no I/O.

dmb_nft_error() { printf 'nft: %s\n' "$*" >&2; return 1; }

dmb_nft_semantic() {
  # Rule and expression order stay semantic. Normalize only libnftables aliases
  # demonstrated by native readback, then remove handles and dynamic counters.
  jq -Sc '
    walk(if type=="object" then del(.handle) |
      (if has("counter") and (.counter|type)=="object" then .counter |= del(.packets,.bytes) else . end) |
      (if .match?.op=="in" and (.match.right|type)=="object" and (.match.right|has("set")) then .match.op="==" else . end)
      else . end) |
    if has("nftables") then .nftables |= map(select(has("metainfo")|not) |
      if has("rule") then .rule.expr |= (. as $expressions | [range(0;length) as $i |
        if ($expressions[$i].match?.left.meta?.key=="l4proto" and
            $expressions[$i].match.op=="==" and
            ($expressions[$i].match.right=="tcp" or $expressions[$i].match.right=="udp") and
            $expressions[$i+1].match?.left.payload?.protocol==$expressions[$i].match.right)
        then empty else $expressions[$i] end]) else . end)
    else . end'
}

dmb_nft_native() {
  # Semantic counter:{} intentionally omits volatile values, but libnftables
  # requires anonymous counter:null when submitting a new native statement.
  jq -c 'walk(if type=="object" and .counter?=={} then .counter=null else . end)'
}

dmb_nft_selector() {
  local plan=$1 index=$2
  jq -ce --argjson i "$index" '.steps[$i].nft |
    select(type=="object" and .family=="inet" and (.owner_id|test("^[a-f0-9]{32}$")) and
      (.table|test("^[A-Za-z_][A-Za-z0-9_]{0,31}$")) and
      (.chain|test("^[A-Za-z_][A-Za-z0-9_]{0,31}$"))) |
    select((.mode=="exclusive" and .table=="dmitbox_local" and .chain=="input" and (.parent_chain==null)) or
      (.mode=="scoped" and (.chain|startswith("dmb_")) and
       (.parent_chain|type)=="string" and (.parent_chain|test("^[A-Za-z_][A-Za-z0-9_]{0,31}$")) and .parent_chain!=.chain))' "$plan" 
}

dmb_nft_enrollment_path() { printf '%s/enrollments/nft.json\n' "${DMB_STATE_DIR:-/var/lib/dmitbox-local}"; }

dmb_nft_authorize() {
  local selector=$1 manifest
  manifest=$(dmb_nft_enrollment_path) || return 1
  dmb_trusted_path "$manifest" || return 1
  jq -e --argjson expected "$selector" '.schema==1 and .nft==$expected and (.pipeline_sha256|test("^[a-f0-9]{64}$"))' "$manifest" >/dev/null ||
    dmb_nft_error 'selector does not match the enrolled nft authority'
}

# The foreign portion of an enrolled table is a prerequisite, not part of our
# rollback resource. Pin its ordering, parent hook/policy and referenced objects
# while excluding only the independently owned child and tagged jump.
dmb_nft_pipeline_hash() {
  local selector=$1 ruleset=$2
  jq -c --argjson s "$selector" '
    {nftables:[.nftables[] |
      select((.table?.family==$s.family and .table?.name==$s.table) or
        (to_entries[0].value.family==$s.family and to_entries[0].value.table==$s.table)) |
      if $s.mode=="exclusive" then empty
      elif .chain?.name==$s.chain or .rule?.chain==$s.chain or
        (.rule?.chain==$s.parent_chain and .rule?.comment==("dmitbox-local:"+$s.owner_id))
      then empty else . end]}' "$ruleset" | dmb_nft_semantic | sha256sum | cut -d' ' -f1
}
dmb_nft_pipeline_check() {
  local selector=$1 ruleset=$2 manifest expected current
  manifest=$(dmb_nft_enrollment_path) || return
  dmb_trusted_path "$manifest" || return
  expected=$(jq -er '.pipeline_sha256|select(test("^[a-f0-9]{64}$"))' "$manifest") || return 1
  current=$(dmb_nft_pipeline_hash "$selector" "$ruleset") || return
  [[ $current == "$expected" ]] || { dmb_nft_error 'enrolled foreign table pipeline changed; refusing mixed generations'; return 76; }
}

dmb_nft_ruleset() {
  # A netlink query can request kernel module autoload. Status and planning
  # must not cause that side effect; enable the native facility explicitly.
  dmb_nft_facility_ready || { dmb_nft_error 'nf_tables is not observably enabled; load/enable it explicitly outside this tool'; return 69; }
  dmb_run 15 nft -j list ruleset
}
dmb_nft_facility_ready() { [[ -d /sys/module/nf_tables ]]; }

dmb_nft_foreign_guard() {
  local selector=$1 ruleset=$2
  # A child-chain jump must be within its parent table. Other INPUT/ingress
  # chains may still drop after our accept; no ordering trick bypasses them.
  jq -e --argjson s "$selector" '
    [.nftables[] | .chain? // empty | select(.hook=="input" or .hook=="ingress") |
      select(if $s.mode=="exclusive" then
        (.family!=$s.family or .table!=$s.table)
      else (.family!=$s.family or .table!=$s.table or .name!=$s.parent_chain) end)] | length==0' "$ruleset" >/dev/null ||
    dmb_nft_error 'foreign INPUT/ingress hook exists; own accept cannot override its drop policy'
}

dmb_nft_state_from_ruleset() {
  local selector=$1 ruleset=$2
  jq -c --argjson s "$selector" '
    def owned: "dmitbox-local:"+$s.owner_id;
    if $s.mode=="exclusive" then
      [.nftables[] | select((.table?.family==$s.family and .table?.name==$s.table) or
        (to_entries[0].value.family==$s.family and to_entries[0].value.table==$s.table))] as $items |
      if ($items|length)==0 then {present:false,nftables:[]}
      elif ([$items[]|.table? // empty|select(.comment==owned)]|length)!=1 then error("unowned table name collision")
      else {present:true,nftables:$items} end
    else
      [.nftables[]|.chain? // empty|select(.family==$s.family and .table==$s.table and .name==$s.chain)] as $child |
      [.nftables[]|.rule? // empty|select(.family==$s.family and .table==$s.table and .chain==$s.parent_chain and .comment==owned)] as $jump |
      if ($child|length)==0 and ($jump|length)==0 then {present:false,nftables:[]}
      elif ($child|length)!=1 or $child[0].comment!=owned or ($jump|length)!=1 or
        $jump[0].expr!=[{jump:{target:$s.chain}}] then error("invalid scoped ownership or jump")
      else {present:true,nftables:([
        .nftables[]|select(.chain?.family==$s.family and .chain?.table==$s.table and .chain?.name==$s.chain)] +
        [.nftables[]|select(.rule?.family==$s.family and .rule?.table==$s.table and .rule?.chain==$s.chain)] +
        [.nftables[]|select(.rule?.family==$s.family and .rule?.table==$s.table and .rule?.chain==$s.parent_chain and .rule?.comment==owned)])} end
    end' "$ruleset" | dmb_nft_semantic
}

dmb_nft_capture() {
  local plan=$1 index=$2 selector ruleset state
  selector=$(dmb_nft_selector "$plan" "$index") || return 1
  dmb_nft_authorize "$selector" || return 1
  dmb_nft_manager_guard || return 1
  ruleset=$(dmb_nft_ruleset) || return 1
  dmb_nft_foreign_guard "$selector" <(printf '%s\n' "$ruleset") || return 1
  dmb_nft_pipeline_check "$selector" <(printf '%s\n' "$ruleset") || return
  state=$(dmb_nft_state_from_ruleset "$selector" <(printf '%s\n' "$ruleset")) || return
  dmb_nft_state_validate "$selector" "$state" || return
  printf '%s\n' "$state"
}

dmb_nft_state_validate() {
  local selector=$1 state=$2
  jq -e --argjson s "$selector" '
    def owner: "dmitbox-local:"+$s.owner_id;
    type=="object" and (.present|type)=="boolean" and (.nftables|type)=="array" and
    (if .present==false then .nftables==[] else
      (.nftables|length)>0 and all(.nftables[];
        if has("table") then $s.mode=="exclusive" and .table.family==$s.family and .table.name==$s.table and .table.comment==owner
        elif has("chain") then .chain.family==$s.family and .chain.table==$s.table and .chain.name==$s.chain and .chain.comment==owner and
          (if $s.mode=="exclusive" then .chain.type=="filter" and .chain.hook=="input" and .chain.prio==0 and (.chain.policy=="accept" or .chain.policy=="drop") else .chain.hook==null end)
        elif has("rule") then .rule.family==$s.family and .rule.table==$s.table and
          (if .rule.chain==$s.chain then true else .rule.chain==$s.parent_chain and .rule.comment==owner and .rule.expr==[{jump:{target:$s.chain}}] end)
        else false end)
    end)' <<< "$state" >/dev/null || dmb_nft_error 'invalid owned nft state'
}

dmb_nft_batch() {
  local selector=$1 old=$2 desired=$3 live_ruleset=$4
  dmb_nft_state_validate "$selector" "$desired" || return 1
  jq -c --argjson s "$selector" --argjson old "$old" --argjson desired "$desired" '
    def tag: "dmitbox-local:"+$s.owner_id;
    [if $old.present then
      if $s.mode=="exclusive" then {delete:{table:{family:$s.family,name:$s.table}}}
      elif $desired.present then
        {flush:{chain:{family:$s.family,table:$s.table,name:$s.chain}}}
      else
        (.nftables[]|.rule? // empty|select(.family==$s.family and .table==$s.table and .chain==$s.parent_chain and .comment==tag)|
          if .handle==null then error("missing jump handle") else {delete:{rule:{family:$s.family,table:$s.table,chain:$s.parent_chain,handle:.handle}}} end),
        {flush:{chain:{family:$s.family,table:$s.table,name:$s.chain}}},
        {delete:{chain:{family:$s.family,table:$s.table,name:$s.chain}}}
      end
    else empty end] +
    [if $desired.present then $desired.nftables[] |
      if $s.mode=="scoped" and $old.present and (has("chain") or (has("rule") and .rule.chain==$s.parent_chain)) then empty
      elif has("rule") and .rule.chain==$s.parent_chain then {insert:{rule:.rule}}
      elif has("table") or has("chain") then {create:.}
      else {add:.} end
    else empty end] | {nftables:.}' "$live_ruleset" | dmb_nft_native
}

dmb_nft_transition() {
  local plan=$1 index=$2 txdir=$3 field=$4 selector desired actual live batch expected opposite live_state
  selector=$(dmb_nft_selector "$plan" "$index") || return 1
  dmb_nft_authorize "$selector" || return 1
  desired=$(jq -ce --argjson i "$index" --arg field "$field" '.steps[$i][$field]' "$plan" | dmb_nft_semantic) || return 1
  dmb_nft_state_validate "$selector" "$desired" || return 1
  actual=$(dmb_nft_capture "$plan" "$index") || return 1
  [[ "$actual" != "$desired" ]] || return 0
  if [[ $field == after ]]; then opposite=before; else opposite=after; fi
  expected=$(jq -ce --argjson i "$index" --arg field "$opposite" '.steps[$i][$field]' "$plan" | dmb_nft_semantic) || return 1
  if [[ $actual != "$expected" ]]; then
    [[ $field == before ]] && dmb_nft_owned_reset "$plan" "$index" "$txdir" || { dmb_nft_error 'owned resource generation changed before publication'; return 76; }
  fi
  live=$(mktemp "$txdir/nft-live.XXXXXXXX") || return 1
  batch=$(mktemp "$txdir/nft-batch.XXXXXXXX") || { rm -f -- "$live"; return 1; }
  if ! dmb_nft_ruleset > "$live" || ! dmb_nft_foreign_guard "$selector" "$live" || ! dmb_nft_pipeline_check "$selector" "$live"; then rm -f -- "$live" "$batch"; return 1; fi
  live_state=$(dmb_nft_state_from_ruleset "$selector" "$live") || { rm -f -- "$live" "$batch"; return 1; }
  [[ $live_state == "$actual" ]] || { rm -f -- "$live" "$batch"; dmb_nft_error 'nft generation changed during preparation'; return 76; }
  if ! dmb_nft_batch "$selector" "$actual" "$desired" "$live" > "$batch"; then rm -f -- "$live" "$batch"; return 1; fi
  if ! dmb_run 15 nft -j -c -f "$batch" || ! dmb_run 15 nft -j -f "$batch"; then rm -f -- "$live" "$batch"; return 1; fi
  rm -f -- "$live" "$batch"
  actual=$(dmb_nft_capture "$plan" "$index") || return 1
  [[ "$actual" == "$desired" ]] || dmb_nft_error 'post-publication nft state differs from desired generation'
}

dmb_nft_apply() { dmb_nft_transition "$1" "$2" "$3" after; }
dmb_nft_restore() { dmb_nft_transition "$1" "$2" "$3" before; }
dmb_nft_verify() {
  local plan=$1 index=$2 actual desired
  actual=$(dmb_nft_capture "$plan" "$index") || return 1
  desired=$(jq -ce --argjson i "$index" '.steps[$i].after' "$plan" | dmb_nft_semantic) || return 1
  [[ "$actual" == "$desired" ]] || dmb_nft_error 'nft verification differs from planned state'
}

# Plan helpers emit data only. Enrollment is an explicit separate root action.
# Typed listener and rule values become JSON expressions, never nft command text.
dmb_nft_plan_state() {
  local selector=$1 listeners=$2 rules=${3:-'[]'} policy=${4:-drop} address
  jq -e 'type=="array" and all(.[];(.address|type)=="string")' <<< "$listeners" >/dev/null || return 2
  while IFS= read -r address; do dmb_ip_parse "$address" >/dev/null || return 2; done < <(jq -r '.[].address' <<< "$listeners")
  [[ "$policy" == drop || "$policy" == accept ]] || return 1
  jq -cen --argjson s "$selector" --argjson listeners "$listeners" --argjson rules "$rules" --arg policy "$policy" '
    def portvalid: type=="number" and floor==. and .>=1 and .<=65535;
    def addrvalid: type=="string" and (test("^[0-9.]+$") or test("^[0-9A-Fa-f:]+$") and contains(":"));
    def baseexpr($protocol;$port): [
      {match:{op:"==",left:{meta:{key:"l4proto"}},right:$protocol}},
      {match:{op:"==",left:{payload:{protocol:$protocol,field:"dport"}},right:$port}}];
    def rule($expr): {rule:{family:$s.family,table:$s.table,chain:$s.chain,expr:$expr}};
    def listener:
      select(.port|portvalid) | select(.address|addrvalid) |
      rule((if .address=="0.0.0.0" or .address=="::" then [] else
        [{match:{op:"==",left:{payload:{protocol:(if .address|contains(":") then "ip6" else "ip" end),field:"daddr"}},right:.address}}] end) +
        (if .address=="0.0.0.0" then [{match:{op:"==",left:{meta:{key:"nfproto"}},right:"ipv4"}}]
         elif .address=="::" then [{match:{op:"==",left:{meta:{key:"nfproto"}},right:"ipv6"}}] else [] end) +
        baseexpr("tcp";.port)+[{accept:null}]);
    if ($listeners|type)!="array" or any($listeners[]; (.port|portvalid|not) or (.address|addrvalid|not)) then error("invalid listener") else . end |
    if ($rules|type)!="array" or any($rules[]; (.action!="allow" and .action!="deny") or (.protocol!="tcp" and .protocol!="udp") or (.port|portvalid|not) or has("source")) then error("invalid typed rule") else . end |
    {present:true,nftables:(
      (if $s.mode=="exclusive" then [
        {table:{family:$s.family,name:$s.table,comment:("dmitbox-local:"+$s.owner_id)}},
        {chain:{family:$s.family,table:$s.table,name:$s.chain,type:"filter",hook:"input",prio:0,policy:$policy,comment:("dmitbox-local:"+$s.owner_id)}},
        rule([{match:{op:"==",left:{meta:{key:"iifname"}},right:"lo"}},{accept:null}]),
        rule([{match:{op:"in",left:{ct:{key:"state"}},right:{set:["established","related"]}}},{accept:null}]),
        rule([{match:{op:"==",left:{ct:{key:"state"}},right:"invalid"}},{drop:null}]),
        rule([{match:{op:"==",left:{meta:{key:"l4proto"}},right:{set:["icmp","ipv6-icmp"]}}},{accept:null}])]
      else [{chain:{family:$s.family,table:$s.table,name:$s.chain,comment:("dmitbox-local:"+$s.owner_id)}}] end) +
      [$listeners[]|listener] +
      [$rules[]|rule(baseexpr(.protocol;.port)+[if .action=="allow" then {accept:null} else {drop:null} end])] +
      (if $s.mode=="scoped" then [{rule:{family:$s.family,table:$s.table,chain:$s.parent_chain,comment:("dmitbox-local:"+$s.owner_id),expr:[{jump:{target:$s.chain}}]}}] else [] end))}' | dmb_nft_semantic
}

dmb_nft_plan_exclusive() {
  local owner=$1 listeners=$2 rules=${3:-'[]'} selector state
  [[ "$owner" =~ ^[a-f0-9]{32}$ ]] || return 1
  selector=$(jq -cn --arg owner "$owner" '{mode:"exclusive",family:"inet",table:"dmitbox_local",chain:"input",parent_chain:null,owner_id:$owner}') || return 1
  state=$(dmb_nft_plan_state "$selector" "$listeners" "$rules") || return 1
  jq -cn --argjson s "$selector" --argjson state "$state" '{adapter:"nft",resource:"nft-inet-dmitbox_local",nft:$s,after:$state}'
}

dmb_nft_plan_scoped() {
  local owner=$1 table=$2 parent=$3 child=$4 listeners=$5 rules=${6:-'[]'} selector state
  [[ "$owner" =~ ^[a-f0-9]{32}$ && "$table" =~ ^[A-Za-z_][A-Za-z0-9_]{0,31}$ && "$parent" =~ ^[A-Za-z_][A-Za-z0-9_]{0,31}$ && "$child" =~ ^dmb_[A-Za-z0-9_]{1,28}$ && "$parent" != "$child" ]] || return 1
  selector=$(jq -cn --arg owner "$owner" --arg table "$table" --arg parent "$parent" --arg child "$child" '{mode:"scoped",family:"inet",table:$table,chain:$child,parent_chain:$parent,owner_id:$owner}') || return 1
  state=$(dmb_nft_plan_state "$selector" "$listeners" "$rules" accept) || return 1
  jq -cn --argjson s "$selector" --argjson state "$state" '{adapter:"nft",resource:("nft-inet-"+$s.table+"-"+$s.chain),nft:$s,after:$state}'
}

dmb_nft_enroll() {
  local config=$1 selector ruleset existing pipeline
  jq -e '.schema==1 and (.nft|type)=="object"' "$config" >/dev/null || return 2
  selector=$(dmb_nft_selector <(jq '{steps:[{nft:.nft}]}' "$config") 0) || return 2
  dmb_nft_manager_guard || return 1
  ruleset=$(dmb_nft_ruleset) || return 1
  dmb_nft_foreign_guard "$selector" <(printf '%s\n' "$ruleset") || return 1
  if [[ $(jq -r .mode <<< "$selector") == scoped ]]; then
    jq -e --argjson s "$selector" '[.nftables[]|.chain? // empty|select(.family==$s.family and .table==$s.table and .name==$s.parent_chain and .type=="filter" and .hook=="input")]|length==1' <<< "$ruleset" >/dev/null || return 1
  fi
  existing=$(dmb_nft_state_from_ruleset "$selector" <(printf '%s\n' "$ruleset")) || return 1
  dmb_nft_state_validate "$selector" "$existing" || return
  pipeline=$(dmb_nft_pipeline_hash "$selector" <(printf '%s\n' "$ruleset")) || return
  jq -nSc --argjson selector "$selector" --argjson baseline "$existing" --arg pipeline "$pipeline" '{schema:1,nft:$selector,baseline:$baseline,pipeline_sha256:$pipeline}'
}

# Enrollment never grants mixed control to competing firewall managers.
# Detection is read-only. Inactive installed software alone is not a conflict.
dmb_nft_systemd_service_guard() {
  local service=$1 data load active enabled job
  case $service in ufw.service|firewalld.service|shorewall.service|nftables.service) ;; *) return 64;; esac
  data=$(dmb_run 5 systemctl show --no-pager --property=LoadState,ActiveState,UnitFileState,Job "$service") || { dmb_nft_error "cannot inspect competing manager state: $service"; return 1; }
  load=$(sed -n 's/^LoadState=//p' <<< "$data")
  active=$(sed -n 's/^ActiveState=//p' <<< "$data")
  enabled=$(sed -n 's/^UnitFileState=//p' <<< "$data")
  job=$(sed -n 's/^Job=//p' <<< "$data")
  case $load in
    not-found) [[ $active == inactive ]] || return 1;;
    loaded|masked)
      [[ $active == inactive || $active == failed ]] || { dmb_nft_error "competing manager is active/changing: $service"; return 1; }
      [[ -z $job || $job == 0 ]] || { dmb_nft_error "competing manager has a queued job: $service"; return 1; }
      case $enabled in disabled|masked|masked-runtime) ;; *) dmb_nft_error "competing manager may reload policy at boot: $service"; return 1;; esac;;
    *) dmb_nft_error "unknown competing manager state: $service"; return 1;;
  esac
}
dmb_nft_manager_guard() {
  local entry comm family save names data service
  for family in ip ip6; do
    names=/proc/net/${family}_tables_names
    [[ -r $names ]] || continue
    grep -qx filter "$names" || continue
    if [[ $family == ip ]]; then save=iptables-legacy-save; else save=ip6tables-legacy-save; fi
    command -v "$save" >/dev/null || { dmb_nft_error "legacy $family filter ownership cannot be inspected"; return 1; }
    data=$(dmb_run 5 "$save" -t filter) || return 1
    if ! grep -Eq '^:INPUT ACCEPT ' <<< "$data" || grep -Eq '^-A INPUT([[:space:]]|$)' <<< "$data"; then
      dmb_nft_error "legacy $family INPUT policy has separate ownership"; return 1
    fi
  done
  for entry in /proc/[0-9]*/comm; do
    [[ -r $entry ]] || continue
    IFS= read -r comm < "$entry" || continue
    case "$comm" in firewalld|ufw|shorewall|ferm) dmb_nft_error "competing active firewall manager: $comm"; return 1;; esac
  done
  if [[ -d /run/systemd/system ]]; then
    dmb_need systemctl || return 1
    for service in ufw.service firewalld.service shorewall.service nftables.service; do
      dmb_nft_systemd_service_guard "$service" || return
    done
  elif command -v rc-service >/dev/null; then
    local service
    for service in ufw firewalld shorewall nftables; do
      if dmb_run 5 rc-service "$service" status >/dev/null 2>&1; then
        dmb_nft_error "competing active firewall manager: $service"; return 1
      fi
    done
  fi
}

dmb_nft_plan() {
  local config=$1 manifest selector listeners rules policy state before origin
  manifest=$(dmb_nft_enrollment_path) || return 1
  dmb_trusted_path "$manifest" || return 1
  selector=$(jq -ce '.nft' "$manifest") || return 1
  jq -e --argjson s "$selector" '.schema==1 and .nft==$s' "$config" >/dev/null || return 2
  listeners=$(jq -ce '.listeners|select(type=="array")' "$config") || return 2
  rules=$(jq -ce '.rules // []' "$config") || return 2
  policy=$(jq -er '.policy // "drop"' "$config") || return 2
  local address
  while IFS= read -r address; do dmb_ip_parse "$address" >/dev/null || return 2; done < <(jq -r '.[].address' <<< "$listeners")
  state=$(dmb_nft_plan_state "$selector" "$listeners" "$rules" "$policy") || return 2
  before=$(dmb_nft_capture <(jq -cn --argjson s "$selector" '{steps:[{nft:$s}]}') 0) || return 1
  origin=$(dmb_confirmation_origin) || { dmb_nft_error 'cannot identify an authenticated origin transport'; return 78; }
  jq -cnS --argjson s "$selector" --argjson before "$before" --argjson after "$state" --argjson origin "$origin" --argjson allow_console "$(jq -c '.confirmation.allow_console_confirmation==true' "$DMB_STATE_DIR/enrollments/ssh.json")" '{schema:1,adapter:"nft",resources:[("nft-"+$s.family+"-"+$s.table)],steps:[{id:"nft-policy",resource:("nft-"+$s.family+"-"+$s.table),adapter:"nft",nft:$s,before:$before,after:$after}],requires_confirmation:true,confirmation_enrollment:"ssh",allow_console_confirmation:$allow_console,origin_transport:$origin,confirm_port:$origin.local_port}'
}

dmb_nft_snapshot() {
  local plan=$1 index=$2 txdir=$3 raw data hash authority
  raw="$txdir/snapshots/nft-$index.raw.json"
  [[ ! -e $raw ]] || return 1
  data=$(dmb_nft_ruleset) || return 1
  jq -e '.nftables|type=="array"' <<< "$data" >/dev/null || return 1
  hash=$(printf '%s\n' "$data" | sha256sum | cut -d' ' -f1) || return 1
  printf '%s\n' "$data" | dmb_atomic_write "$raw" 0600 "$hash" || return 1
  sha256sum "$raw" | dmb_atomic_write "$raw.sha256" 0600 || return
  authority=$(dmb_nft_enrollment_path) || return
  dmb_trusted_path "$authority" || return
  hash=$(sha256sum "$authority" | cut -d' ' -f1) || return
  dmb_atomic_write "$txdir/snapshots/nft-$index-authority.json" 0600 "$hash" < "$authority" || return
  printf '%s\n' "$hash" | dmb_atomic_write "$txdir/snapshots/nft-$index-authority.sha256" 0600
}

dmb_nft_boot_id() { cat /proc/sys/kernel/random/boot_id; }
# Narrow readonly proof that a fresh boot cleared an exclusively owned kernel
# table. It is not permission to repair an arbitrary missing/mismatched table.
# Scoped parents are foreign resources and are never recreated by this path.
dmb_nft_owned_reset() {
  local plan=$1 index=$2 tx=$3 selector origin boot actual authority saved hash expected raw first original observed before
  selector=$(dmb_nft_selector "$plan" "$index") || return
  [[ $(jq -er .mode <<< "$selector") == exclusive ]] || return 76
  dmb_trusted_path "$tx/state.json" || return
  origin=$(jq -er '.boot_id|select(test("^[a-f0-9-]{36}$"))' "$tx/state.json") || return
  boot=$(dmb_nft_boot_id) || return
  [[ $boot =~ ^[a-f0-9-]{36}$ && $boot != "$origin" ]] || return 76
  before=$(jq -ceS --argjson i "$index" '.steps[$i].before' "$plan") || return
  [[ $(jq -r .present <<< "$before") == true ]] || return 76
  dmb_nft_state_validate "$selector" "$before" || return
  authority=$(dmb_nft_enrollment_path) || return
  saved=$tx/snapshots/nft-$index-authority.json
  for actual in "$authority" "$saved" "$tx/snapshots/nft-$index-authority.sha256"; do dmb_trusted_path "$actual" || return; done
  IFS= read -r expected < "$tx/snapshots/nft-$index-authority.sha256" || return
  [[ $expected =~ ^[a-f0-9]{64}$ ]] || return
  hash=$(sha256sum "$saved" | cut -d' ' -f1) || return
  [[ $hash == "$expected" && $(sha256sum "$authority" | cut -d' ' -f1) == "$expected" ]] || return 76
  raw=$tx/snapshots/nft-$index.raw.json
  dmb_trusted_path "$raw" && dmb_trusted_path "$raw.sha256" || return
  read -r expected _ < "$raw.sha256" || return
  [[ $expected =~ ^[a-f0-9]{64}$ && $(sha256sum "$raw" | cut -d' ' -f1) == "$expected" ]] || return 76
  first=$(jq -er --argjson s "$selector" '[.steps|to_entries[]|select(.value.nft==$s)]|first.key' "$plan") || return
  original=$(jq -ceS --argjson i "$first" '.steps[$i].before' "$plan") || return
  [[ $(jq -r .present <<< "$original") == true ]] || return 76
  dmb_nft_state_validate "$selector" "$original" || return
  observed=$(dmb_nft_state_from_ruleset "$selector" "$raw") || return
  [[ $observed == "$original" ]] || return 76
  # capture rechecks manager ownership, foreign hooks and the enrolled pipeline.
  actual=$(dmb_nft_capture "$plan" "$index") || return
  [[ $actual == '{"nftables":[],"present":false}' ]] || return 76
}

dmb_nft_verify_restore() {
  local actual expected
  actual=$(dmb_nft_capture "$1" "$2") || return 1
  expected=$(jq -ceS --argjson i "$2" '.steps[$i].before' "$1") || return 1
  [[ $actual == "$expected" ]]
}
