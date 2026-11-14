# Independent rollback supervision. This file is concatenated into the release.
# Filesystem failure, uninterruptible kernel I/O, or power loss can prevent any
# shell supervisor from guaranteeing rollback; preserve evidence and fail closed.
dmb_recovery_boot_id() { cat /proc/sys/kernel/random/boot_id; }
dmb_recovery_uptime() { local up rest; read -r up rest < /proc/uptime; printf '%s\n' "${up%%.*}"; }
dmb_recovery_systemd_dir() { printf '/etc/systemd/system\n'; }
dmb_recovery_openrc_dir() { printf '/etc/init.d\n'; }
dmb_recovery_install_dir() { printf '/usr/local/libexec/dmitbox-local\n'; }
dmb_recovery_error() { printf 'dmitbox recovery: %s\n' "$*" >&2; }
# Recovery infrastructure is itself an owned resource. Prefixes alone do not
# authorize overwriting a unit or scanner installed by an administrator.
# INTENT records allow interrupted publication to be checked on the next run.
dmb_recovery_owned_write() (
  local path=$1 mode=$2 expected=${3:-} registry lock_fd tmp desired current=absent record='{}' previous next
  [[ -z $expected || $expected =~ ^[a-f0-9]{64}$ ]] || return 2
  registry=$(dmb_state_root)/recovery-files.json
  dmb_trusted_path "$path" allow_missing || return 1
  dmb_trusted_path "$registry" allow_missing || return 1
  dmb_tx_lock_path "$(dmb_lock_root)/recovery-files.lock" || return 1
  exec {lock_fd}>"$(dmb_lock_root)/recovery-files.lock" || return 1
  flock -w 2 -x "$lock_fd" || return 75
  local -a DMB_LOCK_FDS=("${DMB_LOCK_FDS[@]-}" "$lock_fd")
  if [[ -e $registry ]]; then
    jq -e '.schema==1 and (.files|type=="object")' "$registry" >/dev/null || return 1
    record=$(jq -cS . "$registry") || return 1
  else record='{"schema":1,"files":{}}'; fi
  if [[ -e $path ]]; then
    [[ -f $path && ! -L $path ]] || return 1
    current=$(sha256sum -- "$path") || return 1; current=${current%% *}
  fi
  previous=$(jq -cr --arg path "$path" '.files[$path] // null' <<< "$record") || return 1
  if [[ $previous == null ]]; then
    [[ $current == absent ]] || { dmb_recovery_error "Unenrolled recovery file already exists: $path"; return 76; }
  else
    jq -e --arg current "$current" '.after==$current or (.phase=="INTENT" and .before==$current)' <<< "$previous" >/dev/null || { dmb_recovery_error "Recovery file changed outside its ownership record: $path"; return 76; }
  fi
  tmp=$(mktemp "$(dmb_state_root)/.recovery-publish.XXXXXXXX") || return 1
  trap 'rm -f -- "$tmp"' EXIT
  cat > "$tmp" || return 1
  desired=$(sha256sum -- "$tmp") || return 1; desired=${desired%% *}
  [[ -z $expected || $desired == "$expected" ]] || return 1
  next=$(jq -cS --arg path "$path" --arg before "$current" --arg after "$desired" '.files[$path]={phase:"INTENT",before:$before,after:$after}' <<< "$record") || return 1
  printf '%s\n' "$next" | dmb_atomic_write "$registry" 0600 || return 1
  dmb_atomic_write "$path" "$mode" "$desired" < "$tmp" || return 1
  jq -cS --arg path "$path" '.files[$path].phase="DONE"' <<< "$next" | dmb_atomic_write "$registry" 0600
)
dmb_recovery_owned_remove() (
  local path=$1 registry lock_fd current record expected
  registry=$(dmb_state_root)/recovery-files.json
  dmb_trusted_path "$path" allow_missing || return 1
  dmb_trusted_path "$registry" || return 1
  dmb_tx_lock_path "$(dmb_lock_root)/recovery-files.lock" || return 1
  exec {lock_fd}>"$(dmb_lock_root)/recovery-files.lock" || return 1
  flock -w 2 -x "$lock_fd" || return 75
  local -a DMB_LOCK_FDS=("${DMB_LOCK_FDS[@]-}" "$lock_fd")
  record=$(jq -cS 'select(.schema==1 and (.files|type=="object"))' "$registry") || return 1
  expected=$(jq -r --arg path "$path" '.files[$path].after // empty' <<< "$record") || return 1
  if [[ -z $expected ]]; then [[ ! -e $path ]] || return 76; return 0; fi
  if [[ -e $path ]]; then
    current=$(sha256sum -- "$path") || return 1; current=${current%% *}
    [[ $current == "$expected" ]] || { dmb_recovery_error "Preserving externally modified recovery file: $path"; return 76; }
    rm -- "$path" || return 1
    sync -f "${path%/*}" || return 1
  fi
  jq -cS --arg path "$path" 'del(.files[$path])' <<< "$record" | dmb_atomic_write "$registry" 0600
)
dmb_recovery_owned_check() {
  local path=$1 registry expected actual
  dmb_trusted_path "$path" allow_missing || return 1
  [[ -e $path ]] || return 0
  registry=$(dmb_state_root)/recovery-files.json
  dmb_trusted_path "$registry" || return 1
  expected=$(jq -er --arg path "$path" '.files[$path].after' "$registry") || return 1
  actual=$(sha256sum -- "$path") || return 1
  [[ ${actual%% *} == "$expected" ]]
}
dmb_recovery_backend() {
  if [[ -d /run/systemd/system ]] && command -v systemctl >/dev/null 2>&1; then printf 'systemd\n'
  elif [[ -f /run/openrc/softlevel && -x /sbin/openrc-run ]] && command -v rc-service >/dev/null 2>&1 && command -v rc-update >/dev/null 2>&1 && command -v supervise-daemon >/dev/null 2>&1; then printf 'openrc\n'
  else dmb_recovery_error 'No supported independent recovery supervisor (systemd or OpenRC)'; return 1; fi
}
dmb_recovery_id_valid() { [[ ${1:-} =~ ^tx-[a-f0-9]{32}$ ]]; }
dmb_recovery_valid_record() {
  local id=$1 tx=$2
  dmb_recovery_id_valid "$id" || return 1
  dmb_trusted_path "$tx" || return 1
  dmb_trusted_path "$tx/recovery.json" || return 1
  dmb_trusted_path "$tx/worker.sh" || return 1
  jq -e --arg id "$id" '.id==$id and .version==1 and (.nonce|test("^[a-f0-9]{32}$")) and (.worker_sha256|test("^[a-f0-9]{64}$")) and (.backend=="systemd" or .backend=="openrc")' "$tx/recovery.json" >/dev/null || return 1
  local actual expected
  actual=$(sha256sum -- "$tx/worker.sh") || return 1; actual=${actual%% *}
  expected=$(jq -r '.worker_sha256' "$tx/recovery.json") || return 1
  [[ $actual == "$expected" ]] || { dmb_recovery_error "Pinned recovery worker changed: $id"; return 1; }
}
dmb_recovery_copy_worker() {
  local id=$1 tx=$2 source tmp expected actual
  source=$(dmb_artifact_path) || return 1
  dmb_trusted_path "$source" || return 1
  [[ -f $source && ! -L $source ]] || return 1
  # Never overwrite a previously pinned worker while a transaction exists.
  if [[ -e $tx/worker.sh ]]; then return 1; fi
  tmp=$(mktemp "$tx/.worker.XXXXXXXX") || return 1
  if ! cp -- "$source" "$tmp" || ! chmod 0700 "$tmp"; then rm -f -- "$tmp"; return 1; fi
  expected=$(sha256sum -- "$source") || { rm -f -- "$tmp"; return 1; }; expected=${expected%% *}
  actual=$(sha256sum -- "$tmp") || { rm -f -- "$tmp"; return 1; }; actual=${actual%% *}
  [[ $expected == "$actual" ]] || { rm -f -- "$tmp"; return 1; }
  # atomic_write supplies the core's checked sync + rename durability barriers.
  dmb_atomic_write "$tx/worker.sh" 0700 "$expected" < "$tmp" || { rm -f -- "$tmp"; return 1; }
  rm -f -- "$tmp"
  printf '%s\n' "$expected"
}
dmb_recovery_render_watch_unit() {
  local id=$1 tx=$2 nonce=$3
  # Generated arguments contain only restricted transaction ids/nonces and the
  # trusted fixed state-root path, never user-supplied shell commands.
  cat <<EOF
[Unit]
Description=DMITBox independent recovery $id
After=local-fs.target
[Service]
Type=simple
ExecStart=/bin/bash $tx/worker.sh --internal-recover-watch $id $nonce
Restart=on-failure
RestartSec=2s
TimeoutStopSec=50s
KillMode=control-group
[Install]
WantedBy=multi-user.target
EOF
}
dmb_recovery_render_timer() {
  local id=$1 remaining=$2
  [[ $remaining =~ ^[0-9]+$ ]] && (( remaining > 0 && remaining <= 3600 )) || return 1
  cat <<EOF
[Unit]
Description=DMITBox independent monotonic deadline $id
[Timer]
OnActiveSec=${remaining}s
AccuracySec=1s
Unit=dmitbox-$id-expire.service
[Install]
WantedBy=timers.target
EOF
}
dmb_recovery_render_expire_unit() {
  local id=$1 tx=$2
  cat <<EOF
[Unit]
Description=DMITBox deadline recovery $id
After=local-fs.target
[Service]
Type=oneshot
ExecStart=/bin/bash $tx/worker.sh --internal-recover-once $id
TimeoutStartSec=180s
KillMode=control-group
EOF
}
dmb_recovery_render_openrc_watch() {
  local id=$1 tx=$2 nonce=$3
  cat <<EOF
#!/sbin/openrc-run
name="DMITBox independent recovery $id"
command="/bin/bash"
command_args="$tx/worker.sh --internal-recover-watch $id $nonce"
supervisor="supervise-daemon"
respawn_delay="2"
respawn_max="0"
pidfile="/run/dmitbox-local-$id.pid"
retry="TERM/50/KILL/5"
depend() { need localmount; }
EOF
}
dmb_recovery_install_boot_gate() (
  local backend=$1 artifact artifact_hash install units init install_fd
  dmb_tx_lock_path "$(dmb_lock_root)/recovery-install.lock" || return 1
  exec {install_fd}>"$(dmb_lock_root)/recovery-install.lock" || return 1
  flock -w 2 -x "$install_fd" || return 75
  local -a DMB_LOCK_FDS=("${DMB_LOCK_FDS[@]-}" "$install_fd")
  artifact=$(dmb_artifact_path) || return 1
  artifact_hash=$(sha256sum -- "$artifact") || return 1; artifact_hash=${artifact_hash%% *}
  install=$(dmb_recovery_install_dir) || return 1
  [[ $install =~ ^/[A-Za-z0-9_/.-]+$ ]] || return 1
  dmb_trusted_path "$install" allow_missing || return 1
  # Parent ownership is validated; the leaf is explicitly restricted.
  # shellcheck disable=SC2174
  mkdir -p -m 0700 -- "$install" || return 1
  dmb_trusted_path "$install" || return 1
  # The stable boot scanner dispatches each pending transaction's pinned worker.
  # Replacing this scanner does not replace any transaction worker or snapshot.
  dmb_recovery_owned_write "$install/recovery-boot.sh" 0700 "$artifact_hash" < "$artifact" || return 1
  if [[ $backend == systemd ]]; then
    units=$(dmb_recovery_systemd_dir) || return 1
    dmb_trusted_path "$units" || return 1
    dmb_recovery_owned_write "$units/dmitbox-recovery-boot.service" 0644 <<EOF
[Unit]
Description=Recover incomplete DMITBox changes before networking
DefaultDependencies=no
After=local-fs.target
Before=network-pre.target systemd-sysctl.service ssh.service sshd.service
Wants=network-pre.target
[Service]
Type=oneshot
ExecStart=/bin/bash $install/recovery-boot.sh --recover-all
TimeoutStartSec=180s
[Install]
WantedBy=sysinit.target
EOF
    dmb_run 15 systemctl daemon-reload || return 1
    local drops
    drops=$(dmb_run 5 systemctl show --value --property=DropInPaths dmitbox-recovery-boot.service) || return 1
    [[ -z ${drops//[[:space:]]/} ]] || { dmb_recovery_error 'Boot recovery unit has external drop-ins'; return 76; }
    dmb_run 15 systemctl enable dmitbox-recovery-boot.service || return 1
    dmb_run 10 systemctl is-enabled --quiet dmitbox-recovery-boot.service || return 1
  elif [[ $backend == openrc ]]; then
    init=$(dmb_recovery_openrc_dir) || return 1
    dmb_trusted_path "$init" || return 1
    [[ ! -e /etc/conf.d/dmitbox-recovery-boot && ! -L /etc/conf.d/dmitbox-recovery-boot ]] || { dmb_recovery_error 'Boot recovery has external OpenRC configuration'; return 76; }
    dmb_recovery_owned_write "$init/dmitbox-recovery-boot" 0755 <<EOF
#!/sbin/openrc-run
name="Recover incomplete DMITBox changes before networking"
command="/bin/bash"
command_args="$install/recovery-boot.sh --recover-all"
depend() { need localmount; before networking net sshd; }
EOF
    dmb_run 15 rc-update add dmitbox-recovery-boot boot || return 1
    # Exact service presence is checked; successful rc-update alone is insufficient.
    # Consume the complete producer output: grep -q can close the pipe early
    # and turn a successful OpenRC query into SIGPIPE with pipefail enabled.
    local boot_services
    boot_services=$(dmb_run 10 rc-update show boot) || return 1
    grep -E '(^|[[:space:]])dmitbox-recovery-boot([[:space:]]|$)' <<< "$boot_services" >/dev/null || return 1
  else return 1; fi
)
dmb_recovery_arm() {
  local id=$1 tx=${2:-} backend hash nonce unitdir init ready_until now deadline remaining
  dmb_require_root || return 1
  dmb_need jq sha256sum bash mktemp cp chmod || return 1
  [[ -n $tx ]] || tx=$(dmb_tx_dir "$id") || return 1
  dmb_recovery_id_valid "$id" || return 1
  dmb_trusted_path "$tx" || return 1
  [[ $tx =~ ^/[A-Za-z0-9_/.-]+$ ]] || return 1
  backend=$(dmb_recovery_backend) || return 1
  hash=$(dmb_recovery_copy_worker "$id" "$tx") || return 1
  nonce=$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n') || return 1
  [[ $nonce =~ ^[a-f0-9]{32}$ ]] || return 1
  jq -n --arg id "$id" --arg hash "$hash" --arg nonce "$nonce" --arg backend "$backend" '{version:1,id:$id,worker_sha256:$hash,nonce:$nonce,backend:$backend}' | dmb_atomic_write "$tx/recovery.json" 0600 || return 1
  dmb_recovery_install_boot_gate "$backend" || return 1
  if [[ $backend == systemd ]]; then
    unitdir=$(dmb_recovery_systemd_dir) || return 1
    dmb_recovery_render_watch_unit "$id" "$tx" "$nonce" | dmb_recovery_owned_write "$unitdir/dmitbox-$id.service" 0644 || return 1
    now=$(dmb_recovery_uptime) || return 1
    deadline=$(jq -er '.deadline_uptime' "$tx/state.json") || return 1
    remaining=$((deadline-now)); (( remaining > 0 )) || return 1
    dmb_recovery_render_timer "$id" "$remaining" | dmb_recovery_owned_write "$unitdir/dmitbox-$id.timer" 0644 || return 1
    dmb_recovery_render_expire_unit "$id" "$tx" | dmb_recovery_owned_write "$unitdir/dmitbox-$id-expire.service" 0644 || return 1
    dmb_run 15 systemctl daemon-reload || return 1
    dmb_run 15 systemctl enable --now "dmitbox-$id.timer" "dmitbox-$id.service" || return 1
  else
    init=$(dmb_recovery_openrc_dir) || return 1
    [[ ! -e /etc/conf.d/dmitbox-$id && ! -L /etc/conf.d/dmitbox-$id ]] || return 76
    dmb_recovery_render_openrc_watch "$id" "$tx" "$nonce" | dmb_recovery_owned_write "$init/dmitbox-$id" 0755 || return 1
    dmb_run 15 rc-update add "dmitbox-$id" default || return 1
    dmb_run 15 rc-service "dmitbox-$id" start || return 1
  fi
  now=$(dmb_recovery_uptime) || return 1; ready_until=$((now + 12))
  while :; do
    if dmb_recovery_ready "$id" "$tx"; then return 0; fi
    now=$(dmb_recovery_uptime) || return 1
    (( now < ready_until )) || { dmb_recovery_error "Recovery worker did not become ready: $id"; return 1; }
    sleep 1
  done
}
dmb_recovery_ready() {
  local id=$1 tx=${2:-} pid nonce hash boot backend mainpid drops
  [[ -n $tx ]] || tx=$(dmb_tx_dir "$id") || return 1
  dmb_recovery_valid_record "$id" "$tx" || return 1
  dmb_trusted_path "$tx/ready.json" || return 1
  nonce=$(jq -r '.nonce' "$tx/recovery.json") || return 1
  hash=$(jq -r '.worker_sha256' "$tx/recovery.json") || return 1
  boot=$(dmb_recovery_boot_id) || return 1
  jq -e --arg id "$id" --arg nonce "$nonce" --arg hash "$hash" --arg boot "$boot" '.id==$id and .nonce==$nonce and .worker_sha256==$hash and .boot_id==$boot and (.pid|type=="number") and .pid>1' "$tx/ready.json" >/dev/null 2>&1 || return 1
  pid=$(jq -r '.pid' "$tx/ready.json") || return 1
  kill -0 "$pid" 2>/dev/null || return 1
  # Prove the process belongs to the expected transaction, not a reused PID.
  [[ -r /proc/$pid/cmdline ]] || return 1
  # BusyBox grep does not consistently implement -z. Bash reads the kernel's
  # NUL-separated arguments without relying on a distribution-specific grep.
  local argument nonce_found=0
  while IFS= read -r -d '' argument; do
    [[ $argument != "$nonce" ]] || nonce_found=1
  done < "/proc/$pid/cmdline"
  ((nonce_found==1)) || return 1
  backend=$(jq -r '.backend' "$tx/recovery.json") || return 1
  if [[ $backend == systemd ]]; then
    dmb_run 5 systemctl is-active --quiet "dmitbox-$id.service" || return 1
    dmb_run 5 systemctl is-active --quiet "dmitbox-$id.timer" || return 1
    mainpid=$(dmb_run 5 systemctl show --value --property=MainPID "dmitbox-$id.service") || return 1
    [[ $mainpid == "$pid" ]] || return 1
    drops=$(dmb_run 5 systemctl show --value --property=DropInPaths "dmitbox-$id.service" "dmitbox-$id.timer" "dmitbox-$id-expire.service") || return 1
    [[ -z ${drops//[[:space:]]/} ]] || return 1
    dmb_run 5 systemctl show --value --property=Triggers "dmitbox-$id.timer" | grep -Fxq "dmitbox-$id-expire.service" || return 1
  else dmb_run 5 rc-service "dmitbox-$id" status; fi
}
dmb_recovery_attempt_rollback() (
  local id=$1 status=0 start now tx recovery_fd op
  tx=$(dmb_tx_dir "$id") || return 1
  dmb_tx_lock_path "$(dmb_lock_root)/recovery-$id.lock" || return 1
  exec {recovery_fd}>"$(dmb_lock_root)/recovery-$id.lock" || return 1
  flock -w 1 -x "$recovery_fd" || { exec {recovery_fd}>&-; return 75; }
  local -a DMB_LOCK_FDS=("${DMB_LOCK_FDS[@]-}" "$recovery_fd")
  # Cancellation precedes the engine lock: the actor must release its lock
  # after a terminated child exits. Never cancel an active inverse worker.
  if [[ -f $tx/active.json ]]; then
    op=$(jq -er '.operation' "$tx/active.json") || return 1
    case $op in apply|verify) dmb_tx_active_cancel "$tx" || return 1;; esac
  fi
  start=$(dmb_recovery_uptime) || return 1
  while :; do
    status=0; dmb_tx_rollback "$id" || status=$?
    if (( status == 0 )); then exec {recovery_fd}>&-; return 0; fi
    # Engine exit 75 means a bounded lock was busy. All other failures are
    # persistent engine outcomes, not permission to retry blindly forever.
    if (( status != 75 )); then exec {recovery_fd}>&-; return "$status"; fi
    now=$(dmb_recovery_uptime) || return 1
    (( now - start < 45 )) || { dmb_recovery_error "Recovery lock remained unavailable: $id"; exec {recovery_fd}>&-; return 1; }
    sleep 1
  done
)
dmb_recovery_watch_signal() {
  local id=$1 tx=$2 state
  state=$(jq -r '.state' "$tx/state.json" 2>/dev/null) || return 1
  case $state in COMMITTED|ROLLED_BACK) return 0 ;; esac
  dmb_recovery_attempt_rollback "$id"
}
dmb_recovery_watch() {
  local id=${1:-} nonce=${2:-} tx hash boot state deadline originboot now prepared_until actor startticks observed watcher_pid=$BASHPID
  dmb_require_root || return 1
  tx=$(dmb_tx_dir "$id") || return 1
  dmb_recovery_valid_record "$id" "$tx" || return 1
  [[ $nonce == "$(jq -r '.nonce' "$tx/recovery.json")" ]] || return 1
  dmb_trusted_path "$tx/state.json" || return 1
  dmb_trusted_path "$tx/plan.json" || return 1
  jq -e --arg id "$id" '.id==$id and (.deadline_uptime|type=="number") and (.boot_id|type=="string")' "$tx/state.json" >/dev/null || return 1
  jq -e '.schema==1 and (.steps|type=="array")' "$tx/plan.json" >/dev/null || return 1
  hash=$(jq -r '.worker_sha256' "$tx/recovery.json") || return 1
  boot=$(dmb_recovery_boot_id) || return 1
  now=$(dmb_recovery_uptime) || return 1; prepared_until=$((now + 15))
  trap 'dmb_recovery_watch_signal "$id" "$tx"; exit $?' HUP INT TERM
  jq -n --arg id "$id" --arg nonce "$nonce" --arg hash "$hash" --arg boot "$boot" --argjson pid "$watcher_pid" '{id:$id,nonce:$nonce,worker_sha256:$hash,boot_id:$boot,pid:$pid}' | dmb_atomic_write "$tx/ready.json" 0600 || return 1
  while :; do
    dmb_recovery_valid_record "$id" "$tx" || return 1
    state=$(jq -r '.state' "$tx/state.json") || return 1
    case $state in
      COMMITTED|ROLLED_BACK) trap - HUP INT TERM; return 0 ;;
      CONFLICT|ROLLBACK_FAILED) trap - HUP INT TERM; return 1 ;;
      PREPARED|ARMED|APPLIED|VERIFIED) ;;
      *) dmb_recovery_error "Invalid transaction state: $id"; return 1 ;;
    esac
    originboot=$(jq -r '.boot_id' "$tx/state.json") || return 1
    deadline=$(jq -r '.deadline_uptime' "$tx/state.json") || return 1
    [[ $deadline =~ ^[0-9]+$ ]] || return 1
    now=$(dmb_recovery_uptime) || return 1
    if [[ $originboot != "$boot" ]] || (( now >= deadline )) || { [[ $state == PREPARED ]] && (( now >= prepared_until )); }; then
      trap - HUP INT TERM
      dmb_recovery_attempt_rollback "$id"; return $?
    fi
    actor=$(jq -r '.actor_pid // empty' "$tx/state.json") || return 1
    if [[ $state != VERIFIED && -n $actor ]]; then
      [[ $actor =~ ^[0-9]+$ ]] || return 1
      startticks=$(jq -r '.actor_start_ticks // empty' "$tx/state.json") || return 1
      observed=''
      if [[ -r /proc/$actor/stat ]]; then read -r _ observed < <(dmb_tx_proc_fields "$actor") || return 1; fi
      if ! kill -0 "$actor" 2>/dev/null || [[ -n $startticks && $observed != "$startticks" ]]; then
        trap - HUP INT TERM
        dmb_recovery_attempt_rollback "$id"; return $?
      fi
    fi
    sleep 1
  done
}
dmb_recovery_disarm() {
  local id=$1 tx backend state units init
  dmb_require_root || return 1
  tx=$(dmb_tx_dir "$id") || return 1
  dmb_recovery_valid_record "$id" "$tx" || return 1
  state=$(jq -r '.state' "$tx/state.json") || return 1
  [[ $state == COMMITTED || $state == ROLLED_BACK ]] || { dmb_recovery_error 'Unresolved recovery cannot be disarmed'; return 1; }
  backend=$(jq -r '.backend' "$tx/recovery.json") || return 1
  # Worker naturally exits on terminal state. Do not synchronously stop its own
  # service from inside rollback: systemctl/rc-service would kill that worker.
  if [[ $backend == systemd ]]; then
    units=$(dmb_recovery_systemd_dir) || return 1
    dmb_recovery_owned_check "$units/dmitbox-$id.service" || return 76
    dmb_recovery_owned_check "$units/dmitbox-$id.timer" || return 76
    dmb_recovery_owned_check "$units/dmitbox-$id-expire.service" || return 76
    if [[ -e $units/dmitbox-$id.timer ]]; then dmb_run 10 systemctl disable --now "dmitbox-$id.timer" || return 1; fi
    if [[ -e $units/dmitbox-$id.service ]]; then dmb_run 10 systemctl disable "dmitbox-$id.service" || return 1; fi
    dmb_recovery_owned_remove "$units/dmitbox-$id.service" || return 1
    dmb_recovery_owned_remove "$units/dmitbox-$id.timer" || return 1
    dmb_recovery_owned_remove "$units/dmitbox-$id-expire.service" || return 1
    sync -f "$units" || return 1
    dmb_run 10 systemctl daemon-reload || return 1
  else
    init=$(dmb_recovery_openrc_dir) || return 1
    dmb_recovery_owned_check "$init/dmitbox-$id" || return 76
    if [[ -e $init/dmitbox-$id ]]; then dmb_run 10 rc-update del "dmitbox-$id" default || return 1; fi
    dmb_recovery_owned_remove "$init/dmitbox-$id" || return 1
    sync -f "$init" || return 1
  fi
}
dmb_recovery_boot() (
  local root tx id state status=0 rc global_fd
  local -a intents
  dmb_require_root || return 1
  root=$(dmb_state_root) || return 1
  [[ -d $root/transactions ]] || return 0
  dmb_trusted_path "$root/transactions" || return 1
  dmb_tx_init || return 1
  dmb_tx_lock_path "$(dmb_lock_root)/global.lock" || return 1
  exec {global_fd}>"$(dmb_lock_root)/global.lock" || return 1
  flock -w 2 -x "$global_fd" || return 75
  dmb_tx_reap_preparations || return 1
  exec {global_fd}>&-
  for tx in "$root"/transactions/tx-*; do
    [[ -d $tx ]] || continue
    id=${tx##*/}; dmb_recovery_id_valid "$id" || { status=1; continue; }
    if [[ ! -f $tx/state.json && -f $tx/preparation.json ]]; then
      # Reaper validated the dead constructor and released only its metadata.
      continue
    fi
    dmb_trusted_path "$tx/state.json" || { status=1; continue; }
    state=$(jq -r '.state' "$tx/state.json") || { status=1; continue; }
    case $state in
      COMMITTED|ROLLED_BACK)
        # Interrupted unit/reservation cleanup must remain retryable.
        dmb_tx_rollback "$id" || status=1
        continue;;
      PREPARED)
        intents=("$tx"/journal/*-INTENT.json)
        if [[ ! -e ${intents[0]} && ! -e $tx/active.json ]]; then
          # No mutation is permitted before ARMED + INTENT. Missing worker is
          # expected when a reboot happened between prepare and arm.
          dmb_tx_rollback "$id" || status=1
          continue
        fi;;
    esac
    # Never source a recorded transaction's code or data in the scanner process.
    dmb_recovery_valid_record "$id" "$tx" || { status=1; continue; }
    rc=0; dmb_run 60 /bin/bash "$tx/worker.sh" --internal-recover-once "$id" || rc=$?
    (( rc == 0 )) || status=1
  done
  return "$status"
)
