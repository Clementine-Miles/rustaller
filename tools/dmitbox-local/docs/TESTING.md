# Reproducing verification

Development tools are installed separately. The distributed script never installs a missing dependency as a side effect of status, planning or recovery.

```sh
python3 build/build.py
DMITBOX_DEV_SHELLCHECK=/path/to/shellcheck bash tests/run.sh --integration
bash tests/run.sh --strict-integration
```

Python is used only for the development build. The first command must produce the standalone artifact before artifact-level testing. The runner creates a private directory under `artifacts/`, captures each check's output, records source SHA-256 sums and tool/environment versions, and writes `results.json`. Exit 77 means a skipped test, not success. Strict integration mode fails when any required check is skipped. `DMITBOX_DEV_SHELLCHECK` is a test-runner setting; it is not accepted by the production artifact.

The archived files under `upstream/` are reference inputs. The runner never executes them.

Native namespace tests never mutate the caller's network namespace. NFT integration covers real kernel counter changes, atomic rejection, unrelated NAT/container/set preservation, same-table jump position, parent-generation conflicts and exclusive reset recovery with an explicitly mocked boot ID. Static IPv6 and pool tests exercise real addresses, DAD and inverses; manager discovery and external-verifier results are identified fixture mocks.

Guest tests require explicitly prepared local inputs; they never download prerequisites. Set `DMITBOX_DEV_STATIC_BUSYBOX` to a reviewed static BusyBox binary and prepare the cache described by `tests/integration/build-guests.py`. `guest-fixtures` covers minimal native systemd/OpenRC init fixtures; `ssh-guest` covers actual kernel-backed OpenSSH authentication primitives. Their logs identify their assertions and omissions. The separate `full-distro-matrix` requirement remains SKIP until full Debian/Arch/Alpine configuration coverage exists, so strict integration cannot mistake these fixtures for full release acceptance. Missing kernel random-generator support is likewise an explicit NAT66 qualification skip, even when the fixed-source SNAT subtests passed.

## Evidence levels

| Check | What it establishes | What it cannot establish |
| --- | --- | --- |
| Bash syntax and ShellCheck | Parse validity and static diagnostics | Correct state transitions or restoration |
| Mock-driven unit tests | Assertions about controlled command results and injected failures | Native manager, kernel or reboot behavior |
| Namespace prerequisites | Real isolated dual-stack links, source routing and MTU facilities | DMITBox adapter behavior or external connectivity |
| Adapter namespace integration | Only the explicitly asserted native adapter postconditions | Init boot ordering, external SSH authentication or host reboot |
| Disposable VM matrix | Guest-specific boot, init, manager and reboot postconditions | Untested distributions, kernels or cloud networking |
| Controlled external verifier | Reachability from that identified verifier and context | Every route, client or future connection |

A zero command status, matching semantic hash, active timer, valid `sshd -t`, or local listening socket does not prove restoration or external accessibility. Test reports must identify which postconditions were observed and which remain uncertain.

## Required release scenarios

Release acceptance requires Debian and Arch systemd guests plus an Alpine OpenRC guest. Each must exercise a normal commit, timeout, signal, forced termination, reboot while pending, rollback retry and a genuine rollback failure. Include concurrent transactions, an unrelated administrator edit, interrupted cleanup, disk exhaustion, read-only state storage and wall-clock jumps. The original monotonic deadline must not be extended during confirmation.

Network coverage must include IPv4-only, IPv6-only, dual-stack, multiple interfaces and source-specific tables, DHCP/RA ownership, resolver search/routing domains and DNS security fields, competing firewall managers, nftables rule order and dynamic counters, SSH socket activation, multiple listener addresses and newly authenticated connections before and after retiring old access. NAT66/pool activation additionally requires an identified controlled external verifier and tests of partial setup failures, DAD and renumbering.

Run destructive guest tests only in disposable guests with a separate console. A host namespace cannot replace reboot tests. If QEMU, KVM, guest images, OpenRC, nftables or an external verifier is absent, record the scenario as blocked rather than substituting a mock pass.
