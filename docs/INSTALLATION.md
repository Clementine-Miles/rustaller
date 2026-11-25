# Installation

**Development preview: installation erases the entire selected disk. Full
four-distribution BIOS/UEFI acceptance is pending.** Use a disposable machine,
back up data and retain an independent console/VNC path. Unit tests and a static
binary do not establish that a VPS can reinstall successfully.

## Requirements

- Linux x86_64, root, a writable whole disk and sufficient free RAM: Alpine and
  Debian 512 MiB, Arch 1024 MiB, Gentoo 2048 MiB. Swap does not lower these floors.
- Source PID 1 is systemd or openrc-init; kexec tools/kernel support must pass
  preflight. Containers, memory-backed source roots, Xen PV/PVH/control domains,
  detected restricted confidential guests and enabled Secure Boot are rejected.
- BIOS with 512-byte logical sectors, or genuine 64-bit EFI with 512/4096-byte
  sectors. BIOS/4Kn and inaccessible or ambiguous firmware state are rejected.
- Working official HTTPS repositories, correct trust material and a supported
  Ethernet policy. Automatic discovery requires one verified manager per link.
  An explicit MAC-bound policy is available for unsupported source managers and
  simple Ethernet configurations. See [configuration](CONFIGURATION.md).

| Target | Installation | Required kernel |
| --- | --- | --- |
| Alpine latest stable | APK, OpenRC, ifupdown-ng/dhcpcd | `linux-virt` |
| Debian latest stable | debootstrap minbase, APT, systemd/networkd/resolved | `linux-image-cloud-amd64` |
| Arch rolling | pacstrap/pacman, systemd/networkd/resolved, authenticated CachyOS packages | `linux-cachyos-server` |
| Gentoo rolling | amd64-nomultilib OpenRC stage3, Portage, dracut | `sys-kernel/gentoo-kernel-bin` |

All targets install from Alpine Live and use rustaller-boot. XFS is the default
root filesystem and ext4 is the alternative. The Alpine Live handoff kernel is
selected independently of the installed target. The installer checks kernel
availability, root filesystem and driver requirements, and the complete
kernel/initramfs pair. An unavailable required kernel stops installation.

## Arch repositories

CachyOS repository availability, signed databases and packages, and the
dependency solution must validate. Repository selection uses the CPU features
available inside the guest. The baseline repository supports guests that lack
x86-64-v3. Verified x86-64-v3 and v4 can use their corresponding repositories.
Zen-specific repositories are excluded. Updating Arch uses a native full package
transaction so the server kernel, modules and initramfs stay in sync.

## Gentoo stage3 and init

Only the official glibc `stage3-amd64-nomultilib-openrc` is accepted. The installer
checks the latest-stage3 metadata, release signatures and SHA-512 payload digest,
and requires the exact architecture, ABI, init, timestamped archive name and size.
The matching no-multilib OpenRC Portage profile and `ABI_X86=64` are validated.
Gentoo uses netifrc, dhcpcd/openresolv, native OpenRC runlevels, installkernel and
dracut. The binary distribution kernel uses the generic amd64 baseline.

New plans use schema 12 and a matching embedded worker. If a stored plan comes
from an earlier schema, regenerate it with the current CLI or TUI and review its
effective policy before confirming. The installer rejects earlier plan files
before erasure. Existing target boot configuration and update helpers retain
their independent compatibility contract.

## Installation footprint

Minimal is the default. It retains the native package manager, init,
networking, SSH, certificates, required kernel and initramfs, filesystem
maintenance tools and native boot hooks. Debian requests maintenance tools for
the selected filesystem. The distribution can still install other tools as
dependencies. Gentoo retains the stage3's Portage, Python and toolchain.
Use `--profile standard`, `--with-tools`, `--keep-docs` or
`--keep-package-cache` to select a different footprint.

## Disk layout and boot

| Running firmware/disk | Table | Partitions |
| --- | --- | --- |
| BIOS, below 2 TiB, 512-byte sectors | MBR | 1: FAT32; 2: root |
| UEFI x86_64, 512/4096-byte sectors | GPT | 1: FAT32 ESP; 2: root |
| BIOS, at least 2 TiB, 512-byte sectors | GPT | 1: 1 MiB BIOS stage2; 2: FAT32; 3: root |

The FAT32 partition is 512 MiB by default and mounted at `/boot` in every target.
Consider `--boot-size-mib 1024` or `2048` (512–4096) for large initramfs,
multiple kernels and retained boot generations. Actual artifact size and recovery
reserve must fit; the refresh fails safely if the selected capacity is insufficient.
The first MiB is reserved for alignment/BIOS embedding; GPT BIOS stage2 uses its
own partition. Root uses the remaining space. Distribution kernel/initramfs files
and Rustaller's generation directories reside on the FAT32 filesystem.
The geometry check rejects read-only/unknown sectors, BIOS/4Kn and insufficient
space before erasure. The 512 MiB root formatting floor is not a distribution
size recommendation; leave ample disk space for packages and updates.

