# Test evidence

Recorded at `2026-11-14T05:49:33Z`. Source checksums and environment/tool versions accompany the [machine-readable report](evidence/results.json). Logs are copied unchanged; their checksums are in `docs/evidence/SHA256SUMS`.

| Check | Result | Exit code |
| --- | --- | --- |
| `syntax` | **PASS** | 0 |
| `python-syntax` | **PASS** | 0 |
| `shellcheck` | **PASS** | 0 |
| `test-cli.sh` | **PASS** | 0 |
| `test-confirmation.sh` | **PASS** | 0 |
| `test-dns.sh` | **PASS** | 0 |
| `test-filesystem.sh` | **PASS** | 0 |
| `test-gai.sh` | **PASS** | 0 |
| `test-ipv6.sh` | **PASS** | 0 |
| `test-nat66.sh` | **PASS** | 0 |
| `test-nft.sh` | **PASS** | 0 |
| `test-pool.sh` | **PASS** | 0 |
| `test-recovery.sh` | **PASS** | 0 |
| `test-routing.sh` | **PASS** | 0 |
| `test-runtime.sh` | **PASS** | 0 |
| `test-ssh.sh` | **PASS** | 0 |
| `test-transaction.sh` | **PASS** | 0 |
| `test-validation.sh` | **PASS** | 0 |
| `namespace-prerequisites` | **PASS** | 0 |
| `network-adapters` | **PASS** | 0 |
| `pool-adapter` | **PASS** | 0 |
| `ipv6-adapter` | **PASS** | 0 |
| `nat66-adapter` | **SKIP** | 77 |
| `nft-adapters` | **PASS** | 0 |
| `ssh-guest` | **PASS** | 0 |
| `guest-fixtures` | **PASS** | 0 |
| `full-distro-matrix` | **SKIP** | 77 |
| `source-stability` | **PASS** | 0 |

Reported failures: **0**. Reported skips: **2**. Strict integration was **enabled**.

Reproduce with `python3 build/build.py` and `bash tests/run.sh --strict-integration`, then publish the chosen report using `python3 tests/publish-evidence.py artifacts/REPORT/results.json`. Source hashes identify the tested inputs; rerun after any source change.

A PASS applies only to the assertions in that check. A SKIP is missing evidence. Namespace facilities and mocked service behavior do not prove boot recovery or external accessibility. See [test levels](TESTING.md) and [limitations](LIMITATIONS.md) before interpreting these results as release acceptance.
