# Disposable BIOS / UEFI acceptance

## XFS / dd swapfile change — 2026-11-02

Automatic roots use XFS with GRUB-compatible flags and a pre-partition Live module check. Alpine retains OpenRC. Optional swapfiles use GNU dd zero writes, private files, mkswap and one fstab entry; only target boot activates them. The guest probe requires XFS and prints free -h, swapon --show and fstab.

**No complete installation VM case passed for this change.** All four targets, both firmware modes, XFS boot and swap activation remain NOT VERIFIED. Live-only evidence below does not establish acceptance.

Historical validation: 273 workspace + 49 Shell tests, formatting/lints/syntax and matching static musl bundle passed. Unit cases cover real dd writes to temporary files, mocked inventory/mkswap, failures, collisions and OpenRC links. Logs: `/workspace/scratch/check.log`, `bundle.log`; not shipped with source.

The `xtask` harness uses unprivileged QEMU/TCG, fresh per-case qcow2 overlays and private OVMF variables. It never attaches host block devices, changes host networking/firmware or reboots the development host.

## UEFI and virtualization audit — 2026-10-29

Before handoff/wipe:

- Source EFI must be 64-bit with Secure Boot disabled. Missing `SecureBoot` is accepted only on accessible efivarfs establishing genuine absence (`EFI_NOT_FOUND`); unmounted, unreadable or malformed state stops preparation. Check again before handoff; Live rechecks firmware mode/bitness because variables may be unavailable after kexec. No Secure Boot disabling, key enrollment or signed shim is provided.
- Reject containers (including marker/PID 1 fallbacks), Xen PV/PVH, unknown modes/control domains and detected SEV-SNP/TDX guests. Xen HVM still needs normal kexec/storage/network checks; missing confidential-VM markers do not prove universal compatibility.
- Require a writable disk with known logical sectors in source and immediately before formatting: 512-byte accepted, 4096-byte only under EFI; BIOS/4Kn rejected.

Boot/storage requirements:

- GPT ESP: 512 MiB (`1MiB`–`513MiB`), ESP flag, `mkfs.fat -F 32 -s 1`. This replaces the undersized/default-FAT layout and accommodates FAT32 clusters on 4Kn; ESP capacity is separate from root size.
- Kernel/initramfs must be nonempty regular files; `EFI/BOOT/BOOTX64.EFI` must be x86_64 PE32+ EFI. GRUB uses `--removable --no-nvram`; provider fallback behavior still needs testing.
- Alpine includes ATA/SCSI/VirtIO/NVMe/Xen/Hyper-V module paths; Debian uses `MODULES=most`, Arch skips Live-based `autodetect`, Gentoo uses `dracut --no-hostonly`. Configuration cannot supply drivers absent from the target kernel.
- Partition refresh removes only bound-target device links. Private apkovl disables tiny-cloud to prevent provider/NoCloud `cidata` overriding installer configuration.

| Environment | Current policy | Acceptance needed |
|---|---|---|
| KVM/QEMU, Proxmox guests, OpenStack guests | Conditional on actual firmware, disk, kexec and network facts | Real KVM acceleration and provider networks; TCG is not KVM acceptance |
| VMware / ESXi | Same gates; SCSI initramfs path includes kernel-provided PVSCSI | ESXi firmware, PVSCSI, vmxnet3 and one-reboot handoff |
| Hyper-V generation 1/2 | BIOS or 64-bit EFI with Secure Boot disabled; kernel-provided VMBus/storvsc drivers | Real Hyper-V storage/network, generation 2 firmware and kexec |
| Xen HVM | Conditional | HVM firmware, xen-blkfront/xen-netfront and kexec |
| Xen PV/PVH, Xen control domain, containers | Rejected | Separate provider-aware boot workflow would be necessary |
| Confidential guests / microVMs / provider-supplied kernels | Known confidential guest markers rejected; other boot contracts not established | No general disk-boot or one-reboot guarantee |

Provider-supplied kernels, changed firmware/controllers and firmware ignoring fallback paths require separate adaptation. Retain console access.

Historical checks: 271 workspace + 49 Shell tests and lints/syntax passed. Shell parity records three intentional EFI changes: ESP size, ESP flag and FAT32 formatting. `--disk-bus virtio-blk|virtio-scsi|sata|nvme|pvscsi` binds the corresponding disk identity and records emulated hardware.

### Actual VM findings in this audit

preview.8 rejected UEFI without SecureBoot; after the efivarfs distinction, preview.9 reached Live under OVMF/virtio-scsi. SeaBIOS/virtio-blk also completed kexec into Live. BIOS stopped for diagnosis and UEFI timed out; neither completed installation.

