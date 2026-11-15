# Testing and installation evidence

## Safe local checks

```console
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-features
cargo xtask check
cargo xtask bundle
cargo xtask vm-acceptance --help
git diff --check
```

`xtask check` runs Rust formatting, Clippy, workspace tests and diff hygiene.
The bundle verifies static x86_64 musl ELF, the matching runtime marker and exact
worker embedding; the worker has no TUI dependencies. Both bundled `--help`
commands are safe smoke checks. The upstream reference is never executed.

Tests use `MockRunner`, isolated temporary files and controlled subprocesses.
They cover plan/CLI validation and bounded serialization, private credentials,
source authentication, host preflight/handoff, disk identity and destructive
confirmation, storage geometry/cleanup, distribution package/boot requirements,
complete target network/DNS rendering/publication, temporary Live networking,
first-boot MAC repair and TUI/terminal restoration. Failure cases must stop unsafe
continuation or reboot. Rendering tests are not native boot or connectivity tests.
Fixture timezone files are compiled IANA public-domain data; they do not read the
host timezone database. No test activates host swap or changes host networking.

## Previous consolidation evidence (5fbfaa1)

The consolidation was checked with plan schema 7 and the matching runtime marker:

| Check | Result |
| --- | --- |
| Formatting, strict workspace Clippy, all-feature workspace tests, diff hygiene | PASS |
| Updated Rust-only `xtask check` | PASS |
| Static musl bundle and exact embedded-worker verification | PASS |
| Debug/bundled host and worker help; VM harness help | PASS |
| Local preview packaging and SHA-256 integrity | PASS |
| Live official-source audit | Debian metadata reachable; Alpine, Arch and Gentoo failed with HTTP 403 |
| Fresh disposable VM execution | Not run: QEMU and `/dev/kvm` unavailable |

The live audit did not download/authenticate complete installation payloads.
No new full installation pass is established. The VM source seed now enables an
explicit DHCPv4-only networkd profile for the next boot and masks competing
source network services; this provisioning change still needs real VM validation.

## Integration test ownership

The root integration files are grouped by observable behavior. Common private
temporary directories live in `tests/support/`; the small Rust subprocess
fixture exercises real command I/O without a second installer.

| Files under `tests/` | Responsibility |
| --- | --- |
| `planning.rs`, `configuration.rs` | CLI/plan encoding, validation, target accounts/settings and safe publication |
| `host.rs`, `sources.rs` | Inspection, disk confirmation, authenticated handoff and source metadata |
| `network_config.rs`, `live_network.rs` | Complete target output and temporary Live connectivity |
| `storage.rs`, `installation.rs`, `targets.rs` | Storage guards, lifecycle and distribution requirements |
| `boot.rs`, `initrd.rs`, `firstboot.rs` | GRUB, private worker handoff and Alpine MAC repair |
| `runtime.rs`, `terminal.rs` | Commands, worker compatibility, CLI/TUI terminals and credential privacy |

Host module unit tests cover applied DHCPv6/RA policy separately from address
observations, unsupported owners/drop-ins, stale and unsafe files, routing rules
and tables, ambiguous/gateway-less defaults and incompatible source selection.
Systemd target tests cover MAC matching without name binding for Debian, Arch and
Gentoo; Alpine retains name-repair, idempotence and failure-path coverage.

## Historical VM evidence

These observations predate the current source and are not fresh acceptance.
The original serial/QMP/image evidence was not distributed with this repository.

| Scenario | SeaBIOS | OVMF/UEFI | Meaning |
| --- | --- | --- | --- |
| Alpine Live ISO boot to login | PASS | PASS | Historical QEMU/TCG firmware/Live smoke only |
| Host → kexec → Alpine Live | Reached Live | Reached Live | Historical handoff evidence; later Live fixes still require a fresh run |
| Full Alpine target installation | Failed/timed out | Not completed | No accepted target boot |
| Full Debian target installation | Not run | Not run | Unverified |
| Full Arch target installation | Not run | Not run | Unverified |
| Full Gentoo target installation | Not run | Not run | Unverified |

