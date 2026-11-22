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
commands are safe smoke checks.

Tests use `MockRunner`, isolated temporary files and controlled subprocesses.
They cover plan/CLI validation and bounded serialization, private credentials,
source authentication, host preflight/handoff, disk identity and destructive
confirmation, storage geometry/cleanup, distribution package/boot requirements,
complete target network/DNS rendering/publication, temporary Live networking,
first-boot MAC repair and TUI/terminal restoration. Failure cases must stop unsafe
continuation or reboot. Rendering tests are not native boot or connectivity tests.
Fixture timezone files are compiled IANA public-domain data; they do not read the
host timezone database. No test activates host swap or changes host networking.

## Current migration evidence (2026-11-15)

Remote HEAD was checked before editing and matched
`4601ce9036fdf494f77463b1ec15fdb9517ac635` (Limine/Btrfs baseline).
The untouched revision passed 293 workspace tests in a separate checkout.
Plan schema is now 9 with a matching embedded-worker runtime marker; schema 8
is rejected rather than reinterpreting its fixed systemd Gentoo policy.

Validation commands and their final results are recorded in
[MIGRATION-REVIEW.md](MIGRATION-REVIEW.md). Unit/mock success, static bundling,
metadata reachability, and native command fixtures are separate from a real
installation. The current environment has no QEMU/qemu-img, `/dev/kvm`, OVMF,
or prepared source image. No current target installation, kernel update, reboot,
or SSH/network VM pass is claimed.

The read-only source audit reached Alpine 3.24.4, Debian 13/trixie, Arch databases,
and both Gentoo no-multilib variants at timestamp `20261108T170107Z`.
It parsed metadata and checked payload/signature endpoint reachability; it did
not download stage3 archives or authenticate OpenPGP signatures. Production
installation still requires its pinned keys/signature/hash checks.

## Integration test ownership

The root integration files are grouped by observable behavior. Common private
temporary directories live in `tests/support/`; the small Rust subprocess
fixture exercises real command I/O without a second installer.

| Files under `tests/` | Responsibility |
| --- | --- |
| `planning.rs`, `gentoo_plan.rs`, `configuration.rs` | CLI/plan encoding, validation, target accounts/settings and safe publication |
| `host.rs`, `sources.rs` | Inspection, disk confirmation, authenticated handoff and source metadata |
| `network_config.rs`, `live_network.rs` | Complete target output and temporary Live connectivity |
| `storage.rs`, `installation.rs`, `targets.rs` | Storage guards, lifecycle and distribution requirements |
| `boot.rs`, `initrd.rs`, `firstboot.rs` | Target boot, private worker handoff and Alpine MAC repair |
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

On memory-constrained hosts, use `--memory-mib 1536` (default 3072).
Use `--accel kvm` only after verifying nested KVM; the default remains TCG and
there is no silent fallback. Both settings are recorded in `hardware.txt`.
The source seed uses the fixed private SLIRP address 10.0.2.15/24 and networkd;
cloud-init reapplies its profile before preflight. Target plans explicitly select
SLIRP DNS `10.0.2.3`, since static addressing supplies no DHCP DNS lease. This
avoids DHCP-added host routes unsupported by the installer's strict source
routing contract.
`source-state.log` records initial disk/network state. Recovery SSH periodically
captures `live-install.log` and stops the case early on a reported worker failure.

Obtain publisher-authenticated input images and verify their supplied hashes.
Inputs must be regular files without qcow2 backing chains. Each case uses fresh
scratch images and private firmware variables, never host block devices. Preserve
serial/QMP/SSH/probe logs and explicit pass/fail/not-run status. The harness's
proxy relay preserves the configured proxy and public CA; it does not bypass TLS.

A full pass requires authenticated source SSH, Live/install-complete markers,
exactly one OS-requested final firmware reboot, installed-disk boot, target SSH, correct kernel,
filesystem/UUID fstab, FAT32 `/boot` UUID against fstab/boot state, boot configuration/payloads, default
route/DNS/package refresh and filesystem-specific swap policy. The probe validates
XFS/ext4 swap activation; Btrfs must use `@`, zstd and no swapfile.

`--filesystem all --target all --firmware both` runs 24 default-family cases.
Add `--kernel all --gentoo-init both` to run 60 official-family/init combinations.
Arch CachyOS remains a separate explicit `--target arch --kernel cachyos` case.
Use `--target gentoo --gentoo-init both --filesystem all --firmware both` for
12 Gentoo cases. `--kernel` also accepts a target-specific individual choice.
`--boot-size-mib 1024` (512–4096) allows acceptance of larger images/generations.
`--disk-size-gib 3072 --firmware bios` expands only a fresh sparse overlay to
exercise the three-partition GPT case.

Raw QMP RESETs remain in `qmp.log`. For SeaBIOS, a soft reset may enter
`handle_resume32` and explicitly request a second hard reset. Two raw events are
accepted only with exactly one matching full soft-to-hard sequence in the
captured `firmware-debug.log`; extra resets, missing evidence and UEFI pairs fail.
The rule is also checked for the native-update reboot, with no time debouncing.
Use `--keep-disks yes` to retain successful disposable overlays for subsequent
recovery testing (the default still removes successful overlays).

