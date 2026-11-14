# Regular-file ownership adapter. Raw snapshots and inode identity are separate
# from the stable logical state used by the transaction engine.
dmb_file_metadata() {
  local path=$1 acl attrs
  dmb_need getfacl getfattr base64 jq || return
  acl=$(LC_ALL=C getfacl -cpn -- "$path" | base64 -w0) || return
  attrs=$(LC_ALL=C getfattr --absolute-names --encoding=hex -d -m - -- "$path" | sed '/^#/d;/^$/d' | LC_ALL=C sort | base64 -w0) || return
  jq -cn --arg mode "$(stat -c %a -- "$path")" --argjson uid "$(stat -c %u -- "$path")" --argjson gid "$(stat -c %g -- "$path")" --arg acl "$acl" --arg attrs "$attrs" '{mode:$mode,uid:$uid,gid:$gid,acl_b64:$acl,xattrs_b64:$attrs}'
}
dmb_file_state() {
  local path=$1 meta hash
  dmb_trusted_path "$path" allow_missing || return
  if [[ ! -e $path ]]; then printf '{"exists":false}\n'; return; fi
  [[ -f $path && ! -L $path && $(stat -c %h -- "$path") == 1 ]] || { dmb_error 'Only regular files with one hard link are supported'; return 78; }
  meta=$(dmb_file_metadata "$path") || return
  hash=$(sha256sum -- "$path") || return; hash=${hash%% *}
  jq -cn --argjson meta "$meta" --arg hash "$hash" '$meta+{exists:true,type:"file",sha256:$hash}'
}
dmb_file_desired() {
  local path=$1 content=$2 mode=${3:-600} before hash acl u g o
  [[ -f $content && ! -L $content && $mode =~ ^[0-7]{3}$ ]] || return 64
  before=$(dmb_file_state "$path") || return
  hash=$(sha256sum -- "$content") || return; hash=${hash%% *}
  if [[ $(jq -r .exists <<< "$before") == true ]]; then jq -cS --arg hash "$hash" '.sha256=$hash' <<< "$before"; return; fi
  u=$((8#${mode:0:1})); g=$((8#${mode:1:1})); o=$((8#${mode:2:1}))
  local perms='' n bit; for n in "$u" "$g" "$o"; do for bit in 4 2 1; do if ((n&bit)); then case $bit in 4) perms+=r;; 2) perms+=w;; 1) perms+=x;; esac; else perms+=-; fi; done; done
  acl=$(printf 'user::%s\ngroup::%s\nother::%s\n\n' "${perms:0:3}" "${perms:3:3}" "${perms:6:3}" | base64 -w0)
  jq -cn --arg hash "$hash" --arg mode "$mode" --arg acl "$acl" '{exists:true,type:"file",sha256:$hash,mode:$mode,uid:0,gid:0,acl_b64:$acl,xattrs_b64:""}'
}
dmb_file_capture() {
  local path=$1 dir=$2 slot=$3 before after identity
  dmb_valid_name "$slot" || return 64
  dmb_trusted_path "$dir" || return
  before=$(dmb_file_state "$path" | jq -cS .) || return
  [[ ! -e $dir/$slot.json && ! -e $dir/$slot.raw ]] || return 78
  if [[ $(jq -r .exists <<< "$before") == true ]]; then
    identity=$(stat -c '%d:%i:%h' -- "$path") || return
    cp -a -- "$path" "$dir/$slot.raw" || return
    after=$(dmb_file_state "$dir/$slot.raw" | jq -cS .) || return
    [[ $before == "$after" && $identity == "$(stat -c '%d:%i:%h' -- "$path")" ]] || return 76
    sync -f "$dir/$slot.raw" || return
  else identity=absent; fi
  jq -cn --arg path "$path" --arg identity "$identity" --argjson state "$before" '{schema:1,path:$path,identity:$identity,state:$state}' | dmb_atomic_write "$dir/$slot.json" 0600
}
dmb_file_restore() {
  local path=$1 dir=$2 slot=$3 expected=$4 current before raw tmp parent
  dmb_valid_name "$slot" || return 64
  dmb_trusted_path "$dir/$slot.json" || return
  [[ $(jq -er .path "$dir/$slot.json") == "$path" ]] || return 76
  current=$(dmb_file_state "$path" | jq -cS .) || return
  before=$(jq -cS .state "$dir/$slot.json") || return
  [[ $current != "$before" ]] || return 0
  [[ $(printf '%s\n' "$current" | sha256sum | cut -d' ' -f1) == "$expected" ]] || return 76
  parent=${path%/*}
  if [[ $(jq -r .exists <<< "$before") == false ]]; then rm -- "$path" || return; sync -f "$parent" || return
  else
    raw=$dir/$slot.raw
    [[ $(dmb_file_state "$raw" | jq -cS .) == "$before" ]] || return 76
    tmp=$(mktemp "$parent/.dmitbox-restore.XXXXXXXXXXXX") || return
    if ! cp --preserve=all -- "$raw" "$tmp"; then rm -f -- "$tmp"; return 74; fi
    # A successful cp alone does not establish ACL/xattr/security-context
    # preservation. Never replace the live resource with an unverified copy.
    if [[ $(dmb_file_state "$tmp" | jq -cS .) != "$before" ]]; then rm -f -- "$tmp"; return 78; fi
    if ! sync -f "$tmp" || ! mv -T -- "$tmp" "$path" || ! sync -f "$parent"; then rm -f -- "$tmp"; return 74; fi
  fi
  [[ $(dmb_file_state "$path" | jq -cS .) == "$before" ]]
}
dmb_fs_plan_validate() {
  local plan=$1 index=$2 adapter purpose path content algorithm expected_content
  adapter=$(jq -er --argjson i "$index" '.steps[$i].adapter // .adapter' "$plan") || return 64
  # SSH uses the file primitives after validating its enrolled fragment and
  # port-only payload. It must not inherit generic file-write authority.
  [[ $adapter == fs ]] || return 0
  purpose=$(jq -er --argjson i "$index" '.steps[$i].purpose' "$plan") || return 64
  path=$(jq -er --argjson i "$index" '.steps[$i].path' "$plan") || return 64
  case $purpose in
    gai) [[ $path == "$DMB_ETC_ROOT/gai.conf" ]] || return 64;;
    tcp-profile)
      [[ $path == /etc/sysctl.d/90-dmitbox-local-tcp.conf ]] || return 64
      jq -e --argjson i "$index" '.steps[$i] | .after.exists==true and (.before==null or .before.exists==false)' "$plan" >/dev/null || return 64
      content=$(jq -er --argjson i "$index" '.steps[$i].content_b64' "$plan" | base64 -d) || return 64
      algorithm=${content##* = }
      [[ $algorithm == bbr || $algorithm == cubic ]] || return 64
      printf -v expected_content '# dmitbox-local profile schema=1\nnet.ipv4.tcp_congestion_control = %s' "$algorithm"
      [[ $content == "$expected_content" ]] || return 64
      # A generic file step cannot silently install a boot-time tuning profile
      # without the corresponding explicitly measured, named runtime change.
      jq -e --arg algorithm "$algorithm" '. as $p|any(.steps[]; (.adapter // $p.adapter)=="tcp" and .key=="net.ipv4.tcp_congestion_control" and .after.value==$algorithm and .measurement.requested==true)' "$plan" >/dev/null || return 64;;
    recovery-probe) [[ $path == "$DMB_STATE_DIR/recovery-probe" ]] || return 64;;
    *) dmb_error 'Generic filesystem mutation requires a supported purpose and its exact owned path'; return 64;;
  esac
}
dmb_fs_capture() { local path; dmb_fs_plan_validate "$1" "$2" || return; path=$(jq -er --argjson i "$2" '.steps[$i].path' "$1") || return; dmb_file_state "$path"; }
dmb_fs_snapshot() {
  dmb_fs_plan_validate "$1" "$2" || return
  local path id; path=$(jq -er --argjson i "$2" '.steps[$i].path' "$1") || return
  id=$(jq -er --argjson i "$2" '.steps[$i].id' "$1") || return
  # A repeated file resource needs an adapter capable of synthesizing the
  # intermediate snapshot. The generic adapter deliberately has no such claim.
  jq -e --argjson i "$2" --arg path "$path" 'all(.steps[:$i][]; .path!=$path)' "$1" >/dev/null || return 69
  dmb_file_capture "$path" "$3/snapshots" "$id"
}
dmb_fs_apply() {
  local plan=$1 i=$2 path mode before expected tmp parent hash expected_state
  dmb_fs_plan_validate "$plan" "$i" || return
  path=$(jq -er --argjson i "$i" '.steps[$i].path' "$plan") || return
  mode=$(jq -er --argjson i "$i" '.steps[$i].after.mode // "600"' "$plan") || return
  before=$(jq -cS --argjson i "$i" '.steps[$i].before' "$plan") || return
  [[ $(dmb_file_state "$path" | jq -cS .) == "$before" ]] || return 76
  expected=$(jq -r --argjson i "$i" '.steps[$i].after.exists' "$plan") || return
  expected_state=$(jq -cS --argjson i "$i" '.steps[$i].after' "$plan") || return
  if [[ $expected == false ]]; then rm -- "$path" || return; sync -f "${path%/*}"; return; fi
  hash=$(jq -er --argjson i "$i" '.steps[$i].after.sha256' "$plan") || return
  if [[ $(jq -r .exists <<< "$before") == true ]]; then jq -r --argjson i "$i" '.steps[$i].content_b64' "$plan" | base64 -d | dmb_atomic_write "$path" "$mode" "$hash" "$expected_state"
  else
    parent=${path%/*}; dmb_trusted_path "$parent" || return
    tmp=$(mktemp "$parent/.dmitbox-create.XXXXXXXXXXXX") || return
    if ! jq -r --argjson i "$i" '.steps[$i].content_b64' "$plan" | base64 -d > "$tmp"; then rm -f -- "$tmp"; return 74; fi
    [[ $(sha256sum -- "$tmp" | cut -d' ' -f1) == "$hash" ]] || { rm -f -- "$tmp"; return 74; }
    chmod "$mode" "$tmp" || { rm -f -- "$tmp"; return 74; }
    # A default directory ACL or security label may differ from a planner's
    # predicted creation metadata. Refuse before linking any live resource.
    if [[ $(dmb_file_state "$tmp" | jq -cS .) != "$expected_state" ]]; then rm -f -- "$tmp"; return 78; fi
    sync -f "$tmp" && ln -T -- "$tmp" "$path"; local rc=$?
    rm -f -- "$tmp"; ((rc==0)) || return "$rc"
    sync -f "$parent"
  fi
}
dmb_fs_verify() { local actual expected; actual=$(dmb_fs_capture "$1" "$2" | jq -cS .) || return; expected=$(jq -cS --argjson i "$2" '.steps[$i].after' "$1") || return; [[ $actual == "$expected" ]]; }
dmb_fs_verify_restore() { local actual expected; actual=$(dmb_fs_capture "$1" "$2" | jq -cS .) || return; expected=$(jq -cS --argjson i "$2" '.steps[$i].before' "$1") || return; [[ $actual == "$expected" ]]; }
dmb_fs_restore() {
  local path id expected
  dmb_fs_plan_validate "$1" "$2" || return
  path=$(jq -er --argjson i "$2" '.steps[$i].path' "$1") || return
  id=$(jq -er --argjson i "$2" '.steps[$i].id' "$1") || return
  expected=$(jq -cS --argjson i "$2" '.steps[$i].after' "$1" | sha256sum | cut -d' ' -f1) || return
  dmb_file_restore "$path" "$3/snapshots" "$id" "$expected"
}
