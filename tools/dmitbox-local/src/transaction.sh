# Durable, compensating transactions. Storage/firmware failures can defeat flush
# guarantees; no Bash transaction can promise recovery from arbitrary I/O loss.

dmb_tx_id_valid() { [[ ${1-} =~ ^tx-[a-f0-9]{32}$ ]]; }
dmb_tx_adapter_valid() {
  case ${1-} in dns|routing|dns_file|dns_nm|dns_networkd|dns_resolved|dns_ifupdown|mtu|iface|ipv6|ipv6_static|pool|nat66|ssh|nft|tcp|swap|maintenance|fs|f2b) return 0;; *) return 1;; esac
}
dmb_tx_dir() {
  dmb_tx_id_valid "${1-}" || return 2
  printf '%s/transactions/%s\n' "$(dmb_state_root)" "$1"
}
dmb_tx_namespace_check() {
  local namespace current_namespace init_namespace
  # Init launches the independent worker in its own namespaces. Capturing a
  # different namespace would arm recovery for a different resource universe.
  for namespace in mnt net pid user; do
    current_namespace=$(readlink "/proc/self/ns/$namespace") || return 78
    init_namespace=$(readlink "/proc/1/ns/$namespace") || return 78
    [[ $current_namespace == "$init_namespace" ]] || { printf 'Recovery cannot supervise a different %s namespace\n' "$namespace" >&2; return 78; }
  done
}
dmb_tx_init() {
  dmb_require_root || return 1
  dmb_need bash jq flock sha256sum mktemp timeout setsid readlink stat sync od tr awk cat mkdir chmod rm || return 1
  dmb_tx_namespace_check || return
  local base locks
  base=$(dmb_state_root); locks=$(dmb_lock_root)
  dmb_trusted_path "$base" allow_missing || return 1
  dmb_trusted_path "$locks" allow_missing || return 1
  # Each state directory is chmodded below after trusted-parent validation.
  # shellcheck disable=SC2174
  mkdir -p -m 700 -- "$base/transactions" "$base/reservations" "$locks" || return 1
  chmod 700 -- "$base" "$base/transactions" "$base/reservations" "$locks" || return 1
  dmb_trusted_path "$base/transactions" || return 1
  dmb_trusted_path "$base/reservations" || return 1
}
dmb_tx_canon() { jq -ceS .; }
dmb_tx_hash() { sha256sum | awk '{print $1}'; }
dmb_tx_uptime() { local n rest; read -r n rest < /proc/uptime || return 1; printf '%s\n' "${n%%.*}"; }
dmb_tx_boot_id() { cat /proc/sys/kernel/random/boot_id; }
dmb_tx_lock_path() {
  local path=$1
  dmb_trusted_path "$path" allow_missing || return 1
  # A symlink could truncate its target at redirection; a FIFO could block
  # before flock's timeout even begins. The private parent prevents untrusted
  # writers racing this check; root writers remain outside that guarantee.
  [[ ! -L $path ]] || return 1
  if [[ -e $path ]]; then [[ -f $path && $(stat -c %h -- "$path") == 1 ]] || return 1; fi
}
dmb_tx_confirmation_capability() {
  local ticks
  if declare -F dmb_confirmation_now_ticks >/dev/null; then
    ticks=$(dmb_confirmation_now_ticks) || return 69
    [[ $ticks =~ ^[0-9]+$ ]] || return 69
  fi
}
dmb_tx_state_write() {
  local tx=$1 next=$2 extra=${3:-'{}'} old generation payload
  case "$next" in PREPARED|ARMED|APPLIED|VERIFIED|COMMITTED|ROLLED_BACK|CONFLICT|ROLLBACK_FAILED) ;; *) return 2;; esac
  old=$(cat "$tx/state.json") || return 1
  local prior
  prior=$(jq -er '.state' <<< "$old") || return 1
  case "$prior:$next" in
    PREPARED:PREPARED|PREPARED:ARMED|PREPARED:ROLLED_BACK|PREPARED:CONFLICT|PREPARED:ROLLBACK_FAILED|ARMED:APPLIED|ARMED:ROLLED_BACK|ARMED:CONFLICT|ARMED:ROLLBACK_FAILED|APPLIED:VERIFIED|APPLIED:ROLLED_BACK|APPLIED:CONFLICT|APPLIED:ROLLBACK_FAILED|VERIFIED:VERIFIED|VERIFIED:COMMITTED|VERIFIED:ROLLED_BACK|VERIFIED:CONFLICT|VERIFIED:ROLLBACK_FAILED|CONFLICT:CONFLICT|CONFLICT:ROLLED_BACK|CONFLICT:ROLLBACK_FAILED|ROLLBACK_FAILED:ROLLBACK_FAILED|ROLLBACK_FAILED:CONFLICT|ROLLBACK_FAILED:ROLLED_BACK) ;;
    *) printf 'Invalid transaction transition: %s -> %s\n' "$prior" "$next" >&2; return 2;;
  esac
  generation=$(jq -er '.generation + 1' <<< "$old") || return 1
  payload=$(jq -ce --arg state "$next" --argjson generation "$generation" --argjson extra "$extra" '. + $extra + {state:$state,generation:$generation}' <<< "$old") || return 1
  printf '%s\n' "$payload" | dmb_atomic_write "$tx/state.json" 0600
}
dmb_tx_plan_validate() {
  local plan=$1 adapter op i count
  jq -e '
    .schema == 1 and (.steps|type=="array" and length>0 and length<=128) and
    (.resources|type=="array" and length>0 and length<=512) and
    all(.resources[]; type=="string" and test("^[A-Za-z0-9][A-Za-z0-9_./:-]{0,511}$")) and
    ((.resources|unique|length)==(.resources|length)) and
    ([.steps[].id]|unique|length)==(.steps|length) and
    all(.steps[]; (.id|type=="string" and test("^[A-Za-z0-9][A-Za-z0-9_.:-]{0,99}$")) and
      has("after") and ((.phase // "apply")=="apply" or .phase=="commit") and
      ((.restore_rank // 0)|type=="number" and floor==. and .>=-100000 and .<=100000)) and
    (. as $p | all(.steps[]; .resource as $r | $p.resources|index($r)!=null)) and
    ((.timeout_seconds // 180)|type=="number" and floor==. and .>=60 and .<=3600)
  ' "$plan" >/dev/null || return 2
  jq -e '. as $p | all(.steps|to_entries[]; . as $s | if (($s.value.adapter // $p.adapter)=="fs") then
      ([ $p.steps[:$s.key][] | select(.resource==$s.value.resource) ]|length)==0
      else true end)' "$plan" >/dev/null || return 2
  count=$(jq '.steps|length' "$plan") || return 1
  for ((i=0;i<count;i++)); do
    adapter=$(jq -er --argjson i "$i" '.steps[$i].adapter // .adapter' "$plan") || return 2
    dmb_tx_adapter_valid "$adapter" || return 2
    if [[ $adapter == ssh ]]; then dmb_ssh_plan_shape "$plan" || return 2; fi
    for op in capture apply verify restore; do declare -F "dmb_${adapter}_${op}" >/dev/null || return 2; done
    if [[ $adapter == fs ]] && declare -F dmb_fs_plan_validate >/dev/null; then
      dmb_fs_plan_validate "$plan" "$i" || return 2
    fi
  done
}
# Resource labels are user-facing, not authority. Derive extra lock keys from
# actual adapter targets, so choosing a different label cannot evade ownership.
dmb_tx_expand_resources() {
  local plan=$1 result i count field value canonical key adapter family table identity
  result=$(jq -cS . "$plan") || return 1
  count=$(jq '.steps|length' "$plan") || return 1
  for ((i=0;i<count;i++)); do
    adapter=$(jq -er --argjson i "$i" '.steps[$i].adapter // .adapter' "$plan") || return 2
    for field in path file fragment; do
      value=$(jq -er --argjson i "$i" --arg f "$field" '.steps[$i][$f] // empty' "$plan") || continue
      [[ $value == /* ]] || return 2
      dmb_trusted_path "$value" allow_missing || return 1
      canonical=$(readlink -m -- "$value") || return 1
      key="owned:file:$(printf '%s' "$canonical" | dmb_tx_hash)"
      result=$(jq -cS --arg key "$key" '.resources=([.resources[],$key]|unique)' <<< "$result") || return 1
    done
    value=$(jq -r --argjson i "$i" '.steps[$i].iface // empty' "$plan") || return 1
    if [[ -n $value ]]; then
      [[ $value =~ ^[A-Za-z0-9_.:-]{1,15}$ ]] || return 2
      key="owned:link:$value"
      result=$(jq -cS --arg key "$key" '.resources=([.resources[],$key]|unique)' <<< "$result") || return 1
    fi
    value=$(jq -r --argjson i "$i" '.steps[$i].key // empty' "$plan") || return 1
    if [[ -n $value ]]; then
      [[ $value =~ ^[A-Za-z0-9_.-]{1,255}$ ]] || return 2
      key="owned:sysctl:$value"
      result=$(jq -cS --arg key "$key" '.resources=([.resources[],$key]|unique)' <<< "$result") || return 1
    fi
    if [[ $adapter == nft || $adapter == nat66 ]]; then
      family=$(jq -er --argjson i "$i" '.steps[$i].nft.family' "$plan") || return 2
      table=$(jq -er --argjson i "$i" '.steps[$i].nft.table' "$plan") || return 2
      [[ $family =~ ^(inet|ip|ip6)$ && $table =~ ^[A-Za-z_][A-Za-z0-9_]{0,63}$ ]] || return 2
      key="owned:nft:$family:$table"
      result=$(jq -cS --arg key "$key" '.resources=([.resources[],$key]|unique)' <<< "$result") || return 1
    fi
    if [[ $adapter == ssh || $adapter == f2b ]]; then
      result=$(jq -cS '.resources=([.resources[],"owned:ssh-service"]|unique)' <<< "$result") || return 1
    fi
    if [[ $adapter == dns_nm ]]; then
      identity=$(jq -er --argjson i "$i" '.steps[$i].identity' "$plan") || return 2
      key="owned:nm:$(printf '%s' "$identity" | dmb_tx_hash)"
      result=$(jq -cS --arg key "$key" '.resources=([.resources[],$key]|unique)' <<< "$result") || return 1
    fi
  done
  printf '%s\n' "$result"
}
dmb_tx_adapter_capture() {
  local plan=$1 i=$2 adapter
  adapter=$(jq -er --argjson i "$i" '.steps[$i].adapter // .adapter' "$plan") || return 2
  dmb_tx_adapter_valid "$adapter" || return 2
  "dmb_${adapter}_capture" "$plan" "$i" | dmb_tx_canon
}
# Only the pinned worker may dispatch adapter operations. Every native adapter
# command must itself be bounded; the outer worker also has a process-group cap.
dmb_tx_adapter_dispatch() {
  local op=$1 id=$2 i=$3 tx adapter
  dmb_require_root || return 1
  case "$op" in capture|apply|verify|restore|verify_restore|owned_reset|cleanup|snapshot) ;; *) return 2;; esac
  tx=$(dmb_tx_dir "$id") || return 2
  dmb_trusted_path "$tx" || return 1
  [[ $i =~ ^[0-9]+$ ]] || return 2
  adapter=$(jq -er --argjson i "$i" '.steps[$i].adapter // .adapter' "$tx/plan.json") || return 2
  dmb_tx_adapter_valid "$adapter" || return 2
  if [[ $op == snapshot ]] && ! declare -F "dmb_${adapter}_snapshot" >/dev/null; then return 0; fi
  "dmb_${adapter}_${op}" "$tx/plan.json" "$i" "$tx"
}
# /proc stat command names may contain spaces and closing parentheses.
dmb_tx_proc_fields() {
  local pid=$1 data
  [[ $pid =~ ^[1-9][0-9]*$ && -r /proc/$pid/stat ]] || return 1
  IFS= read -r data < "/proc/$pid/stat" || return 1
  data=${data##*) }
  local -a fields
  read -r -a fields <<< "$data"
  (( ${#fields[@]} >= 20 )) || return 1
  printf '%s %s\n' "${fields[2]}" "${fields[19]}"
}
dmb_tx_close_locks() {
  local fd
  for fd in "${dmb_tx_resource_fds[@]-}" "${dmb_tx_lock_fd-}" "${dmb_tx_global_fd-}" "${DMB_LOCK_FDS[@]-}"; do
    [[ $fd =~ ^[0-9]+$ ]] && exec {fd}>&-
  done
  return 0
}
# Child waits for a durable, authenticated gate. It cannot mutate before its
# process group is recorded, allowing an independent watcher to cancel it.
dmb_tx_adapter_guarded() {
  local op=$1 id=$2 i=$3 nonce=$4 tx started now
  dmb_require_root || return 1
  [[ $nonce =~ ^[a-f0-9]{32}$ ]] || return 2
  tx=$(dmb_tx_dir "$id") || return 2
  started=$(dmb_tx_uptime) || return 1
  while :; do
    if [[ -f $tx/active-gate.json ]] && jq -e --arg nonce "$nonce" '.nonce==$nonce' "$tx/active-gate.json" >/dev/null 2>&1; then break; fi
    now=$(dmb_tx_uptime) || return 1
    (( now-started < 8 )) || return 1
    sleep 0.1
  done
  dmb_tx_integrity "$tx" || return 1
  case $op in apply|verify|restore|verify_restore|owned_reset|cleanup) ;; *) return 2;; esac
  dmb_tx_journal_valid "$tx" "$i" INTENT || return 1
  jq -e --arg nonce "$nonce" --arg op "$op" --argjson i "$i" '.nonce==$nonce and .operation==$op and .step==$i' "$tx/active.json" >/dev/null || return 1
  local group ticks parent own_pid=$BASHPID
  read -r group ticks < <(dmb_tx_proc_fields "$own_pid") || return 1
  parent=$(jq -er '.pgid' "$tx/active.json") || return 1
  [[ $group == "$parent" ]] || return 1
  if [[ $op == apply ]]; then
    local state
    state=$(jq -er '.state' "$tx/state.json") || return 1
    [[ $state == ARMED || $state == VERIFIED ]] || return 1
    dmb_tx_deadline_ok "$tx" || return 1
    dmb_recovery_ready "$id" "$tx" || return 1
  fi
  dmb_tx_adapter_dispatch "$op" "$id" "$i"
}
dmb_tx_group_running() {
  local target=$1 entry stat tail
  local -a fields
  [[ $target =~ ^[1-9][0-9]*$ ]] || return 2
  for entry in /proc/[0-9]*/stat; do
    IFS= read -r stat 2>/dev/null < "$entry" || continue
    tail=${stat##*) }; read -r -a fields <<< "$tail"
    [[ ${fields[2]:-} == "$target" && ${fields[0]:-Z} != Z && ${fields[0]:-X} != X ]] && return 0
  done
  return 1
}
dmb_tx_active_cancel() {
  local tx=$1 pid pgid ticks actualgroup actualticks nonce boot now
  [[ -f $tx/active.json ]] || return 0
  dmb_trusted_path "$tx/active.json" || return 1
  pid=$(jq -er '.pid' "$tx/active.json") || return 1
  pgid=$(jq -er '.pgid' "$tx/active.json") || return 1
  ticks=$(jq -er '.start_ticks' "$tx/active.json") || return 1
  boot=$(jq -er '.boot_id' "$tx/active.json") || return 1
  [[ $pid =~ ^[1-9][0-9]*$ && $pgid == "$pid" && $ticks =~ ^[0-9]+$ ]] || return 1
  [[ $boot == "$(dmb_tx_boot_id)" ]] || return 0
  if read -r actualgroup actualticks < <(dmb_tx_proc_fields "$pid"); then
    [[ $actualgroup == "$pgid" && $actualticks == "$ticks" ]] || return 1
  else
    # A killed group leader can leave descendants. A still-existing group with
    # no leader PID cannot have been newly created using that absent PID.
    [[ ! -e /proc/$pid ]] || return 1
  fi
  dmb_tx_group_running "$pgid" || return 0
  kill -TERM -- "-$pgid" 2>/dev/null || return 1
  now=$(dmb_tx_uptime) || return 1
  local until=$((now+3))
  while dmb_tx_group_running "$pgid"; do
    now=$(dmb_tx_uptime) || return 1
    if (( now >= until )); then
      kill -KILL -- "-$pgid" 2>/dev/null || return 1
      sleep 0.1
      dmb_tx_group_running "$pgid" && return 1
      break
    fi
    sleep 0.1
  done
  return 0
}
dmb_tx_adapter_run() {
  local op=$1 tx=$2 i=$3 id expected actual nonce pid group ticks result=0 tries
  id=${tx##*/}
  if [[ -f $tx/worker.sh && -f $tx/recovery.json ]]; then
    expected=$(jq -er '.worker_sha256' "$tx/recovery.json") || return 1
    actual=$(sha256sum "$tx/worker.sh" | awk '{print $1}') || return 1
    [[ $expected == "$actual" ]] || return 1
    nonce=$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n') || return 1
    rm -f -- "$tx/active-gate.json" || return 1
    ( dmb_tx_close_locks; exec setsid timeout --signal=TERM --kill-after=3 30 bash "$tx/worker.sh" --internal-adapter-guarded "$op" "$id" "$i" "$nonce" ) &
    pid=$!
    for ((tries=0;tries<30;tries++)); do
      if read -r group ticks < <(dmb_tx_proc_fields "$pid") && [[ $group == "$pid" ]]; then break; fi
      sleep 0.01
    done
    if [[ ${group:-} != "$pid" || ! ${ticks:-} =~ ^[0-9]+$ ]]; then kill "$pid" 2>/dev/null || :; wait "$pid" 2>/dev/null || :; return 1; fi
    if ! jq -cn --argjson pid "$pid" --argjson pgid "$group" --arg ticks "$ticks" --arg boot "$(dmb_tx_boot_id)" --arg op "$op" --argjson i "$i" --arg nonce "$nonce" '{pid:$pid,pgid:$pgid,start_ticks:$ticks,boot_id:$boot,operation:$op,step:$i,nonce:$nonce}' | dmb_atomic_write "$tx/active.json" 0600; then
      kill -TERM -- "-$pid" 2>/dev/null || :; wait "$pid" 2>/dev/null || :; return 1
    fi
    if ! jq -cn --arg nonce "$nonce" '{nonce:$nonce}' | dmb_atomic_write "$tx/active-gate.json" 0600; then
      dmb_tx_active_cancel "$tx" || :; wait "$pid" 2>/dev/null || :; return 1
    fi
    wait "$pid" || result=$?
    rm -f -- "$tx/active.json" "$tx/active-gate.json" || return 1
    sync -f "$tx" || return 1
    return "$result"
  else
    [[ $op == snapshot ]] || return 1
    dmb_tx_adapter_dispatch "$op" "$id" "$i"
  fi
}
dmb_tx_resources_lock() {
  local plan=$1 key hash fd
  dmb_tx_resource_fds=()
  while IFS= read -r key; do
    hash=$(printf '%s' "$key" | dmb_tx_hash) || return 1
    dmb_tx_lock_path "$(dmb_lock_root)/resource-$hash.lock" || return 1
    exec {fd}>"$(dmb_lock_root)/resource-$hash.lock" || return 1
    if ! flock -w 2 -x "$fd"; then exec {fd}>&-; return 75; fi
    dmb_tx_resource_fds+=("$fd")
  done < <(jq -r '.resources|sort|.[]' "$plan")
}
dmb_tx_resources_unlock() { local fd; for fd in "${dmb_tx_resource_fds[@]-}"; do [[ $fd =~ ^[0-9]+$ ]] && exec {fd}>&-; done; dmb_tx_resource_fds=(); }
dmb_tx_reserve() {
  local tx=$1 id=${1##*/} key hash owner path
  while IFS= read -r key; do
    hash=$(printf '%s' "$key" | dmb_tx_hash) || return 1
    path=$(dmb_state_root)/reservations/$hash.json
    if [[ -e $path ]]; then
      dmb_trusted_path "$path" || return 1
      owner=$(jq -er '.transaction_id' "$path") || return 1
      if [[ $owner != "$id" ]] && dmb_tx_id_valid "$owner"; then
        local oldtx oldstate
        oldtx=$(dmb_tx_dir "$owner") || return 1
        if [[ -f $oldtx/state.json ]]; then
          dmb_trusted_path "$oldtx/state.json" || return 1
          oldstate=$(jq -er '.state' "$oldtx/state.json") || return 1
          if [[ $oldstate == COMMITTED || $oldstate == ROLLED_BACK ]]; then
            rm -f -- "$path" || return 1
            continue
          fi
        fi
      fi
      [[ $owner == "$id" ]] || { printf 'Resource reserved: %s (%s)\n' "$key" "$owner" >&2; return 75; }
    fi
  done < <(jq -r '.resources[]' "$tx/plan.json")
  while IFS= read -r key; do
    hash=$(printf '%s' "$key" | dmb_tx_hash) || return 1
    jq -cn --arg id "$id" --arg resource "$key" '{transaction_id:$id,resource:$resource}' | dmb_atomic_write "$(dmb_state_root)/reservations/$hash.json" 0600 || return 1
  done < <(jq -r '.resources[]' "$tx/plan.json")
}
dmb_tx_release() {
  local tx=$1 id=${1##*/} key hash path owner
  while IFS= read -r key; do
    hash=$(printf '%s' "$key" | dmb_tx_hash) || return 1
    path=$(dmb_state_root)/reservations/$hash.json
    [[ -e $path ]] || continue
    owner=$(jq -er '.transaction_id' "$path") || return 1
    [[ $owner == "$id" ]] || continue
    rm -f -- "$path" || return 1
  done < <(jq -r '.resources[]' "$tx/plan.json")
  sync -f "$(dmb_state_root)/reservations"
}
dmb_tx_reap_preparations() {
  local tx id pid ticks group actual boot
  for tx in "$(dmb_state_root)"/transactions/tx-*; do
    [[ -d $tx && ! -f $tx/state.json && -f $tx/preparation.json ]] || continue
    id=${tx##*/}; dmb_tx_id_valid "$id" || return 1
    dmb_trusted_path "$tx/preparation.json" || return 1
    jq -e --arg id "$id" '.id==$id and (.pid|type=="number") and (.start_ticks|type=="string") and (.boot_id|type=="string")' "$tx/preparation.json" >/dev/null || return 1
    [[ ! -e $tx/journal/0000-INTENT.json ]] || return 1
    pid=$(jq -r '.pid' "$tx/preparation.json"); ticks=$(jq -r '.start_ticks' "$tx/preparation.json"); boot=$(jq -r '.boot_id' "$tx/preparation.json")
    if [[ $boot == "$(dmb_tx_boot_id)" ]] && read -r group actual < <(dmb_tx_proc_fields "$pid") && [[ $actual == "$ticks" ]]; then continue; fi
    # No mutation can precede state.json, recovery READY, ARMED and INTENT.
    # Release reservation metadata only; retain snapshots and failure evidence.
    dmb_tx_release "$tx" || return 1
  done
}
dmb_tx_prepare_cleanup() {
  local tx=${1:-}
  [[ -n $tx && -f $tx/plan.json && ! -f $tx/state.json ]] || return 0
  dmb_tx_release "$tx" || { printf 'Preparation failed; retained reservation evidence: %s\n' "$tx" >&2; return 1; }
}
dmb_tx_prepare() (
  dmb_tx_init || exit 1
  local source=$1 tx id plan capture resource previous provided i count now limit boot
  dmb_trusted_path "$source" || exit 1
  dmb_tx_plan_validate "$source" || exit 2
  dmb_tx_confirmation_capability || exit $?
  dmb_tx_lock_path "$(dmb_lock_root)/global.lock" || exit 1
  exec {dmb_tx_global_fd}>"$(dmb_lock_root)/global.lock" || exit 1
  flock -w 2 -x "$dmb_tx_global_fd" || exit 75
  dmb_tx_reap_preparations || exit 1
  id="tx-$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')"
  dmb_tx_id_valid "$id" || exit 1
  tx=$(dmb_tx_dir "$id") || exit 1
  mkdir -m 700 -- "$tx" "$tx/journal" "$tx/snapshots" || exit 1
  trap 'rc=$?; dmb_tx_prepare_cleanup "$tx" || :; dmb_tx_resources_unlock; exit "$rc"' EXIT
  local prep_pid=$BASHPID prep_ticks
  read -r _ prep_ticks < <(dmb_tx_proc_fields "$prep_pid") || exit 1
  jq -cn --arg id "$id" --argjson pid "$prep_pid" --arg ticks "$prep_ticks" --arg boot "$(dmb_tx_boot_id)" '{id:$id,pid:$pid,start_ticks:$ticks,boot_id:$boot}' | dmb_atomic_write "$tx/preparation.json" 0600 || exit 1
  plan=$(dmb_tx_expand_resources "$source") || exit 1
  printf '%s\n' "$plan" | dmb_atomic_write "$tx/plan.json" 0600 || exit 1
  dmb_tx_plan_validate "$tx/plan.json" || exit 2
  dmb_tx_reserve "$tx" || exit $?
  dmb_tx_resources_lock "$tx/plan.json" || exit $?
  count=$(jq '.steps|length' "$tx/plan.json") || exit 1
  for ((i=0;i<count;i++)); do
    resource=$(jq -r --argjson i "$i" '.steps[$i].resource' "$tx/plan.json") || exit 1
    previous=$(jq -cS --argjson i "$i" --arg r "$resource" '[.steps[:$i][]|select(.resource==$r)]|last|if .==null then {found:false} else {found:true,value:.after} end' "$tx/plan.json") || exit 1
    if [[ $(jq -r '.found' <<< "$previous") == true ]]; then
      capture=$(jq -cS '.value' <<< "$previous") || exit 1
    else
      capture=$(dmb_tx_adapter_capture "$tx/plan.json" "$i") || exit 1
    fi
    if jq -e --argjson i "$i" '.steps[$i]|has("before")' "$tx/plan.json" >/dev/null; then
      provided=$(jq -cS --argjson i "$i" '.steps[$i].before' "$tx/plan.json") || exit 1
      [[ $provided == "$capture" ]] || { printf 'Expected resource version changed before prepare: %s\n' "$resource" >&2; exit 76; }
    fi
    plan=$(jq -cS --argjson i "$i" --argjson before "$capture" '.steps[$i].before=$before' "$tx/plan.json") || exit 1
    printf '%s\n' "$plan" | dmb_atomic_write "$tx/plan.json" 0600 || exit 1
    dmb_tx_adapter_run snapshot "$tx" "$i" || exit 1
  done
  now=$(dmb_tx_uptime); limit=$(jq -r '.timeout_seconds // 180' "$tx/plan.json"); boot=$(dmb_tx_boot_id)
  jq -cn --arg hash "$(sha256sum "$tx/plan.json" | awk '{print $1}')" '{sha256:$hash}' | dmb_atomic_write "$tx/plan-hash.json" 0600 || exit 1
  if declare -F dmb_confirmation_prepare >/dev/null; then dmb_confirmation_prepare "$tx" || exit 1; fi
  jq -cn --arg id "$id" --arg boot "$boot" --argjson deadline "$((now+limit))" '{schema:1,id:$id,state:"PREPARED",generation:1,boot_id:$boot,deadline_uptime:$deadline,confirmation_round:0,error:null}' | dmb_atomic_write "$tx/state.json" 0600 || exit 1
  printf '%s\n' "$id"
)
dmb_tx_integrity() {
  local tx=$1 expected actual
  dmb_trusted_path "$tx" || return 1
  local file
  for file in plan.json plan-hash.json state.json; do dmb_trusted_path "$tx/$file" || return 1; done
  dmb_tx_plan_validate "$tx/plan.json" || return 1
  expected=$(jq -er '.sha256' "$tx/plan-hash.json") || return 1
  actual=$(sha256sum "$tx/plan.json" | awk '{print $1}') || return 1
  [[ $expected == "$actual" ]]
}
dmb_tx_deadline_ok() {
  local tx=$1 boot deadline now
  boot=$(jq -er '.boot_id' "$tx/state.json") || return 1
  [[ $boot == "$(dmb_tx_boot_id)" ]] || return 1
  deadline=$(jq -er '.deadline_uptime' "$tx/state.json") || return 1
  now=$(dmb_tx_uptime) || return 1
  (( now < deadline ))
}
dmb_tx_journal() {
  local tx=$1 i=$2 phase=$3 payload
  payload=$(jq -cS --argjson i "$i" --arg phase "$phase" '{step:$i,event:$phase,resource:.steps[$i].resource,before:.steps[$i].before,after:.steps[$i].after}' "$tx/plan.json") || return 1
  printf '%s\n' "$payload" | dmb_atomic_write "$tx/journal/$(printf '%04d' "$i")-$phase.json" 0600
}
dmb_tx_journal_valid() {
  local tx=$1 i=$2 phase=$3 actual expected path
  path=$tx/journal/$(printf '%04d' "$i")-$phase.json
  dmb_trusted_path "$path" || return 1
  actual=$(jq -cS . "$path") || return 1
  expected=$(jq -cS --argjson i "$i" --arg phase "$phase" '{step:$i,event:$phase,resource:.steps[$i].resource,before:.steps[$i].before,after:.steps[$i].after}' "$tx/plan.json") || return 1
  [[ $actual == "$expected" ]]
}
dmb_tx_step() {
  local tx=$1 i=$2 before after current
  before=$(jq -cS --argjson i "$i" '.steps[$i].before' "$tx/plan.json") || return 1
  after=$(jq -cS --argjson i "$i" '.steps[$i].after' "$tx/plan.json") || return 1
  current=$(dmb_tx_adapter_capture "$tx/plan.json" "$i") || return 1
  if [[ $current != "$before" ]]; then
    dmb_tx_state_write "$tx" CONFLICT '{"error":"Resource changed before apply"}' || :
    return 76
  fi
  dmb_recovery_ready "${tx##*/}" "$tx" || return 1
  dmb_tx_deadline_ok "$tx" || return 1
  dmb_tx_journal "$tx" "$i" INTENT || return 1
  dmb_tx_adapter_run apply "$tx" "$i" || return 1
  current=$(dmb_tx_adapter_capture "$tx/plan.json" "$i") || return 1
  [[ $current == "$after" ]] || return 1
  dmb_tx_adapter_run verify "$tx" "$i" || return 1
  dmb_tx_deadline_ok "$tx" || return 1
  dmb_tx_journal "$tx" "$i" DONE
}
dmb_tx_apply() (
  dmb_tx_init || exit 1
  local id=$1 tx state i count phase rc=0
  tx=$(dmb_tx_dir "$id") || exit 2
  dmb_tx_integrity "$tx" || exit 1
  dmb_tx_lock_path "$(dmb_lock_root)/$id.lock" || exit 1
  exec {dmb_tx_lock_fd}>"$(dmb_lock_root)/$id.lock" || exit 1
  flock -w 2 -x "$dmb_tx_lock_fd" || exit 75
  state=$(jq -r '.state' "$tx/state.json") || exit 1
  [[ $state == PREPARED ]] || { printf 'Cannot apply transaction in %s\n' "$state" >&2; exit 2; }
  dmb_tx_confirmation_capability || exit $?
  dmb_tx_resources_lock "$tx/plan.json" || exit $?
  trap 'dmb_tx_resources_unlock' EXIT
  # Reservations were created under the global lock in prepare. Validate ownership
  # without creating anything while holding resource locks.
  dmb_tx_reservation_check "$tx" || exit $?
  local actor_ticks actor_pid=$BASHPID
  read -r _ actor_ticks < <(dmb_tx_proc_fields "$actor_pid") || exit 1
  dmb_tx_state_write "$tx" PREPARED "$(jq -cn --argjson pid "$actor_pid" --arg ticks "$actor_ticks" '{actor_pid:$pid,actor_start_ticks:$ticks}')" || exit 1
  dmb_recovery_arm "$id" "$tx" || exit 1
  dmb_recovery_ready "$id" "$tx" || exit 1
  dmb_tx_deadline_ok "$tx" || exit 1
  dmb_tx_state_write "$tx" ARMED || exit 1
  count=$(jq '.steps|length' "$tx/plan.json") || exit 1
  for ((i=0;i<count;i++)); do
    phase=$(jq -r --argjson i "$i" '.steps[$i].phase // "apply"' "$tx/plan.json") || exit 1
    [[ $phase == apply ]] || continue
    dmb_tx_deadline_ok "$tx" || { rc=1; break; }
    dmb_tx_step "$tx" "$i" || { rc=$?; break; }
  done
  if (( rc==0 )); then
    dmb_tx_deadline_ok "$tx" || rc=1
  fi
  if (( rc==0 )); then
    local threshold='{}' confirmation_ticks
    if declare -F dmb_confirmation_now_ticks >/dev/null; then
      confirmation_ticks=$(dmb_confirmation_now_ticks) || exit 1
      [[ $confirmation_ticks =~ ^[0-9]+$ ]] || exit 1
      threshold=$(jq -cn --argjson ticks "$confirmation_ticks" '{confirmation_min_start_ticks:$ticks}') || exit 1
    fi
    dmb_tx_state_write "$tx" APPLIED && dmb_tx_state_write "$tx" VERIFIED "$threshold" || exit 1
    printf '%s\n' "$id"
  else
    # Drop locks before invoking the same independently usable recovery path.
    dmb_tx_resources_unlock; exec {dmb_tx_lock_fd}>&-
    dmb_tx_rollback "$id" || :
    exit "$rc"
  fi
)
dmb_tx_confirm() (
  dmb_tx_init || exit 1
  local id=$1 mode=${2:-ssh} tx state proof round i count phase commits=0 rc=0 observed_generation
  tx=$(dmb_tx_dir "$id") || exit 2
  dmb_tx_integrity "$tx" || exit 1
  [[ $(jq -er '.state' "$tx/state.json") == VERIFIED ]] || exit 2
  observed_generation=$(jq -er '.generation' "$tx/state.json") || exit 1
  dmb_tx_deadline_ok "$tx" || exit 1
  # The user may take time to authenticate/type. No transaction or resource
  # lock is held across that interaction; the watchdog remains independent.
  proof=$(dmb_confirmation_check "$tx" "$mode" | dmb_tx_canon) || exit 1
  dmb_tx_lock_path "$(dmb_lock_root)/$id.lock" || exit 1
  exec {dmb_tx_lock_fd}>"$(dmb_lock_root)/$id.lock" || exit 1
  flock -w 2 -x "$dmb_tx_lock_fd" || exit 75
  state=$(jq -r '.state' "$tx/state.json") || exit 1
  [[ $state == VERIFIED ]] || exit 2
  [[ $(jq -er '.generation' "$tx/state.json") == "$observed_generation" ]] || exit 76
  jq -e --arg id "$id" --argjson generation "$observed_generation" '.transaction_id==$id and .generation==$generation' <<< "$proof" >/dev/null || exit 76
  dmb_tx_deadline_ok "$tx" || exit 1
  dmb_tx_resources_lock "$tx/plan.json" || exit $?
  trap 'dmb_tx_resources_unlock' EXIT
  round=$(jq -r '.confirmation_round // 0' "$tx/state.json") || exit 1
  printf '%s\n' "$proof" | dmb_atomic_write "$tx/proof-$((round+1)).json" 0600 || exit 1
  count=$(jq '.steps|length' "$tx/plan.json") || exit 1
  if (( round==0 )); then
    for ((i=0;i<count;i++)); do
      phase=$(jq -r --argjson i "$i" '.steps[$i].phase // "apply"' "$tx/plan.json") || exit 1
      [[ $phase == commit ]] || continue
      commits=$((commits+1))
      dmb_tx_deadline_ok "$tx" || { rc=1; break; }
      dmb_tx_step "$tx" "$i" || { rc=$?; break; }
    done
    if (( rc!=0 )); then
      dmb_tx_resources_unlock; exec {dmb_tx_lock_fd}>&-
      dmb_tx_rollback "$id" || :
      exit "$rc"
    fi
    if (( commits>0 )); then
      local threshold='{"confirmation_round":1}' confirmation_ticks
      if declare -F dmb_confirmation_now_ticks >/dev/null; then
        confirmation_ticks=$(dmb_confirmation_now_ticks) || exit 1
        [[ $confirmation_ticks =~ ^[0-9]+$ ]] || exit 1
        threshold=$(jq -cn --argjson ticks "$confirmation_ticks" '{confirmation_round:1,confirmation_min_start_ticks:$ticks}') || exit 1
      fi
      dmb_tx_state_write "$tx" VERIFIED "$threshold" || exit 1
      dmb_confirmation_prepare "$tx" || exit 1
      printf 'Final policy applied; reconnect and confirm again before the original deadline: %s\n' "$id"
      exit 0
    fi
  else
    [[ $(jq -r '.transport_id' "$tx/proof-1.json") != "$(jq -r '.transport_id' <<< "$proof")" || $mode == console ]] || exit 1
  fi
  # Compare the final expected state for each resource, not superseded stages.
  while IFS= read -r i; do
    local current expected
    current=$(dmb_tx_adapter_capture "$tx/plan.json" "$i") || exit 1
    expected=$(jq -cS --argjson i "$i" '.steps[$i].after' "$tx/plan.json") || exit 1
    [[ $current == "$expected" ]] || { dmb_tx_state_write "$tx" CONFLICT '{"error":"Resource changed before confirmation"}'; exit 76; }
    dmb_tx_adapter_run verify "$tx" "$i" || exit 1
  done < <(jq -r '.steps|to_entries|group_by(.value.resource)|.[]|last|.key' "$tx/plan.json")
  dmb_tx_deadline_ok "$tx" || exit 1
  dmb_tx_state_write "$tx" COMMITTED || exit 1
  dmb_recovery_disarm "$id" || printf 'Committed, but recovery unit cleanup needs inspection: %s\n' "$id" >&2
  dmb_tx_resources_unlock
  dmb_tx_lock_path "$(dmb_lock_root)/global.lock" || exit 1
  exec {dmb_tx_global_fd}>"$(dmb_lock_root)/global.lock" || exit 1
  flock -w 2 -x "$dmb_tx_global_fd" || exit 75
  dmb_tx_release "$tx" || exit 1
  printf 'COMMITTED %s\n' "$id"
)
dmb_tx_cleanup_step() {
  local adapter
  adapter=$(jq -er --argjson i "$2" '.steps[$i].adapter // .adapter' "$1/plan.json") || return 1
  dmb_tx_adapter_valid "$adapter" || return 1
  if declare -F "dmb_${adapter}_cleanup" >/dev/null; then dmb_tx_adapter_run cleanup "$1" "$2" || return $?; fi
  return 0
}
dmb_tx_has_restore_verify() {
  local adapter
  adapter=$(jq -er --argjson i "$2" '.steps[$i].adapter // .adapter' "$1/plan.json") || return 1
  dmb_tx_adapter_valid "$adapter" && declare -F "dmb_${adapter}_verify_restore" >/dev/null
}
dmb_tx_has_owned_reset() {
  local adapter
  adapter=$(jq -er --argjson i "$2" '.steps[$i].adapter // .adapter' "$1/plan.json") || return 1
  dmb_tx_adapter_valid "$adapter" && declare -F "dmb_${adapter}_owned_reset" >/dev/null
}
dmb_tx_reservation_check() {
  local tx=$1 key path hash
  while IFS= read -r key; do
    hash=$(printf '%s' "$key" | dmb_tx_hash) || return 1
    path=$(dmb_state_root)/reservations/$hash.json
    dmb_trusted_path "$path" || return 1
    jq -e --arg id "${tx##*/}" --arg key "$key" '.transaction_id==$id and .resource==$key' "$path" >/dev/null || return 76
  done < <(jq -r '.resources[]' "$tx/plan.json")
}
dmb_tx_rollback() (
  dmb_tx_init || exit 1
  local id=$1 tx state i current before after rc=0 failed=0 conflict=0
  tx=$(dmb_tx_dir "$id") || exit 2
  dmb_tx_integrity "$tx" || exit 1
  dmb_tx_lock_path "$(dmb_lock_root)/$id.lock" || exit 1
  exec {dmb_tx_lock_fd}>"$(dmb_lock_root)/$id.lock" || exit 1
  flock -w 2 -x "$dmb_tx_lock_fd" || exit 75
  state=$(jq -r '.state' "$tx/state.json") || exit 1
  case "$state" in COMMITTED|ROLLED_BACK)
    dmb_recovery_disarm "$id" || :
    dmb_tx_lock_path "$(dmb_lock_root)/global.lock" || exit 1
    exec {dmb_tx_global_fd}>"$(dmb_lock_root)/global.lock" || exit 1
    flock -w 2 -x "$dmb_tx_global_fd" || exit 75
    dmb_tx_release "$tx"; exit $?;; PREPARED|ARMED|APPLIED|VERIFIED|CONFLICT|ROLLBACK_FAILED) ;; *) exit 2;; esac
  dmb_tx_resources_lock "$tx/plan.json" || exit $?
  trap 'dmb_tx_resources_unlock' EXIT
  dmb_tx_reservation_check "$tx" || { dmb_tx_state_write "$tx" CONFLICT '{"error":"Transaction no longer owns resource reservations"}' || :; exit 76; }
  dmb_tx_active_cancel "$tx" || { dmb_tx_state_write "$tx" ROLLBACK_FAILED '{"error":"Cannot prove active mutator was stopped"}' || :; exit 1; }
  while IFS= read -r i; do
    [[ -f $tx/journal/$(printf '%04d' "$i")-INTENT.json ]] || continue
    dmb_tx_journal_valid "$tx" "$i" INTENT || { failed=1; continue; }
    if [[ -f $tx/journal/$(printf '%04d' "$i")-RESTORED.json ]]; then
      dmb_tx_journal_valid "$tx" "$i" RESTORED || failed=1
      continue
    fi
    dmb_tx_cleanup_step "$tx" "$i" || { failed=1; continue; }
    before=$(jq -cS --argjson i "$i" '.steps[$i].before' "$tx/plan.json") || { failed=1; continue; }
    after=$(jq -cS --argjson i "$i" '.steps[$i].after' "$tx/plan.json") || { failed=1; continue; }
    current=$(dmb_tx_adapter_capture "$tx/plan.json" "$i") || { failed=1; continue; }
    if [[ $current == "$before" ]]; then
      if dmb_tx_has_restore_verify "$tx" "$i"; then
        if ! dmb_tx_adapter_run verify_restore "$tx" "$i"; then
          # Configuration can already be restored while a prior reload failed.
          # The idempotent inverse must reconcile its owned runtime state too.
          dmb_tx_journal "$tx" "$i" RESTORE_INTENT || { failed=1; continue; }
          dmb_tx_adapter_run restore "$tx" "$i" || { failed=1; continue; }
          current=$(dmb_tx_adapter_capture "$tx/plan.json" "$i") || { failed=1; continue; }
          [[ $current == "$before" ]] || { failed=1; continue; }
          dmb_tx_adapter_run verify_restore "$tx" "$i" || { failed=1; continue; }
        fi
      fi
      dmb_tx_cleanup_step "$tx" "$i" || { failed=1; continue; }
      dmb_tx_journal "$tx" "$i" RESTORED || failed=1; continue
    fi
    if [[ $current != "$after" ]]; then
      # A kernel resource may disappear on reboot. Only its own pinned adapter
      # may prove that exact reset condition and authority to rebuild it; all
      # other third states remain conflicts. The inverse rechecks the proof.
      if ! dmb_tx_has_owned_reset "$tx" "$i" || ! dmb_tx_adapter_run owned_reset "$tx" "$i"; then conflict=1; continue; fi
    fi
    dmb_tx_journal "$tx" "$i" RESTORE_INTENT || { failed=1; continue; }
    if ! dmb_tx_adapter_run restore "$tx" "$i"; then failed=1; continue; fi
    current=$(dmb_tx_adapter_capture "$tx/plan.json" "$i") || { failed=1; continue; }
    if [[ $current == "$before" ]]; then
      if dmb_tx_has_restore_verify "$tx" "$i"; then dmb_tx_adapter_run verify_restore "$tx" "$i" || { failed=1; continue; }; fi
      dmb_tx_cleanup_step "$tx" "$i" || { failed=1; continue; }
      dmb_tx_journal "$tx" "$i" RESTORED || failed=1
    else failed=1; fi
  done < <(jq -r '.steps|to_entries|sort_by([(.value.restore_rank // 0),(-.key)])|.[].key' "$tx/plan.json")
  # A previous RESTORED journal entry proves a past action, not the present
  # state. Re-read each touched resource at its original before-version.
  while IFS= read -r i; do
    local resource touched=0 j
    resource=$(jq -r --argjson i "$i" '.steps[$i].resource' "$tx/plan.json") || { failed=1; continue; }
    while IFS= read -r j; do
      [[ -f $tx/journal/$(printf '%04d' "$j")-INTENT.json ]] && touched=1
    done < <(jq -r --arg r "$resource" '.steps|to_entries|.[]|select(.value.resource==$r)|.key' "$tx/plan.json")
    (( touched )) || continue
    before=$(jq -cS --argjson i "$i" '.steps[$i].before' "$tx/plan.json") || { failed=1; continue; }
    current=$(dmb_tx_adapter_capture "$tx/plan.json" "$i") || { failed=1; continue; }
    [[ $current == "$before" ]] || conflict=1
  done < <(jq -r '.steps|to_entries|group_by(.value.resource)|.[]|first|.key' "$tx/plan.json")
  if (( failed )); then dmb_tx_state_write "$tx" ROLLBACK_FAILED '{"error":"Restoration or durable recording failed"}' || :; exit 1; fi
  if (( conflict )); then dmb_tx_state_write "$tx" CONFLICT '{"error":"External state differs; conflicting resources were not overwritten"}' || :; exit 76; fi
  dmb_tx_state_write "$tx" ROLLED_BACK '{"restoration_evidence":"adapter readback and available restore postconditions; external connectivity unverified"}' || exit 1
  dmb_recovery_disarm "$id" || :
  dmb_tx_resources_unlock
  dmb_tx_lock_path "$(dmb_lock_root)/global.lock" || exit 1
  exec {dmb_tx_global_fd}>"$(dmb_lock_root)/global.lock" || exit 1
  flock -w 2 -x "$dmb_tx_global_fd" || exit 75
  dmb_tx_release "$tx"
)
dmb_tx_execute() { local id; id=$(dmb_tx_prepare "$1") || return $?; dmb_tx_apply "$id"; }
dmb_tx_status() { local tx; dmb_require_root || return 1; tx=$(dmb_tx_dir "$1") || return 2; dmb_trusted_path "$tx" || return 1; jq -cS . "$tx/state.json"; }
dmb_tx_list() { local path; dmb_require_root || return 1; for path in "$(dmb_state_root)"/transactions/tx-*/state.json; do [[ -f $path && ! -L $path ]] || continue; jq -cS '{id,state,generation,confirmation_round,error}' "$path" || return 1; done; }