`--kernel-update yes` runs a native selected-package update, initramfs rebuild,
boot refresh, another reboot and target SSH/network probe. Logs preserve the
before/after kernel release: a repository with no newer candidate establishes
rebuild/reboot evidence only. Neither upgrade-hook correctness nor a new kernel
version is inferred from a mock command succeeding. The standalone probe also
verifies the selected kernel package/family and Gentoo init/profile/64-bit ABI.

The `smoke --iso` input is the official Alpine Live test image, not an installer
feature. Its stock boot files and source-image bootloader remain harness inputs;
new target installations use rustaller-boot.

## Rust-native bootloader acceptance

The following results use disposable QEMU/TCG images. The raw firmware smoke
loaded a Debian cloud kernel and two ordered initrd archives into a test PID 1.
That test covers the loader and kernel handoff. The installation cases also
exercise the target package hooks and verify the installed system over SSH.

| Scenario | Result |
| --- | --- |
| OVMF fresh boot and recovery from invalid primary TOML | PASS, test PID 1 |
| SeaBIOS MBR current generation and GPT previous generation | PASS, test PID 1 |
| Alpine `linux-virt`, SeaBIOS/MBR, ext4 | PASS, full install, update, reboot and SSH |
| Alpine `linux-virt`, SeaBIOS/GPT, XFS | PASS, full install, update, reboot and SSH on a 3 TiB sparse disk |
| Alpine `linux-virt`, OVMF/GPT, Btrfs | PASS, installed system rebuilt with `mkinitfs` and rebooted over SSH with matched release artifacts |
| Debian `linux-image-cloud-amd64`, OVMF/GPT, ext4 | PASS, full install, APT transaction, `update-initramfs`, reboot and SSH |
| Arch `linux-lts`, OVMF/GPT, ext4 | PASS, full install, pacman transaction, `mkinitcpio`, reboot and SSH |
| Gentoo OpenRC no-multilib, SeaBIOS/MBR, ext4 | Partial. BIOS and dracut reached the installed root from a disposable copy. The automatic run stopped at the Portage cache guard before the fix. |
| Gentoo systemd no-multilib, OVMF/GPT, Btrfs | Partial. Verified stage3 and profile. The TCG case timed out in Portage before the first target reboot. |

The Alpine ext4 case also booted the previous generation on a clone of the
installed disk. IPv6 Internet connectivity and hardware microcode application
remain unverified. [Initial evidence](evidence/2026-11-21/native-boot.txt)
[Follow-up evidence](evidence/2026-11-22/native-followup.txt)

## Historical Limine/filesystem acceptance matrix

Fresh KVM acceptance and reviewed evidence are recorded in
[VERIFICATION-FOLLOWUP-2026-11-19.md](VERIFICATION-FOLLOWUP-2026-11-19.md).
Both baseline cases passed after the evidenced SeaBIOS reset correction. Gated
expansion passed Alpine BIOS/XFS, Debian UEFI/Btrfs and Arch UEFI/ext4, including
native regeneration/reboot. Alpine and Debian previous-generation boots and
unmounted-boot generator rejection were also exercised in disposable guests.
The [initial report](ACCEPTANCE-2026-11-17.md) preserves failed retries and fixes.
PASS applies to the recorded revision/kernel/controller, not every combination.
Historical Live smoke above is not evidence of target installation.

| Distribution | BIOS/MBR XFS | BIOS/MBR ext4 | BIOS/MBR Btrfs | UEFI/GPT XFS | UEFI/GPT ext4 | UEFI/GPT Btrfs |
| --- | --- | --- | --- | --- | --- | --- |
| Alpine | PASS | PASS | Not run | Not run | Not run | Not run |
| Debian | Not run | Not run | Not run | Not run | PASS | PASS |
| Arch | Not run | Not run | Not run | Not run | PASS | Not run |
| Gentoo OpenRC no-multilib | Not run | Not run | Not run | Not run | Not run | Not run |
| Gentoo systemd no-multilib | Not run | Not run | Not run | Not run | Not run | Not run |

BIOS/GPT stage2, 4Kn UEFI, transitions to newer kernel versions, unexercised
package-hook combinations, CachyOS and provider-specific firmware still need
fresh VM or hardware evidence. Gentoo was not run within this host role's
conservative 1536 MiB RAM budget. The linked reports distinguish available-host
KVM results from blocked multi-host coverage and the unexecuted full matrix.

## Regression boundaries

`boot.rs` table-tests plan/layout/render combinations, GPT stage2 commands,
UUID fstab/fsck rules, Btrfs formatting and subvolume cleanup, compression/schema
roundtrips, kernel version ordering and update generations. Mocked resource tests
reject digest mismatch before compilation. Native temporary-file tests exercise
missing initramfs, unsafe symlinks/hardlinks, invalid firmware payloads, unmounted
or wrong-UUID FAT, low capacity, interrupted copies, corrupted active config,
previous-generation recovery, numeric ordering, microcode and kernel removals.
Generator guard syntax and dracut wrapper staging/diagnostic deferral use controlled
local fixtures; they do not establish actual distribution package transaction behavior.

ISA tests cover every cumulative v1/v2/v3 feature, all required v4 features,
missing OSXSAVE, incomplete XCR0 state, heterogeneous/feature-masked CPUs, malformed
CPU fields, topology, and caller affinity preservation. Inspection tests exercise
missing utilities/proc/sys facts, malformed lsblk/routes/mounts, changing inventories,
memory/swap/zram, firmware unknowns and cached responsive panels. Controlled native
command tests enforce output bounds/deadlines, including descendant output pipes.

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
