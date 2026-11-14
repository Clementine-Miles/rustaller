#!/usr/bin/env bash
# Minimal init fixtures only. This does not substitute for full distribution or
# external-connectivity acceptance. Inputs must be prepared explicitly offline.
set -euo pipefail
ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)
command -v python3 >/dev/null || { printf 'SKIP: Python development runner unavailable\n'; exit 77; }
exec python3 -I "$ROOT/tests/integration/run-guests.py"
