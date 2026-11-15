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
| Gentoo rolling | systemd stage3, Portage, dracut, Limine | auto/standard (`gentoo-kernel-bin`) |

No distribution version argument is accepted. All targets install from Alpine
Live with Limine 9.6.5. XFS is default; ext4 and Btrfs are selectable.

## Disk layout and boot

| Running firmware/disk | Table | Partitions |
| --- | --- | --- |
| BIOS, below 2 TiB, 512-byte sectors | MBR | 1: FAT32; 2: root |
| UEFI x86_64, 512/4096-byte sectors | GPT | 1: FAT32 ESP; 2: root |
| BIOS, at least 2 TiB, 512-byte sectors | GPT | 1: 1 MiB BIOS stage2; 2: FAT32; 3: root |

The FAT32 partition is 512 MiB by default and mounted at `/efi` in every target.
Use `--boot-size-mib 1024` (512–4096) for large initramfs or multiple kernels.
The first MiB is reserved for alignment/BIOS embedding; GPT BIOS stage2 uses its
own partition. Root uses the remaining space, and native `/boot` stays on root.
The geometry check rejects read-only/unknown sectors, BIOS/4Kn and insufficient
space before erasure. The 512 MiB root formatting floor is not a distribution
size recommendation; leave ample disk space for packages and updates.

UEFI installs `/efi/EFI/BOOT/BOOTX64.EFI` without NVRAM entries. BIOS runs the
pinned native Limine installer against the identity-bound whole disk, with the
explicit GPT stage2 partition when needed. `/efi/limine.conf` is authoritative;
serial Linux console remains `ttyS0,115200n8`. Firmware honoring the fallback
path and serial behavior require real boot testing.

Btrfs uses an `@` root subvolume and zstd:3 by default. Select compression 0–15
with `--btrfs-compression` (0 disables it). Btrfs swapfiles are explicitly
rejected; XFS/ext4 swapfiles remain supported. No snapshot manager is installed.

## Build and inspect

```console
cargo xtask bundle
./target/x86_64-unknown-linux-musl/release/rustaller --help
./target/x86_64-unknown-linux-musl/release/rustaller debian --inspect
```

A prebuilt release entry is `rustaller-x86_64-linux-musl`. Obtain its matching
`SHA256SUMS`, verify the downloaded binary, then run the saved executable; release
packaging and draft status are explained in [development](DEVELOPMENT.md).

`--inspect` performs read-only preflight, displays detected host and computed target
panels (including unknown facts), and reports disk identity and the exact
confirmation token. Actual dispatch requires that full token, never a generic
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
- `--force-boot-mode bios|efi` asserts the detected mode; a mismatch is rejected.
- `--ssh-port`, `--web-port` and `--frpc-config` configure installation access/logs.
- `--minimal`, `--with-tools`, `--keep-docs` and `--keep-package-cache` control
  package policy; `--swapfile-mib` and `--swappiness` are opt-in target settings.

See [configuration](CONFIGURATION.md) for ranges and compatibility. Installation
failure retains Alpine Live rescue. Runtime acceptance remains listed in
[testing](TESTING.md).
