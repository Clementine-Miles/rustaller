# Authenticated-transport confirmation. This module never trusts SSH_CONNECTION.
# shellcheck shell=bash

_dmb_confirmation_error() { printf 'Confirmation: %s\n' "$*" >&2; return 1; }
_dmb_confirmation_root() { (( EUID == 0 )); }
_dmb_confirmation_digest() { sha256sum | awk '{print $1}'; }
_dmb_confirmation_manifest() {
  local txdir=${1:-} path
  path="${DMB_STATE_DIR:-/var/lib/dmitbox-local}/enrollments/ssh.json"
  if [[ -n "$txdir" ]]; then
    local enrolled
    enrolled=$(jq -r '.confirmation_enrollment // empty' "$txdir/plan.json") || return 1
    # Plans can name the enrolled resource, not select arbitrary manifest paths.
    [[ -z "$enrolled" || "$enrolled" == ssh ]] || return 1
  fi
  dmb_trusted_path "$path" || return 1
  jq -e '.schema == 1 and (.confirmation|type=="object")' "$path" >/dev/null || return 1
  printf '%s\n' "$path"
}
_dmb_confirmation_proc_fields() {
  local proc=$1 pid=$2 text tail
  [[ "$pid" =~ ^[1-9][0-9]*$ && -d "$proc/$pid" && ! -L "$proc/$pid" ]] || return 1
  IFS= read -r text < "$proc/$pid/stat" || return 1
  [[ "$text" == "$pid ("* && "$text" == *') '* ]] || return 1
  # comm can contain spaces and ')'; fields after its final ')' are numeric.
  tail=${text##*) }
  local -a fields=()
  IFS=' ' read -r -a fields <<< "$tail"
  [[ ${#fields[@]} -ge 20 && ${fields[1]} =~ ^[0-9]+$ && ${fields[19]} =~ ^[0-9]+$ ]] || return 1
  printf '%s %s\n' "${fields[1]}" "${fields[19]}"
}
_dmb_confirmation_cgroup() {
  local proc=$1 pid=$2 number controllers path
  while IFS=: read -r number controllers path; do
    if [[ "$number" == 0 && -z "$controllers" ]] || [[ ",$controllers," == *,name=systemd,* ]]; then
      [[ "$path" == /* && "$path" != *$'\n'* ]] || return 1
      printf '%s\n' "$path"; return 0
    fi
  done < "$proc/$pid/cgroup"
  return 1
}
_dmb_confirmation_binary() {
  local manifest=$1 kind=$2 proc=$3 pid=$4 exe digest uid mode
  exe=$(readlink -- "$proc/$pid/exe") || return 1
  [[ "$exe" == /* && "$exe" != *' (deleted)' ]] || return 1
  dmb_trusted_path "$exe" || return 1
  [[ -f "$exe" && -x "$exe" ]] || return 1
  uid=$(stat -Lc '%u' -- "$proc/$pid/exe") || return 1
  mode=$(stat -Lc '%a' -- "$proc/$pid/exe") || return 1
  [[ "$uid" == 0 && "$mode" =~ ^[0-7]{3,4}$ ]] || return 1
  (( (8#$mode & 0022) == 0 )) || return 1
  digest=$(sha256sum -- "$proc/$pid/exe" | awk '{print $1}') || return 1
  jq -e --arg kind "$kind" --arg path "$exe" --arg digest "$digest" '
    .confirmation[$kind] | type=="array" and any(.[]; .path==$path and .sha256==$digest)
  ' "$manifest" >/dev/null || return 1
  printf '%s\n' "$exe"
}
_dmb_confirmation_group_allowed() {
  local manifest=$1 kind=$2 cgroup=$3
  jq -e --arg key "$kind" --arg group "$cgroup" '.confirmation[$key] | type=="array" and index($group)!=null' "$manifest" >/dev/null
}
_dmb_confirmation_ss() { ss -H -n -t -e -p -O state established; }
_dmb_confirmation_endpoint() {
  local value=$1 address port
  if [[ "$value" == \[*\]:* ]]; then
    address=${value#\[}; address=${address%\]:*}; port=${value##*:}
  else
    address=${value%:*}; port=${value##*:}
  fi
  [[ "$address" =~ ^[0-9a-fA-F:.]+$ && "$port" =~ ^[0-9]{1,5}$ ]] || return 1
  (( 10#$port > 0 && 10#$port <= 65535 )) || return 1
  printf '%s\t%s\n' "$address" "$((10#$port))"
}
_dmb_confirmation_socket() {
  local proc=$1 pid=$2 listing=$3 fd link inode line recv send local_endpoint peer_endpoint rest
  local local_address local_port peer_address peer_port found='' candidate
  local -A observed_inodes=()
  for fd in "$proc/$pid/fd/"*; do
    [[ -L "$fd" ]] || continue
    link=$(readlink -- "$fd") || continue
    [[ "$link" =~ ^socket:\[([0-9]+)\]$ ]] || continue
    inode=${BASH_REMATCH[1]}
    [[ ! -v observed_inodes[$inode] ]] || continue
    observed_inodes[$inode]=1
    while IFS= read -r line; do
      [[ " $line " =~ [[:space:]]ino:$inode([[:space:]]|$) ]] || continue
      IFS=' ' read -r recv send local_endpoint peer_endpoint rest <<< "$line"
      [[ "$recv" =~ ^[0-9]+$ && "$send" =~ ^[0-9]+$ ]] || return 1
      IFS=$'\t' read -r local_address local_port < <(_dmb_confirmation_endpoint "$local_endpoint")
      IFS=$'\t' read -r peer_address peer_port < <(_dmb_confirmation_endpoint "$peer_endpoint")
      [[ -n "$local_address" && -n "$local_port" && -n "$peer_address" && -n "$peer_port" ]] || return 1
      candidate=$(jq -cn --arg inode "$inode" --arg la "$local_address" --argjson lp "$local_port" --arg pa "$peer_address" --argjson pp "$peer_port" '{socket_inode:$inode,local_address:$la,local_port:$lp,peer_address:$pa,peer_port:$pp}') || return 1
      [[ -z "$found" || "$found" == "$candidate" ]] || return 1
      found=$candidate
    done <<< "$listing"
  done
  [[ -n "$found" ]] || return 1
  printf '%s\n' "$found"
}
_dmb_confirmation_proc_uid() {
  local proc=$1 pid=$2 key real effective saved filesystem rest
  while IFS=$' \t' read -r key real effective saved filesystem rest; do
    [[ $key == Uid: ]] || continue
    [[ $real =~ ^[0-9]{1,10}$ && $real == "$effective" && $real == "$saved" && $real == "$filesystem" ]] || return 1
    ((10#$real<4294967295)) || return 1
    printf '%s\n' "$((10#$real))"; return 0
  done < "$proc/$pid/status"
  return 1
}
_dmb_confirmation_local_user() {
  local uid=$1 name _password candidate _gid _gecos _home _shell found=''
  # A local account file is deterministic and makes no implicit NSS/network
  # request. Directory-service-only identities need a dedicated native adapter.
  dmb_trusted_path /etc/passwd || return 1
  while IFS=: read -r name _password candidate _gid _gecos _home _shell; do
    [[ $candidate == "$uid" ]] || continue
    [[ $name =~ ^[A-Za-z_][A-Za-z0-9_.-]{0,31}\$?$ && -z $found ]] || return 1
    found=$name
  done < /etc/passwd
  [[ -n $found ]] || return 1
  printf '%s\n' "$found"
}
# Explicit proc root/start arguments exist only for library fixtures. Public CLI
# always calls this with literal /proc and Bash's kernel PID, never environment.
_dmb_confirmation_transport_at() {
  local manifest=$1 proc=$2 pid=$3 listing boot fields parent start exe group socket count=0
  local session_uid='' process_uid login_user audit_uid='' observed_audit current_fields
  local -A seen=()
  listing=$(_dmb_confirmation_ss) || return 1
  IFS= read -r boot < "$proc/sys/kernel/random/boot_id" || return 1
  [[ "$boot" =~ ^[0-9a-fA-F-]{36}$ ]] || return 1
  if [[ -r $proc/$pid/loginuid ]]; then
    IFS= read -r observed_audit < "$proc/$pid/loginuid" || [[ -n $observed_audit ]] || return 1
    [[ $observed_audit =~ ^[0-9]{1,10}$ ]] || return 1
    [[ $observed_audit == 4294967295 ]] || audit_uid=$observed_audit
  fi
  while [[ "$pid" != 0 && "$pid" != 1 ]]; do
    (( count += 1 )); (( count <= 128 )) || return 1
    [[ ! -v seen[$pid] ]] || return 1; seen[$pid]=1
    fields=$(_dmb_confirmation_proc_fields "$proc" "$pid") || return 1
    IFS=' ' read -r parent start <<< "$fields"
    if exe=$(_dmb_confirmation_binary "$manifest" sshd_binaries "$proc" "$pid" 2>/dev/null); then
      process_uid=$(_dmb_confirmation_proc_uid "$proc" "$pid") || return 1
      if [[ $process_uid != 0 ]]; then
        [[ -z $session_uid || $session_uid == "$process_uid" ]] || return 1
        session_uid=$process_uid
      fi
      group=$(_dmb_confirmation_cgroup "$proc" "$pid") || { pid=$parent; continue; }
      if ! _dmb_confirmation_group_allowed "$manifest" ssh_cgroups "$group"; then pid=$parent; continue; fi
      if socket=$(_dmb_confirmation_socket "$proc" "$pid" "$listing"); then
        [[ -n $session_uid ]] || session_uid=0
        [[ -z $audit_uid || $audit_uid == "$session_uid" ]] || return 1
        login_user=$(_dmb_confirmation_local_user "$session_uid") || return 1
        jq -e --arg user "$login_user" '.contexts|type=="array" and any(.[];.user==$user)' "$manifest" >/dev/null || return 1
        current_fields=$(_dmb_confirmation_proc_fields "$proc" "$pid") || return 1
        [[ $current_fields == "$fields" ]] || return 1
        jq -cnS --argjson socket "$socket" --arg boot "$boot" --argjson pid "$pid" --arg start "$start" --arg exe "$exe" --arg cgroup "$group" --arg user "$login_user" --argjson uid "$session_uid" \
          '$socket + {kind:"ssh",boot_id:$boot,pid:$pid,start_ticks:$start,exe:$exe,cgroup:$cgroup,login_user:$user,login_uid:$uid}'
        return $?
      fi
    fi
    pid=$parent
  done
  return 1
}
dmb_confirmation_transport() {
  _dmb_confirmation_root || return 1
  local manifest=${1:-}
  [[ -n "$manifest" ]] || manifest=$(_dmb_confirmation_manifest) || return 1
  dmb_trusted_path "$manifest" || return 1
  _dmb_confirmation_transport_at "$manifest" /proc "$BASHPID"
}
_dmb_confirmation_console_at() {
  local manifest=$1 proc=$2 pid=$3 tty_path=$4 device fields parent start exe group boot count=0 found=''
  [[ "$tty_path" == /dev/* && "$tty_path" != /dev/pts/* && "$tty_path" != /dev/tty && -c "$tty_path" && ! -L "$tty_path" ]] || return 1
  dmb_trusted_path "${tty_path%/*}" || return 1
  [[ $(stat -Lc '%u' -- "$tty_path") == 0 ]] || return 1
  device=$(stat -Lc '%t:%T' -- "$tty_path") || return 1
  jq -e --arg path "$tty_path" --arg device "$device" '.confirmation.console_devices | type=="array" and any(.[];.path==$path and .device==$device)' "$manifest" >/dev/null || return 1
  IFS= read -r boot < "$proc/sys/kernel/random/boot_id" || return 1
  while [[ "$pid" != 0 && "$pid" != 1 ]]; do
    ((count+=1)); ((count<=128)) || return 1
    fields=$(_dmb_confirmation_proc_fields "$proc" "$pid") || return 1
    IFS=' ' read -r parent start <<< "$fields"
    # A console-looking tty inside an SSH ancestry never authorizes OOB bypass.
    if _dmb_confirmation_binary "$manifest" sshd_binaries "$proc" "$pid" >/dev/null 2>&1; then return 1; fi
    if exe=$(_dmb_confirmation_binary "$manifest" console_binaries "$proc" "$pid" 2>/dev/null); then
      group=$(_dmb_confirmation_cgroup "$proc" "$pid") || return 1
      _dmb_confirmation_group_allowed "$manifest" console_cgroups "$group" || return 1
      [[ -z "$found" ]] || { pid=$parent; continue; }
      found=$(jq -cnS --arg device "$device" --arg tty "$tty_path" --arg boot "$boot" --argjson pid "$pid" --arg start "$start" --arg exe "$exe" --arg cgroup "$group" \
        '{kind:"console",device:$device,tty:$tty,boot_id:$boot,pid:$pid,start_ticks:$start,exe:$exe,cgroup:$cgroup}') || return 1
    fi
    pid=$parent
  done
  [[ -n "$found" ]] || return 1
  printf '%s\n' "$found"
}
_dmb_confirmation_controlling_tty() {
  local manifest=$1 proc=$2 pid=$3 stat_line tail encoded device major minor candidate observed found=''
  [[ $pid =~ ^[1-9][0-9]*$ && ! -L $proc/$pid ]] || return 1
  IFS= read -r stat_line < "$proc/$pid/stat" || return 1
  [[ $stat_line == "$pid ("* && $stat_line == *') '* ]] || return 1
  tail=${stat_line##*) }
  local -a stat_fields=()
  IFS=' ' read -r -a stat_fields <<< "$tail"
  [[ ${#stat_fields[@]} -ge 5 && ${stat_fields[4]} =~ ^-?[0-9]+$ ]] || return 1
  encoded=${stat_fields[4]}; ((encoded!=0)) || return 1
  # Linux tty_nr is the encoded dev_t, exposed as a signed 32-bit number.
  device=$((encoded & 0xffffffff))
  major=$(((device >> 8) & 0xfff))
  minor=$(((device & 0xff) | ((device >> 12) & 0xfff00)))
  printf -v device '%x:%x' "$major" "$minor"
  while IFS= read -r candidate; do
    [[ $candidate == /dev/* && $candidate != /dev/pts/* && $candidate != /dev/tty && -c $candidate && ! -L $candidate ]] || continue
    observed=$(stat -Lc '%t:%T' -- "$candidate") || continue
    if [[ $observed == "$device" ]]; then
      [[ -z $found || $found == "$candidate" ]] || return 1
      found=$candidate
    fi
  done < <(jq -r '.confirmation.console_devices[]?.path' "$manifest")
  [[ -n $found ]] || return 1
  printf '%s\n' "$found"
}
dmb_confirmation_console() {
  local manifest=$1 tty_path
  _dmb_confirmation_root || return 1
  # tty </dev/tty reports the alias /dev/tty on common systems; identify the
  # controlling kernel device from /proc instead of trusting that alias or env.
  tty_path=$(_dmb_confirmation_controlling_tty "$manifest" /proc "$BASHPID") || return 1
  _dmb_confirmation_console_at "$manifest" /proc "$BASHPID" "$tty_path"
}

_dmb_confirmation_identity() {
  jq -ceS 'if .kind=="ssh" then {kind,boot_id,socket_inode,local_address,local_port,peer_address,peer_port}
           elif .kind=="console" then {kind,boot_id,device,tty} else error("unsupported transport") end'
}
_dmb_confirmation_fresh() {
  local origin=$1 current=$2 old_identity new_identity
  [[ "$origin" != null && -n "$origin" ]] || return 1
  old_identity=$(printf '%s\n' "$origin" | _dmb_confirmation_identity) || return 1
  new_identity=$(printf '%s\n' "$current" | _dmb_confirmation_identity) || return 1
  [[ "$new_identity" != "$old_identity" ]]
}
_dmb_confirmation_read_challenge() {
  local value='' fd timeout_seconds=${1:-1}
  exec {fd}<>/dev/tty || return 1
  printf 'Enter the one-use confirmation challenge: ' >&"$fd"
  IFS= read -r -s -t "$timeout_seconds" -n 65 value <&"$fd" || { exec {fd}>&-; return 1; }
  printf '\n' >&"$fd"
  exec {fd}>&-
  [[ "$value" =~ ^[0-9a-f]{64}$ ]] || return 1
  printf '%s' "$value" | _dmb_confirmation_digest
  unset value
}
dmb_confirmation_prepare() {
  local txdir=$1 challenge digest fd
  _dmb_confirmation_root && dmb_trusted_path "$txdir" || return 1
  command -v jq >/dev/null && command -v od >/dev/null && command -v sha256sum >/dev/null || return 1
  exec {fd}<>/dev/tty || { _dmb_confirmation_error 'a controlling terminal is required to display the challenge'; return 1; }
  challenge=$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n') || { exec {fd}>&-; return 1; }
  [[ "$challenge" =~ ^[0-9a-f]{64}$ ]] || { exec {fd}>&-; return 1; }
  digest=$(printf '%s' "$challenge" | _dmb_confirmation_digest) || { exec {fd}>&-; return 1; }
  printf '%s\n' "$digest" | dmb_atomic_write "$txdir/challenge.sha256" 0600 || { exec {fd}>&-; return 1; }
  printf '\nOne-use confirmation challenge (keep private): %s\n' "$challenge" >&"$fd"
  unset challenge
  exec {fd}>&-
}
_dmb_confirmation_context_match() {
  local txdir=$1 manifest=$2 current=$3 round=$4 contexts entry user addr laddr port canonical
  # After listener retirement use its final native sshd -T context; other
  # transactions use the explicitly enrolled connection contexts.
  contexts=$(jq -c --argjson round "$round" '
    [.steps[]? | select(.adapter=="ssh" and
      (($round==0 and (.phase//"apply")=="apply") or
       ($round>=1 and .phase=="commit")))] | last |
    if .==null then null else [.after_effective[].context] end' "$txdir/plan.json") || return 1
  [[ $contexts != null ]] || contexts=$(jq -ce '.contexts' "$manifest") || return 1
  while IFS= read -r entry; do
    user=$(jq -er .user <<< "$entry") || continue
    [[ $user == "$(jq -er .login_user <<< "$current")" ]] || continue
    addr=$(jq -er .addr <<< "$entry") || continue
    laddr=$(jq -er .laddr <<< "$entry") || continue
    port=$(jq -er .lport <<< "$entry") || continue
    canonical=$(dmb_ip_parse "$addr") || continue
    [[ $canonical == "$(dmb_ip_parse "$(jq -er .peer_address <<< "$current")")" ]] || continue
    canonical=$(dmb_ip_parse "$laddr") || continue
    [[ $canonical == "$(dmb_ip_parse "$(jq -er .local_address <<< "$current")")" ]] || continue
    [[ $port == "$(jq -er .local_port <<< "$current")" ]] || continue
    return 0
  done < <(jq -c '.[]' <<< "$contexts")
  _dmb_confirmation_error 'authenticated connection is outside the tested user/address/listener context'
}
dmb_confirmation_check() {
  local txdir=$1 mode=${2:-ssh} manifest current baseline proof digest expected generation id round remaining now deadline boot minimum_ticks intended_user
  _dmb_confirmation_root && dmb_trusted_path "$txdir" || return 1
  for proof in plan.json state.json challenge.sha256; do dmb_trusted_path "$txdir/$proof" || return 1; done
  manifest=$(_dmb_confirmation_manifest "$txdir") || return 1
  id=$(jq -er '.id|select(type=="string")' "$txdir/state.json") || return 1
  generation=$(jq -er '.generation|select(type=="number" and floor==. and .>=0)' "$txdir/state.json") || return 1
  round=$(jq -r '.confirmation_round // 0' "$txdir/state.json") || return 1
  case "$mode" in
    ssh)
      current=$(dmb_confirmation_transport "$manifest") || { _dmb_confirmation_error 'cannot prove an authenticated SSH transport from kernel state'; return 1; }
      intended_user=$(jq -er '.confirm_user // .origin_transport.login_user // empty' "$txdir/plan.json") || { _dmb_confirmation_error 'the intended confirmation user must be identified explicitly'; return 1; }
      jq -e --arg user "$intended_user" '.contexts|any(.[];.user==$user)' "$manifest" >/dev/null || return 1
      jq -e --arg user "$intended_user" '.login_user==$user and (.login_uid|type)=="number"' <<< "$current" >/dev/null || { _dmb_confirmation_error 'authenticated user differs from the intended confirmation user'; return 1; }
      _dmb_confirmation_context_match "$txdir" "$manifest" "$current" "$round" || return 1
      minimum_ticks=$(jq -er '.confirmation_min_start_ticks|select(type=="number" and floor==. and .>=0)' "$txdir/state.json") || { _dmb_confirmation_error 'missing post-application connection freshness boundary'; return 1; }
      jq -e --argjson minimum "$minimum_ticks" '(.start_ticks|tonumber)>$minimum' <<< "$current" >/dev/null || { _dmb_confirmation_error 'connection predates the latest applied policy; open a new SSH TCP connection'; return 1; }
      if (( round >= 1 )); then
        dmb_trusted_path "$txdir/proof-1.json" || return 1
        baseline=$(jq -ce '.transport' "$txdir/proof-1.json") || return 1
      else baseline=$(jq -ce '.origin_transport' "$txdir/plan.json") || return 1; fi
      _dmb_confirmation_fresh "$baseline" "$current" || { _dmb_confirmation_error 'a fresh TCP connection is required; disable SSH ControlMaster reuse'; return 1; }
      jq -e --argjson current "$current" '
        (.confirm_port // null) as $port | (.confirm_addresses // []) as $addresses |
        ($port==null or $current.local_port==$port) and
        ($addresses|length==0 or index($current.local_address)!=null or index("0.0.0.0")!=null or index("::")!=null)
      ' "$txdir/plan.json" >/dev/null || { _dmb_confirmation_error 'connection does not reach the target listener'; return 1; }
      ;;
    console)
      jq -e '.allow_console_confirmation == true' "$txdir/plan.json" >/dev/null || return 1
      jq -e '.confirmation.allow_console_confirmation == true' "$manifest" >/dev/null || return 1
      current=$(dmb_confirmation_console "$manifest") || { _dmb_confirmation_error 'cannot prove an enrolled local console'; return 1; }
      ;;
    *) return 2 ;;
  esac
  IFS= read -r expected < "$txdir/challenge.sha256" || return 1
  [[ "$expected" =~ ^[0-9a-f]{64}$ ]] || return 1
  boot=$(jq -er '.boot_id' "$txdir/state.json") || return 1
  [[ $boot == "$(cat /proc/sys/kernel/random/boot_id)" ]] || return 1
  deadline=$(jq -er '.deadline_uptime' "$txdir/state.json") || return 1
  read -r now _ < /proc/uptime || return 1
  now=${now%%.*}; remaining=$((deadline-now)); ((remaining>0)) || return 1
  digest=$(_dmb_confirmation_read_challenge "$remaining") || return 1
  [[ "$digest" == "$expected" ]] || { _dmb_confirmation_error 'challenge did not match'; return 1; }
  digest=$(printf '%s\n' "$current" | _dmb_confirmation_identity | _dmb_confirmation_digest) || return 1
  jq -cnS --arg id "$id" --arg mode "$mode" --argjson generation "$generation" --argjson transport "$current" --arg transport_id "$digest" \
    '{schema:1,transaction_id:$id,mode:$mode,generation:$generation,transport:$transport,transport_id:$transport_id,remote_access_verified:($mode=="ssh")}'
}
dmb_confirmation_verify() { dmb_confirmation_check "$@"; }

# Starting an operation from an OOB console requires its independent enrollment
# and kernel/tty identity. This does not assert external network reachability.
dmb_confirmation_origin() {
  local manifest=${1:-}
  [[ -n $manifest ]] || manifest=$(_dmb_confirmation_manifest) || return
  if dmb_confirmation_transport "$manifest" 2>/dev/null; then return 0; fi
  jq -e '.confirmation.allow_console_confirmation==true' "$manifest" >/dev/null || return 78
  dmb_confirmation_console "$manifest"
}

# Kernel process start_ticks and this boundary share CLK_TCK and boot identity.
# Uptime is monotonic across wall-clock adjustments; no external clock is used.
dmb_confirmation_now_ticks() {
  local uptime rest hz seconds fraction scale=1 i
  hz=$(getconf CLK_TCK) || return 1
  [[ $hz =~ ^[1-9][0-9]{0,5}$ ]] || return 1
  IFS=' ' read -r uptime rest < /proc/uptime || return 1
  [[ $uptime =~ ^[0-9]+[.][0-9]{1,6}$ ]] || return 1
  seconds=${uptime%.*}; fraction=${uptime#*.}
  for ((i=0;i<${#fraction};i++)); do scale=$((scale*10)); done
  printf '%s\n' "$((10#$seconds*hz + 10#$fraction*hz/scale))"
}
