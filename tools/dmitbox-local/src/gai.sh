# glibc address selection is local policy, not a connectivity test. Preserve all
# unrelated entries and refuse competing active precedence rules.
dmb_gai_plan() {
  local action=${1-} path=$DMB_ETC_ROOT/gai.conf before content encoded hash after id source original
  case $action in v4|default|restore) ;; *) return 64;; esac
  dmb_need jq getfacl getfattr base64 || return
  before=$(dmb_file_state "$path" | jq -cS .) || return
  if [[ $action == restore ]]; then
    local candidate found=''
    for candidate in "$DMB_STATE_DIR"/transactions/tx-*/plan.json; do
      [[ -f $candidate && ! -L $candidate ]] || continue
      dmb_trusted_path "$candidate" || return
      [[ $(jq -r .state "${candidate%/*}/state.json") == COMMITTED ]] || continue
      if jq -e --arg path "$path" 'any(.steps[];.adapter=="fs" and .path==$path and .purpose=="gai")' "$candidate" >/dev/null; then
        source=$(jq -cS --arg path "$path" '.steps[]|select(.adapter=="fs" and .path==$path and .purpose=="gai")' "$candidate") || return
        [[ $(jq -cS .after <<< "$source") == "$before" ]] || continue
        [[ -z $found ]] || { dmb_error 'Multiple matching gai restore points; select an explicit audited inverse plan'; return 76; }
        found=$source; id=$(jq -r .id <<< "$source")
        original=${candidate%/*}/snapshots/$id.raw
        if [[ $(jq -r .before.exists <<< "$source") == true ]]; then
          [[ -f $original ]] || return 78
          [[ $(dmb_file_state "$original" | jq -cS .) == "$(jq -cS .before <<< "$source")" ]] || return 76
          encoded=$(base64 -w0 "$original") || return
        else encoded=''; fi
      fi
    done
    [[ -n $found ]] || { dmb_error 'No unique verified committed gai restore point'; return 69; }
    after=$(jq -cS .before <<< "$found") || return
  else
    content=''; [[ ! -e $path ]] || content=$(cat -- "$path") || return
    [[ $(printf '%s\n' "$content" | grep -c '^# BEGIN dmitbox-local gai$') -le 1 ]] || return 76
    [[ $(printf '%s\n' "$content" | grep -c '^# END dmitbox-local gai$') -le 1 ]] || return 76
    if [[ $content == *'# BEGIN dmitbox-local gai'* || $content == *'# END dmitbox-local gai'* ]]; then
      printf '%s\n' "$content" | awk '/^# BEGIN dmitbox-local gai$/{if(open)exit 1;open=1;next} /^# END dmitbox-local gai$/{if(!open)exit 1;open=0;next} END{if(open)exit 1}' || return 76
    fi
    content=$(printf '%s\n' "$content" | sed '/^# BEGIN dmitbox-local gai$/,/^# END dmitbox-local gai$/d') || return
    if printf '%s\n' "$content" | grep -Eq '^[[:space:]]*precedence[[:space:]]'; then dmb_error 'Existing precedence policy has independent ownership'; return 76; fi
    if [[ $action == v4 ]]; then
      content+=$'\n# BEGIN dmitbox-local gai\nprecedence ::1/128 50\nprecedence ::/0 40\nprecedence 2002::/16 30\nprecedence ::/96 20\nprecedence ::ffff:0:0/96 100\n# END dmitbox-local gai'
    fi
    encoded=$(printf '%s\n' "$content" | base64 -w0)
    hash=$(printf '%s' "$encoded" | base64 -d | sha256sum | cut -d' ' -f1) || return
    if [[ $(jq -r .exists <<< "$before") == true ]]; then after=$(jq -cS --arg hash "$hash" '.sha256=$hash' <<< "$before") || return
    else
      local acl
      acl=$(printf 'user::rw-\ngroup::r--\nother::r--\n\n' | base64 -w0)
      after=$(jq -cn --arg hash "$hash" --arg acl "$acl" '{exists:true,type:"file",sha256:$hash,mode:"644",uid:0,gid:0,acl_b64:$acl,xattrs_b64:""}') || return
    fi
  fi
  jq -cn --arg path "$path" --arg content "$encoded" --argjson before "$before" --argjson after "$after" '{schema:1,adapter:"fs",resources:[("file:"+$path)],allow_console_confirmation:true,steps:[{id:"gai",adapter:"fs",resource:("file:"+$path),path:$path,purpose:"gai",before:$before,after:$after,content_b64:$content}],verification:{external_access:"requires_fresh_transport_or_enrolled_console"}}'
}
dmb_gai() (
  dmb_require_root || return
  local tmp; tmp=$(dmb_private_tmp) || return
  trap 'rm -rf -- "$tmp"' EXIT
  dmb_gai_plan "$1" > "$tmp/plan.json" || return
  dmb_apply_file "$tmp/plan.json"
)
