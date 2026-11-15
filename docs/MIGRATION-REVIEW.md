# Boot, inspection, kernels and Gentoo migration review

## Starting point and scope

The remote default-branch HEAD was checked before edits and was still
`4601ce9036fdf494f77463b1ec15fdb9517ac635`, “Migrate target boot to pinned Limine
and add Btrfs roots”. Its untouched workspace passed 293 tests. The audit found
separate root-filesystem `/boot` and FAT32 `/efi`, flat distribution artifact
scanning, a shared Limine refresh worker, target-specific kernel mapping, fixed
systemd Gentoo sources, and a cached CLI/TUI preview. Those integration points
were extended rather than replaced.

The existing Alpine, Debian, Arch, Gentoo, Live rescue, kexec, reset infrastructure,
CLI/TUI, SSH/key generation, networking/DNS/IPv4/IPv6/MTU/TCP, profiles/packages,
XFS/ext4/Btrfs, existing swap policy, FRP, holds/recovery/logging, authenticated
sources, disk identity and matching-worker checks remain. Existing reset execution
is still unavailable. No Local ISO/netboot.xyz, GRUB, EFISTUB or UKI route was
introduced. Regression tests passed; this does not establish native boot support
for every retained feature.

NodeQuality and both supplied ECS URLs were fetched and read as references,
never executed. Their procfs/sysfs, topology and device-information approaches
informed collection; benchmarking, package installation, remote scripts, network
speed tests and provider/geolocation lookups were excluded. They are not runtime
dependencies. References:

- https://raw.githubusercontent.com/LloydAsp/NodeQuality/refs/heads/main/NodeQuality.sh
- https://gitlab.com/spiritysdx/za/-/raw/main/ecs.sh
- https://raw.githubusercontent.com/spiritLHLS/ecs/main/ecs.sh

## Resulting boot architecture

BIOS/MBR keeps FAT32 `/boot` plus root and Limine BIOS embedding. UEFI/GPT keeps
FAT32 ESP `/boot` plus root and `/boot/EFI/BOOT/BOOTX64.EFI`, without NVRAM writes.
Large BIOS/GPT disks retain the dedicated BIOS Boot partition. Limine remains
9.6.5 at its existing commit and authenticated resource hashes.

Distribution-managed regular files and isolated immutable generations share FAT32
`/boot`. Refresh scans only flat native files, validates matching initramfs and
optional microcode, verifies the mount's recorded UUID, locks on the Linux root,
and checks complete-generation capacity plus a 1 MiB reserve. It copies with
bounded lengths and source/destination verification, syncs payloads and metadata,
validates all references, and publishes the active configuration. Current and
previous generations remain referenced; previous menu entries and
`limine.conf.previous` retain recovery choices. Interrupted unreferenced staging
can be pruned, and an invalid active config can be restored from a validated
previous one. Missing pairs and low capacity preserve the published configuration.

Mount masks emulate private modes; `quiet` explicitly handles unsupported native
chmod/chown requests. Debian's symlink generation is disabled. Gentoo uses legacy
regular-file installkernel, dracut compatibility layout, and matching no-multilib
profiles. Package and generator guards check the mounted FAT UUID before writes.
A new boot-state schema rejects the old `/efi` layout instead of writing into it.

The default remains 512 MiB, configurable through 4096 MiB. Actual free space is
checked at publication; large initramfs, multiple kernels or retained generations
can require 1024–2048 MiB or more. FAT is outside Btrfs `@` snapshots; restoring a
root snapshot requires separately restoring matching kernel/initramfs/modules.

## Inspection and ISA

Typed optional facts cover System, CPU/ISA, Virtualization, Memory, Storage,
Firmware, Network, and Installation compatibility. This includes OS/kernel/init/
libc/uptime, guest topology/caches/frequency/features, swap/zram/balloon indicators,
device identities/relationships/geometry/TRIM/mounts/utilization, routes/addresses/
MTU/MAC/link/speed/DNS and known source DHCP/static policy. Unknown facts remain
explicit. Firmware vendor is separate from active BIOS/UEFI; containers with
filtered sysfs do not receive a guessed firmware mode. Mount secrets are redacted.

Collection uses bounded proc/sys reads and local commands capped at two seconds
and 64 KiB; subprocess groups are terminated on completion/timeout. Optional
utility failures do not hide other facts. CLI `--inspect` and TUI use offline
inventory and the same target renderer, with no source metadata requests. The TUI
renders cached facts in responsive panels. Incomplete inventory cannot be encoded
or used to publish target configuration; strict installation checks remain.

