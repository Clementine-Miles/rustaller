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
- Working official HTTPS repositories, correct trust material and supported,
  unambiguous source networking. Configuration ownership and routing are checked;
  an address marked `dynamic` is not proof of DHCPv6 or SLAAC policy. Unsupported
  managers/topologies stop preflight. Current automatic network discovery requires
  an applied, explicit, flat systemd-networkd profile and no competing manager;
  ifupdown/NetworkManager source policies are unsupported. See
  [configuration](CONFIGURATION.md) for the exact constraints.

| Target | Installation | Kernel choices |
| --- | --- | --- |
| Alpine latest stable | APK, OpenRC, ifupdown-ng/dhcpcd | standard/lts (`linux-lts`), virt (`linux-virt`) |
| Debian latest stable | debootstrap minbase, APT, systemd/networkd/resolved | standard/generic, cloud |
| Arch rolling | pacstrap/pacman, systemd/networkd/resolved | standard, lts, opt-in cachyos |
| Gentoo rolling | systemd stage3, Portage, dracut, GRUB | auto/standard (`gentoo-kernel-bin`) |

No version argument is accepted. All targets install from Alpine Live. XFS is
the default root filesystem; ext4 is optional. EFI has a 512 MiB FAT32 ESP.
GRUB is retained for both firmware modes; EFI Stub is not implemented.

## Build and inspect

```console
cargo xtask bundle
./target/x86_64-unknown-linux-musl/release/rustaller --help
./target/x86_64-unknown-linux-musl/release/rustaller debian --inspect
```

A prebuilt release entry is `rustaller-x86_64-linux-musl`. Obtain its matching
`SHA256SUMS`, verify the downloaded binary, then run the saved executable; release
packaging and draft status are explained in [development](DEVELOPMENT.md).

`--inspect` performs read-only preflight and reports disk identity and the exact
confirmation token. Actual dispatch requires that full token, never a generic
`--yes`. Interactive execution prompts for it; noninteractive execution requires
`--confirm-erase 'EXACT_TOKEN_FROM_INSPECTION'`. Identity is checked again before
handoff and in Live. Confirmation is not a substitute for backups.

## CLI and TUI

Run without arguments on an interactive terminal to open the English TUI.
Choose a distribution, configure the account/SSH, target, network and optional
settings, then review. Drafts are separate per selection. Save keeps a draft in
memory; discard restores its entry snapshot. Returning from an error does not
retry an action. `Esc` cancels the current edit; `Ctrl-C` exits the application. Credential fields are masked.

Example of read-only installation inspection:

```console
./rustaller debian --inspect --target-disk /dev/vda --filesystem ext4 \
  --kernel generic --username admin --ssh-key /root/client.pub \
  --hostname my-vps --timezone Etc/UTC --dns 1.1.1.1,8.8.8.8
```

Remove `--inspect` only when ready for preflight, private credential prompts and
explicit disk confirmation. Public-key sources may be inline values, absolute
files, HTTPS URLs, `github:USER` or `gitlab:USER`; the host resolves and validates
keys before handoff. Password and public-key authentication are mutually
exclusive. Non-root users require a separate root password; root SSH stays
closed. Command-line passwords can appear in process listings/history; use the
interactive private prompts where possible.

The TUI can invoke native `ssh-keygen` with a passphrase. Only its public key
enters the plan. Back up the private key to the client before erasing its disk.

## Rescue, holds and optional settings

- `./rustaller alpine --hold 1` enters Alpine Live rescue with SSH and does not
  partition/install a target. It still requires preflight and exact confirmation.
- `--hold 0` pauses after preparation; an interactive `KEXEC` confirmation
  continues. Noninteractive hold leaves the source running.
- `--hold 2` retains Live after target installation; the user reboots manually.
- `--force-boot-mode bios|efi` selects a compatible layout, not different firmware.
- `--ssh-port`, `--web-port` and `--frpc-config` configure installation access/logs.
- `--minimal`, `--with-tools`, `--keep-docs` and `--keep-package-cache` control
  package policy; `--swapfile-mib` and `--swappiness` are opt-in target settings.

See [configuration](CONFIGURATION.md) for ranges and compatibility. Installation
failure retains Alpine Live rescue. Runtime acceptance remains listed in
[testing](TESTING.md).

## netboot.xyz and Local ISO

These routes reboot through the existing GRUB without partitioning or formatting.
They require Linux x86_64, root, supported BIOS/64-bit EFI with Secure Boot
disabled, and one inspected GRUB under `/boot/grub` or `/boot/grub2`. Its
1024-byte environment must be on plain ext2/3/4 without encryption/LVM/RAID;
the generated configuration must load and clear `next_entry` and source adjacent
`custom.cfg`. Required platform modules and native GRUB tools must exist.
The operator must explicitly attest that this GRUB is the firmware's active
bootloader; files alone cannot prove BIOS disk selection. Other loaders fail
preflight. Firmware mode cannot be switched. No NVRAM writes are made.

```console
./rustaller netboot.xyz --inspect
./rustaller iso --iso /root/linux.iso --inspect
./rustaller iso --iso /root/linux.iso --iso-kernel /casper/vmlinuz \
  --iso-initrd /casper/initrd --iso-cmdline 'boot=casper iso-scan/filename=%ISO_PATH%' --inspect
```

Review the output, then rerun without `--inspect`; noninteractive callers use
`--confirm-handoff 'EXACT_BOOT-ONCE_TOKEN'`. This interrupts connectivity and
reboots. `--confirm-erase` is reserved for installation. TUI routes use the
same implementation, with read-only preflight before native confirmation.

netboot.xyz downloads the official HTTPS iPXE artifact and checksum manifest,
checks SHA-256 and the firmware-specific image format, then uses BIOS
`linux16` or UEFI `chainloader`. iPXE needs its own working NIC/network/DHCP;
source static networking is not transferred into iPXE.

Local ISO reuses read-only loop inspection. Media must expose its own
`/boot/grub/loopback.cfg` (or an explicit config path), or the supplied Linux
kernel and initrd paths. Windows layouts, encrypted storage, unsafe paths and
boot files escaping the mount are rejected. `%ISO_PATH%` and `%ISO_UUID%`
are supported substitutions. Root must trust the ISO, including its GRUB code;
the SHA-256 confirmation detects changes, not publisher authenticity. These
are media-specific Linux boot contracts, not universal ISO compatibility.

The original default entry is retained. The new entry is visible only when the
one-shot default selects it. GRUB consumes `next_entry` before entering media.
Preparation failures restore prior custom configuration/environment where writes
occurred; rollback failures retain private backups and an explicit error.
A killed process after arming can leave a one-shot request. From a rescue console,
use the matching native `grub-editenv /boot/grub/grubenv unset next_entry`
(adjust grub2 paths), read `rustaller-media/state.before` for original presence/modes, restore
`custom.before` if originally present or remove
the newly created custom file, and inspect/remove only the private
`rustaller-media` directory. Do not remove an outstanding ISO mount recursively.
Successful boots leave guarded staging/backups for manual cleanup after returning
to the original OS. BIOS/UEFI media acceptance is still unverified.
