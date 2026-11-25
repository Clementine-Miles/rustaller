# Rustaller

A lightweight Rust-first Linux x86_64 reinstallation tool derived from
[bin456789/reinstall](https://github.com/bin456789/reinstall), GPL-3.0-only.
It inspects the source host, authenticates installation resources, enters Alpine
Live through kexec, installs Alpine/Debian/Arch/Gentoo and configures the target.

**Development preview with real disk-erasing actions. Full distribution BIOS/UEFI
acceptance is pending. Back up data and keep independent console access.**

| Target | Kernel package | Init |
| --- | --- | --- |
| Alpine | `linux-virt` | OpenRC |
| Debian | `linux-image-cloud-amd64` | systemd |
| Arch | `linux-cachyos-server` | systemd |
| Gentoo | `sys-kernel/gentoo-kernel-bin` | OpenRC |

```console
cargo xtask bundle
./target/x86_64-unknown-linux-musl/release/rustaller --help
./target/x86_64-unknown-linux-musl/release/rustaller debian --inspect
```

Without arguments on a terminal, the bundled host opens its English TUI.
Installation requires root, successful preflight and the exact disk-identity
confirmation. Supply at least one `--ssh-key` for installation or Live SSH.
The default `admin` account uses public-key authentication and passwordless
administrative sudo. Select `--username root` explicitly to allow root SSH.
`--inspect` is read-only and works without a key. New installations use XFS by
default or ext4, with optional DNS/network settings and a target-only swapfile.
The automatic path requires kexec and performs one final reboot; failure retains
Live rescue. New systems use [rustaller-boot](docs/BOOT.md) with FAT32 at `/boot` and a root
partition. Large BIOS disks use a GPT stage2 partition. The TUI compares detected
and planned systems. The default
installation profile is Minimal. Run `rustaller reset` to cancel a prepared
handoff and keep the source system running.

Host inspection includes guest-visible CPU/ISA levels, memory/swap, storage,
virtualization, firmware and networking without benchmarks or third-party scripts.
Arch requires authenticated CachyOS packages and a repository supported by the
guest CPU. Gentoo uses the authenticated `amd64-nomultilib-openrc` stage3.
The testing guide distinguishes current acceptance from historical results.

Source networking supports verified Ethernet configurations from networkd,
NetworkManager, ifupdown and dhcpcd. An explicit MAC-bound address policy is
available in the CLI and TUI. Wi-Fi handoff requires a supported reassociation
path. [Network configuration and limits](docs/CONFIGURATION.md)

- [Architecture](docs/ARCHITECTURE.md)
- [Installation and requirements](docs/INSTALLATION.md)
- [Target configuration](docs/CONFIGURATION.md)
- [Security and trust boundaries](docs/SECURITY.md)
- [Testing and actual acceptance limits](docs/TESTING.md)
- [Historical QEMU acceptance report and evidence](docs/VERIFICATION-FOLLOWUP-2026-11-19.md)
- [Development, builds, releases and provenance](docs/DEVELOPMENT.md)

[中文](README.md) · [License](LICENSE)
