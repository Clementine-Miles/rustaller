# rustaller — Personal VPS reinstall tool (Linux-only)

A scoped derivative of [bin456789/reinstall](https://github.com/bin456789/reinstall), based on
`98f1e21013233d42750cc2e7a298a0818461d686`. rustaller is licensed under GNU GPLv3 (`GPL-3.0-only`); see [LICENSE](LICENSE). Upstream source notices and attribution are retained.

The Rust/musl rewrite has an optional lightweight ratatui interface with hierarchical configuration menus. **Host preparation, authenticated Alpine Live, kexec handoff and the four-target installation driver are connected and include real disk-erasing actions. Full BIOS/UEFI installation acceptance is still in progress; releases remain development previews.**
The English TUI selects a system before opening its account/SSH, host, kernel, network, DNS, minimal-profile and optional target-swapfile settings. It also provides a separate current-system route, native ssh-keygen, and Save/Discard/Cancel when returning from a changed directory. See the [TUI guide](docs/TUI.md).
The [configuration guide](docs/CONFIGURATION.md) explains kexec into Live followed by one final reboot. See [minimal profiles](docs/MINIMAL.md) for package scope and size limitations.
See the [source audit](docs/SOURCES.md) for official artifact/repository checks and [target configuration](docs/POSTINSTALL.md) for official kernel, hostname and timezone options. Installation uses native signature verification and checked target directories; a metadata audit does not establish installation success.
See the [GitHub release guide](docs/GITHUB-RELEASE.md) for builds, CI, release assets, curl startup and size measurements; the [agent handoff](docs/AGENT-HANDOFF.md) lists the current implementation priorities. The interface below describes the retained Shell baseline.
See [Chinese documentation](README.md), [architecture and asset map](docs/ARCHITECTURE.md), [change and verification notes](docs/TRIMMING.md), and [historical decisions](docs/HISTORY.md).

> Automatic reinstallation erases the entire target disk, including all partitions.
> Rust automatic installation leaves the source OS through kexec, then rebuilds the target disk and GRUB. Target UEFI uses the removable fallback path. The retained Shell boot/reset interfaces below can also write NVRAM/BootNext.
> Back up your data and have provider console/VNC access. Disposable VM acceptance is in progress; the four-target BIOS/UEFI matrix has not passed yet.

## Rust entry point

```console
cargo xtask bundle
./target/x86_64-unknown-linux-musl/release/rustaller
```

Running without arguments on an interactive terminal opens the English TUI with a centered ASCII RUSTALLER title. See the [release guide](docs/GITHUB-RELEASE.md) for pinned GitHub download commands; the VPS needs no Rust toolchain.

Use `./rustaller debian --inspect` to view the detected disk, firmware and identity confirmation token without installing or executing kexec. Installation requires an SSH public key or password and the complete confirmation token in the native terminal. Non-interactive installation requires explicit `--confirm-erase`. Rust embeds its matching worker and log page, uses official authenticated resources, and does not require a custom `confhome`.

The source shutdown adapters currently require systemd or openrc-init as PID 1. Unsupported hosts stop rather than falling back to two reboots. Alpine Live rescue uses `hold=1` without automatic partitioning. Rust retains local ISO/netboot.xyz/reset menus and validation components, but their execution is not connected; the Shell baseline below still provides these interfaces.

`Configure current system` persists existing-account public keys, SSH port, hostname, timezone, DNS and supported network settings, with private backups and a changed-file report. It does not restart services or load sysctl values into the running kernel. Static DNS file changes can affect new lookups immediately. Drafts last only for this session; Save does not apply them.

## Scope

**Hosts and target systems are Linux x86_64 only.** Compatibility with different x86_64 Linux source distributions is preserved.
Automatic targets are latest stable Alpine, latest stable Debian, rolling Arch and rolling Gentoo; **all four install from Alpine Live**. Arch Linux ARM and all other ARM paths are unsupported.

Alpine and Debian versions are resolved from official stable metadata; version arguments are rejected. Debian is bootstrapped from Alpine with `debootstrap --variant=minbase`, then configured with systemd-networkd, SSH, a kernel and GRUB. No desktop/standard task or recommended packages are installed. Arch and Gentoo retain their upstream rolling installers; Gentoo retains its upstream systemd stage3, GRUB and gentoo-kernel-bin/dracut path.

The retained Shell baseline and Rust defaults preserve existing swap state. Rust also supports explicit target /swapfile configuration, activated by the new system's native boot process without changing source/Live swap. No swap partition is added. The Shell baseline retains direct netboot.xyz, local Linux ISO loopback and reset; Rust execution of those manual-media/reset routes is pending. Windows, Debian Installer, ARM target paths, other target installers, arbitrary RAW/DD targets and geographic detection have been removed. Use Alpine Live for manual recovery/DD.

Networking (IPv4/IPv6, static/DHCP and multiple NICs), SSH keys/passwords, ports, frpc and disk-ID identification remain. Physical RAM floors: Alpine 512 MB, Debian 512 MB, Arch 1024 MB and Gentoo 2048 MB. Both host preflight and Alpine Live check these floors; Alpine Live checks again before disk cleanup or partitioning. Containers, enabled Secure Boot and preparation from a tmpfs-root Live OS are rejected.

## Shell baseline: host your own matching assets

Do not run a trimmed `reinstall.sh` while downloading the untrimmed upstream `trans.sh`.
Serve these files from a VPS-readable HTTPS directory, preserving relative paths:

`trans.sh`, `initrd-network.sh`, `fix-eth-name.sh`, `logviewer.html`.

There are 4 runtime assets, excluding the entry script, example, documentation and tests. TTY selection, frpc URL resolution, and Alpine/systemd service templates are now in `trans.sh`, so they are not downloaded or executed as separate scripts. Debian Installer is removed; `debian.cfg` and `get-xda.sh` are no longer present or deployed.
The current compatibility marker is `PERSONAL-LINUX-ONLY-20261119-04`. Upgrade all matching assets together.

Use an immutable directory or your own pinned Git commit. URLs with embedded credentials/query strings or special characters are currently unsupported; this is not a private GitHub token download interface.

```console
export REINSTALL_CONFHOME=https://your.example/reinstall/pinned-version
bash reinstall.sh alpine
```

Alternatively pass `--confhome https://your.example/reinstall/pinned-version`.
This is required for automatic installation and Alpine Live, but not for reset, netboot.xyz or local ISO boot.
There is no upstream fallback; original upstream GitHub/CNB asset bases are rejected. Runtime version mismatches fail closed, including `trans.sh update`.

## Shell baseline: commands

These commands document the interface; they are not automatically executed:

```console
bash reinstall.sh alpine
bash reinstall.sh debian
bash reinstall.sh arch
bash reinstall.sh gentoo
bash reinstall.sh alpine --hold 1
bash reinstall.sh netboot.xyz
bash reinstall.sh reset
```

`alpine --hold 1` boots a memory OS with SSH without automatically partitioning disks.
netboot.xyz opens an interactive network-boot menu, not an extension of the four-target automatic installer.
`reset` can cancel prepared boot entries before reboot; it cannot undo formatting that has already happened.

Options: `--username`, `--password`, `--ssh-key`, `--ssh-port`, `--web-port`,
`--frpc-config`, `--no-cloud-kernel` (generic Alpine/Debian kernel), `--hold 0|1|2`,
`--target-disk /dev/DISK`, `--force-boot-mode bios|efi`, `--confhome`, `--debug`, `--help`.

Keys can be literal public keys, local files, HTTP(S) URLs, `github:user` or `gitlab:user`.
Passwords and SSH keys are mutually exclusive. Unspecified credentials prompt interactively; blank passwords generate random passwords.
Do not put secrets on the command line unless you accept shell-history exposure.
The frpc example is SSH-only: replace its documentation address and token with your own frps settings. Additional Linux service proxies can still be configured manually.
netboot.xyz/local ISO reject credentials, SSH/Web ports, frpc, hold and target-disk options.
`--help` does not require root or access the network.
Removed options include `--ci`, `--installer`, `--img`, `--minimal`, Windows-specific options and `--force-cn`.

## Shell baseline: already-downloaded local Linux ISO

This is an interactive GRUB loopback boot interface, not an ISO-to-disk writer or universal unattended installer.
It does not copy the large ISO or automatically repartition the disk.

For an ISO containing `/boot/grub/loopback.cfg`:

```console
bash reinstall.sh iso --iso /images/systemrescue.iso
```

Use `--iso-config /path/inside/iso` to select another loopback config. Otherwise supply real Linux kernel/initrd paths and the ISO-specific media-discovery arguments:

```console
bash reinstall.sh iso --iso /images/custom-live.iso \
  --iso-kernel /live/vmlinuz --iso-initrd /live/initrd.img \
  --iso-cmdline 'boot=live findiso=%ISO_PATH%'
```

The example is Debian-live style and is not applicable to arbitrary ISOs.
`%ISO_PATH%` expands to the ISO's GRUB filesystem path; `%ISO_UUID%` expands to its filesystem UUID.

Limitations:

- The ISO/initramfs must support loopback media discovery. GRUB reading a kernel does not prove that the kernel can find its installer media.
- Requires grub-probe/grub-mkrelpath or grub2 equivalents. BIOS hosts using only extlinux are rejected rather than having their bootloader replaced.
- Encrypted storage is unsupported; GRUB must be able to read the ISO filesystem.
- Paths must be absolute without whitespace, dot segments or shell/GRUB metacharacters.
- Explicit cmdlines do not support quoting, newlines, semicolons, backslashes, `$`, parentheses or braces.
- The ISO is mounted read-only to check the configuration/kernel/initrd paths; common non-Linux installer layouts are rejected.
- ISO files in `/reinstall-tmp/` or managed boot-file paths are rejected before cleanup.
- Do not erase the ISO-containing partition until the installer has finished reading it. Erasing that same disk requires ISO-specific RAM-loading support and sufficient RAM, or an independent retained disk.
- No generic network/SSH/unattended configuration is injected. Use the provider console/VNC.
- UEFI boot can still download an auxiliary GRUB EFI binary and write NVRAM.

## Shell baseline: download policy and retained upstream details

No `is_in_china`, location probing, China override or geographic netconf fields remain.
Official/non-mainland distribution sources are used. The original overseas IPv6-only frpc fallback to an IPv6-capable Nanjing mirror remains because GitHub releases lack IPv6; this is independent of geography.
Fedora 44 remains solely an auxiliary EFI GRUB binary source, not an installation target. Its version is independent of target validation and will need future maintenance.

The log viewer uses websocketd and a pinned library from official cdnjs.cloudflare.com. Unused Nginx/Lighttpd backends have been removed.
First-boot NIC repair supports the generated ifupdown/systemd-networkd formats only; this does not restrict Linux source-host compatibility.

## Verification

The test driver is Rust; there are no new Python/Node test scripts. Development requires Rust, Bash, BusyBox and Git:

```console
cargo test --manifest-path tests/verify/Cargo.toml
shellcheck --severity=warning reinstall.sh trans.sh initrd-network.sh fix-eth-name.sh
shellcheck --severity=warning --shell=sh tests/fixtures/fix-eth-name-openrc.initd
```

The Shell tests above use syntax-only parsing, isolated functions, unique temporary files, mocks and byte-for-byte upstream/fixture comparisons. Coverage includes x86_64 routing, RAM floors, no swap mutation and Debian debootstrap/networkd/fstab/GRUB. They never execute the installer entry point except safe `--help`. Use `cargo xtask check` for the Rust checks as well. Destructive acceptance runs separately inside disposable QEMU guests, never on the development host.
Set `TEST_BASH` and `TEST_BUSYBOX` to absolute executable paths if necessary. Upstream parity tests require the recorded upstream commit in Git.

Passing tests do not establish successful installation. Disposable BIOS/UEFI VPS acceptance tests are still required before production use.