Historical Live smoke used Alpine 3.24, kernel 6.18.58-0-virt, QEMU 10.0.13,
SeaBIOS 1.16.3 and OVMF 2025.02 under TCG. Subsequent fixes addressed OpenRC
background logging and authenticated RAM modloop discovery. Those fixes do not
turn incomplete installations into passes. TCG does not establish KVM/provider
acceptance; VMware, Hyper-V, Xen HVM, real 4Kn and provider boot contracts remain
untested. Secure Boot, Xen PV/PVH/control domains and containers remain rejected.

## Disposable VM harness

The retained Rust harness supports `smoke`, `prepare-source` and `install`:

```console
cargo xtask vm-acceptance --help
cargo xtask vm-acceptance install \
  --tools /absolute/tools-root \
  --base /absolute/prepared-source.qcow2 --sha512 VERIFIED_SOURCE_SHA512 \
  --key /absolute/test-key --public-key /absolute/test-key.pub \
  --installer /absolute/rustaller-x86_64-linux-musl \
  --target all --firmware both --disk-bus virtio-blk \
  --timeout 3600 --output /absolute/new-results-directory
```

Obtain publisher-authenticated input images and verify their supplied hashes.
Inputs must be regular files without qcow2 backing chains. Each case uses fresh
scratch images and private firmware variables, never host block devices. Preserve
serial/QMP/SSH/probe logs and explicit pass/fail/not-run status. The harness's
proxy relay preserves the configured proxy and public CA; it does not bypass TLS.

A full pass requires authenticated source SSH, Live/install-complete markers,
exactly one final QMP reset, boot from the installed disk, target SSH, correct
architecture/kernel/filesystem/UUID fstab/GRUB and EFI fallback, working default
route/DNS/package refresh, and requested target-boot swap activation. The existing
probe fixture has specific XFS/kernel/swap expectations; extend it before claiming
the broader kernel/ext4/profile matrix.

Required future coverage includes all distributions × BIOS/UEFI × XFS/ext4,
explicit kernels, IPv4/IPv6-only/dual-stack, multiple NICs, DHCP/RA renewal and
source-policy refusals, target sysctl activation, hostname inheritance, non-root
credentials/root SSH policy, swap allocation/activation and CachyOS ISA/upgrade
retry. Failure scenarios include changed/ambiguous disk identity, mounted/active
swap, read-only disks, low RAM, downloads/signatures, missing drivers and final
SSH/GRUB validation. See [security](SECURITY.md) for code-level boundaries.

## Boot integration and TUI verification

The phased implementation adds Rust tests for firmware-specific netboot formats,
checksum mismatch, exact reboot confirmation, unsupported GRUB prerequisites,
state changes, publication/arming/reboot failures and restoration. Both ISO modes
reach the shared publisher in mocked BIOS/UEFI cases. ISO tests inspect read-only
mount/unmount behavior; digest changes block execution. TUI tests cover separate
preflight, blocked execution, repeated keys, focused validation and shared secret
snapshots. PTY tests include interruption during native ssh-keygen prompts.

The retained VM harness now accepts `media` with `--target netboot.xyz|iso`,
`--installer`, the normal disposable source/key/firmware options, and a required
`--expect-serial TEXT` marker. ISO needs `--iso` and `--sha256`; optional
`--iso-config`, `--iso-kernel`, `--iso-initrd`, `--iso-cmdline` mirror the CLI.
It dispatches the real scoped confirmation inside a fresh guest, then requires
one firmware reset and the requested marker in new serial output. This proves
only the selected marker/boot scope; it is not distribution installation acceptance.
The guest must satisfy the documented GRUB filesystem/module contract.

QEMU and /dev/kvm are unavailable here. Netboot artifact GETs returned HTTP 403;
upstream README/build recipes were reviewed via the official Git repository.
No actual netboot or ISO BIOS/UEFI boot pass is established, including the new
harness path. No development-host boot state was changed.

For this implementation, formatting, strict all-target/all-feature Clippy,
workspace tests, xtask check, static musl bundling with exact worker embedding,
VM harness help, debug/static executable help and diff hygiene passed. CLI media
inspection refused missing root privileges, and media --confirm-erase was rejected
before preparation. Documentation link checks passed. These checks establish
static/mocked behavior and terminal recovery, not real firmware boot success.
