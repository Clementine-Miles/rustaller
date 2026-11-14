#!/usr/bin/env bash
# Private file fixtures; no host configuration or services are changed.
set -euo pipefail
ROOT=$(cd -- "$(dirname -- "$0")/.." && pwd -P)
# shellcheck source=../src/core.sh
source "$ROOT/src/core.sh"
# shellcheck source=../src/filesystem.sh
source "$ROOT/src/filesystem.sh"
# shellcheck source=../src/gai.sh
source "$ROOT/src/gai.sh"
if ! command -v getfattr >/dev/null; then
  for tools in "$ROOT/.cache/dev-tools/root/usr/bin" "$ROOT/.cache/vm-validation/tools/usr/bin"; do
    if [[ -x $tools/getfattr ]]; then PATH=$tools:$PATH; break; fi
  done
fi
for tool in getfacl getfattr setfacl setfattr; do
  command -v "$tool" >/dev/null || { printf 'SKIP missing metadata dependency %s\n' "$tool"; exit 77; }
done
export DMB_GAI_FIXTURE_ROOT
DMB_GAI_FIXTURE_ROOT=$(mktemp -d)
trap 'rm -rf -- "$DMB_GAI_FIXTURE_ROOT"' EXIT
export DMB_ETC_ROOT=$DMB_GAI_FIXTURE_ROOT/etc
export DMB_STATE_DIR=$DMB_GAI_FIXTURE_ROOT/state
export DMB_RUNTIME_DIR=$DMB_GAI_FIXTURE_ROOT/run
mkdir -p "$DMB_ETC_ROOT" "$DMB_STATE_DIR/transactions"
# Trust override exists only in this library fixture, never in the distribution.
dmb_trusted_path() { [[ $1 == "$DMB_GAI_FIXTURE_ROOT" || $1 == "$DMB_GAI_FIXTURE_ROOT/"* ]] && [[ ! -L $1 ]]; }
printf '# administrator comment\nlabel ::1/128 0\n' > "$DMB_ETC_ROOT/gai.conf"
chmod 640 "$DMB_ETC_ROOT/gai.conf"
setfacl -m u:65534:r-- "$DMB_ETC_ROOT/gai.conf"
setfattr -n user.dmitbox-test -v 'gai metadata' "$DMB_ETC_ROOT/gai.conf"
gai_before=$(dmb_file_state "$DMB_ETC_ROOT/gai.conf" | jq -cS .)
dmb_gai_plan v4 > "$DMB_GAI_FIXTURE_ROOT/plan.json"
[[ $(dmb_file_state "$DMB_ETC_ROOT/gai.conf" | jq -cS .) == "$gai_before" ]]
jq -e '.steps[0].purpose=="gai" and .steps[0].adapter=="fs"' "$DMB_GAI_FIXTURE_ROOT/plan.json" >/dev/null
gai_content=$(jq -r '.steps[0].content_b64' "$DMB_GAI_FIXTURE_ROOT/plan.json" | base64 -d)
[[ $gai_content == *'# administrator comment'* && $gai_content == *'label ::1/128 0'* && $gai_content == *'precedence ::ffff:0:0/96 100'* ]]
first=$DMB_STATE_DIR/transactions/tx-11111111111111111111111111111111
mkdir -p "$first/snapshots"
cp "$DMB_GAI_FIXTURE_ROOT/plan.json" "$first/plan.json"
dmb_fs_snapshot "$first/plan.json" 0 "$first"
dmb_fs_apply "$first/plan.json" 0 "$first"
dmb_fs_verify "$first/plan.json" 0 "$first"
gai_after=$(dmb_file_state "$DMB_ETC_ROOT/gai.conf" | jq -cS .)
[[ $(jq -cS 'del(.sha256)' <<< "$gai_after") == "$(jq -cS 'del(.sha256)' <<< "$gai_before")" ]]
printf '{"state":"COMMITTED"}\n' > "$first/state.json"
# Reapplication is a logical no-op; the managed table does not duplicate.
dmb_gai_plan v4 > "$DMB_GAI_FIXTURE_ROOT/reapply.json"
[[ $(jq -cS '.steps[0].after' "$DMB_GAI_FIXTURE_ROOT/reapply.json") == "$gai_after" ]]
# Restore is a new reviewable transaction, backed by the verified raw snapshot.
dmb_gai_plan restore > "$DMB_GAI_FIXTURE_ROOT/restore.json"
[[ $(jq -cS '.steps[0].after' "$DMB_GAI_FIXTURE_ROOT/restore.json") == "$gai_before" ]]
second=$DMB_STATE_DIR/transactions/tx-22222222222222222222222222222222
mkdir -p "$second/snapshots"
cp "$DMB_GAI_FIXTURE_ROOT/restore.json" "$second/plan.json"
printf '{"state":"PREPARED"}\n' > "$second/state.json"
dmb_fs_snapshot "$second/plan.json" 0 "$second"
dmb_fs_apply "$second/plan.json" 0 "$second"
dmb_fs_verify "$second/plan.json" 0 "$second"
[[ $(dmb_file_state "$DMB_ETC_ROOT/gai.conf" | jq -cS .) == "$gai_before" ]]
# Foreign policy and malformed ownership markers are never adopted.
printf 'precedence ::/0 99\n' >> "$DMB_ETC_ROOT/gai.conf"
! dmb_gai_plan v4 >/dev/null
! dmb_gai_plan default >/dev/null
printf '# BEGIN dmitbox-local gai\n' > "$DMB_ETC_ROOT/gai.conf"
! dmb_gai_plan v4 >/dev/null
printf '# END dmitbox-local gai\n' > "$DMB_ETC_ROOT/gai.conf"
! dmb_gai_plan default >/dev/null
printf '# foreign replacement\n' > "$DMB_ETC_ROOT/gai.conf"
! dmb_gai_plan restore >/dev/null
! dmb_gai_plan force >/dev/null
# A manually forged generic file plan must not bypass a typed SSH/firewall/DNS
# adapter by choosing another purpose or an unsupported absolute path.
for foreign in /etc/ssh/sshd_config /etc/nftables.conf /etc/resolv.conf /etc/sysctl.conf; do
  jq --arg path "$foreign" '.steps[0].path=$path' "$first/plan.json" > "$DMB_GAI_FIXTURE_ROOT/forged.json"
  ! dmb_fs_plan_validate "$DMB_GAI_FIXTURE_ROOT/forged.json" 0
  ! dmb_fs_apply "$DMB_GAI_FIXTURE_ROOT/forged.json" 0 "$first"