UEFI installs `/boot/EFI/BOOT/BOOTX64.EFI` without NVRAM entries. BIOS runs the
release-built secondary loader on the validated whole disk. It uses the reserved
MBR gap or the dedicated GPT BIOS Boot partition. Both firmware paths use
`/boot/rustaller/boot.toml`. Kernel updates preserve the previous generation.
The TUI boot page offers detected partition sizes where both current mount
sources resolve to partitions on the selected disk. Reuse copies their sizes
in whole MiB into the new plan. Manual FAT32 and root sizes are also available.
[Boot configuration and recovery](BOOT.md)

XFS and ext4 support an optional target swapfile. Source inspection and
cancellation also work with an existing Btrfs source root where the other
preflight requirements are met.

## Build and inspect

```console
cargo xtask bundle
./target/x86_64-unknown-linux-musl/release/rustaller --help
./target/x86_64-unknown-linux-musl/release/rustaller --inspect
./target/x86_64-unknown-linux-musl/release/rustaller debian --inspect
```

A prebuilt release entry is `rustaller-x86_64-linux-musl`. Obtain its matching
`SHA256SUMS`, verify the downloaded binary, then run the saved executable; release
packaging and draft status are explained in [development](DEVELOPMENT.md).

`--inspect` displays read-only host diagnostics and a computed target preview,
including unknown or unavailable optional facts. It collects no benchmarks,
third-party scripts, online IP/provider lookups or uploads. CPU features describe
what the guest can use; hypervisors can mask the physical host's capabilities.
Diagnostic collection is separate from installation authorization: incompatible
firmware, source networking or disk identity still block installation. An eligible
preflight reports disk identity and the exact confirmation token. Actual dispatch requires that full token, never a generic
`--yes`. Interactive execution prompts for it; noninteractive execution requires
`--confirm-erase 'EXACT_TOKEN_FROM_INSPECTION'`. Identity is checked again before
handoff and in Live. Confirmation is not a substitute for backups.

## CLI and TUI

Run without arguments on an interactive terminal to open the English TUI.
Choose a distribution, configure the account/SSH, target, network and optional
settings, then review and run read-only preflight. Current/target panels are side by side
on wide terminals and stacked on narrow terminals; uninspected facts stay unknown.
Drafts are separate per selection. Save keeps a draft in
memory; discard restores its entry snapshot. Returning from an error does not
retry an action. `Esc` cancels the current edit. `Ctrl-C` exits the application.

Example of read-only installation inspection:

```console
./rustaller debian --inspect --target-disk /dev/vda --filesystem ext4 \
  --hostname my-vps --timezone Etc/UTC --dns 1.1.1.1,8.8.8.8
```

When ready to install, remove `--inspect`, supply at least one `--ssh-key` such
as `/root/client.pub`, and complete preflight and explicit disk confirmation.
Public-key sources can be inline values, absolute public-key files, HTTPS URLs,
`github:USER` or `gitlab:USER`. Repeat `--ssh-key` for multiple keys. The host
resolves and validates keys before handoff. The default `admin` account has
passwordless sudo with direct root SSH disabled. Set `--username` to choose
another account or explicitly select `--username root` for root key-only
access. The account and port apply in Live and on the installed target.
Read-only inspection and reset need no keys.

The TUI can invoke native `ssh-keygen` with a passphrase. Only its public key
enters the plan. Back up the private key to the client before erasing its disk.

## Rescue, holds and optional settings

- `./rustaller alpine --hold 1 --ssh-key /root/client.pub` enters Alpine Live rescue with SSH and does not
  partition/install a target. It still requires preflight and exact confirmation.
  The Live local and serial consoles provide an independent recovery shell.
- `--hold 0` pauses after preparation; an interactive `KEXEC` confirmation
  continues. Noninteractive hold leaves the source running. Run
  `sudo ./rustaller reset` to discard the staged installation.
- `--hold 2` retains Live after target installation; the user reboots manually.
- `--force-boot-mode bios|efi` asserts the detected mode; a mismatch is rejected.
- `--ssh-port` and `--frpc-config` configure installation access. In Alpine Live, read `/var/log/rustaller.log` locally or over the recovery SSH connection.
- `--minimal`, `--with-tools`, `--keep-docs` and `--keep-package-cache` control
  package policy; `--swapfile-mib` and `--swappiness` are opt-in target settings.

See [configuration](CONFIGURATION.md) for ranges and compatibility. Installation
failure retains Alpine Live rescue. Runtime acceptance remains listed in
[testing](TESTING.md).

### Cancel a pending installation

`rustaller reset` cancels Rustaller's own pending operation. It returns
success with no pending operation. Preparation writes only to a private
`/var/tmp/rustaller-handoff` directory. Reset removes the verified workspace
and leaves the running system available. If a staging command is active,
reset records a cancellation request and cleanup follows when the command
returns. The source partition table, bootloader, network and SSH settings
are untouched.

After handoff is initiated, reset refuses cancellation from that source
boot because Linux does not expose the loaded image's identity. Review
the native init and kexec state at the console before rebooting.

From the Alpine Live console or recovery SSH, run `/rustaller-worker reset`
as root. The default SSH account can use `sudo /rustaller-worker reset`.
Before disk erasure it marks the operation cancelled and verifies the
recorded disk identity, partition table ID and firmware mode. Confirm the
original firmware boot path at the console and reboot manually to return.
If disk erasure has started, Live reset refuses and keeps the rescue
environment available.
