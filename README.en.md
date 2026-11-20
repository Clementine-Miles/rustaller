# Rustaller

A lightweight Rust-first Linux x86_64 reinstallation tool derived from
[bin456789/reinstall](https://github.com/bin456789/reinstall), GPL-3.0-only.
It inspects the source host, authenticates installation resources, enters Alpine
Live through kexec, installs Alpine/Debian/Arch/Gentoo and configures the target.

**Development preview with real disk-erasing actions. Full distribution BIOS/UEFI
acceptance is pending. Back up data and keep independent console access.**

Five real KVM/QEMU installation cases passed: Alpine BIOS/ext4 and BIOS/XFS,
Debian UEFI/ext4 and UEFI/Btrfs, and Arch UEFI/ext4, including target SSH,
network/DNS, native kernel-package transactions, initramfs/Limine regeneration
and reboot. Alpine/Debian also passed
previous-generation recovery boots. The [acceptance report and evidence](docs/VERIFICATION-FOLLOWUP-2026-11-19.md)
record exact revisions, reproducible failures and coverage limits.

```console
cargo xtask bundle
./target/x86_64-unknown-linux-musl/release/rustaller --help
./target/x86_64-unknown-linux-musl/release/rustaller debian --inspect
```

Without arguments on a terminal, the bundled host opens its English TUI.
Installation requires root, successful preflight and the exact disk-identity
confirmation. `--inspect` is read-only. Optional settings include XFS/ext4/Btrfs,
distribution kernels, SSH/accounts, DNS/network policy and a target-only swapfile.
The automatic path requires kexec and performs one final reboot; failure retains
Live rescue. New systems use Limine 9.6.5 with FAT32 at `/boot` and a root
partition; large BIOS disks use a GPT stage2 exception. Btrfs uses `@` with
optional zstd; Btrfs swapfiles are rejected. The TUI compares detected and planned
systems. Reset infrastructure is retained; execution remains unavailable.

Host inspection includes guest-visible CPU/ISA levels, memory/swap, storage,
virtualization, firmware and networking without benchmarks or third-party scripts.
Arch adds hardened and Zen kernels; Gentoo offers authenticated
`amd64-nomultilib-openrc` (new-plan default) and `amd64-nomultilib-systemd` stage3
variants via `--gentoo-init`. Root snapshots do not include FAT32 `/boot`.
Native regeneration/reboot passed at the tested kernel versions. Transitions to
newer kernel versions, additional kernel/init combinations and multi-host coverage
still require further real VM acceptance; see the testing limits below.

Source network capture currently requires an applied explicit flat networkd
profile; unsupported managers and routing stop preparation before erasure.

- [Architecture](docs/ARCHITECTURE.md)
- [Installation and requirements](docs/INSTALLATION.md)
- [Target configuration](docs/CONFIGURATION.md)
- [Security and trust boundaries](docs/SECURITY.md)
- [Testing and actual acceptance limits](docs/TESTING.md)
- [Real QEMU acceptance report and evidence](docs/VERIFICATION-FOLLOWUP-2026-11-19.md)
- [Development, builds, releases and provenance](docs/DEVELOPMENT.md)

[中文](README.md) · [License](LICENSE)
