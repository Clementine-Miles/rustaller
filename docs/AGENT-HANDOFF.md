# rustaller Continuation Guide

Read root directory first `AGENTS.md`、[Architecture](ARCHITECTURE.md)and[acceptance records](ACCEPTANCE.md)。This article lists development priorities，[history](HISTORY.md)Interpret old decisions; no need to rely on old sessions or local files not shipped with source code。

## Current status

- Linux x86_64，English multi-level TUI；Four Goals Certified Alpine Live and kexec Install and only reboot once at the end. Already have real liquidation capabilities, four systems BIOS/UEFI Full acceptance has not yet passed, keep development preview。
- root optional XFS（default）/ext4，reserve GRUB（SeaBIOS/UEFI）and Alpine/OpenRC。EFI Stub Canceled，[Evaluate](EFI-STUB-OPTIONS.md)Just for history。
- Live Check selected file system modules and deductions before liquidation ESP/Aligned root capacity。XFS Retains compatibility features with `-K`，ext4 Whole volume prohibited discard and fully initialized inode/journal；fstab、initramfs、GRUB configure according to the selection。swapfile Supports both, only the new target is activated upon startup, the original system/Live No swapon/swapoff。
- plan schema=6、manifest schema=1、Rust mark `RUSTALLER-CONFIG-20261112-06`，Reject old plans and mix-ups worker。Shell Four resources and old tags remain unchanged。
- Kernel defaults and options, hostname inheritance, non- root User independence root See the password [POSTINSTALL.md](POSTINSTALL.md)；public DNS/Custom See with file system input [CONFIGURATION.md](CONFIGURATION.md)。Gentoo Kernel/init remain unchanged。
- Arch explicit selection CachyOS Only enable fixed official scripts, full keys and OS Available ISA Warehouse inspection; first target boot to complete all-inclusive upgrade、initramfs/GRUB Update, if failed, retain the status and try again without additional automatic restart. Resource fixed value see [SOURCES.md](SOURCES.md)。
- storehouse：https://github.com/Clementine-Miles/rustaller 。Cargo main bag、CLI for `rustaller`，worker for `rustaller-worker`；GNU GPLv3，Preserve upstream claims and attribution。
- host crate + xtask，`tests/verify` independent. The core has no third-party runtime dependencies；tui Optional，libc Test only、sha2 Development tools only. Module declarations focus on `lib.rs`，public Live The command default value is `command::live_command`，environment with retries left in the adapter。
- Source code package retained Cargo.lock、public key、fixture、`.github`/`.cargo`，exclude `.git`、Build product、VM、Logs and private keys。

history XFS Modification passed 273 workspace + 49 Shell Test, after the reference project is repaired, it is 277 + 49；these records and static builds do not constitute new VM Evidence of installation. latest check to correspond to submitted CI Subject to。

## Restore development environment

tool chain/musl target See `rust-toolchain.toml`。requires Bash、Git、BusyBox、ShellCheck 0.11+、ssh-keygen；VM Additional required QEMU、SeaBIOS、OVMF、xorriso Mirror with certified discardable source。

```console
git fetch --no-tags --depth=1 https://github.com/bin456789/reinstall.git 98f1e21013233d42750cc2e7a298a0818461d686:refs/rustaller/upstream-baseline
cargo xtask check
cargo xtask bundle
target/x86_64-unknown-linux-musl/release/rustaller --help
```

Already have a warehouse? clone Then copy the source code and keep the hidden directory and history; the bare source code can be git init。CI Automatically obtain fixed upstream matching objects。

Passable `TEST_BUSYBOX`、`TEST_BASH`、`TEST_SHELLCHECK` Specify tools. The following is an example of the old workspace path, which must be replaced when changing the environment.：

```console
PATH=/home/agent/.cargo/bin:$PATH \
TEST_BUSYBOX=/workspace/scratch/rustaller-tools/usr/bin/busybox \
TEST_SHELLCHECK=/workspace/scratch/rustaller-tools-current/usr/bin/shellcheck \
cargo xtask check
```

examine/Do not modify the source code during the build; only add validation for failures or new changes. Untracked source code must not be used as garbage git clean/reset Clear。

## Function entrance

| Function | Code entry | current boundary |
|---|---|---|
| English multi-level TUI、ASCII Title, draft by target, return to save/throw away | src/tui/；bin/rustaller.rs | share CLI Verification, credential masking, result return cannot be automatically retried |
| Host check with kexec | host/preflight.rs、handoff.rs | systemd/openrc-init PID 1；No two restarts and fallback |
| Live Module, installation and scheduling | worker/install.rs | latest module/Service repair to be completed VM Retest |
| Four target installation with minimal package policy | targets/install.rs、packages.rs、debian.rs、rolling.rs | There is no successful acceptance of the complete installation; we cannot promise that the final dozens of MB |
| Native ssh-keygen | host/ssh_keygen.rs、TUI | Call system tools, configurable rounds/cipher/type/comment，Public key serialization only |
| DNS/Network and optional swapfile | shared/configure.rs、network.rs、swapfile.rs | preset/Custom Submit only selected values；swap New goals only XFS/ext4 |
| Original hostname and privacy root Password input | host/hostname.rs、password.rs | Handover after testing of the original system; non- root Required for installation，root SSH Still disabled |
| Optional CachyOS | targets/cachyos.rs | Fixed official scripts with keys、ISA Warehouse, independent first boot update service |
| ISO/netboot.xyz/reset | host/iso.rs、boot.rs、args.rs、TUI | Rust The actual handover is not connected and must not be described as available |
| Development and VM acceptance | xtask/src/ | Not distributed with the target runtime |

