#!/usr/bin/env bash
# Development-only runner. Never sources or executes the archived upstream script.
set -uo pipefail
IFS=$'\n\t'

ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
integration=0
strict_integration=0
case ${1:-} in
  '') ;;
  --integration) integration=1 ;;
  --strict-integration) integration=1; strict_integration=1 ;;
  *) printf 'Usage: bash tests/run.sh [--integration|--strict-integration]\n' >&2; exit 2 ;;
esac
command -v jq >/dev/null || { printf 'Test runner requires jq.\n' >&2; exit 2; }
umask 077
mkdir -p -- "$ROOT/artifacts" || exit 2
report=$(mktemp -d "$ROOT/artifacts/test-$(date -u +%Y%m%dT%H%M%SZ)-XXXXXXXX") || exit 2
results="$report/results.jsonl"
: > "$results"
failed=0
skipped=0
{
  printf 'utc=%s\n' "$(date -u +%FT%TZ)"
  printf 'kernel=%s\n' "$(uname -srmo)"
  printf 'bash=%s\n' "$BASH_VERSION"
  printf 'jq=%s\n' "$(jq --version)"
  if command -v python3 >/dev/null; then printf 'python=%s\n' "$(python3 --version)"; fi
  printf 'init_executable=%s\n' "$(readlink /proc/1/exe || :)"
  if [[ -f /etc/os-release ]]; then cat /etc/os-release; fi
} > "$report/environment.txt"

record() {
  local name=$1 status=$2 code=$3 detail=$4
  jq -cn --arg name "$name" --arg status "$status" --argjson code "$code" --arg detail "$detail" \
    '{name:$name,status:$status,exit_code:$code,detail:$detail}' >> "$results"
  printf '%-5s %s\n' "$status" "$name"
  [[ $status != FAIL ]] || failed=$((failed+1))
  [[ $status != SKIP ]] || skipped=$((skipped+1))
}
run_case() {
  local name=$1; shift
  local code=0 log="$report/${name//\//_}.log"
  "$@" > "$log" 2>&1 || code=$?
  case $code in
    0) record "$name" PASS "$code" "$log" ;;
    77) record "$name" SKIP "$code" "$log" ;;
    *) record "$name" FAIL "$code" "$log"; tail -n 12 -- "$log" ;;
  esac
}

