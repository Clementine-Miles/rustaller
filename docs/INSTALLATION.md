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
| Debian latest stable | debootstrap minbase, APT, systemd/networkd/resolved | standard/generic (`linux-image-amd64`), cloud (`linux-image-cloud-amd64`) |
| Arch rolling | pacstrap/pacman, systemd/networkd/resolved | standard (`linux`), lts (`linux-lts`), hardened (`linux-hardened`), zen (`linux-zen`), opt-in cachyos (`linux-cachyos`) |
| Gentoo rolling | amd64-nomultilib stage3, OpenRC or systemd, Portage, dracut | auto/standard (`sys-kernel/gentoo-kernel-bin`) |

No distribution version argument is accepted. All targets install from Alpine
Live with rustaller-boot. XFS is default; ext4 and Btrfs are selectable.
`--kernel` accepts the package spellings shown above as target-specific aliases;
the older standard/generic/cloud/lts/virt/cachyos choices remain available where
valid. Alpine's `cloud` alias selects `linux-virt`. The Alpine Live handoff kernel
is selected separately from the installed target kernel. Package availability,
root-filesystem/driver evidence and a complete kernel/initramfs pair are checked
during installation; these choices do not imply completed VM boot acceptance.

## Gentoo stage3 and init

New Gentoo plans default to `--gentoo-init=openrc`; select
`--gentoo-init=systemd` explicitly for systemd. This option is rejected for other
distributions. The TUI exposes it under Gentoo's host settings and previews the
stage3, init system and 64-bit-only ABI.

| Init choice | Official stage3 category |
| --- | --- |
| OpenRC (default) | `stage3-amd64-nomultilib-openrc` |
| systemd | `stage3-amd64-nomultilib-systemd` |

Selection uses the corresponding official latest-stage3 metadata, authenticated
signed metadata and SHA-512 payload verification. Architecture, ABI, init variant,
timestamped archive name and size must match; there is no fallback across variants
or to a generic multilib, musl or x32 archive. The matching no-multilib Portage
profile and `ABI_X86=64` are validated. CPU ISA detection does not change the
generic amd64 optimization baseline or the no-multilib ABI policy.
OpenRC uses Gentoo netifrc, dhcpcd/openresolv, native OpenRC runlevels and SSH;
systemd retains networkd/resolved and native units. Both share the binary
distribution kernel, dracut, rustaller-boot and supported root filesystems. No source
kernel compilation or v3-specific optimization is selected automatically.

The new plan schema is 10 and uses a matching worker runtime. Older plans are
rejected instead of reinterpreting their former systemd default as OpenRC.

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

Btrfs uses an `@` root subvolume and zstd:3 by default. Select compression 0–15
with `--btrfs-compression` (0 disables it). Btrfs swapfiles are explicitly
rejected; XFS/ext4 swapfiles remain supported. No snapshot manager is installed.
FAT32 `/boot` is outside Btrfs snapshots: a root snapshot restore does not restore
matching kernels/initramfs, which must be restored and verified separately.

## Build and inspect

```console
cargo xtask bundle
./target/x86_64-unknown-linux-musl/release/rustaller --help
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
- `--ssh-port` and `--frpc-config` configure installation access. In Alpine Live, read `/var/log/rustaller.log` locally or over the recovery SSH connection.
- `--minimal`, `--with-tools`, `--keep-docs` and `--keep-package-cache` control
  package policy; `--swapfile-mib` and `--swappiness` are opt-in target settings.

See [configuration](CONFIGURATION.md) for ranges and compatibility. Installation
failure retains Alpine Live rescue. Runtime acceptance remains listed in
[testing](TESTING.md).