## P0：Retest Live Entrance

SeaBIOS/virtio-blk、OVMF/virtio-scsi Already actual kexec Enter Live，None have completed the installation。UEFI of SecureBoot Variables mistakenly intercepted and readable efivarfs is indeed missing and not mounted in/Unreadable。

BIOS Two startup failures were found. The source code has been repaired but a new round of complete re-inspection is missing.：

1. OpenRC Log redirection missing background mode；`host/handoff.rs::LIVE_SERVICE` Added `command_background=yes`、pidfile、`need modloop`。
2. stock modloop Search exclusions RAM root; current source `worker/install.rs::prepare_live_modules` Require root、Alpine、`rustaller_live=1` and memory root, read-only mount authentication squashfs，Check and build the module after running the kernel/Firmware link. serial port console put on cmdline at last。

Next step：

1. Restart from current source code bundle，Run first Alpine BIOS/virtio-blk，Run again UEFI/virtio-scsi。
2. Lack `RUSTALLER_LIVE_START` time check modloop/rustaller Serve、APK cover、`/lib/modules`、findmnt、`/var/log/rustaller.log` and serial port。
3. Confirm module, storage/NIC、recover SSH Then locate the first failed command; repair and rerun the corresponding case, and add regression verification for the actual failure.。

The old evidence is in the original workspace `/workspace/scratch/virt-audit-20261029/`（Contains `alpine-bios-console.ppm`、BIOS/EFI Log), not distributed with the source code; the new environment must rebuild the image and evidence。

## P1：Complete installation and virtual hardware acceptance

```console
cargo xtask vm-acceptance --help
cargo xtask vm-acceptance install \
  --tools /absolute/tools-root \
  --base /absolute/source-prepared.qcow2 --sha512 VERIFIED_SOURCE_SHA512 \
  --key /absolute/test-key --public-key /absolute/test-key.pub \
  --installer /absolute/rustaller \
  --target alpine --firmware both --disk-bus virtio-blk \
  --timeout 1800 --output /absolute/new-results-directory
```

Use after authenticating the source image prepare-source；Create a separate directory each time、qcow2 overlay、OVMF Variable copy, no host block device, inheritance proxy/CA and keep TLS。For detailed interfaces, see [ACCEPTANCE.md](ACCEPTANCE.md)。

Covering four systems × BIOS/UEFI，Expand again virtio-scsi、SATA、NVMe、PVSCSI；The source kernel must also support the controller。TCG Device simulation is not a substitute for reality KVM/ESXi/Hyper-V/Xen HVM acceptance。

Each case must be seen Live/start and install-complete、dispatch back QMP Exactly once RESET、New disk starts, correct distro/hostname/kernel、public key SSH、Selected file system/UUID fstab、GRUB/UEFI fallback、DNS/default routes, native package-metadata refresh and target-boot activation swapfile。IPv6-only、many NIC、Special routing and truth 4Kn Check separately。

New configurations are subject to acceptance: each explicit kernel × XFS/ext4 × BIOS/UEFI，source hostname inherit/Missing, not root Users and root Password and disable root SSH，public/Customize DNS Parsing after switching and restarting。CachyOS Verify baselines separately/v3/v4/Applicable znver4、Warehouse signature, first upgrade successful/Network failure retry, updated kernel/GRUB and no additional automatic restarts; there is no complete set of these new cases VM acceptance evidence。

Failure coverage identity change/Multiple disks with the same identity and mounting/swap Occupied, read-only disk, low RAM、download/Signing failed, driver missing、GRUB/SSH Verification failed. Rejection conditions before liquidation may not be postponed, and failed installations may not be eventually restarted.。

UEFI for 64 Bit、Secure Boot closure，512 MiB FAT32 ESP、single sector cluster、removable/no-nvram、PE Header verification. reject BIOS/4Kn、32-bit EFI、Xen PV/PVH/control domain、Containers and detected restrictions confidential VM。External kernel launch and provider firmware fallback policies must be adapted separately。

## P2：remaining functionality

- ISO：Enable read-only preflight and media verification、GRUB loopback、Safe handover and rollback; no universal commitment ISO，Not mixed with automatic liquidation。
- netboot.xyz/reset：Complementary execution and failure recovery to prevent accidental automatic installation。
- minimal：Measuring root occupancy and startup RAM，Then cut according to the native dependencies and retain SSH、network、CA、Module, boot, and package maintenance capabilities。
- Release: fixed version download and verification; keep preview before complete installation and acceptance, not made public draft Not used as an anonymous download portal。

## Engineering and Delivery Requirements

obey `AGENTS.md`：Four goals unified Live，RAM lower limit 512/512/1024/2048 MB Double check, complete identity confirmation, no universal `--yes`，plan 0600，Credentials are not written cmdline，The signature cannot be downgraded and will not be restarted if it fails.。TUI only host，Development and orchestration use Rust；reserve Shell Four resources and upstream Git Subject。

Update the version in the acceptance record/firmware/Controller, results, first failure and evidence location, updates P0/P1。Package source code and SHA-256 Exclude products, images, logs and private keys from time to time; do not exclude them without explicit user request commit/push/release。

Reference project review fixed fstab ordering and fresh-target hosts、Hyper-V PCI module and add GRUB XFS identify/Grammar check, see [REFERENCE-REVIEW.md](REFERENCE-REVIEW.md)；No new additions VM Installation pass evidence。