BIOS exposed OpenRC log redirection without background mode and stock modloop excluding the RAM root. Source fixes add background/pidfile, depend on modloop, and invoke Rust `live-modules`. It requires root, Alpine, the private Live marker and memory root; mounts authenticated squashfs read-only, matches the kernel and links modules/firmware. Serial console is last. **These fixes still lack a completed fresh VM installation.**

Evidence in the original workspace: `/workspace/scratch/virt-audit-20261029/`, including `alpine-bios-console.ppm`, BIOS/EFI serial/QMP logs and `check-source-final.log`. VMs stopped; files/images are excluded from source. Real KVM/ESXi/Hyper-V/Xen HVM and 4Kn boot remain pending.

Research used primary sources:

- [UEFI 2.10, media and filesystem access](https://uefi.org/specs/UEFI/2.10/13_Protocols_Media_Access.html), especially 13.3: FAT32 system partitions and the distinction from removable FAT12/FAT16 media.
- [UEFI 2.10, boot manager](https://uefi.org/specs/UEFI/2.10/03_Boot_Manager.html), 3.5.1.1: architecture-specific fallback filenames and PE machine type.
- [libvirt Secure Boot](https://libvirt.org/kbase/secureboot.html): capable firmware, enrolled keys and actual enforcement are distinct; existing NVRAM is not changed merely by editing domain XML.
- [Linux EFI secure-boot state helper](https://github.com/torvalds/linux/blob/master/include/linux/efi.h), `efi_get_secureboot_mode`: a genuinely absent variable is distinct from failure to query firmware.
- [Linux Xen sysfs implementation](https://github.com/torvalds/linux/blob/master/drivers/xen/sys-hypervisor.c): actual PV/PVH/HVM guest-type values and domain flags.
- [Linux Hyper-V confidential-computing documentation](https://docs.kernel.org/virt/hyperv/coco.html): additional memory-sharing and kexec requirements.
- [Alpine mkinitfs](https://github.com/alpinelinux/mkinitfs/blob/master/mkinitfs.in) and its [Live init](https://github.com/alpinelinux/mkinitfs/blob/master/initramfs-init.in): feature/module dependency handling and tiny-cloud selection.
- [Arch mkinitcpio manual source](https://github.com/archlinux/mkinitcpio/blob/master/man/mkinitcpio.8.adoc): presets and skipping autodetect for the final rebuild.
## Evidence recorded on 2026-10-26

| Check | BIOS / SeaBIOS | UEFI / OVMF | Meaning |
|---|---|---|---|
| Alpine Live ISO boots to login | PASS | PASS | Firmware, boot loader, kernel, initramfs and Live userspace booted |
| Rust host → kexec → Alpine Live → Alpine target | FAIL (preview.5) | NOT RUN | kexec reached Live; target installation did not complete |
| Rust host → kexec → Alpine Live → Debian target | NOT RUN | NOT RUN | Requires the integrated installer binary |
| Rust host → kexec → Alpine Live → Arch target | NOT RUN | NOT RUN | Requires the integrated installer binary |
| Rust host → kexec → Alpine Live → Gentoo target | NOT RUN | NOT RUN | Requires the integrated installer binary |

Firmware/Live smoke cannot change `installer_complete` or justify a stable release. The v5b serial log reached Live after kexec/apkovl/package loading, then timed out without `RUSTALLER_INSTALL_COMPLETE`. Earlier v5 failed on noexec `/run`; the harness now stages executables in private `/var/tmp`. preview.7's later fixes did not establish a complete pass.

Smoke evidence: `/workspace/scratch/rustaller-vm-tools/smoke-20261026-0654/` (serial/QEMU/QMP/results). Both firmware modes reached Alpine 3.24, kernel `6.18.58-0-virt`, x86_64 login. Normal SeaBIOS/syslinux and OVMF/GRUB ISO boot was used, not QEMU `-kernel`; only console arguments changed. Historical tools: QEMU 10.0.13, SeaBIOS 1.16.3, OVMF 2025.02, xorriso 1.5.6 from authenticated Debian APT. No KVM device was available; tests used TCG.

## Input authentication

Alpine smoke input:

- ISO: `https://dl-cdn.alpinelinux.org/alpine/v3.24/releases/x86_64/alpine-virt-3.24.3-x86_64.iso`.
- Detached signature verified against official primary fingerprint `0482D84022F52DF1C4E7CD43293ACD0907D9495A`.
- SHA-256: `cdc1bc0efc64d8d611bf3733f54868172a545888dfd726f518bf279bcb0ff1af`.

Disposable source image:

- `https://cloud.debian.org/images/cloud/trixie/20261021-2638/debian-13-genericcloud-amd64-20261021-2638.qcow2`.
- SHA-512: `16e0965312c56feb331049d45d2b1b5778f3f942a33d4300eedbb97527707f2bea87c0006f3e4cc7a25fadbff739c4e235a51fb9bcd1b66fd554f7f8e80ba45c`.
- Actual bytes matched Debian SHA512SUMS over validated HTTPS. This is TLS/publisher-checksum authentication, not OpenPGP; current image checksum signatures are not supplied (`SHA512SUMS.sign` returned 404), consistent with the [Cloud FAQ](https://cloud.debian.org/images/cloud/).

The source image is a disposable development host, not a target rootfs or substitute for installer package/signature verification. The harness checks supplied hashes, not their origin: obtain them from authenticated publisher metadata. Inputs must be nonempty regular files, not symlinks/devices; qcow2 backing chains are rejected.

## Commands

Inspect the interface:

```console
cargo xtask vm-acceptance --help
```

Smoke example:

```console
cargo xtask vm-acceptance smoke \
  --tools /absolute/path/to/extracted-tools-root \
  --iso /absolute/path/to/alpine-virt-3.24.3-x86_64.iso \
  --sha256 cdc1bc0efc64d8d611bf3733f54868172a545888dfd726f518bf279bcb0ff1af \
  --firmware both --timeout 240 \
  --output /absolute/path/to/new-smoke-results
```

`prepare-source` boots the authenticated Debian image with NoCloud, dedicated SSH key, host tools and inherited proxy/CA, then powers off and writes a standalone source image plus checksum. Output directories must be new:

```console
cargo xtask vm-acceptance prepare-source \
  --tools /absolute/path/to/extracted-tools-root \
  --base /absolute/path/to/debian-cloud.qcow2 --sha512 VERIFIED_SHA512 \
  --key /absolute/path/to/test-private-key \
  --public-key /absolute/path/to/test-public-key.pub \
  --timeout 1500 --output /absolute/path/to/new-source-results
```

Full installation acceptance requires a static installer bundle:

```console
cargo xtask vm-acceptance install \
  --tools /absolute/path/to/extracted-tools-root \
  --base /absolute/path/to/source-prepared.qcow2 --sha512 PREPARED_SOURCE_SHA512 \
  --key /absolute/path/to/test-private-key \
  --public-key /absolute/path/to/test-public-key.pub \
  --installer /absolute/path/to/rustaller-x86_64-linux-musl \
  --target all --firmware both --timeout 3600 \
  --output /absolute/path/to/new-install-results
```

The harness copies it into the disposable guest, runs `--inspect`, then passes the exact bus-specific token through `--confirm-erase`, never generic `--yes`. Only the test public key is accepted; source/target known-host files are separate because reinstallation changes host keys. Repeat for additional disk buses; source kernels must support them too.

## Full-case pass criteria

1. Selected firmware boots the source and dedicated public-key SSH works.
2. Confirmed real installation reaches `RUSTALLER_LIVE_START` and `RUSTALLER_INSTALL_COMPLETE`.
3. QMP records exactly one reset after dispatch: the final reboot, not kexec.
4. The new disk boots the requested distro/hostname and public-key SSH works again.
5. Probe verifies x86_64, selected kernel, XFS root, UUID fstab, GRUB and applicable EFI fallback.
6. IPv4 default route, repository DNS resolution and native metadata refresh work.
7. Target boot activates the requested 64 MiB swapfile, 0600 with one fstab entry and swappiness=10.

The probe uses controlled native argv, not arbitrary scripts, and never installs/reboots the host. IPv6 Internet, multiple NICs and provider layouts require separate tests.

Inherited proxy and public CA remain enabled. The Rust relay forwards only to that proxy; private transport config remains private, with public CA readable by `_apt`. Production removes temporary transport configuration through its own lifecycle. TLS/policy are not bypassed.

Retain per-case serial/QMP/SSH/probe diagnostics and explicit pass/fail/unrun results. Remove large overlays only after recording results; preserve source images, logs and OVMF copies.

## XFS VPS follow-up

EFI Stub was cancelled. GRUB remains; pre-wipe capacity accounts for ESP/alignment, `mkfs.xfs -K` skips whole-volume discard, dd allocation checks block counts, and parsing uses `LC_ALL=C`. Historical 275 workspace + 49 Shell tests, lints/syntax and musl bundle passed (`/workspace/scratch/xfs-check.log`, `xfs-bundle.log`); no new installation VM case.

## Reference review follow-up

[REFERENCE-REVIEW.md](REFERENCE-REVIEW.md) records inspected upstream blobs. Fixes cover fstab order, fresh-target hosts, Alpine Hyper-V PCI and final GRUB XFS/config checks. Historical 277 workspace + 49 Shell tests and bundle passed (`/workspace/scratch/reference-check.log`, `reference-bundle.log`); no new VM acceptance evidence.
