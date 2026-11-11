# Disposable BIOS / UEFI acceptance

## XFS / dd swapfile change — 2026-11-02

Rust automatic targets now use XFS, with GRUB-compatible feature flags and an
explicit Live XFS module check before partitioning. Alpine Live and Alpine
target OpenRC remain unchanged. Optional swapfiles use GNU dd with real zero
writes, private temporary files, mkswap and one fstab entry; only native target
boot activates swap. The guest probe now requires XFS and prints free -h,
swapon --show and fstab after boot.

No new BIOS/UEFI installation VM case was run for this change. All four targets,
both firmware modes, XFS root boot, and actual XFS swap activation remain NOT
VERIFIED. Previous Live-only evidence below does not establish XFS acceptance.
Unit tests exercise real dd writes to temporary regular files, mocked inventory
and mkswap, allocation/format failures, collisions, and OpenRC service linkage.

Validation completed in this workspace: 273 workspace tests and 49 Shell tests;
Rustfmt, Clippy, ShellCheck 0.11, Bash/BusyBox ash syntax, git diff --check,
and the matching static musl host/worker bundle all passed. Logs are in
`/workspace/scratch/check.log` and `/workspace/scratch/bundle.log` (not shipped
in the source archive). These results do not replace the pending VM cases.

The VM harness is a development tool under `xtask`, separate from the distributed host and worker. It runs unprivileged QEMU with software emulation. It never attaches a host block device, changes host networking, writes host firmware variables, or reboots the development machine. Guest installation and formatting are limited to a fresh per-case qcow2 overlay and a private copy of OVMF variables.

## UEFI and virtualization audit — 2026-10-29

The Rust installer now checks these conditions before handing off or wiping a disk:

- Source EFI must report 64-bit firmware and disabled Secure Boot. The actual `SecureBoot` UEFI global variable is read; firmware without that variable is accepted only when an accessible efivarfs mount establishes genuine absence, following Linux's `EFI_NOT_FOUND` policy. An unmounted variable directory, unreadable state or malformed variable stops preparation; expose efivarfs read-only if it was not mounted. CPU architecture and kernel lockdown alone are insufficient. The installer does not disable Secure Boot, enroll keys or install a signed shim chain.
- The firmware mode and bitness are checked again in Live. EFI runtime variables can be unavailable after kexec, so the Secure Boot check is made in the source and repeated immediately before handoff.
- Container marker fallbacks cover Docker, Podman, systemd containers, OpenVZ and PID 1's `container=` environment even without `systemd-detect-virt`. Xen PV/PVH, unknown Xen guest modes and control domains are rejected. Xen HVM passes this gate but still needs the ordinary kexec/storage/network checks. Detected SEV-SNP/TDX guest devices are rejected pending confidential-VM acceptance; absence of such a device is not proof that every confidential-computing mode was excluded.
- `lsblk` must report a writable disk and known logical sector size, both in the source and immediately before Live formatting. 512-byte sectors are accepted; 4096-byte logical sectors are accepted only with EFI. BIOS on 4Kn is deliberately unsupported by this installer.

Boot and storage changes:

- EFI layouts use a 512 MiB GPT ESP (`1MiB` to `513MiB`), the ESP flag and `mkfs.fat -F 32 -s 1`. The previous 100 MiB/default-FAT layout could produce FAT16 and did not accommodate FAT32's minimum cluster count on 4Kn. This reserved capacity is separate from minimal root filesystem size.
- Target kernel/initramfs files must be nonempty regular files. The fallback `EFI/BOOT/BOOTX64.EFI` is checked for an x86_64 PE32+ EFI application header. GRUB remains `--removable --no-nvram`; stale provider boot entries and firmware fallback policies still require actual platform acceptance.
- Alpine mkinitfs explicitly includes ATA, SCSI, VirtIO, NVMe, Xen PCI/front-end and Hyper-V bus module paths, resolving native module dependencies. Debian explicitly uses `MODULES=most`. Arch skips Live-based `autodetect` during its final mkinitcpio rebuild. Gentoo retains `dracut --no-hostonly`. These settings include drivers supplied by the selected kernel; they cannot manufacture drivers omitted from a cloud kernel.
- Partition refresh removes device links only for the bound target disk and its partitions. Other data disks, similar device names, ordinary files and symlink targets remain intact.
- The private Alpine apkovl disables tiny-cloud so an attached provider/NoCloud `cidata` disk cannot take over the installer's configuration.

