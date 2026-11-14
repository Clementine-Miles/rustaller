# SSH service adapter. Only an explicitly enrolled, Port-only include is owned.
# Include/Match files remain byte-for-byte untouched. Socket activation is not
# approximated: enrollment rejects it until a dedicated backend is verified.
# shellcheck shell=bash

dmb_ssh_error() { printf 'ssh: %s\n' "$*" >&2; return 1; }
dmb_ssh_manifest_path() { printf '%s/enrollments/ssh.json\n' "$DMB_STATE_DIR"; }
dmb_ssh_manifest() {
  local IFS=$' \t\n'
  local path; path=$(dmb_ssh_manifest_path) || return 1
  dmb_trusted_path "$path" || return 1
  jq -e '.schema==1 and (.driver=="systemd-service" or .driver=="openrc-service")' "$path" >/dev/null 2>&1 || return 1
  printf '%s\n' "$path"
}
dmb_ssh_config_valid() {
  local IFS=$' \t\n'
  local manifest=$1 config fragment owner driver service binary context
  jq -e '.schema==1 and (.owner_id|test("^[a-f0-9]{32}$")) and
    (.driver=="systemd-service" or .driver=="openrc-service") and
    (.config|type=="string") and (.fragment|type=="string") and
    (.sshd|type=="string") and (.contexts|type=="array" and length>0 and length<=32) and
    all(.contexts[]; (.user|test("^[A-Za-z_][A-Za-z0-9_.-]{0,31}[$]?$")) and
       (.host|test("^[A-Za-z0-9_.:-]+$")) and (.addr|type=="string") and (.laddr|type=="string") and
       (.lport|type=="number" and floor==. and .>0 and .<=65535)) and
    (.confirmation|type=="object")' "$manifest" >/dev/null || return 2
  config=$(jq -er .config "$manifest"); fragment=$(jq -er .fragment "$manifest")
  binary=$(jq -er .sshd "$manifest"); driver=$(jq -er .driver "$manifest"); service=$(jq -er .service "$manifest")
  [[ $config == /etc/ssh/* && $fragment == /etc/ssh/* && $fragment != "$config" ]] || return 2
  dmb_trusted_path "$config" && dmb_trusted_path "$fragment" && dmb_trusted_path "$binary" || return 1
  [[ -f $config && -f $fragment && -x $binary ]] || return 1
  if [[ $driver == systemd-service ]]; then dmb_valid_unit "$service" && [[ $service == *.service ]] || return 2
  else [[ $service =~ ^[A-Za-z0-9_-]{1,64}$ ]] || return 2; fi
  while IFS= read -r context; do
    dmb_ip_parse "$(jq -er .addr <<< "$context")" >/dev/null || return 2
    dmb_ip_parse "$(jq -er .laddr <<< "$context")" >/dev/null || return 2
  done < <(jq -c '.contexts[]' "$manifest")
}
# Conservative Include parser is used only to establish a complete version
# inventory. Native sshd still parses and checks all policy semantics. Ambiguous
# escaping, quoted include paths and relative includes are refused pre-write.
_dmb_ssh_inventory_walk() {
  local IFS=$' \t\n'
  local path=$1 fragment=$2 depth=$3 line key rest pattern found child raw hash mode
  ((depth<32)) || return 78
  [[ ! -v DMB_SSH_VISITED[$path] ]] || return 0
  DMB_SSH_VISITED[$path]=1; ((${#DMB_SSH_VISITED[@]}<=256)) || return 78
  dmb_trusted_path "$path" || return 1
  [[ -f $path ]] || return 1
  if [[ $path != "$fragment" ]]; then
    raw=$(sha256sum "$path") || return 1; hash=${raw%% *}
    mode=$(stat -c '%u:%g:%a' "$path") || return 1
    jq -cn --arg path "$path" --arg hash "$hash" --arg metadata "$mode" '{path:$path,sha256:$hash,metadata:$metadata}' || return 1
  fi
  while IFS= read -r line || [[ -n $line ]]; do
    line=${line%%#*}; read -r key rest <<< "$line"
    [[ ${key,,} == include ]] || continue
    [[ -n $rest && $rest != *[\"\'\\]* ]] || return 78
    local -a patterns=(); read -r -a patterns <<< "$rest"
    for pattern in "${patterns[@]}"; do
      [[ $pattern == /etc/ssh/* && $pattern != *'/../'* && $pattern != */.. ]] || return 78
      # shellcheck disable=SC2053 # Pattern comparison detects wildcard overlap.
      if [[ $fragment == $pattern && $pattern != "$fragment" ]]; then
        dmb_ssh_error 'owned include is also matched by another Include pattern'; return 78
      fi
      found=$(compgen -G "$pattern" | LC_ALL=C sort) || :
      [[ -n $found ]] || continue
      while IFS= read -r child; do _dmb_ssh_inventory_walk "$child" "$fragment" "$((depth+1))" || return; done <<< "$found"
    done
  done < "$path"
}
dmb_ssh_inventory() {
  local IFS=$' \t\n'
  local manifest=$1 config fragment lines
  config=$(jq -er .config "$manifest") || return 1; fragment=$(jq -er .fragment "$manifest") || return 1
  local -A DMB_SSH_VISITED=()
  lines=$(_dmb_ssh_inventory_walk "$config" "$fragment" 0) || return 1
  printf '%s\n' "$lines" | jq -scS 'sort_by(.path)'
}
dmb_ssh_ports() {
  local IFS=$' \t\n'
  local fragment=$1 owner=$2 line key value marker=0 ports='[]'
  while IFS= read -r line || [[ -n $line ]]; do
    [[ $line == "# dmitbox-local:$owner" ]] && marker=$((marker+1))
    line=${line%%#*}; read -r key value <<< "$line"
    [[ -n $key ]] || continue
    [[ ${key,,} == port && $value =~ ^[0-9]{1,5}$ ]] || return 78
    ((10#$value>0 && 10#$value<=65535)) || return 78
    ports=$(jq -c --argjson p "$((10#$value))" '.+[$p]' <<< "$ports") || return 1
  done < "$fragment"
  ((marker==1)) || return 78
  jq -ce 'select(length>0 and length<=16 and (unique|length)==length)|sort' <<< "$ports"
}
dmb_ssh_service_pid() {
  local IFS=$' \t\n'
  local manifest=$1 driver service pid pidfile binary exe hash expected
  driver=$(jq -er .driver "$manifest"); service=$(jq -er .service "$manifest")
  binary=$(jq -er .sshd "$manifest")
  if [[ $driver == systemd-service ]]; then
    pid=$(dmb_run 5 systemctl show "$service" -p MainPID --value) || return
  else
    pidfile=$(jq -er '.pidfile|select(type=="string")' "$manifest") || return 78
    [[ $pidfile == /run/* ]] && dmb_trusted_path "$pidfile" || return 78
    IFS= read -r pid < "$pidfile" || return
  fi
  [[ $pid =~ ^[1-9][0-9]*$ && -d /proc/$pid ]] || return 78
  exe=$(readlink "/proc/$pid/exe") || return
  [[ $exe == "$binary" ]] || { dmb_ssh_error 'enrolled service does not own the SSH daemon'; return 78; }
  hash=$(sha256sum "/proc/$pid/exe"); hash=${hash%% *}
  expected=$(sha256sum "$binary"); expected=${expected%% *}
  [[ $hash == "$expected" ]] || return 76
  printf '%s\n' "$pid"
}
dmb_ssh_service_guard() {
  local IFS=$' \t\n'
  local manifest=$1 driver service triggers
  driver=$(jq -er .driver "$manifest"); service=$(jq -er .service "$manifest")
  if [[ $driver == systemd-service ]]; then
    [[ -d /run/systemd/system ]] || return 78
    dmb_need systemctl || return
    triggers=$(dmb_run 5 systemctl show "$service" -p TriggeredBy --value) || return 1
    [[ -z $triggers ]] || { dmb_ssh_error 'socket-triggered SSH requires an independently verified socket backend'; return 78; }
    dmb_run 5 systemctl -q is-active "$service" || return 1
  else
    [[ ! -d /run/systemd/system ]] && command -v rc-service >/dev/null || return 78
    dmb_run 5 rc-service "$service" status >/dev/null || return 1
    local path comm
    for path in /proc/[0-9]*/comm; do IFS= read -r comm < "$path" || continue
      case $comm in inetd|xinetd) dmb_ssh_error 'inetd-style SSH ownership is unsupported'; return 78;; esac
    done
  fi
  dmb_ssh_service_pid "$manifest" >/dev/null
}

dmb_ssh_effective() {
  local IFS=$' \t\n'
  local manifest=$1 config=$2 context=$3 binary args
  binary=$(jq -er .sshd "$manifest") || return 1
  args=$(jq -er '"user="+.user+",host="+.host+",addr="+.addr+",laddr="+.laddr+",lport="+(.lport|tostring)' <<< "$context") || return 1
  dmb_run 10 "$binary" -T -f "$config" -C "$args"
}
dmb_ssh_effective_policy() {
  local IFS=$' \t\n'
  # Port and ListenAddress change intentionally. All authentication/Match policy
  # must remain identical for each explicitly enrolled connection context.
  LC_ALL=C sed '/^port /d;/^listenaddress /d' | sha256sum | cut -d' ' -f1
}
dmb_ssh_effective_endpoints() {
  local IFS=$' \t\n'
  local output=$1 line key value endpoint address port result='[]'
  while IFS=' ' read -r key value; do
    [[ $key == listenaddress ]] || continue
    endpoint=$(_dmb_confirmation_endpoint "$value") || return 1
    IFS=$'\t' read -r address port <<< "$endpoint"
    address=$(dmb_ip_parse "$address") || return 1
    result=$(jq -c --arg a "$address" --argjson p "$port" '.+[{address:$a,port:$p}]' <<< "$result") || return 1
  done <<< "$output"
  jq -ce 'unique|sort_by(.address,.port)|select(length>0)' <<< "$result"
}
# Native policy simulation changes exactly one explicit anchor in a private copy.
dmb_ssh_simulate() {
  local IFS=$' \t\n'
  local manifest=$1 content=$2 config fragment private line key rest anchors=0 effective context ports policy='' endpoints='' candidate
  config=$(jq -er .config "$manifest"); fragment=$(jq -er .fragment "$manifest")
  private=$(dmb_scratch_tmp) || return 1
  if ! printf '%s' "$content" | base64 -d > "$private/ports.conf"; then rm -rf -- "$private"; return 1; fi
  while IFS= read -r line || [[ -n $line ]]; do
    read -r key rest <<< "${line%%#*}"
    if [[ ${key,,} == include && $rest == "$fragment" ]]; then
      anchors=$((anchors+1)); printf 'Include %s/ports.conf\n' "$private"
    else printf '%s\n' "$line"; fi
  done < "$config" > "$private/sshd_config"
  if ((anchors!=1)); then rm -rf -- "$private"; dmb_ssh_error 'one explicit owned Include anchor in main config is required'; return 78; fi
  local results='[]'
  while IFS= read -r context; do
    effective=$(dmb_ssh_effective "$manifest" "$private/sshd_config" "$context") || { rm -rf -- "$private"; return 1; }
    # The kernel proves peer IP, not a DNS-derived client hostname. In the
    # supported UseDNS=no case sshd uses that numeric peer as Match Host.
    # Refuse a hypothetical hostname context that could hide LocalPort policy.
    if [[ $(awk '$1=="usedns" {print $2}' <<< "$effective") != no ||
          $(jq -er .host <<< "$context") != "$(jq -er .addr <<< "$context")" ]]; then
      rm -rf -- "$private"
      dmb_ssh_error 'port migration requires UseDNS no and a numeric peer-IP Host context; DNS-derived Match Host is unsupported'
      return 78
    fi
    ports=$(awk '$1=="port" {print $2}' <<< "$effective" | jq -sc 'sort|unique') || { rm -rf -- "$private"; return 1; }
    policy=$(dmb_ssh_effective_policy <<< "$effective") || { rm -rf -- "$private"; return 1; }
    endpoints=$(dmb_ssh_effective_endpoints "$effective") || { rm -rf -- "$private"; return 1; }
    candidate=$(jq -cn --argjson c "$context" --argjson p "$ports" --arg hash "$policy" --argjson e "$endpoints" '{context:$c,ports:$p,policy_sha256:$hash,endpoints:$e}') || return 1
    results=$(jq -c --argjson row "$candidate" '.+[$row]' <<< "$results") || return 1
  done < <(jq -c '.contexts[]' "$manifest")
  rm -rf -- "$private" || return 1
  printf '%s\n' "$results"
}
dmb_ssh_enroll() {
  local IFS=$' \t\n'
  local config=$1 inventory owner fragment ports content simulation binary hash pid group binaries helper
  dmb_ssh_config_valid "$config" && dmb_ssh_service_guard "$config" || return
  inventory=$(dmb_ssh_inventory "$config") || return 1
  owner=$(jq -er .owner_id "$config"); fragment=$(jq -er .fragment "$config")
  ports=$(dmb_ssh_ports "$fragment" "$owner") || return 78
  content=$(base64 -w0 "$fragment") || return 1
  simulation=$(dmb_ssh_simulate "$config" "$content") || return
  jq -e --argjson ports "$ports" 'all(.[];.ports==$ports)' <<< "$simulation" >/dev/null || { dmb_ssh_error 'effective Ports are also owned elsewhere'; return 78; }
  binary=$(jq -er .sshd "$config"); hash=$(sha256sum "$binary"); hash=${hash%% *}
  pid=$(dmb_ssh_service_pid "$config") || return
  group=$(_dmb_confirmation_cgroup /proc "$pid") || { dmb_ssh_error 'cannot enroll daemon cgroup identity'; return 78; }
  binaries=$(jq -cn --arg path "$binary" --arg hash "$hash" '[{path:$path,sha256:$hash}]') || return
  # OpenSSH 9.8+ splits authenticated sessions into root-owned native helpers.
  # Pin only known installed paths, never caller-provided arbitrary executables.
  for helper in /usr/lib/openssh/sshd-session /usr/lib/openssh/sshd-auth /usr/lib/ssh/sshd-session /usr/lib/ssh/sshd-auth /usr/libexec/sshd-session /usr/libexec/sshd-auth; do
    [[ -e $helper ]] || continue
    dmb_trusted_path "$helper" && [[ -f $helper && -x $helper ]] || return 78
    hash=$(sha256sum "$helper"); hash=${hash%% *}
    binaries=$(jq -c --arg path "$helper" --arg hash "$hash" '.+[{path:$path,sha256:$hash}]' <<< "$binaries") || return
  done
  hash=$(sha256sum "$binary"); hash=${hash%% *}
  jq -cS --argjson inventory "$inventory" --argjson effective "$simulation" --arg hash "$hash" --arg group "$group" --argjson binaries "$binaries" '. + {inventory:$inventory,effective:$effective,sshd_sha256:$hash} |
    .confirmation.sshd_binaries=$binaries | .confirmation.ssh_cgroups=[$group]' "$config"
}
dmb_ssh_guard() {
  local IFS=$' \t\n'
  local manifest=$1 inventory hash binary
  dmb_ssh_config_valid "$manifest" && dmb_ssh_service_guard "$manifest" || return
  binary=$(jq -er .sshd "$manifest"); hash=$(sha256sum "$binary"); hash=${hash%% *}
  [[ $hash == "$(jq -er .sshd_sha256 "$manifest")" ]] || return 76
  inventory=$(dmb_ssh_inventory "$manifest") || return 1
  [[ $inventory == "$(jq -cS .inventory "$manifest")" ]] || { dmb_ssh_error 'unowned SSH configuration version changed'; return 76; }
}
dmb_ssh_capture() {
  local IFS=$' \t\n'
  local manifest path; manifest=$(dmb_ssh_manifest) || return
  dmb_ssh_guard "$manifest" || return
  path=$(jq -er .fragment "$manifest") || return
  dmb_file_state "$path"
}
dmb_ssh_snapshot() {
  local IFS=$' \t\n'
  local manifest path id
  manifest=$(dmb_ssh_manifest) || return; path=$(jq -er .fragment "$manifest"); id=$(jq -er --argjson i "$2" '.steps[$i].id' "$1")
  dmb_ssh_plan_shape "$1" || return
  if [[ $2 == 1 ]]; then
    dmb_ssh_payload_check "$1" 1 "$manifest" || return
    dmb_ssh_payload_check "$1" 2 "$manifest" || return
    dmb_ssh_firewall_check "$1" "$manifest" || return
  fi
  dmb_file_capture "$path" "$3/snapshots" "$id"
}
dmb_ssh_plan_shape() {
  jq -e '.adapter=="ssh" and .requires_confirmation==true and
    (.confirm_port|type=="number" and floor==. and .>=1 and .<=65535) and
    (.steps|length)==4 and
    [.steps[].adapter]==["nft","ssh","ssh","nft"] and
    (.steps[0].phase//"apply")=="apply" and (.steps[1].phase//"apply")=="apply" and
    .steps[2].phase=="commit" and .steps[3].phase=="commit" and
    .steps[0].resource==.steps[3].resource and .steps[1].resource==.steps[2].resource and
    .steps[0].after==.steps[3].before and .steps[1].after==.steps[2].before and
    .steps[0].nft==.steps[3].nft' "$1" >/dev/null || return 64
}
dmb_ssh_payload_check() (
  local plan=$1 index=$2 manifest=$3 temp field expected contexts native content before_policy after_policy
  dmb_ssh_plan_shape "$plan" || return
  [[ $index == 1 || $index == 2 ]] || return 64
  temp=$(dmb_scratch_tmp) || return
  trap 'rm -rf -- "$temp"' EXIT
  for field in before after; do
    if [[ $index == 1 && $field == before ]]; then contexts=$(jq -c '.contexts' "$manifest") || return
    else contexts=$(jq -c --argjson port "$(jq -r .confirm_port "$plan")" '.contexts|map(.lport=$port)' "$manifest") || return; fi
    expected=$(jq -ceS --argjson i "$index" --arg field "$field" '.steps[$i][($field+"_effective")]' "$plan") || return
    [[ $(jq -cS '[.[].context]' <<< "$expected") == "$(jq -cS . <<< "$contexts")" ]] || return 76
    jq -c --argjson contexts "$contexts" '.contexts=$contexts' "$manifest" > "$temp/context.json" || return
    if [[ $field == before ]]; then content=$(jq -er --argjson i "$index" '.steps[$i].before_content_b64' "$plan") || return
    else content=$(jq -er --argjson i "$index" '.steps[$i].content_b64' "$plan") || return; fi
    native=$(dmb_ssh_simulate "$temp/context.json" "$content" | jq -ceS .) || return
    [[ $native == "$expected" ]] || return 76
    if [[ $field == before ]]; then before_policy=$(jq -c '[.[].policy_sha256]' <<< "$native")
    else after_policy=$(jq -c '[.[].policy_sha256]' <<< "$native"); fi
  done
  [[ $before_policy == "$after_policy" ]] || { dmb_ssh_error 'port payload changes authentication policy for the enrolled context'; return 76; }
  if [[ $index == 2 ]]; then
    jq -e --argjson port "$(jq -r .confirm_port "$plan")" 'all(.[];.ports==[$port] and all(.endpoints[];.port==$port))' <<< "$native" >/dev/null || return 76
  fi
)
dmb_ssh_firewall_check() {
  local plan=$1 manifest=$2 selector before owner endpoints expected actual
  selector=$(jq -ce .steps[0].nft "$plan") || return
  dmb_nft_authorize "$selector" || return
  before=$(jq -ce .steps[0].before "$plan") || return
  owner=$(jq -er .owner_id "$manifest") || return
  local index
  for index in 0 3; do
    if [[ $index == 0 ]]; then endpoints=$(jq -ce '.steps[1].after_effective|map(.endpoints)|unique|select(length==1)|.[0]' "$plan") || return
    else endpoints=$(jq -ce '.steps[2].after_effective|map(.endpoints)|unique|select(length==1)|.[0]' "$plan") || return; fi
    expected=$(dmb_ssh_nft_desired "$selector" "$before" "$endpoints" "$owner" | jq -ceS .) || return
    actual=$(jq -ceS --argjson i "$index" '.steps[$i].after' "$plan") || return
    [[ $actual == "$expected" ]] || return 76
  done
}
dmb_ssh_reload() {
  local IFS=$' \t\n'
  local manifest=$1 driver service binary config
  driver=$(jq -er .driver "$manifest"); service=$(jq -er .service "$manifest")
  binary=$(jq -er .sshd "$manifest"); config=$(jq -er .config "$manifest")
  dmb_run 10 "$binary" -t -f "$config" || return
  if [[ $driver == systemd-service ]]; then dmb_run 15 systemctl reload "$service"
  else dmb_run 15 rc-service "$service" reload; fi
}
dmb_ssh_step_validate() {
  local IFS=$' \t\n'
  local plan=$1 index=$2 manifest=$3 path owner field content state hash temp ports
  dmb_ssh_payload_check "$plan" "$index" "$manifest" || return
  dmb_ssh_firewall_check "$plan" "$manifest" || return
  path=$(jq -er --argjson i "$index" '.steps[$i].path' "$plan") || return 2
  [[ $path == "$(jq -er .fragment "$manifest")" ]] || return 76
  owner=$(jq -er .owner_id "$manifest") || return 2
  jq -e --argjson i "$index" '.steps[$i] | .before.exists==true and .after.exists==true and
    (.before|del(.sha256))==(.after|del(.sha256))' "$plan" >/dev/null || return 2
  temp=$(dmb_scratch_tmp) || return
  for field in before after; do
    if [[ $field == before ]]; then content=$(jq -er --argjson i "$index" '.steps[$i].before_content_b64' "$plan")
    else content=$(jq -er --argjson i "$index" '.steps[$i].content_b64' "$plan"); fi
    if ! printf '%s' "$content" | base64 -d > "$temp/ports"; then rm -rf -- "$temp"; return 2; fi
    ports=$(dmb_ssh_ports "$temp/ports" "$owner") || { rm -rf -- "$temp"; return 2; }
    hash=$(sha256sum "$temp/ports"); hash=${hash%% *}
    state=$(jq -er --argjson i "$index" --arg field "$field" '.steps[$i][$field].sha256' "$plan") || { rm -rf -- "$temp"; return 2; }
    [[ $hash == "$state" ]] || { rm -rf -- "$temp"; return 76; }
    # A typed port-only payload cannot silently weaken auth policy. Policy
    # readback still compares each explicitly enrolled Match context afterward.
  done
  rm -rf -- "$temp"
}
dmb_ssh_apply() {
  local IFS=$' \t\n'
  local manifest; manifest=$(dmb_ssh_manifest) || return
  dmb_ssh_guard "$manifest" || return
  dmb_ssh_step_validate "$1" "$2" "$manifest" || return
  dmb_fs_apply "$@" || return
  dmb_ssh_reload "$manifest"
}
dmb_ssh_verify_field() {
  local IFS=$' \t\n'
  local plan=$1 index=$2 field=$3 manifest expected context effective hash ports endpoints wanted state
  manifest=$(dmb_ssh_manifest) || return; dmb_ssh_guard "$manifest" || return
  expected=$(jq -ceS --argjson i "$index" --arg field "$field" '.steps[$i][$field]' "$plan") || return
  state=$(dmb_ssh_capture "$plan" "$index" | jq -cS .) || return
  [[ $state == "$expected" ]] || return 76
  local config binary
  config=$(jq -er .config "$manifest"); binary=$(jq -er .sshd "$manifest")
  dmb_run 10 "$binary" -t -f "$config" || return
  while IFS= read -r wanted; do
    context=$(jq -ce .context <<< "$wanted")
    effective=$(dmb_ssh_effective "$manifest" "$config" "$context") || return
    hash=$(dmb_ssh_effective_policy <<< "$effective") || return
    ports=$(awk '$1=="port" {print $2}' <<< "$effective" | jq -sc 'sort|unique') || return
    endpoints=$(dmb_ssh_effective_endpoints "$effective") || return
    [[ $hash == "$(jq -er .policy_sha256 <<< "$wanted")" && $ports == "$(jq -c .ports <<< "$wanted")" && $endpoints == "$(jq -c .endpoints <<< "$wanted")" ]] || return 76
  done < <(jq -c --argjson i "$index" --arg field "$field" '.steps[$i][($field+"_effective")][]' "$plan")
  # Effective settings and local listeners are local postconditions only; the
  # transaction remains pending until the separate fresh-login proof succeeds.
  dmb_ssh_listener_check "$manifest" "$endpoints"
}
dmb_ssh_listener_check() {
  local IFS=$' \t\n'
  local manifest=$1 expected=$2 listing state _recv _send local_ep _peer rest endpoint address port observed_endpoints='[]' pid link binary hash enrolled_hash service_pid
  listing=$(dmb_run 5 ss -H -l -n -t -p -O) || return
  binary=$(jq -er .sshd "$manifest"); enrolled_hash=$(jq -er .sshd_sha256 "$manifest")
  service_pid=$(dmb_ssh_service_pid "$manifest") || return
  while IFS= read -r line; do
    read -r state _recv _send local_ep _peer rest <<< "$line"
    [[ $state == LISTEN && $rest == *'"sshd"'* ]] || continue
    [[ $rest =~ pid=([0-9]+) ]] || return 1; pid=${BASH_REMATCH[1]}
    [[ $pid == "$service_pid" ]] || return 76
    link=$(readlink "/proc/$pid/exe") || return
    [[ $link == "$binary" ]] || return 76
    hash=$(sha256sum "/proc/$pid/exe"); hash=${hash%% *}; [[ $hash == "$enrolled_hash" ]] || return 76
    endpoint=$(_dmb_confirmation_endpoint "$local_ep") || return
    IFS=$'\t' read -r address port <<< "$endpoint"; address=$(dmb_ip_parse "$address") || return
    observed_endpoints=$(jq -c --arg a "$address" --argjson p "$port" '.+[{address:$a,port:$p}]' <<< "$observed_endpoints") || return
  done <<< "$listing"
  observed_endpoints=$(jq -c 'unique|sort_by(.address,.port)' <<< "$observed_endpoints") || return
  [[ $observed_endpoints == "$expected" ]] || { dmb_ssh_error 'local SSH socket inventory differs from the intended listeners'; return 76; }
}
dmb_ssh_verify() { dmb_ssh_verify_field "$1" "$2" after; }
dmb_ssh_verify_restore() { dmb_ssh_verify_field "$1" "$2" before; }
dmb_ssh_restore() {
  local IFS=$' \t\n'
  local plan=$1 index=$2 manifest path before current content mode
  manifest=$(dmb_ssh_manifest) || return; dmb_ssh_guard "$manifest" || return
  dmb_ssh_step_validate "$plan" "$index" "$manifest" || return
  path=$(jq -er .fragment "$manifest"); before=$(jq -ceS --argjson i "$index" '.steps[$i].before' "$plan") || return
  current=$(dmb_file_state "$path" | jq -cS .) || return
  if [[ $current != "$before" ]]; then
    [[ $current == "$(jq -ceS --argjson i "$index" '.steps[$i].after' "$plan")" ]] || return 76
    content=$(jq -er --argjson i "$index" '.steps[$i].before_content_b64' "$plan") || return
    mode=$(jq -er '.mode' <<< "$before") || return
    printf '%s' "$content" | base64 -d | dmb_atomic_write "$path" "$mode" "$(jq -er '.sha256' <<< "$before")" "$before" || return
  fi
  dmb_ssh_reload "$manifest" || return
  dmb_ssh_verify_restore "$@"
}

dmb_ssh_nft_desired() {
  local IFS=$' \t\n'
  local selector=$1 current=$2 endpoints=$3 owner=$4 fresh tag
  fresh=$(dmb_nft_plan_state "$selector" "$endpoints" '[]' accept) || return
  tag="dmitbox-local:ssh:$owner"
  jq -cnS --argjson s "$selector" --argjson current "$current" --argjson fresh "$fresh" --arg tag "$tag" '
    [$fresh.nftables[]|.rule? // empty|select(.chain==$s.chain and any(.expr[]; .match?.left.payload?.field=="dport"))|{rule:(.+{comment:$tag})}] as $permits |
    {present:true,nftables:([$current.nftables[]|select(has("table") or has("chain"))]+$permits+
      [$current.nftables[]|select((has("table") or has("chain"))|not)|select(.rule?.comment!=$tag)])}'
}
dmb_ssh_plan_port() {
  local IFS=$' \t\n'
  local port=$1 manifest nft_manifest old_ports stage_ports owner fragment before_content stage_content final_content
  local before stage final before_effective stage_effective final_effective selector nft_before nft_stage nft_final origin endpoints context_dir
  [[ $port =~ ^[0-9]{1,5}$ ]] && ((10#$port>0 && 10#$port<=65535)) || return 64
  port=$((10#$port)); manifest=$(dmb_ssh_manifest) || return
  dmb_ssh_guard "$manifest" || return
  # A separate ban manager may synthesize its own netfilter ownership and port
  # policy. Do not silently leave that policy on the retired listener.
  local proc comm
  for proc in /proc/[0-9]*/comm; do IFS= read -r comm < "$proc" || continue
    [[ $comm != fail2ban-server ]] || { dmb_ssh_error 'active fail2ban requires a dedicated coordinated jail adapter'; return 78; }
  done
  owner=$(jq -er .owner_id "$manifest"); fragment=$(jq -er .fragment "$manifest")
  old_ports=$(dmb_ssh_ports "$fragment" "$owner") || return
  jq -e --argjson p "$port" 'index($p)==null' <<< "$old_ports" >/dev/null || { dmb_ssh_error 'requested port is already configured'; return 64; }
  before_content=$(base64 -w0 "$fragment") || return
  stage_ports=$(jq -c --argjson p "$port" '.+[$p]|sort' <<< "$old_ports") || return
  stage_content=$({ printf '# dmitbox-local:%s\n' "$owner"; jq -r '.[]|"Port "+tostring' <<< "$stage_ports"; } | base64 -w0) || return
  final_content=$(printf '# dmitbox-local:%s\nPort %s\n' "$owner" "$port" | base64 -w0) || return
  before=$(dmb_file_state "$fragment" | jq -cS .) || return
  before_effective=$(dmb_ssh_simulate "$manifest" "$before_content") || return
  context_dir=$(dmb_scratch_tmp) || return
  if ! jq --argjson port "$port" '.contexts |= map(.lport=$port)' "$manifest" > "$context_dir/manifest.json"; then rm -rf -- "$context_dir"; return 1; fi
  stage_effective=$(dmb_ssh_simulate "$context_dir/manifest.json" "$stage_content") || { rm -rf -- "$context_dir"; return 1; }
  final_effective=$(dmb_ssh_simulate "$context_dir/manifest.json" "$final_content") || { rm -rf -- "$context_dir"; return 1; }
  rm -rf -- "$context_dir" || return
  jq -e --argjson old "$before_effective" --argjson stage "$stage_ports" --argjson p "$port" '
    all(to_entries[]; . as $entry | .value.ports==[$p] and
      .value.policy_sha256==$old[$entry.key].policy_sha256 and
      all(.value.endpoints[];.port==$p))' <<< "$final_effective" >/dev/null || { dmb_ssh_error 'final effective policy/listeners cannot be proven to preserve the enrolled context'; return 78; }
  jq -e --argjson old "$before_effective" --argjson ports "$stage_ports" 'all(to_entries[];. as $e|.value.ports==$ports and .value.policy_sha256==$old[$e.key].policy_sha256)' <<< "$stage_effective" >/dev/null || return 78
  endpoints=$(jq -ce 'map(.endpoints)|unique|select(length==1)|.[0]' <<< "$stage_effective") || return 78
  # Any listener already in use by another service prevents mutation. Existing
  # SSH listeners are allowed only if they are part of enrolled effective state.
  dmb_ssh_listener_check "$manifest" "$(jq -ce 'map(.endpoints)|unique|select(length==1)|.[0]' <<< "$before_effective")" || return
  local occupied
  occupied=$(dmb_run 5 ss -H -l -n -t -O "sport = :$port") || return
  [[ -z $occupied ]] || { dmb_ssh_error 'target port is already bound'; return 78; }
  stage=$(jq -cnS --argjson before "$before" --arg hash "$(printf '%s' "$stage_content" | base64 -d | sha256sum | cut -d' ' -f1)" '$before+{sha256:$hash}') || return
  final=$(jq -cnS --argjson before "$before" --arg hash "$(printf '%s' "$final_content" | base64 -d | sha256sum | cut -d' ' -f1)" '$before+{sha256:$hash}') || return
  nft_manifest=$(dmb_nft_enrollment_path) || return; dmb_trusted_path "$nft_manifest" || return
  selector=$(jq -ce '.nft' "$nft_manifest") || return
  nft_before=$(dmb_nft_capture <(jq -cn --argjson s "$selector" '{steps:[{nft:$s}]}') 0) || return
  [[ $(jq -r .present <<< "$nft_before") == true ]] || { dmb_ssh_error 'enroll and establish a scoped nft policy before migrating SSH'; return 78; }
  nft_stage=$(dmb_ssh_nft_desired "$selector" "$nft_before" "$endpoints" "$owner") || return
  nft_final=$(dmb_ssh_nft_desired "$selector" "$nft_before" "$(jq -ce '.[0].endpoints' <<< "$final_effective")" "$owner") || return
  origin=$(dmb_confirmation_origin "$manifest") || { dmb_ssh_error 'start from a provable authenticated SSH transport or explicitly enrolled OOB console'; return 78; }
  jq -cnS --argjson s "$selector" --arg path "$fragment" --argjson before "$before" --argjson stage "$stage" --argjson final "$final" \
    --arg bc "$before_content" --arg sc "$stage_content" --arg fc "$final_content" --argjson be "$before_effective" --argjson se "$stage_effective" --argjson fe "$final_effective" \
    --argjson nb "$nft_before" --argjson ns "$nft_stage" --argjson nf "$nft_final" --argjson origin "$origin" --argjson port "$port" --argjson allow_console "$(jq -c '.confirmation.allow_console_confirmation==true' "$manifest")" '
    ("nft-"+$s.family+"-"+$s.table) as $nr |
    {schema:1,adapter:"ssh",resources:["ssh-service",$nr],timeout_seconds:300,origin_transport:$origin,confirm_port:$port,confirmation_enrollment:"ssh",requires_confirmation:true,allow_console_confirmation:$allow_console,
    steps:[
      {id:"ssh-firewall-stage",adapter:"nft",resource:$nr,nft:$s,before:$nb,after:$ns,restore_rank:-10},
      {id:"ssh-listener-stage",adapter:"ssh",resource:"ssh-service",path:$path,before:$before,after:$stage,before_content_b64:$bc,content_b64:$sc,before_effective:$be,after_effective:$se,restore_rank:10},
      {id:"ssh-listener-final",phase:"commit",adapter:"ssh",resource:"ssh-service",path:$path,before:$stage,after:$final,before_content_b64:$sc,content_b64:$fc,before_effective:$se,after_effective:$fe,restore_rank:10},
      {id:"ssh-firewall-final",phase:"commit",adapter:"nft",resource:$nr,nft:$s,before:$ns,after:$nf,restore_rank:-10}]}'
}