mapfile -t sources < <(find "$ROOT/src" "$ROOT/tests" "$ROOT/build" -type f -name '*.sh' -print | LC_ALL=C sort)
[[ ! -f $ROOT/dist/dmitbox-local.sh ]] || sources+=("$ROOT/dist/dmitbox-local.sh")
mapfile -d '' -t evidence_inputs < <(find "$ROOT/src" "$ROOT/tests" "$ROOT/build" -type f ! -path '*/__pycache__/*' -print0 | LC_ALL=C sort -z)
[[ ! -f $ROOT/dist/dmitbox-local.sh ]] || evidence_inputs+=("$ROOT/dist/dmitbox-local.sh")
sha256sum -- "${evidence_inputs[@]}" > "$report/source-sha256.txt"
syntax_code=0
for source_file in "${sources[@]}"; do bash -n "$source_file" >> "$report/syntax.log" 2>&1 || syntax_code=1; done
if ((syntax_code==0)); then record syntax PASS 0 "$report/syntax.log"; else record syntax FAIL 1 "$report/syntax.log"; fi
mapfile -t python_sources < <(find "$ROOT/tests" "$ROOT/build" -type f -name '*.py' -print | LC_ALL=C sort)
if ((${#python_sources[@]})); then
  if command -v python3 >/dev/null; then
    run_case python-syntax python3 -I -c 'import ast,pathlib,sys; [ast.parse(pathlib.Path(p).read_text(),filename=p) for p in sys.argv[1:]]' "${python_sources[@]}"
  else record python-syntax SKIP 77 'Python development tools are unavailable'; fi
fi

shellcheck_bin=$(command -v shellcheck || :)
if [[ -n ${DMITBOX_DEV_SHELLCHECK:-} ]]; then shellcheck_bin=$DMITBOX_DEV_SHELLCHECK; fi
if [[ -n $shellcheck_bin && -x $shellcheck_bin ]]; then
  "$shellcheck_bin" --version >> "$report/environment.txt" 2>&1
  # Modules intentionally resolve functions and globals supplied by the assembled
  # artifact; run together so analysis has their complete symbol definitions.
  run_case shellcheck "$shellcheck_bin" -s bash --severity=warning "${sources[@]}"
else record shellcheck SKIP 77 'shellcheck unavailable; set DMITBOX_DEV_SHELLCHECK to a development tool path'; fi

mapfile -t cases < <(find "$ROOT/tests" -maxdepth 1 -type f -name 'test-*.sh' -print | LC_ALL=C sort)
if ((${#cases[@]}==0)); then record unit-suite FAIL 1 'No unit test cases were found'; fi
for case_file in "${cases[@]}"; do run_case "${case_file##*/}" bash "$case_file"; done

if ((integration)); then
  run_case namespace-prerequisites bash "$ROOT/tests/integration/namespace-prerequisites.sh"
  if [[ -x $ROOT/tests/integration/network-adapters.sh ]]; then
    run_case network-adapters bash "$ROOT/tests/integration/network-adapters.sh"
  else record network-adapters SKIP 77 'Adapter namespace integration suite is not yet implemented'; fi
  if [[ -f $ROOT/tests/integration/pool-adapter.sh ]]; then run_case pool-adapter bash "$ROOT/tests/integration/pool-adapter.sh"; fi
  if [[ -f $ROOT/tests/integration/ipv6-adapter.sh ]]; then run_case ipv6-adapter bash "$ROOT/tests/integration/ipv6-adapter.sh"; fi
  if [[ -f $ROOT/tests/integration/nat66-adapter.sh ]]; then
    run_case nat66-adapter bash "$ROOT/tests/integration/nat66-adapter.sh"
  else record nat66-adapter SKIP 77 'Native NAT66 adapter qualification is unavailable'; fi
  if [[ -f $ROOT/tests/integration/nft-adapters.sh ]]; then
    run_case nft-adapters bash "$ROOT/tests/integration/nft-adapters.sh"
  else record nft-adapters SKIP 77 'Native nftables adapter suite is unavailable'; fi
  if [[ -f $ROOT/tests/integration/ssh-guest.py ]] && command -v python3 >/dev/null; then
    run_case ssh-guest python3 "$ROOT/tests/integration/ssh-guest.py"
  else record ssh-guest SKIP 77 'Minimal native SSH guest fixture is unavailable'; fi
  if [[ -x $ROOT/tests/integration/guest-matrix.sh ]]; then
    run_case guest-fixtures bash "$ROOT/tests/integration/guest-matrix.sh"
  else record guest-fixtures SKIP 77 'Disposable native init guest fixture harness is unavailable'; fi
  # Minimal native init fixtures exercise specific boot assertions. They cannot
  # stand in for full Debian/Arch/Alpine images and their competing managers.
  record full-distro-matrix SKIP 77 'Full Debian and Arch systemd plus Alpine OpenRC distribution/manager acceptance matrix remains unverified'
else record integration SKIP 77 'Not requested; use --strict-integration for release acceptance'; fi

# A report must not silently combine several revisions while agents or an
# administrator edit the source tree during a run.
mapfile -d '' -t final_inputs < <(find "$ROOT/src" "$ROOT/tests" "$ROOT/build" -type f ! -path '*/__pycache__/*' -print0 | LC_ALL=C sort -z)
[[ ! -f $ROOT/dist/dmitbox-local.sh ]] || final_inputs+=("$ROOT/dist/dmitbox-local.sh")
if sha256sum -- "${final_inputs[@]}" > "$report/source-sha256-end.txt" && cmp -s "$report/source-sha256.txt" "$report/source-sha256-end.txt"; then
  record source-stability PASS 0 'Source inputs remained unchanged during the run'
else record source-stability FAIL 1 'Source inputs changed during the run; rerun against a stable revision'; fi

jq -s --arg generated "$(date -u +%FT%TZ)" --arg root "$ROOT" \
  --argjson failures "$failed" --argjson skips "$skipped" --argjson strict "$strict_integration" \
  '{schema:1,generated_at:$generated,source_root:$root,failures:$failures,skips:$skips,strict_integration:($strict==1),results:.}' \
  "$results" > "$report/results.json"
printf 'Evidence: %s\n' "$report/results.json"
((failed==0)) || exit 1
if ((strict_integration && skipped>0)); then printf 'Release acceptance failed: required evidence is skipped.\n' >&2; exit 1; fi