Platform boundaries:

| Environment | Current policy | Acceptance needed |
|---|---|---|
| KVM/QEMU, Proxmox guests, OpenStack guests | Conditional on actual firmware, disk, kexec and network facts | Real KVM acceleration and provider networks; TCG is not KVM acceptance |
| VMware / ESXi | Same gates; SCSI initramfs path includes kernel-provided PVSCSI | ESXi firmware, PVSCSI, vmxnet3 and one-reboot handoff |
| Hyper-V generation 1/2 | BIOS or 64-bit EFI with Secure Boot disabled; kernel-provided VMBus/storvsc drivers | Real Hyper-V storage/network, generation 2 firmware and kexec |
| Xen HVM | Conditional | HVM firmware, xen-blkfront/xen-netfront and kexec |
| Xen PV/PVH, Xen control domain, containers | Rejected | Separate provider-aware boot workflow would be necessary |
| Confidential guests / microVMs / provider-supplied kernels | Known confidential guest markers rejected; other boot contracts not established | No general disk-boot or one-reboot guarantee |

Guest-side inspection cannot establish a provider's next-boot policy. A provider that supplies the kernel outside the disk, changes firmware or controllers at restart, or never tries the fallback EFI path is outside the accepted workflow. Retaining a recovery console is still necessary.

Regression validation: 271 workspace tests and 49 Shell baseline tests passed, plus Rustfmt, Clippy with warnings denied, ShellCheck, Bash/ash syntax and diff checks. The unchanged Shell comparison trace records three intentional EFI deviations: ESP size, ESP flag and FAT32 formatting. The new controller selector is `--disk-bus virtio-blk|virtio-scsi|sata|nvme|pvscsi`; confirmation binds `/dev/vda`, `/dev/sda` or `/dev/nvme0n1` accordingly, and every case records its emulated hardware.

### Actual VM findings in this audit

The initial preview.8 UEFI/virtio-scsi run refused firmware without a SecureBoot variable. After the mounted-efivarfs distinction was fixed, preview.9 passed source inspection and reached Live under OVMF/virtio-scsi. The BIOS/virtio-blk run also completed actual kexec into Live. Neither completed target installation: BIOS was stopped for diagnosis; the UEFI case timed out. No complete-install pass is claimed.

The BIOS VGA console identified two concrete startup faults: OpenRC rejected `output_log`/`error_log` without background execution, and the stock Alpine modloop service excluded the RAM root when looking for its image. The current source enables background execution with a PID file, makes the installer depend on modloop, and supplies a native OpenRC module service invoking Rust `live-modules`. That helper requires the private Live marker, root and an in-memory root filesystem, mounts the authenticated local squashfs read-only, verifies the running kernel release and installs module/firmware links. Serial console is now last on the Live kernel command line so init errors are visible remotely. These final startup fixes passed static checks but **have not completed a new full VM installation run**.

Evidence: `/workspace/scratch/virt-audit-20261029/`, including `alpine-bios-console.ppm`, separate BIOS and EFI serial/QMP logs, and `check-source-final.log`. These machine-specific files and VM images are excluded from the source distribution. All audit VMs have stopped. Four-target BIOS/UEFI acceptance, actual KVM/ESXi/Hyper-V/Xen HVM, and real 4Kn boot remain pending.

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

**Firmware / Live smoke is not complete OS installation acceptance.** A successful smoke run cannot change `installer_complete` or justify a stable release.

Status reviewed on 2026-10-28: `install-alpine-bios-v5b/alpine-bios/serial.log` records kexec, loading the private apkovl, successful Live package installation and the Alpine login prompt. No `RUSTALLER_INSTALL_COMPLETE` appeared, and the case timed out. The earlier v5 attempt failed before installer execution because `/run` was noexec; the harness now uses private `/var/tmp` for executable files. These are separate failures. Preview.7 contains the subsequent package and structural fixes but has not passed a new complete VM run. All guest processes from these attempts have stopped; failed overlays and logs remain available under `/workspace/scratch/rustaller-vm-tools/`.