done
jq '.steps[0].purpose="arbitrary"' "$first/plan.json" > "$DMB_GAI_FIXTURE_ROOT/forged.json"
! dmb_fs_capture "$DMB_GAI_FIXTURE_ROOT/forged.json" 0
# Native default ACL inheritance must be rejected before a new live file is
# published if it differs from the planned metadata, even with a zero cp/chmod
# status. Once the conflict is removed the same explicit creation is viable.
printf 'probe\n' > "$DMB_GAI_FIXTURE_ROOT/prototype"
chmod 600 "$DMB_GAI_FIXTURE_ROOT/prototype"
probe_after=$(dmb_file_state "$DMB_GAI_FIXTURE_ROOT/prototype")
probe_content=$(base64 -w0 "$DMB_GAI_FIXTURE_ROOT/prototype")
jq -cn --arg path "$DMB_STATE_DIR/recovery-probe" --argjson after "$probe_after" --arg content "$probe_content" '{adapter:"fs",steps:[{id:"probe",purpose:"recovery-probe",path:$path,before:{exists:false},after:$after,content_b64:$content}]}' > "$DMB_GAI_FIXTURE_ROOT/probe.json"
setfacl -m d:u:65534:r-- "$DMB_STATE_DIR"
! dmb_fs_apply "$DMB_GAI_FIXTURE_ROOT/probe.json" 0 "$first"
[[ ! -e $DMB_STATE_DIR/recovery-probe ]]
setfacl -k "$DMB_STATE_DIR"
dmb_fs_apply "$DMB_GAI_FIXTURE_ROOT/probe.json" 0 "$first"
dmb_fs_verify "$DMB_GAI_FIXTURE_ROOT/probe.json" 0 "$first"
printf 'PASS: gai planning, real ACL/xattrs, apply/readback, no-op reapply, audited inverse, foreign/malformed-policy refusal\n'