ISA classification samples CPUID on every CPU in the calling thread's allowed
affinity mask, intersects capabilities and XCR0, and restores affinity in a
separate short-lived thread. An incomplete scan returns Unknown. Cumulative v1
and v2 requirements precede v3: AVX, AVX2, BMI1, BMI2, F16C, FMA, LZCNT, MOVBE,
XSAVE/OSXSAVE and OS-enabled XMM/YMM state (`XCR0 & 0x6 == 0x6`). XGETBV is
executed only after guarded CPUID checks. v4 additionally requires AVX512F/BW/CD/
DQ/VL and opmask/ZMM state (`XCR0 & 0xe6 == 0xe6`). CPU names never imply support.
Hypervisor masking is respected. This is diagnostic; target optimization remains
generic x86-64. Full methodology and official references are in
[ARCHITECTURE.md](ARCHITECTURE.md#cpu-capability-methodology).

## Kernel and Gentoo choices

| Target | Choices and resolved packages |
| --- | --- |
| Alpine | auto/standard/generic/lts → `linux-lts`; cloud/virt → `linux-virt` |
| Debian | auto/standard/generic → `linux-image-amd64`; cloud → `linux-image-cloud-amd64` |
| Arch | auto/standard → `linux`; lts → `linux-lts`; hardened → `linux-hardened`; zen → `linux-zen`; explicit cachyos → existing `linux-cachyos` workflow |
| Gentoo | auto/standard → `sys-kernel/gentoo-kernel-bin` |

Official package spellings are accepted where applicable and validated against the
target. Invalid combinations are rejected. Native package managers resolve the
selected package and initramfs dependencies before installation; installed module
inventories must contain the root/FAT and observable guest storage/network drivers.
Only the selected kernel is requested. Existing multiple installed kernel pairs
remain supported by refresh. No source kernel build was made mandatory.

Gentoo new plans default to OpenRC. `--gentoo-init=systemd` selects systemd
explicitly; the option is rejected on other targets. Each choice selects only
its official `latest-stage3-amd64-nomultilib-{openrc,systemd}.txt`, exact timestamped
archive and matching stable no-multilib profile. Stage identity and selected init
must agree; there is no generic/multilib/musl/x32 fallback. Final `ABI_X86=64` is
verified. Trusted release keys, signed DIGESTS, archive signature, SHA-512, size
and safe extraction remain mandatory. Plan schema/runtime 9 encode the choice;
old schema 8 fails closed.

Common kernel, SSH, logging, FRP, filesystem and package logic is shared. OpenRC
uses Gentoo netifrc/dhcpcd/openresolv, explicit route families, MAC name binding,
runlevels and native services. Systemd retains networkd/resolved and native units.
64-bit-only ABI policy is independent of detected x86-64-v3/v4 support.

## File-by-file changes

| File | Meaningful change |
| --- | --- |
| `src/shared/storage.rs` | `/boot` mount/fstab masks and unchanged firmware-aware roles |
| `src/targets/limine.rs` | FAT-safe generations, UUID guards, capacity/locking/recovery and native hooks |
| `src/targets/install.rs` | Initial mount checks, package/initramfs integration, drivers, Gentoo services and signed DIGESTS |
| `src/worker/install.rs` | Mount identity before bootstrap and `/boot` cleanup before root unmount |
| `src/bin/rustaller-worker.rs` | Shared mount-check action and native hook dispatch |
| `src/host/inspect.rs` | Typed optional grouped collection, fallback reports and common target preview |
| `src/host/inspect_cpu.rs` | CPUID/XGETBV/affinity ISA and visible CPU topology |
| `src/command.rs` | Enforced diagnostic deadlines, bounded capture and process-group cleanup |
| `src/bin/rustaller.rs` | Offline inspection separated from authenticated installation preparation |
| `src/deployment.rs` | Separate inventory validation and exact no-multilib source identities |
| `src/host/handoff.rs` | Offline inventory and selected Gentoo resource discovery |
| `src/host/sources.rs` | Exact OpenRC/systemd no-multilib index/archive parsers and plan binding |
| `src/postinstall.rs` | Central kernel packages and distribution-specific aliases |
| `src/targets/packages.rs` | Dependencies, package availability and installed-driver requirements |
| `src/targets/debian.rs` | FAT-compatible no-symlink policy and kernel availability |
| `src/targets/rolling.rs` | Arch resolution and shared Gentoo profile/ABI/installkernel/dracut setup |
| `src/shared/network.rs` | Gentoo netifrc/DHCP/DNS/MTU/routes/MAC rules and inventory-only validation |
| `src/shared/swapfile.rs` | Gentoo OpenRC target swap activation |
| `src/config.rs` | Typed optional Gentoo init and target-specific requirements |
| `src/model.rs` | New Gentoo plan default and strict validation |
| `src/codec.rs` | Schema 9 init selection and added kernel discriminants |
| `src/lib.rs` | Matching runtime compatibility marker |
| `src/host/args.rs` | CLI init choice, target-aware kernel parsing and help |
| `src/tui/draft.rs` | Per-target init draft |
| `src/tui/fields.rs` | `/boot` storage labels |
| `src/tui/menus.rs` | Kernel/init menus and Btrfs selector correction |
| `src/tui/plan.rs` | Init serialization and package/ABI/boot previews |
| `src/tui/render.rs` | Responsive blocked-diagnostic panels |
| `src/tui/state.rs` | Blocked reports cannot dispatch installation |
| `src/tui/tests.rs` | Draft, target choices, Btrfs and blocked-panel regressions |
| `tests/boot.rs` | Layouts, families, generations, interruption/recovery/capacity and native guard fixtures |
| `tests/configuration.rs` | Complete kernel mapping and missing installed-driver rejection |
| `tests/gentoo_plan.rs` | Default/explicit init, schema roundtrip and old-schema rejection |
| `tests/fixtures/embedded-worker-version.txt` | Matching debug-worker marker |
| `tests/host.rs` | Rich previews and incomplete-inventory encoding rejection |
| `tests/installation.rs` | Current Gentoo source/profile plan fixtures |
| `tests/network_config.rs` | OpenRC policy/family/collision handling and offline authorization boundary |
| `tests/runtime.rs` | Enforced command/output/deadline and descendant-pipe tests |
| `tests/sources.rs` | Nomultilib identities, mismatched-init/multilib rejection |
| `tests/targets.rs` | Native package resolution, Debian FAT policy and filesystem/driver inventories |
| `xtask/src/sources.rs` | Both Gentoo variants in read-only source audit |
| `xtask/src/vm.rs` | Kernel/init/boot-size matrix and optional update/rebuild/reboot acceptance |
| `xtask/src/vm_guest.rs` | FAT UUID/state/fstab, selected kernel, Gentoo init/profile/ABI and upgrade probes |
| `xtask/src/dist.rs` | Release notes advertise FAT32 `/boot` |
| `README.md`, `README.en.md` | Updated boot layout, kernels, Gentoo choices and limits |
| `docs/ARCHITECTURE.md` | Ownership/lifecycle and official ISA methodology |
| `docs/CONFIGURATION.md` | Schema, native hooks, manual dracut publication and capacity policy |
| `docs/INSTALLATION.md` | Kernel/init matrix, layouts and Btrfs snapshot caveat |
| `docs/SECURITY.md` | FAT publication limits, source binding and diagnostic boundary |
| `docs/TESTING.md` | Current evidence and expanded, explicitly unrun VM matrix |

## Validation and remaining limits

Executed after the implementation freeze with the repository's pinned Rust
1.99.1 toolchain:

| Command/check | Actual result |
| --- | --- |
| `cargo xtask check` | Passed: formatting, strict workspace/all-target/all-feature Clippy, 344 workspace tests, standalone probe formatting and whitespace checks |
| `cargo xtask bundle` | Passed: matching static x86_64 musl host/worker, exact embedding and runtime validation |
| `rustc --edition=2024 --test xtask/src/vm_guest.rs -o /tmp/rustaller-vm-probe-tests` and test binary | Passed: 1 standalone probe test |
| Standalone `rustc --edition=2024 xtask/src/vm_guest.rs` | Compiled successfully |
| `cargo xtask audit-sources` | Passed endpoint/metadata audit for Alpine, Debian, Arch and both Gentoo variants; no payload/signature authentication claim |
| `cargo xtask vm-acceptance --help` | Passed; matrix selectors available |
| Live CLI Gentoo systemd/Btrfs `--inspect` | Exit 0; grouped diagnostics and known target plan shown; unprivileged compatibility blocked and no erase token emitted |
| Static host/worker `--help` | Both exited 0; worker reported runtime `RUSTALLER-CONFIG-20261115-09` |
| Real QEMU install/reboot/SSH | Not run: no QEMU/qemu-img, firmware files or authenticated prepared source image available |

There is no current real-VM acceptance for any new kernel, Gentoo init/firmware/
filesystem combination, native package upgrade/removal transaction, low-space
boot, multi-NIC rename sequence, or kernel-update reboot. The harness supports
these cases and preserves explicit evidence; its successful compilation and mock
fixtures cannot substitute for them. Existing limitations on unsupported source
network managers, complex routing, containers, Secure Boot and provider boot
contracts remain.

FAT rename/fsync and a previous generation do not protect against corruption of
the FAT filesystem itself. A previous kernel/initramfs generation does not retain
the corresponding root `/lib/modules`; package removal or a Btrfs snapshot restore
can require restoring matching modules before relying on that fallback. Manual
custom-output or absolute `/usr/bin/dracut` calls require a subsequent explicit
`limine-refresh`; ordinary PATH regeneration and installkernel publication have
separate integration. No automated snapshot manager or rollback-on-boot-failure
system was introduced.