Actual smoke evidence is under `/workspace/scratch/rustaller-vm-tools/smoke-20261026-0654/`: separate serial logs, QEMU diagnostics, QMP events and `results.txt`. Both firmware modes reached `Welcome to Alpine Linux 3.24`, kernel `6.18.58-0-virt`, x86_64 and a serial login prompt. The harness did not use QEMU `-kernel`; SeaBIOS/syslinux and OVMF/GRUB loaded the ISO normally. Only boot-loader command lines were remastered to add `console=ttyS0,115200`; the vendor kernel, initramfs and userspace were unchanged.

Tooling: Debian QEMU `10.0.13`, SeaBIOS `1.16.3`, OVMF `2025.02`, xorriso `1.5.6`, provisioned user-locally from APT-authenticated Debian repositories. No KVM device is available, so tests use TCG and run more slowly than hardware virtualization.

## Input authentication

Alpine smoke input:

- Official ISO: `https://dl-cdn.alpinelinux.org/alpine/v3.24/releases/x86_64/alpine-virt-3.24.3-x86_64.iso`.
- Its detached OpenPGP signature was actually verified against primary fingerprint `0482D84022F52DF1C4E7CD43293ACD0907D9495A`, also advertised by Alpine's official download page.
- ISO SHA-256: `cdc1bc0efc64d8d611bf3733f54868172a545888dfd726f518bf279bcb0ff1af`.

Disposable source-system image for kexec testing:

- `https://cloud.debian.org/images/cloud/trixie/20261021-2638/debian-13-genericcloud-amd64-20261021-2638.qcow2`.
- SHA-512: `16e0965312c56feb331049d45d2b1b5778f3f942a33d4300eedbb97527707f2bea87c0006f3e4cc7a25fadbff739c4e235a51fb9bcd1b66fd554f7f8e80ba45c`.
- The image was fetched over certificate-validated HTTPS and its actual bytes matched Debian's published SHA512SUMS, also fetched over HTTPS. This is **TLS and publisher-checksum authentication, not an OpenPGP signature**. Debian's [official Cloud FAQ](https://cloud.debian.org/images/cloud/) explicitly states that current per-distribution images use TLS/checksums; detached checksum signatures exist for legacy OpenStack images. `SHA512SUMS.sign` for this current image returned 404.

The cloud image is a disposable development source host. It is not a target installation image or a replacement for the installer's signed Alpine/Arch/Debian/Gentoo package and release verification.

The harness checks file hashes supplied by the caller; it does not establish the origin of an arbitrary caller-supplied hash. Obtain that value from authenticated publisher metadata first. Input paths must be nonempty regular files, not devices or symlinks; source qcow2 images with an existing backing chain are rejected.

## Commands

After building the project, inspect the interface with:

```console
cargo xtask vm-acceptance --help
```

Example smoke run with user-local tools:

```console
cargo xtask vm-acceptance smoke \
  --tools /absolute/path/to/extracted-tools-root \
  --iso /absolute/path/to/alpine-virt-3.24.3-x86_64.iso \
  --sha256 cdc1bc0efc64d8d611bf3733f54868172a545888dfd726f518bf279bcb0ff1af \
  --firmware both --timeout 240 \
  --output /absolute/path/to/new-smoke-results
```

`prepare-source` boots the authenticated Debian Cloud image with native NoCloud configuration, a test-only SSH public key, required host tools, and the inherited proxy/CA transport. It powers off that guest and creates a standalone source image plus a new checksum. A new output directory is mandatory:

```console
cargo xtask vm-acceptance prepare-source \
  --tools /absolute/path/to/extracted-tools-root \
  --base /absolute/path/to/debian-cloud.qcow2 --sha512 VERIFIED_SHA512 \
  --key /absolute/path/to/test-private-key \
  --public-key /absolute/path/to/test-public-key.pub \
  --timeout 1500 --output /absolute/path/to/new-source-results
```

After the static installer bundle is ready, full installation acceptance is explicit:

```console
cargo xtask vm-acceptance install \
  --tools /absolute/path/to/extracted-tools-root \
  --base /absolute/path/to/source-prepared.qcow2 --sha512 PREPARED_SOURCE_SHA512 \
  --key /absolute/path/to/test-private-key \
  --public-key /absolute/path/to/test-public-key.pub \
  --installer /absolute/path/to/reinstall-x86_64-linux-musl \
  --target all --firmware both --timeout 3600 \
  --output /absolute/path/to/new-install-results
```

The installer is copied into the disposable source guest. The harness invokes its read-only `--inspect`, reads the exact identity-bound confirmation token for the selected bus's disk, then passes that token through `--confirm-erase`. It does not use a generic `--yes` switch. SSH uses only the dedicated public key; password/keyboard-interactive authentication is disabled. Source and installed-target host keys use separate per-case known-host files because reinstalling legitimately replaces the guest's host key. Repeat with `--disk-bus virtio-scsi`, `sata`, `nvme` or `pvscsi` to exercise additional controllers; the source kernel must support that controller too.

## Full-case pass criteria

A full case passes only if all of these are observed:

1. The source guest boots using the selected firmware and accepts the dedicated public SSH key.
2. The real host installer dispatches the confirmed installation; serial output reaches `RUSTALLER_LIVE_START` and `RUSTALLER_INSTALL_COMPLETE`.
3. QMP records exactly **one** machine reset after dispatch. kexec itself must not cause a firmware reset; the final reboot does.
4. The installed disk boots to the requested hostname and distribution; public-key-only SSH works again.
5. The static guest probe verifies x86_64, the selected cloud/virt kernel where applicable, XFS root, UUID-bound fstab, GRUB and the UEFI fallback loader where applicable.
6. IPv4 has a default route, DNS resolves an official repository host, and the native package manager successfully refreshes metadata.
7. The explicitly requested 64 MiB swapfile is activated by target boot, has 0600 permissions and one fstab entry, and boot applies swappiness=10.

The probe does not install or reboot the host, and does not execute an arbitrary script. Native guest programs are invoked through Rust-controlled arguments. IPv6 Internet connectivity and provider-specific hardware/network layouts require separate acceptance; an IPv4/QEMU result does not establish them.

The environment's inherited HTTP proxy and public CA bundle are preserved. A Rust TCP relay forwards guest proxy requests only to that inherited proxy, without bypassing policy or disabling TLS. Private proxy environment/configuration remains in private files; public CA certificates must be readable by the native `_apt` download sandbox. Post-boot metadata probes copy a public CA bundle into the disposable guest and use a private transport configuration. The production installer removes temporary installer transport configuration according to its own lifecycle.

Every case retains serial/QMP/SSH/probe diagnostics and a result. Large disposable disk overlays are removed only after recording the result; source images, logs and OVMF variable copies are retained. Failed or unrun cases remain labelled accordingly.

## XFS VPS follow-up

EFI Stub was cancelled by the user; GRUB BIOS/UEFI remains the execution path.
Added a pre-wipe root-size floor accounting for ESP/alignment, mkfs.xfs -K to
skip whole-volume discard during formatting, and swap allocation block-count
validation after dd sync. Capacity parsing runs with LC_ALL=C.
Validation: 275 workspace + 49 Shell tests, formatting/lints/syntax checks and
static musl bundle passed. Logs: /workspace/scratch/xfs-check.log and
/workspace/scratch/xfs-bundle.log. No additional VM installation was run.

## Reference review follow-up

Reviewed bohanwood/debi and bin456789/reinstall via GitHub; see
REFERENCE-REVIEW.md for inspected blob IDs and scope. Corrected target fstab
ordering before final initramfs generation, fresh-target hostname resolution,
and the Alpine Hyper-V PCI module list. Added final native GRUB filesystem and
configuration checks. 277 workspace + 49 Shell tests, all lint/syntax checks,
and matching musl host/worker bundle passed. Logs: reference-check.log and
reference-bundle.log under /workspace/scratch. No new VM installation evidence;
previous outstanding acceptance cases remain outstanding.
