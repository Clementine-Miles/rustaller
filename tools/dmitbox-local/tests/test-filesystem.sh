#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd -- "$(dirname -- "$0")/.." && pwd -P)
# shellcheck source=../src/core.sh
source "$ROOT/src/core.sh"
# shellcheck source=../src/filesystem.sh
source "$ROOT/src/filesystem.sh"
if ! command -v getfattr >/dev/null; then
  for tools in "$ROOT/.cache/dev-tools/root/usr/bin" "$ROOT/.cache/vm-validation/tools/usr/bin"; do
    if [[ -x $tools/getfattr ]]; then PATH=$tools:$PATH; break; fi
  done
fi
command -v getfattr >/dev/null || { printf 'SKIP native attr tools unavailable\n'; exit 77; }
DMB_FIXTURE_ROOT=$(mktemp -d); trap 'rm -rf -- "$DMB_FIXTURE_ROOT"' EXIT
export DMB_STATE_DIR=$DMB_FIXTURE_ROOT/state
export DMB_RUNTIME_DIR=$DMB_FIXTURE_ROOT/run
tmp=$DMB_FIXTURE_ROOT
# Test the production trust boundary before restricting it to a private fixture.
! dmb_trusted_path /tmp
ln -s /etc/hosts "$tmp/symlink"
! dmb_trusted_path "$tmp/symlink"
# Unit fixture override, never assembled into the production artifact.
dmb_trusted_path() { [[ $1 == "$DMB_FIXTURE_ROOT" || $1 == "$DMB_FIXTURE_ROOT/"* ]] && [[ ! -L $1 ]]; }
mkdir "$tmp/snapshots"
printf 'before\n' > "$tmp/resource"
chmod 640 "$tmp/resource"
setfacl -m u:65534:r-- "$tmp/resource"
setfattr -n user.dmitbox-test -v 'metadata preserved' "$tmp/resource"
before=$(dmb_file_state "$tmp/resource" | jq -cS .)
dmb_file_capture "$tmp/resource" "$tmp/snapshots" original
printf 'after\n' | dmb_atomic_write "$tmp/resource" 600
after=$(dmb_file_state "$tmp/resource" | jq -cS .)
[[ $(jq -cS 'del(.sha256)' <<< "$before") == "$(jq -cS 'del(.sha256)' <<< "$after")" ]]
expected=$(printf '%s\n' "$after" | sha256sum | cut -d' ' -f1)
dmb_file_restore "$tmp/resource" "$tmp/snapshots" original "$expected"
[[ $(dmb_file_state "$tmp/resource" | jq -cS .) == "$before" ]]
dmb_file_restore "$tmp/resource" "$tmp/snapshots" original "$expected"
printf 'external admin\n' > "$tmp/resource"
! dmb_file_restore "$tmp/resource" "$tmp/snapshots" original "$expected"
[[ $(cat "$tmp/resource") == 'external admin' ]]
printf 'before\n' > "$tmp/resource"
! dmb_file_capture "$tmp/resource" "$tmp/snapshots" original
# A failing producer must not publish a partial internal state file.
printf 'stable\n' | dmb_atomic_write "$tmp/state.json" 600
saved=$(sha256sum "$tmp/state.json")
dmb_atomic_write "$tmp/state.json" BAD <<< 'broken' && exit 1
[[ $(sha256sum "$tmp/state.json") == "$saved" ]]
! dmb_atomic_write "$tmp/state.json" 600 "$(printf 'expected\n' | sha256sum | cut -d' ' -f1)" <<< 'truncated'
[[ $(sha256sum "$tmp/state.json") == "$saved" ]]
ln "$tmp/resource" "$tmp/alias"
! dmb_file_state "$tmp/resource" >/dev/null
printf 'PASS real ACL/xattr/content restoration, conflicts, hardlinks, idempotence\n'
