# Shared runtime. Production paths are assigned by the generated entry point;
# inherited environment variables never select privileged state or executables.
dmb_error() { printf 'dmitbox: %s\n' "$*" >&2; }
dmb_require_root() { (( EUID == 0 )) || { dmb_error 'Root is required for this operation'; return 77; }; }
dmb_need() { local c; for c in "$@"; do command -v "$c" >/dev/null 2>&1 || { dmb_error "Missing dependency: $c (install explicitly)"; return 69; }; done; }
dmb_state_root() { printf '%s\n' "$DMB_STATE_DIR"; }
dmb_config_root() { printf '%s\n' "$DMB_CONFIG_DIR"; }
dmb_lock_root() { printf '%s\n' "$DMB_LOCK_DIR"; }
dmb_artifact_path() { [[ ${DMB_ARTIFACT:-} == /* && -f $DMB_ARTIFACT ]] || return 1; printf '%s\n' "$DMB_ARTIFACT"; }
dmb_valid_name() { [[ ${1-} =~ ^[A-Za-z0-9][A-Za-z0-9_.:-]{0,127}$ ]]; }
dmb_valid_user() { [[ ${1-} =~ ^[a-zA-Z_][a-zA-Z0-9_.-]{0,31}\$?$ ]]; }
dmb_valid_unit() { [[ ${1-} =~ ^[A-Za-z0-9_@.:-]+\.(service|socket|timer)$ ]]; }
dmb_run() (
  local seconds=${1-} fd; shift || return 64
  [[ $seconds =~ ^[1-9][0-9]{0,3}$ && $# -gt 0 ]] || return 64
  # Close all registered locks in descendants; never evaluate state as code.
  if declare -F dmb_tx_close_locks >/dev/null; then dmb_tx_close_locks || return
  else for fd in "${DMB_LOCK_FDS[@]-}"; do [[ $fd =~ ^[0-9]+$ ]] && exec {fd}>&-; done; fi
  timeout --signal=TERM --kill-after=3 "$seconds" "$@"
)
dmb_timeout() { dmb_run "$@"; }
dmb_trusted_path() {
  local path=${1-} missing=${2-} part current='' uid mode oct
  [[ $path == /* && $path != *$'\n'* && $path != *'/../'* && $path != */.. && $path != *'/./'* ]] || return 78
  local -a parts; IFS=/ read -r -a parts <<< "$path"
  for part in "${parts[@]}"; do
    [[ -n $part ]] || continue
    current+="/$part"
    [[ ! -L $current ]] || { dmb_error "Symlink in protected path: $current"; return 78; }
    if [[ ! -e $current ]]; then [[ $missing == allow_missing ]] && return 0; return 78; fi
    uid=$(stat -c %u -- "$current") || return 78
    mode=$(stat -c %a -- "$current") || return 78
    [[ $uid == 0 && $mode =~ ^[0-7]{3,4}$ ]] || return 78
    oct=$((8#$mode))
    (( (oct & 0022) == 0 )) || { dmb_error "Writable protected ancestor: $current"; return 78; }
  done
}
dmb_private_tmp() {
  local parent=${1:-$DMB_RUNTIME_DIR/tmp}
  dmb_require_root || return
  dmb_trusted_path "$parent" allow_missing || return
  mkdir -p -- "$parent" && chmod 0700 -- "$parent" || return
  dmb_trusted_path "$parent" || return
  mktemp -d "$parent/work.XXXXXXXXXXXX"
}
dmb_scratch_tmp() {
  # Native parsers sometimes require a filename for a hypothetical config.
  # This private, disposable workspace creates no durable program state.
  dmb_require_root || return
  dmb_trusted_path /run || return
  mktemp -d /run/.dmitbox-plan.XXXXXXXXXXXX
}
dmb_atomic_write() (
  local path=$1 mode=$2 expected=${3:-} expected_state=${4:-} parent tmp actual
  [[ $mode =~ ^0?[0-7]{3,4}$ ]] || return 64
  [[ -z $expected || $expected =~ ^[a-f0-9]{64}$ ]] || return 64
  parent=${path%/*}; [[ -n $parent ]] || parent=/
  dmb_trusted_path "$parent" || return
  dmb_trusted_path "$path" allow_missing || return
  [[ ! -e $path || -f $path ]] || return 78
  tmp=$(mktemp "$parent/.dmitbox-write.XXXXXXXXXXXX") || return
  trap 'rm -f -- "$tmp"' EXIT
  chmod "$mode" "$tmp" || return
  # An existing file's metadata is retained; chmod only selects creation mode.
  if [[ -e $path ]]; then cp --attributes-only --preserve=all -- "$path" "$tmp" || return; fi
  cat > "$tmp" || return
  if [[ -n $expected ]]; then
    actual=$(sha256sum -- "$tmp") || return
    [[ ${actual%% *} == "$expected" ]] || return 74
  fi
  # Internal durable records must be complete even when a producer failed
  # before closing its pipe. A zero-length/truncated record is never published.
  if [[ $path == "$DMB_STATE_DIR/"*.json || $path == "$DMB_RUNTIME_DIR/"*.json ]]; then
    jq -e 'true' "$tmp" >/dev/null || return 74
  fi
  if [[ -e $path ]]; then cp --attributes-only --preserve=all -- "$path" "$tmp" || return; fi
  if [[ -n $expected_state ]]; then
    declare -F dmb_file_state >/dev/null || return 69
    actual=$(dmb_file_state "$tmp" | jq -ceS .) || return
    [[ $actual == "$(jq -ceS . <<< "$expected_state")" ]] || return 78
  fi
  sync -f "$tmp" || return
  mv -T -- "$tmp" "$path" || return
  sync -f "$parent" || return
)
dmb_json_hash() { jq -cS . | sha256sum | cut -d' ' -f1; }
dmb_audit() {
  local event=$1 detail=${2:-'{}'} record dir
  dmb_valid_name "$event" || return 64
  dir=$DMB_STATE_DIR/audit
  dmb_trusted_path "$dir" allow_missing || return
  mkdir -p -- "$dir" && chmod 0700 -- "$dir" || return
  record=$(mktemp "$dir/event.XXXXXXXXXXXX.json") || return
  jq -cn --arg event "$event" --arg boot "$(cat /proc/sys/kernel/random/boot_id)" --arg uptime "$(cut -d' ' -f1 /proc/uptime)" --argjson detail "$detail" '{schema:1,event:$event,boot_id:$boot,uptime:$uptime,detail:$detail}' | dmb_atomic_write "$record" 0600
}
