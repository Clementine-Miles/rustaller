# shellcheck shell=bash
# Opt-in runtime tuning and local maintenance. No command installs dependencies.
dmb_tcp_report() {
  dmb_need jq sysctl || return
  local current available qdisc
  current=$(dmb_run 3 sysctl -n net.ipv4.tcp_congestion_control) || return
  available=$(dmb_run 3 sysctl -n net.ipv4.tcp_available_congestion_control) || return
  qdisc=$(dmb_run 3 sysctl -n net.core.default_qdisc) || return
  jq -cn --arg current "$current" --arg available "$available" --arg qdisc "$qdisc" '{schema:1,current:$current,available:($available|split(" ")|map(select(length>0))),default_qdisc:$qdisc,performance:"unmeasured"}'
}
dmb_tcp_measure_request() {
  local file=$1 target port seconds
  [[ -f $file && ! -L $file ]] || return 64
  jq -e '.schema==1 and .requested==true and (.target|type=="string") and (.port|type=="number" and floor==. and .>=1 and .<=65535) and (.seconds|type=="number" and floor==. and .>=2 and .<=10) and ((.min_bits_per_second // 0)|type=="number" and .>=0)' "$file" >/dev/null || return 64
  target=$(jq -er .target "$file") || return
  # A numeric target prevents implicit DNS requests and shell/option ambiguity.
  target=$(dmb_ip_parse "$target") || return 64
  port=$(jq -r .port "$file"); seconds=$(jq -r .seconds "$file")
  jq -cS --arg target "$target" '.target=$target|{schema,requested,target,port,seconds,min_bits_per_second:(.min_bits_per_second // 0)}' "$file"
}
dmb_tcp_persistent_owner_free() {
  local own=$1 file
  [[ ! -e $own && ! -L $own ]] || { dmb_error 'Existing TCP profile needs explicit ownership enrollment'; return 78; }
  for file in /etc/sysctl.conf /etc/sysctl.d/*.conf /run/sysctl.d/*.conf /usr/local/lib/sysctl.d/*.conf /usr/lib/sysctl.d/*.conf /lib/sysctl.d/*.conf; do
    [[ -f $file ]] || continue
    if LC_ALL=C grep -Eq '^[[:space:]]*-?net[./]ipv4[./]tcp_congestion_control[[:space:]]*=' "$file"; then dmb_error "Competing persistent TCP owner: $file"; return 78; fi
  done
}
dmb_tcp_plan() {
  local profile=${1-} request_file=${2-} scope=${3:-runtime} request value current step steps conf content desired
  dmb_need jq sysctl iperf3 || return
  [[ $scope == runtime || $scope == persistent ]] || return 64
  case "$profile" in bbr) value=bbr;; cubic) value=cubic;; *) dmb_error 'Supported measured TCP profiles: bbr, cubic'; return 64;; esac
  request=$(dmb_tcp_measure_request "$request_file") || return
  current=$(dmb_run 3 sysctl -n net.ipv4.tcp_available_congestion_control) || return
  [[ " $current " == *" $value "* ]] || { dmb_error 'Requested algorithm is not already available; load modules explicitly outside this tool'; return 78; }
  step=$(jq -cn --arg value "$value" --argjson request "$request" '{id:"tcp-congestion-control",adapter:"tcp",resource:"sysctl:net.ipv4.tcp_congestion_control",key:"net.ipv4.tcp_congestion_control",measurement:$request,after:{value:$value}}') || return
  steps=$(jq -cn --argjson step "$step" '[$step]') || return
  if [[ $scope == persistent ]]; then
    conf=/etc/sysctl.d/90-dmitbox-local-tcp.conf
    dmb_tcp_persistent_owner_free "$conf" || return
    # Build in a private local workspace only when persistent planning is explicit.
    # No privileged config is written; the resulting fs step owns its new file.
    dmb_require_root || return
    dmb_trusted_path /etc/sysctl.d || return
    content=$(mktemp) || return
    printf '# dmitbox-local profile schema=1\nnet.ipv4.tcp_congestion_control = %s\n' "$value" > "$content" || { rm -f -- "$content"; return 74; }
    desired=$(dmb_file_desired "$conf" "$content" 600) || { rm -f -- "$content"; return; }
    local encoded
    encoded=$(base64 -w0 -- "$content") || { rm -f -- "$content"; return 74; }
    rm -f -- "$content" || return
    steps=$(jq -cn --argjson step "$step" --arg path "$conf" --arg content "$encoded" --argjson desired "$desired" '[{id:"tcp-persistent",adapter:"fs",purpose:"tcp-profile",resource:("file:"+$path),path:$path,content_b64:$content,after:$desired},$step]') || return
  fi
  jq -cn --argjson steps "$steps" --arg scope "$scope" '{schema:1,adapter:"tcp",scope:$scope,timeout_seconds:300,resources:[$steps[].resource],steps:$steps}'
}
dmb_tcp_step() { jq -ce --argjson i "$2" '.steps[$i]|select(.key=="net.ipv4.tcp_congestion_control")' "$1"; }
dmb_tcp_capture() {
  local step key value
  step=$(dmb_tcp_step "$1" "$2") || return 64
  key=$(jq -r .key <<< "$step")
  value=$(dmb_run 3 sysctl -n "$key") || return
  [[ $value =~ ^[A-Za-z0-9_-]{1,32}$ ]] || return 78
  jq -cn --arg value "$value" '{value:$value}'
}
dmb_tcp_probe() {
  local request=$1 target port seconds result min
  [[ $(jq -r '.requested' <<< "$request") == true ]] || return 64
  target=$(jq -er .target <<< "$request") || return
  target=$(dmb_ip_parse "$target") || return 64
  port=$(jq -er .port <<< "$request") || return
  seconds=$(jq -er .seconds <<< "$request") || return
  [[ $port =~ ^[0-9]+$ && $seconds =~ ^[0-9]+$ ]] && ((port>=1 && port<=65535 && seconds>=2 && seconds<=10)) || return 64
  result=$(dmb_run "$((seconds+8))" iperf3 -J -c "$target" -p "$port" -t "$seconds" --connect-timeout 3000) || { dmb_error 'Requested throughput probe failed or was blocked; performance is inconclusive'; return 69; }
  min=$(jq -r '.min_bits_per_second // 0' <<< "$request") || return
  jq -ce --argjson minimum "$min" 'select(.error==null)|.end.sum_received.bits_per_second as $rate|select(($rate|type)=="number" and $rate>0 and $rate>=$minimum)|{schema:1,bits_per_second:$rate,source:"iperf3",minimum:$minimum}' <<< "$result" || { dmb_error 'Throughput postcondition was not proved'; return 69; }
}
dmb_tcp_snapshot() {
  local step result
  step=$(dmb_tcp_step "$1" "$2") || return
  result=$(dmb_tcp_probe "$(jq -c .measurement <<< "$step")") || return
  printf '%s\n' "$result" | dmb_atomic_write "$3/snapshots/tcp-$2-baseline.json" 0600
}
dmb_tcp_set() {
  local key=$1 value=$2 available
  [[ $key == net.ipv4.tcp_congestion_control && $value =~ ^[A-Za-z0-9_-]{1,32}$ ]] || return 64
  available=$(dmb_run 3 sysctl -n net.ipv4.tcp_available_congestion_control) || return
  [[ " $available " == *" $value "* ]] || return 78
  dmb_run 3 sysctl -w "$key=$value" >/dev/null
}
dmb_tcp_apply() {
  local step before current
  step=$(dmb_tcp_step "$1" "$2") || return
  before=$(jq -cS .before <<< "$step"); current=$(dmb_tcp_capture "$1" "$2" | jq -cS .) || return
  [[ $current == "$before" ]] || return 76
  dmb_tcp_set "$(jq -r .key <<< "$step")" "$(jq -r .after.value <<< "$step")"
}
dmb_tcp_verify() {
  local step current expected result baseline
  step=$(dmb_tcp_step "$1" "$2") || return
  current=$(dmb_tcp_capture "$1" "$2" | jq -cS .) || return
  expected=$(jq -cS .after <<< "$step"); [[ $current == "$expected" ]] || return 76
  result=$(dmb_tcp_probe "$(jq -c .measurement <<< "$step")") || return
  baseline=$(jq -ce . "$3/snapshots/tcp-$2-baseline.json") || return
  jq -cn --argjson baseline "$baseline" --argjson after "$result" '{schema:1,baseline:$baseline,after:$after,ratio:($after.bits_per_second/$baseline.bits_per_second),claim:"Measured throughput for the requested target; no general performance guarantee"}' | dmb_atomic_write "$3/snapshots/tcp-$2-after.json" 0600
}
dmb_tcp_restore() {
  local step current before after
  step=$(dmb_tcp_step "$1" "$2") || return
  current=$(dmb_tcp_capture "$1" "$2" | jq -cS .) || return
  before=$(jq -cS .before <<< "$step"); after=$(jq -cS .after <<< "$step")
  [[ $current != "$before" ]] || return 0
  [[ $current == "$after" ]] || return 76
  dmb_tcp_set "$(jq -r .key <<< "$step")" "$(jq -r .before.value <<< "$step")" || return
  [[ $(dmb_tcp_capture "$1" "$2" | jq -cS .) == "$before" ]]
}

# Swap creation never overwrites a resource. Private staging and the final hard
# link must reside on the same ext4/XFS filesystem; activation is a separate step.
dmb_swap_report() {
  dmb_need swapon jq || return
  local report
  report=$(dmb_run 3 swapon --show --json --bytes --output NAME,TYPE,SIZE,USED,PRIO) || return
  jq -ceS 'select((.swapdevices|type)=="array" and all(.swapdevices[]; (.name|type)=="string" and (.prio|type)=="number" and (.prio|floor)==.prio))' <<< "$report"
}
dmb_swap_step() {
  jq -ce --argjson i "$2" '.steps[$i]|select(
    (.operation=="create" or .operation=="activate") and
    (.owner_id|type=="string" and test("^[a-f0-9]{32}$")) and
    (.path|type=="string" and startswith("/") and (test("[[:space:]]")|not)) and
    (.filesystem=="ext4" or .filesystem=="xfs") and
    (.after.exists==true and .after.type=="file" and .after.uid==0 and .after.gid==0 and .after.mode=="600" and .after.nlink==1) and
    (.after.size_bytes|type=="number" and floor==. and .>=16777216 and .<=1099511627776 and .%1048576==0) and
    (.after.swap_uuid|type=="string" and test("^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$")) and
    (if .operation=="create" then .after.active==false and .after.priority==null else .after.active==true and (.after.priority|type=="number" and floor==. and .>=0 and .<=32767) end)
  )' "$1"
}
dmb_swap_path_valid() {
  local path=${1-} parent
  [[ $path == /* && $path != / && $path != */ && $path != *[[:space:]]* && $path != *'//'* && $path != *'/../'* && $path != */.. && $path != *'/./'* && $path != */. ]] || return 64
  parent=${path%/*}; [[ -n $parent ]] || parent=/
  dmb_trusted_path "$parent" || return
  [[ ! -L $path ]] || return 78
}
dmb_swap_filesystem() {
  local path=$1 json fstype options
  json=$(dmb_run 3 findmnt --json --target "$path" --output FSTYPE,OPTIONS) || return
  [[ $(jq '.filesystems|length' <<< "$json") == 1 ]] || return 78
  fstype=$(jq -er '.filesystems[0].fstype' <<< "$json") || return
  options=$(jq -er '.filesystems[0].options' <<< "$json") || return
  [[ $fstype == ext4 || $fstype == xfs ]] || { dmb_error 'Swap creation supports verified local ext4/XFS only'; return 78; }
  [[ ,$options, != *,ro,* ]] || return 78
  printf '%s\n' "$fstype"
}
dmb_swap_capacity() {
  local path=$1 bytes=$2 blocks blocksize data
  data=$(dmb_run 3 stat -f -c '%a %S' -- "$path") || return
  read -r blocks blocksize <<< "$data"
  [[ $blocks =~ ^[0-9]{1,15}$ && $blocksize =~ ^[1-9][0-9]{0,8}$ ]] || return 78
  ((blocks <= 9223372036854775807 / blocksize)) || return 78
  # Reserve room for recovery journals; concurrent writes can still exhaust a
  # filesystem, so rollback first removes verified private staging allocations.
  ((blocks*blocksize >= bytes+67108864)) || { dmb_error 'Insufficient free space plus recovery reserve'; return 78; }
}
dmb_swap_plan_create() {
  local path=${1-} mib=${2-} priority=${3-} parent filesystem owner uuid bytes logical
  dmb_need jq findmnt stat fallocate mkswap swapon swapoff blkid ln sync || return
  dmb_require_root || return
  dmb_swap_path_valid "$path" || return
  [[ ! -e $path && ! -L $path ]] || { dmb_error 'Refusing pre-existing swap path'; return 78; }
  [[ $mib =~ ^[1-9][0-9]{0,6}$ && $priority =~ ^[0-9]{1,5}$ ]] && ((mib>=16 && mib<=1048576 && priority<=32767)) || return 64
  parent=${path%/*}; [[ -n $parent ]] || parent=/
  filesystem=$(dmb_swap_filesystem "$parent") || return
  owner=$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n') || return
  uuid=${owner:0:8}-${owner:8:4}-${owner:12:4}-${owner:16:4}-${owner:20:12}
  bytes=$((mib*1024*1024))
  dmb_swap_capacity "$parent" "$bytes" || return
  logical=$(jq -cn --arg uuid "$uuid" --argjson bytes "$bytes" '{exists:true,type:"file",size_bytes:$bytes,uid:0,gid:0,mode:"600",nlink:1,swap_uuid:$uuid,active:false,priority:null}') || return
  jq -cn --arg path "$path" --arg owner "$owner" --arg fs "$filesystem" --argjson logical "$logical" --argjson priority "$priority" '{schema:1,adapter:"swap",scope:"runtime",timeout_seconds:300,resources:[("swap:"+$path)],steps:[{id:"swap-create",resource:("swap:"+$path),adapter:"swap",operation:"create",path:$path,owner_id:$owner,filesystem:$fs,after:$logical},{id:"swap-activate",resource:("swap:"+$path),adapter:"swap",operation:"activate",path:$path,owner_id:$owner,filesystem:$fs,after:($logical+{active:true,priority:$priority})}],limitations:["Runtime activation only; no fstab editing","Creation requires transaction state and target on the same filesystem","Swapoff can fail under memory pressure; such rollback is reported failed"]}'
}
dmb_swap_capture() {
  local step path uuid active=false priority=null row size uid gid mode nlink
  step=$(dmb_swap_step "$1" "$2") || return
  path=$(jq -er .path <<< "$step") || return
  dmb_swap_path_valid "$path" || return
  if [[ ! -e $path ]]; then printf '{"exists":false}\n'; return; fi
  [[ -f $path && ! -L $path ]] || return 78
  size=$(stat -c %s -- "$path"); uid=$(stat -c %u -- "$path"); gid=$(stat -c %g -- "$path"); mode=$(stat -c %a -- "$path"); nlink=$(stat -c %h -- "$path")
  uuid=$(dmb_run 3 blkid -p -s UUID -o value -- "$path") || return 78
  [[ $(dmb_run 3 blkid -p -s TYPE -o value -- "$path") == swap ]] || return 78
  row=$(dmb_swap_report | jq -c --arg path "$path" '[.swapdevices[]?|select(.name==$path)]') || return
  [[ $(jq length <<< "$row") -le 1 ]] || return 78
  if [[ $(jq length <<< "$row") == 1 ]]; then active=true; priority=$(jq -r '.[0].prio' <<< "$row"); [[ $priority =~ ^-?[0-9]+$ ]] || return 78; fi
  jq -cn --argjson size "$size" --argjson uid "$uid" --argjson gid "$gid" --arg mode "$mode" --argjson nlink "$nlink" --arg uuid "$uuid" --argjson active "$active" --argjson priority "$priority" '{exists:true,type:"file",size_bytes:$size,uid:$uid,gid:$gid,mode:$mode,nlink:$nlink,swap_uuid:$uuid,active:$active,priority:$priority}'
}
dmb_swap_record_path() { local step; step=$(dmb_swap_step "$1" "$2") || return; printf '%s/snapshots/swap-%s.json\n' "$3" "$(jq -er .owner_id <<< "$step")"; }
dmb_swap_identity_check() {
  local path=$1 record=$2 expected actual
  dmb_trusted_path "$record" || return
  expected=$(jq -er .identity "$record") || return
  [[ -f $path && ! -L $path && $(stat -c '%u:%g:%a' -- "$path") == 0:0:600 ]] || return 76
  actual=$(stat -c '%d:%i' -- "$path") || return
  [[ $actual == "$expected" ]] || return 76
}
dmb_swap_apply() {
  local plan=$1 i=$2 tx=$3 step path parent operation before current record stage identity bytes filesystem context
  step=$(dmb_swap_step "$plan" "$i") || return
  path=$(jq -er .path <<< "$step"); operation=$(jq -er .operation <<< "$step")
  parent=${path%/*}; [[ -n $parent ]] || parent=/
  current=$(dmb_swap_capture "$plan" "$i" | jq -cS .) || return
  before=$(jq -cS .before <<< "$step"); [[ $current == "$before" ]] || return 76
  record=$(dmb_swap_record_path "$plan" "$i" "$tx") || return
  if [[ $operation == activate ]]; then
    dmb_swap_identity_check "$path" "$record" || return
    dmb_run 10 swapon --priority "$(jq -r .after.priority <<< "$step")" -- "$path"
    return
  fi
  [[ $(jq -r '.before.exists' <<< "$step") == false ]] || return 78
  dmb_trusted_path "$tx/snapshots" || return
  [[ $(stat -c %d -- "$parent") == "$(stat -c %d -- "$tx/snapshots")" ]] || { dmb_error 'Swap staging and target must share a filesystem'; return 78; }
  filesystem=$(dmb_swap_filesystem "$parent") || return
  [[ $filesystem == "$(jq -r .filesystem <<< "$step")" ]] || return 76
  bytes=$(jq -r .after.size_bytes <<< "$step")
  dmb_swap_capacity "$parent" "$bytes" || return
  stage="$tx/snapshots/swap-$(jq -r .owner_id <<< "$step").stage"
  # The engine owns this private directory from PREPARED; an unexpected stage
  # is never truncated. A killed write remains confined to this known slot.
  [[ ! -e $stage && ! -L $stage && ! -e $record ]] || return 76
  ( umask 077; set -o noclobber; : > "$stage" ) || return
  identity=$(stat -c '%d:%i' -- "$stage") || return
  jq -cn --arg identity "$identity" --arg path "$path" --arg stage "$stage" --arg owner "$(jq -r .owner_id <<< "$step")" '{schema:1,identity:$identity,path:$path,stage:$stage,owner_id:$owner}' | dmb_atomic_write "$record" 0600 || return
  bytes=$(jq -r .after.size_bytes <<< "$step")
  [[ $bytes =~ ^[0-9]+$ ]] && ((bytes>=16777216 && bytes<=1099511627776)) || return 64
  dmb_run 15 fallocate -l "$bytes" -- "$stage" || return
  [[ $(stat -c %s -- "$stage") == "$bytes" ]] || return 74
  (( $(stat -c %b -- "$stage") * 512 >= bytes )) || { dmb_error 'Swap allocation contains holes'; return 78; }
  dmb_run 5 mkswap --uuid "$(jq -r .after.swap_uuid <<< "$step")" -- "$stage" >/dev/null || return
  # A security label must match the final path before the inode is linked.
  if command -v getenforce >/dev/null 2>&1 && [[ $(getenforce) != Disabled ]]; then
    dmb_need matchpathcon chcon || return
    context=$(matchpathcon -n -- "$path") || return
    dmb_run 3 chcon "$context" "$stage" || return
    [[ $(stat -c %C -- "$stage") == "$context" ]] || return 78
  fi
  sync -f "$stage" || return
  # Atomic no-replace creation; unrelated files and links are never overwritten.
  ln -T -- "$stage" "$path" || return
  rm -- "$stage" || return
  sync -f "$parent" || return
  dmb_swap_identity_check "$path" "$record"
}
dmb_swap_verify() {
  local current expected record path
  current=$(dmb_swap_capture "$1" "$2" | jq -cS .) || return
  expected=$(jq -cS --argjson i "$2" '.steps[$i].after' "$1") || return
  [[ $current == "$expected" ]] || return 76
  record=$(dmb_swap_record_path "$1" "$2" "$3") || return
  path=$(jq -er --argjson i "$2" '.steps[$i].path' "$1") || return
  dmb_swap_identity_check "$path" "$record"
}
dmb_swap_restore() {
  local step current before after path parent operation record
  step=$(dmb_swap_step "$1" "$2") || return
  path=$(jq -er .path <<< "$step"); operation=$(jq -er .operation <<< "$step")
  parent=${path%/*}; [[ -n $parent ]] || parent=/
  current=$(dmb_swap_capture "$1" "$2" | jq -cS .) || return
  before=$(jq -cS .before <<< "$step"); after=$(jq -cS .after <<< "$step")
  [[ $current != "$before" ]] || return 0
  [[ $current == "$after" ]] || return 76
  record=$(dmb_swap_record_path "$1" "$2" "$3") || return
  dmb_swap_identity_check "$path" "$record" || return
  if [[ $operation == activate ]]; then dmb_run 15 swapoff -- "$path" || return
  else
    [[ $(jq -r .active <<< "$current") == false && $(stat -c %h -- "$path") == 1 ]] || return 76
    rm -- "$path" || return
    sync -f "$parent" || return
  fi
  [[ $(dmb_swap_capture "$1" "$2" | jq -cS .) == "$before" ]]
}
dmb_swap_cleanup() {
  local step stage record path
  step=$(dmb_swap_step "$1" "$2") || return
  [[ $(jq -r .operation <<< "$step") == create ]] || return 0
  stage="$3/snapshots/swap-$(jq -r .owner_id <<< "$step").stage"
  record=$(dmb_swap_record_path "$1" "$2" "$3") || return
  [[ -e $stage || -L $stage ]] || return 0
  # The stage is always inside the private engine-owned snapshot directory.
  # If a crash preceded the identity record, only an empty regular slot with
  # one link and no activation can be discarded; larger unknown files conflict.
  if [[ ! -e $record ]]; then
    [[ -f $stage && ! -L $stage && $(stat -c '%s:%h:%u:%a' -- "$stage") == 0:1:0:600 ]] || return 76
  else
    [[ $(jq -er .path "$record") == "$(jq -r .path <<< "$step")" && $(jq -er .stage "$record") == "$stage" && $(jq -er .owner_id "$record") == "$(jq -r .owner_id <<< "$step")" ]] || return 76
    dmb_swap_identity_check "$stage" "$record" || return
  fi
  local active_name encoded active_identity stage_identity swap_report
  swap_report=$(dmb_swap_report) || return
  stage_identity=$(stat -c '%d:%i' -- "$stage") || return
  while IFS= read -r encoded; do
    active_name=$(printf '%s' "$encoded" | base64 -d) || return
    active_identity=$(stat -c '%d:%i' -- "$active_name") || return 76
    [[ $active_identity != "$stage_identity" ]] || return 76
  done < <(jq -r '.swapdevices[]|.name|@base64' <<< "$swap_report")
  path=$(jq -er .path <<< "$step") || return
  if [[ -e $path && $(stat -c '%d:%i' -- "$path") == "$(stat -c '%d:%i' -- "$stage")" ]]; then
    # Complete the interrupted link/unlink boundary only while inactive and
    # exactly matching our recorded UUID. Never unlink an active swap inode.
    [[ $(dmb_swap_report | jq --arg path "$path" '[.swapdevices[]?|select(.name==$path)]|length') == 0 ]] || return 76
    [[ $(dmb_run 3 blkid -p -s UUID -o value -- "$path") == "$(jq -r .after.swap_uuid <<< "$step")" ]] || return 76
    rm -- "$path" || return
  fi
  rm -- "$stage" || return
  sync -f "$3/snapshots"
}

# Explicit package maintenance is intentionally outside compensating network
# transactions: package upgrades have no honest generic inverse.
dmb_runtime_capture() {
  local seconds=$1 executable=$2 output rc=0; shift 2
  if ! command -v "$executable" >/dev/null 2>&1; then jq -cn --arg name "$executable" '{available:false,command:$name,exit_code:69,output:"Missing dependency; no installation attempted"}'; return; fi
  output=$(dmb_run "$seconds" "$executable" "$@" 2>&1) || rc=$?
  jq -cn --arg name "$executable" --arg output "$output" --argjson rc "$rc" '{available:true,command:$name,exit_code:$rc,output:$output}'
}
dmb_maintenance_report() {
  dmb_need jq || return
  local family=${1:-system} manager=none name first second third
  case "$family" in
    system)
      for name in apt-get dnf yum pacman apk; do if command -v "$name" >/dev/null 2>&1; then manager=$name; break; fi; done
      jq -cn --arg kernel "$(uname -r)" --arg machine "$(uname -m)" --arg manager "$manager" --arg uptime "$(cut -d' ' -f1 /proc/uptime)" '{schema:1,kernel:$kernel,machine:$machine,package_manager:$manager,uptime_seconds:$uptime,network_probes:false}';;
    resources)
      first=$(dmb_runtime_capture 5 free -b) || return
      second=$(dmb_runtime_capture 5 df -P -B1) || return
      third=$(dmb_runtime_capture 5 ps -eo pid,user,pcpu,pmem,comm) || return
      jq -cn --argjson memory "$first" --argjson filesystems "$second" --argjson processes "$third" '{schema:1,family:"resources",memory:$memory,filesystems:$filesystems,processes:$processes,network_probes:false}';;
    ports)
      first=$(dmb_runtime_capture 5 ss -lntup) || return
      second=$(dmb_runtime_capture 5 ss -s) || return
      jq -cn --argjson listeners "$first" --argjson sockets "$second" '{schema:1,family:"ports",listeners:$listeners,sockets:$sockets,external_accessibility:"unverified",network_probes:false}';;
    security)
      first=$(dmb_runtime_capture 5 fail2ban-client status) || return
      second=$(dmb_runtime_capture 5 fail2ban-client status sshd) || return
      third=$(dmb_runtime_capture 5 ss -lntp) || return
      jq -cn --argjson f2b "$first" --argjson jail "$second" --argjson sockets "$third" '{schema:1,family:"security",fail2ban:$f2b,sshd_jail:$jail,listeners:$sockets,authentication_policy:"Use enrolled SSH context report",external_accessibility:"unverified",network_probes:false}';;
    guest)
      first=$(dmb_runtime_capture 5 systemd-detect-virt) || return
      second=$(dmb_runtime_capture 5 systemctl show qemu-guest-agent.service cloud-init.service --property=Id,LoadState,ActiveState,UnitFileState) || return
      jq -cn --argjson virtualization "$first" --argjson services "$second" '{schema:1,family:"guest",virtualization:$virtualization,systemd_services:$services,cloud_init_installation:"Unsupported until configuration ownership is explicitly enrolled",network_probes:false}';;
    cleanup)
      first=$(dmb_runtime_capture 5 du -sk /var/cache/apt/archives /var/cache/pacman/pkg /var/cache/apk) || return
      second=$(dmb_runtime_capture 5 journalctl --disk-usage) || return
      jq -cn --argjson cache "$first" --argjson journal "$second" '{schema:1,family:"cleanup",package_cache:$cache,journal:$journal,mutations:false,network_probes:false}';;
    time)
      first=$(dmb_runtime_capture 5 timedatectl show --property=Timezone,NTP,NTPSynchronized,LocalRTC) || return
      second=$(dmb_runtime_capture 5 date --iso-8601=seconds) || return
      jq -cn --argjson manager "$first" --argjson clock "$second" '{schema:1,family:"time",manager:$manager,clock:$clock,network_probes:false}';;
    *) return 64;;
  esac
}
dmb_runtime_snapshot() {
  local root dir source slot state link identity xattrs_before xattrs_after snapshot_manifest='[]'
  root=$(dmb_state_root)/snapshots
  dmb_trusted_path "$root" allow_missing || return
  if [[ ! -e $root ]]; then mkdir -m 0700 -- "$root" || return; fi
  dmb_trusted_path "$root" || return
  dir=$(mktemp -d "$root/local.XXXXXXXXXXXX") || return
  chmod 700 "$dir" || return
  dmb_maintenance_report system | dmb_atomic_write "$dir/system.json" 0600 || return
  dmb_maintenance_report resources | dmb_atomic_write "$dir/resources.json" 0600 || return
  for source in /etc/sysctl.conf /etc/gai.conf /etc/fstab /etc/ssh/sshd_config /etc/resolv.conf; do
    [[ -e $source || -L $source ]] || continue
    slot=${source//\//_}; slot=${slot#_}
    dmb_trusted_path "${source%/*}" || return
    if [[ -L $source ]]; then
      # Copy the link itself, never its referent or an arbitrary manager path.
      dmb_need getfattr || return
      link=$(readlink -- "$source") || return
      xattrs_before=$(LC_ALL=C getfattr --no-dereference --absolute-names --encoding=hex -d -m - -- "$source" | sed '/^#/d;/^$/d' | LC_ALL=C sort | base64 -w0) || return
      identity=$(stat -c '%d:%i:%u:%g:%a' -- "$source") || return
      cp -a -- "$source" "$dir/$slot.link" || return
      [[ $(readlink -- "$dir/$slot.link") == "$link" && $(stat -c '%u:%g:%a' -- "$dir/$slot.link") == "$(stat -c '%u:%g:%a' -- "$source")" && $(stat -c '%d:%i:%u:%g:%a' -- "$source") == "$identity" ]] || return 76
      xattrs_after=$(LC_ALL=C getfattr --no-dereference --absolute-names --encoding=hex -d -m - -- "$dir/$slot.link" | sed '/^#/d;/^$/d' | LC_ALL=C sort | base64 -w0) || return
      [[ $xattrs_before == "$xattrs_after" ]] || return 76
      state=$(jq -cn --arg path "$source" --arg target "$link" --arg identity "$identity" --arg attrs "$xattrs_before" '{path:$path,type:"symlink",target:$target,identity:$identity,xattrs_b64:$attrs}') || return
    else
      dmb_file_capture "$source" "$dir" "$slot" || return
      state=$(jq -c --arg path "$source" '.+{path:$path}' "$dir/$slot.json") || return
    fi
    snapshot_manifest=$(jq -c --argjson state "$state" '.+[$state]' <<< "$snapshot_manifest") || return
  done
  jq -cn --argjson files "$snapshot_manifest" '{schema:1,files:$files,restoration:"Manual review required; this is not a bootable backup and does not include private keys or credentials"}' | dmb_atomic_write "$dir/manifest.json" 0600 || return
  sync -f "$dir" || return
  printf '%s\n' "$dir"
}
dmb_runtime_timezone() {
  local zone=${1-} before after
  [[ $zone =~ ^[A-Za-z0-9_+-]+(/[A-Za-z0-9_+-]+)*$ && $zone != posix/* && $zone != right/* ]] || return 64
  [[ -f /usr/share/zoneinfo/$zone ]] || return 78
  dmb_need timedatectl || return
  before=$(dmb_run 5 timedatectl show --property=Timezone --value) || return
  dmb_run 5 timedatectl set-timezone "$zone" || return
  after=$(dmb_run 5 timedatectl show --property=Timezone --value) || return
  [[ $after == "$zone" ]] || { dmb_error 'Timezone readback differs; inspect manager state'; return 76; }
  jq -cn --arg before "$before" --arg after "$after" '{schema:1,before:$before,after:$after,readback_verified:true}'
}
dmb_maintenance_run() (
  local action=${1-} parameter=${2-} manager=none name fd state rc=0 output
  dmb_require_root || return
  case "$action" in update-packages|clean-package-cache|snapshot|set-timezone|enable-time-sync|clean-journal) ;; *) dmb_error 'Maintenance actions: update-packages, clean-package-cache, snapshot, set-timezone ZONE, enable-time-sync, clean-journal SIZE'; return 64;; esac
  # Maintenance can update running services. Refuse while a pending protected
  # operation exists and hold the global lock so no new operation starts.
  dmb_tx_init || return
  dmb_tx_lock_path "$(dmb_lock_root)/global.lock" || return
  exec {fd}>"$(dmb_lock_root)/global.lock" || return
  flock -w 2 -x "$fd" || return 75
  DMB_LOCK_FDS+=("$fd")
  for state in "$(dmb_state_root)"/transactions/tx-*/state.json; do
    [[ -e $state ]] || continue
    case $(jq -er .state "$state") in COMMITTED|ROLLED_BACK) ;; *) dmb_error 'Resolve pending or failed transactions before maintenance'; return 75;; esac
  done
  dmb_audit maintenance-start "$(jq -cn --arg action "$action" '{action:$action,rollback:false}')" || return
  case "$action" in
    snapshot) dmb_runtime_snapshot; rc=$?;;
    set-timezone) dmb_runtime_timezone "$parameter"; rc=$?;;
    enable-time-sync)
      dmb_need timedatectl systemctl || return
      dmb_run 5 systemctl is-active --quiet systemd-timesyncd.service || { dmb_error 'No already active systemd-timesyncd owner; use your enrolled time manager'; return 78; }
      dmb_run 5 timedatectl set-ntp true && output=$(dmb_run 5 timedatectl show --property=NTP --value) && [[ $output == yes ]]; rc=$?;;
    clean-journal)
      [[ $parameter =~ ^[1-9][0-9]{0,5}[KMGT]$ ]] || return 64
      dmb_need journalctl || return
      dmb_run 30 journalctl --vacuum-size="$parameter"; rc=$?;;
    update-packages|clean-package-cache)
      for name in apt-get dnf yum pacman apk; do if command -v "$name" >/dev/null 2>&1; then manager=$name; break; fi; done
      [[ $manager != none ]] || return 69
      case "$manager:$action" in
        apt-get:update-packages) dmb_run 900 apt-get update && dmb_run 3600 apt-get upgrade; rc=$?;;
        apt-get:clean-package-cache) dmb_run 300 apt-get clean; rc=$?;;
        dnf:update-packages) dmb_run 3600 dnf upgrade; rc=$?;;
        dnf:clean-package-cache) dmb_run 300 dnf clean packages; rc=$?;;
        yum:update-packages) dmb_run 3600 yum update; rc=$?;;
        yum:clean-package-cache) dmb_run 300 yum clean packages; rc=$?;;
        pacman:update-packages) dmb_run 3600 pacman -Syu; rc=$?;;
        pacman:clean-package-cache) dmb_run 300 pacman -Sc; rc=$?;;
        apk:update-packages) dmb_run 900 apk update && dmb_run 3600 apk upgrade; rc=$?;;
        apk:clean-package-cache) dmb_run 300 apk cache clean; rc=$?;;
      esac;;
  esac
  dmb_audit maintenance-end "$(jq -cn --arg action "$action" --argjson code "$rc" '{action:$action,exit_code:$code,rollback:false}')" || return
  return "$rc"
)
