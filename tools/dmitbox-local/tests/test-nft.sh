#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
# shellcheck source=../src/validation.sh
source "$ROOT/src/validation.sh"
# shellcheck source=../src/adapters/nft.sh
source "$ROOT/src/adapters/nft.sh"
check() { "$@" || { printf 'FAIL: %s\n' "$*" >&2; exit 1; }; }
reject() { if "$@" >/dev/null 2>&1; then printf 'Unexpected success: %s\n' "$*" >&2; exit 1; fi; }
selector='{"mode":"exclusive","family":"inet","table":"dmitbox_local","chain":"input","parent_chain":null,"owner_id":"0123456789abcdef0123456789abcdef"}'
a='{"nftables":[{"metainfo":{"version":"x"}},{"rule":{"handle":9,"expr":[{"counter":{"packets":1,"bytes":20}},{"accept":null}]}}]}'
b='{"nftables":[{"rule":{"handle":123,"expr":[{"counter":{"packets":100,"bytes":400}},{"accept":null}]}}]}'
[[ $(dmb_nft_semantic <<< "$a") == "$(dmb_nft_semantic <<< "$b")" ]]
c='{"nftables":[{"rule":{"expr":[{"accept":null},{"counter":{"packets":1,"bytes":20}}]}}]}'
[[ $(dmb_nft_semantic <<< "$a") != "$(dmb_nft_semantic <<< "$c")" ]]
# A semantic empty counter must render to native anonymous counter:null.
[[ $(printf '{"counter":{}}' | dmb_nft_native) == '{"counter":null}' ]]
# Named policy fields are not dynamic counters.
[[ $(dmb_nft_semantic <<< '{"nftables":[],"bytes":123}') == '{"bytes":123,"nftables":[]}' ]]
state=$(dmb_nft_plan_state "$selector" '[{"address":"192.0.2.2","port":22},{"address":"::","port":2222}]')
check dmb_nft_state_validate "$selector" "$state"
reject dmb_nft_plan_state "$selector" '[{"address":"999.1.2.3","port":22}]'
reject dmb_nft_plan_state "$selector" '[{"address":"1.2.3.4","port":0}]'
reject dmb_nft_plan_state "$selector" '[]' '[{"action":"allow","protocol":"tcp","port":22,"source":"arbitrary"}]'
reject dmb_nft_state_validate "$selector" '{"present":true,"nftables":[{"table":{"family":"inet","name":"docker"}}]}'
private=$(mktemp -d); trap 'rm -rf -- "$private"' EXIT
printf '{"nftables":[]}\n' > "$private/live"
batch=$(dmb_nft_batch "$selector" '{"present":false,"nftables":[]}' "$state" "$private/live")
check jq -e 'all(.nftables[];has("add") or has("create")) and (.nftables[0]|has("create"))' <<< "$batch"
# Replacing our own table does not flush the global ruleset or other tables.
batch=$(dmb_nft_batch "$selector" "$state" "$state" "$private/live")
check jq -e '.nftables[0].delete.table.name=="dmitbox_local" and all(.nftables[];has("flush")|not)' <<< "$batch"
scoped='{"mode":"scoped","family":"inet","table":"filter","chain":"dmb_access","parent_chain":"input","owner_id":"0123456789abcdef0123456789abcdef"}'
state=$(dmb_nft_plan_state "$scoped" '[{"address":"::","port":22}]')
printf '{"nftables":[{"rule":{"family":"inet","table":"filter","chain":"input","handle":9,"comment":"dmitbox-local:0123456789abcdef0123456789abcdef","expr":[{"jump":{"target":"dmb_access"}}]}}]}\n' > "$private/live"
batch=$(dmb_nft_batch "$scoped" "$state" "$state" "$private/live")
# Existing parent jump is never deleted/reinserted during a child policy update.
check jq -e '.nftables[0].flush.chain.name=="dmb_access" and all(.nftables[];has("delete")|not) and all(.nftables[];.add.rule.chain? != "input")' <<< "$batch"
# A later foreign hook can drop even after an accept: enrollment must refuse.
printf '{"nftables":[{"chain":{"family":"inet","table":"alien","name":"input","hook":"input"}}]}\n' > "$private/live"
reject dmb_nft_foreign_guard "$selector" "$private/live"
# A failed service query must never be treated as proof that a manager is off.
manager_state=$'LoadState=loaded\nActiveState=inactive\nUnitFileState=disabled\nJob=0'
dmb_run() { [[ ! -e $private/query-fails ]] || return 1; printf '%s\n' "$manager_state"; }
check dmb_nft_systemd_service_guard nftables.service
for manager_state in $'LoadState=loaded\nActiveState=active\nUnitFileState=disabled\nJob=0' $'LoadState=loaded\nActiveState=inactive\nUnitFileState=enabled\nJob=0' $'LoadState=loaded\nActiveState=inactive\nUnitFileState=disabled\nJob=99 start' $'LoadState=unknown\nActiveState=inactive'; do
  reject dmb_nft_systemd_service_guard nftables.service
done
manager_state=$'LoadState=not-found\nActiveState=inactive\nUnitFileState=\nJob=0'
check dmb_nft_systemd_service_guard nftables.service
touch "$private/query-fails"
reject dmb_nft_systemd_service_guard nftables.service
printf 'PASS nft semantic/order/scoped batches/ownership/strict addresses\n'
