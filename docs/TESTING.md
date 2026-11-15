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

## Current refactor evidence

The reviewed starting revision was `31cbac103175d61582f9300f882989ed6a8840a299`.
The new plan schema is 8, with a matching embedded-worker runtime marker.
Static/mocked checks are separate from real installation evidence. Formatting,
strict workspace Clippy, workspace tests, `xtask check` and static musl bundle
verification passed. The production Limine resource-preparation function was run
against a private temporary directory: all five pinned resources downloaded,
matched their SHA-256 values and compiled the native utility successfully. Only
resource preparation and help were executed; no bootloader/disk was modified.
This does not prove firmware booting.

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
| `boot.rs`, `initrd.rs`, `firstboot.rs` | Limine, private worker handoff and Alpine MAC repair |
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
  --target all --firmware both --filesystem all --disk-bus virtio-blk \
  --timeout 3600 --output /absolute/new-results-directory
```

Obtain publisher-authenticated input images and verify their supplied hashes.
Inputs must be regular files without qcow2 backing chains. Each case uses fresh
scratch images and private firmware variables, never host block devices. Preserve
serial/QMP/SSH/probe logs and explicit pass/fail/not-run status. The harness's
proxy relay preserves the configured proxy and public CA; it does not bypass TLS.

A full pass requires authenticated source SSH, Live/install-complete markers,
exactly one final QMP reset, installed-disk boot, target SSH, correct kernel,
filesystem/UUID fstab, mounted FAT32, Limine configuration/payloads, default
route/DNS/package refresh and filesystem-specific swap policy. The probe validates
XFS/ext4 swap activation; Btrfs must use `@`, zstd and no swapfile.

`--filesystem all --target all --firmware both` runs the 24 primary combinations.
Individual filesystems can be selected. `--disk-size-gib 3072 --firmware bios`
expands only a fresh sparse overlay to exercise the three-partition GPT case.
The `smoke --iso` input is the official Alpine Live test image, not an installer
feature. Its stock boot files and source-image bootloader remain harness inputs;
new target installations use Limine exclusively.

## Limine/filesystem acceptance matrix

All entries below are **NOT RUN** for the current Limine implementation. Historical
Live smoke above is not evidence of target booting.

| Distribution | BIOS/MBR XFS | BIOS/MBR ext4 | BIOS/MBR Btrfs | UEFI/GPT XFS | UEFI/GPT ext4 | UEFI/GPT Btrfs |
| --- | --- | --- | --- | --- | --- | --- |
| Alpine | Not run | Not run | Not run | Not run | Not run | Not run |
| Debian | Not run | Not run | Not run | Not run | Not run | Not run |
| Arch | Not run | Not run | Not run | Not run | Not run | Not run |
| Gentoo | Not run | Not run | Not run | Not run | Not run | Not run |

BIOS/GPT stage2, 4Kn UEFI, native kernel upgrades, package-hook ordering, CachyOS
first boot, serial output and provider-specific firmware also need fresh VM or
hardware evidence. QEMU and `/dev/kvm` are unavailable in the development
environment, so no current real installation/reboot pass is claimed.

## Regression boundaries

`boot.rs` table-tests all 24 plan/layout/render combinations, GPT stage2 commands,
UUID fstab/fsck rules, Btrfs formatting and subvolume cleanup, compression/schema
roundtrips, kernel version ordering and update generations. Mocked resource tests
reject digest mismatch before compilation. Native temporary-file tests exercise
missing initramfs, unsafe symlinks/hardlinks, invalid firmware payloads, unmounted
FAT and low capacity without replacing the published configuration.

TUI tests preserve per-target drafts, fresh-key submission, cancellation, focused
validation, credential masking and terminal recovery, and add current/target
panel placement at 120-, 60- and 48-column sizes. Set `RUSTALLER_CAPTURE_TUI=1`
when running these tests to save simulated terminal captures under `/tmp`.
They are deterministic renderer fixtures, not host inspection or VM screenshots.

Retained safety coverage includes disk identity/rebinding, exact destructive
confirmation, source authentication, private handoff, unsupported firmware,
read-only/4Kn geometry, mounted/active-swap rejection, unsafe paths, file
publication failures, network rendering/ownership, first boot and refusal to
reboot an incomplete installation. IPv6 external connectivity, all kernels,
multiple NIC renewals, low-memory installations and hardware boot remain unverified.

Official source audit reachability is environment-dependent and does not replace
payload authentication. Previous audits reached Debian metadata while other
endpoints returned HTTP 403; rerun and report actual results for each delivery.
