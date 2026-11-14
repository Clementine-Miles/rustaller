# rustaller — Personal VPS reinstall tool (Linux-only)

A derivative of [bin456789/reinstall](https://github.com/bin456789/reinstall), based on `98f1e21013233d42750cc2e7a298a0818461d686`. Licensed under GNU GPLv3 (`GPL-3.0-only`); see [LICENSE](LICENSE). Upstream source notices and attribution are retained.

The static Rust/musl host provides an English TUI and installs Alpine, Debian, Arch or Gentoo through authenticated Alpine Live and kexec. **This development preview includes real disk-erasing actions. Full BIOS/UEFI installation, filesystem boot and swap activation acceptance remain pending.**

> Automatic installation erases every partition on the target disk. Back up data and retain provider console/VNC access before testing.
> Rust uses kexec into Live and one final reboot with GRUB's UEFI fallback path. Retained Shell boot/reset interfaces can also write NVRAM/BootNext.

[中文](README.md) · [Architecture](docs/ARCHITECTURE.md) · [Configuration](docs/CONFIGURATION.md) · [TUI](docs/TUI.md) · [Builds and releases](docs/GITHUB-RELEASE.md) · [Acceptance](docs/ACCEPTANCE.md)

## Rust entry point

```console
cargo xtask bundle
./target/x86_64-unknown-linux-musl/release/rustaller
```

An interactive terminal with no arguments opens the TUI. Prebuilt downloads need no Rust toolchain; see the release guide.

Use `./rustaller debian --inspect` to read the disk, firmware and confirmation token without installation or kexec. Installation requires a public key or password and the full disk-identity token; non-interactive use requires `--confirm-erase`. The host embeds its matching worker and log page, authenticates official resources and needs no custom `confhome`.

Choose a kernel, XFS/ext4 root and public/custom DNS. Non-root accounts require a separate root password, entered privately in the TUI or interactive CLI; root SSH remains disabled. Hostnames inherit a valid source name or fall back to the distribution name; explicit settings win. See [kernel choices and CachyOS first-boot updates](docs/POSTINSTALL.md).

Source PID 1 must be systemd or openrc-init. Failed kexec support stops preparation without a two-reboot fallback. `hold=1` enters Alpine Live rescue without automatic partitioning. Rust ISO/netboot.xyz/reset execution is not connected; the retained Shell interfaces below provide those routes.

`Configure current system` persists existing-account keys, SSH port, hostname, timezone, DNS and supported network settings, with private backups and a changed-file report. It does not restart services or load sysctl values. Static DNS changes may affect lookups immediately. Save keeps a session draft; Apply writes configuration.

## Scope

- Linux x86_64 hosts and targets only. Containers, enabled Secure Boot and tmpfs-root source systems are rejected.
- All four targets install from Alpine Live: latest stable Alpine/Debian and rolling Arch/Gentoo. Versions come from official metadata; version arguments are rejected.
- Debian uses `debootstrap --variant=minbase`, systemd-networkd, SSH, a kernel and GRUB, without desktop/standard tasks or recommends. Gentoo retains systemd stage3, gentoo-kernel-bin/dracut and GRUB.
- Rust offers XFS (default) or ext4 roots, FAT32 ESPs, Alpine/OpenRC and schema 6 plans. Optional GNU-dd-backed `/swapfile` activates only at target boot. Defaults and Shell preserve existing swap; source/Live swap is never changed and no swap partition is added.
- Static/DHCP, IPv4/IPv6, multiple NICs, SSH credentials, ports, frpc and disk-ID discovery remain. Windows, ARM, Debian Installer, other automatic targets, geographic detection and arbitrary RAW/DD targets are excluded; use Live for manual recovery/DD.

RAM floors are Alpine/Debian 512 MB, Arch 1024 MB and Gentoo 2048 MB, checked on the host and again before Live cleanup/partitioning. See [minimal profiles](docs/MINIMAL.md), [source authentication](docs/SOURCES.md) and platform limits in the acceptance record.

## Shell baseline: matching assets

Serve exactly these 4 matching runtime assets from a VPS-readable HTTPS directory, preserving relative paths: `trans.sh`, `initrd-network.sh`, `fix-eth-name.sh`, `logviewer.html`. Never combine the trimmed `reinstall.sh` with upstream's untrimmed `trans.sh`.

TTY/frpc helpers and service templates are inline. The entry script, examples, docs and tests are excluded from the four assets. Upgrade all assets together with marker `PERSONAL-LINUX-ONLY-20261119-04`.

Use a pinned commit or immutable directory. Embedded credentials, query strings and special URL characters are unsupported.

```console
export REINSTALL_CONFHOME=https://your.example/reinstall/pinned-version
bash reinstall.sh alpine
```

Alternatively use `--confhome https://your.example/reinstall/pinned-version`. Automatic installation and Live require it; reset/netboot.xyz/ISO do not. Upstream GitHub/CNB asset bases are rejected, with no fallback. Mismatches stop execution, including `trans.sh update`.

## Shell baseline: commands

```console
bash reinstall.sh alpine
bash reinstall.sh debian
bash reinstall.sh arch
bash reinstall.sh gentoo
bash reinstall.sh alpine --hold 1
bash reinstall.sh netboot.xyz
bash reinstall.sh reset
```

`alpine --hold 1` starts memory-based rescue with SSH and no automatic partitioning. netboot.xyz is an interactive menu, not an extension of automatic target support. `--hold 2` retains the installed target for inspection before manual reboot; it is not for ISO/netboot.xyz. `reset` cancels pending boot entries but cannot undo formatting.

Options: `--username`, `--password`, `--ssh-key`, `--ssh-port`, `--web-port`, `--frpc-config`, `--no-cloud-kernel`, `--hold 0|1|2`, `--target-disk /dev/DISK`, `--force-boot-mode bios|efi`, `--confhome`, `--debug`, `--help`.

Keys accept inline values, local files, HTTP(S), `github:user` or `gitlab:user`; keys and passwords are exclusive. Missing credentials prompt; blank passwords generate random values. Command-line passwords remain in history. Replace the frpc example's server and token. `--target-disk` selects the whole disk; `--no-cloud-kernel` selects generic Alpine/Debian kernels. `--help` needs neither root nor network.

ISO/netboot.xyz reject credentials, ports, frpc, hold and target-disk options. Shell does not support `--ci`, `--installer`, `--img`, `--minimal`, Windows options or `--force-cn`; Rust minimal profiles are separate. The source OS need not be one of the four targets.

## Shell baseline: local Linux ISO

GRUB loopback boots an already-downloaded ISO for console installation. It neither copies the ISO nor automatically repartitions/writes it to disk.

For an ISO containing `/boot/grub/loopback.cfg`:

```console
bash reinstall.sh iso --iso /images/systemrescue.iso
```

Use `--iso-config /path/inside/iso` to override it. Otherwise supply real kernel/initrd paths and media-discovery arguments; this example is Debian-live-specific:

```console
bash reinstall.sh iso --iso /images/custom-live.iso \
  --iso-kernel /live/vmlinuz --iso-initrd /live/initrd.img \
  --iso-cmdline 'boot=live findiso=%ISO_PATH%'
```

`%ISO_PATH%` and `%ISO_UUID%` expand to the ISO's GRUB filesystem path and filesystem UUID.

- The initramfs must support loopback media discovery; GRUB loading the kernel is insufficient.
- Requires grub-probe/grub-mkrelpath or grub2 equivalents. Encrypted storage and BIOS hosts using only extlinux are rejected.
- Paths must be absolute, without whitespace, dot segments or Shell/GRUB metacharacters. Explicit cmdlines reject quotes, newlines, semicolons, backslashes, `$`, parentheses and braces.
- Read-only mounting checks config/kernel/initrd files. Non-Linux layouts, `/reinstall-tmp/` and managed boot paths are rejected.
- Do not erase the ISO's partition until reading finishes. Same-disk erasure needs ISO-specific RAM-loading support and enough RAM, or an independent retained disk.
- No generic network/SSH/unattended setup is injected. UEFI may still download auxiliary GRUB and write NVRAM.

## Shell baseline: downloads and networking

Geographic detection is removed. Official/non-mainland sources remain, including the IPv6-only frpc fallback to an IPv6-capable Nanjing mirror when GitHub releases are unreachable. Fedora 44 supplies only auxiliary EFI GRUB, not an installation target.

Logs use websocketd and a pinned cdnjs.cloudflare.com library. First-boot NIC repair handles generated ifupdown/networkd formats only.

## Verification

Development requires Rust, Bash, BusyBox, Git and OpenSSH client; test orchestration uses Rust:

```console
cargo test --manifest-path tests/verify/Cargo.toml
shellcheck --severity=warning reinstall.sh trans.sh initrd-network.sh fix-eth-name.sh
shellcheck --severity=warning --shell=sh tests/fixtures/fix-eth-name-openrc.initd
```

Shell tests use syntax checks, isolated functions, temporary mocks and byte-for-byte upstream comparisons, executing only safe entry `--help`. Set `TEST_BASH`/`TEST_BUSYBOX` as needed and retain the pinned upstream Git object. `cargo xtask check` runs the full checks; destructive acceptance is restricted to disposable QEMU guests.

Tests do not establish installation success. Four-target BIOS/UEFI acceptance remains pending. See the [handoff](docs/AGENT-HANDOFF.md), [trimming notes](docs/TRIMMING.md) and [history](docs/HISTORY.md).

## Local administration component

[DMITBox local](tools/dmitbox-local/README.md) is included as an independently invoked standalone Bash component. Build it with `cargo xtask local-tool build`, inspect `bash tools/dmitbox-local/dist/dmitbox-local.sh --help`, and run `cargo xtask local-tool check`. It is not automatically applied by the installer or worker. Review its [ownership prerequisites](tools/dmitbox-local/docs/CONFIGURATION.md), [native evidence](tools/dmitbox-local/docs/NATIVE-EVIDENCE.md) and [unverified boundaries](tools/dmitbox-local/docs/LIMITATIONS.md) before use.
