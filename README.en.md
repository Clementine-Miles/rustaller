# Rustaller

A lightweight Rust-first Linux x86_64 reinstallation tool derived from
[bin456789/reinstall](https://github.com/bin456789/reinstall), GPL-3.0-only.
It inspects the source host, authenticates installation resources, enters Alpine
Live through kexec, installs Alpine/Debian/Arch/Gentoo and configures the target.

**Development preview with real disk-erasing actions. Full distribution BIOS/UEFI
acceptance is pending. Back up data and keep independent console access.**

```console
cargo xtask bundle
./target/x86_64-unknown-linux-musl/release/rustaller --help
./target/x86_64-unknown-linux-musl/release/rustaller debian --inspect
```

Without arguments on a terminal, the bundled host opens its English TUI.
Installation requires root, successful preflight and the exact disk-identity
confirmation. `--inspect` is read-only. Optional settings include XFS/ext4,
distribution kernels, SSH/accounts, DNS/network policy and a target-only swapfile.
The automatic path requires kexec and performs one final reboot; failure retains
Live rescue. ISO/netboot.xyz/reset execution is not connected.

Source network capture currently requires an applied explicit flat networkd
profile; unsupported managers and routing stop preparation before erasure.

- [Architecture](docs/ARCHITECTURE.md)
- [Installation and requirements](docs/INSTALLATION.md)
- [Target configuration](docs/CONFIGURATION.md)
- [Security and trust boundaries](docs/SECURITY.md)
- [Testing and actual acceptance limits](docs/TESTING.md)
- [Development, builds, releases and provenance](docs/DEVELOPMENT.md)

[中文](README.md) · [License](LICENSE)
