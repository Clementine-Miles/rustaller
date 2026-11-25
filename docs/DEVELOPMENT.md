# Development

## Workspace and tools

Use the pinned `rust-toolchain.toml` toolchain (Rust 1.99.1, rustfmt, Clippy,
`x86_64-unknown-linux-musl`). The workspace includes `rustaller`, its three boot crates and
`xtask`; `tui` is an optional host feature using ratatui/crossterm. Core/worker
application logic uses Rust and the standard library. `libc` is test-only;
`sha2` remains required by xtask for release integrity.

```console
cargo xtask check
cargo xtask bundle
./target/x86_64-unknown-linux-musl/release/rustaller --help
cargo xtask bundle --no-tui
cargo xtask audit-sources
cargo xtask vm-acceptance --help
```

Native commands remain the correct interfaces for Linux inventory, authenticated
downloads, OpenSSH, kexec, filesystem/mount tools, package managers and native
initramfs generation. Do not implement another installer, general administration
layer, plugin registry or recovery executable. OpenRC integration is generated
minimal native service glue invoking the Rust worker; no separate service scripts
are maintained. See [architecture](ARCHITECTURE.md) for module ownership.
Arch's CachyOS repository setup verifies the checksum-pinned official bootstrap
archive as provenance. Rustaller configures the applicable repositories and
requires native database and package authentication. It requires runtime
acceptance like the other distribution bootstrap paths.

## Bundles and preview releases

`bundle` builds the worker with `--no-default-features`, validates its static ELF
and runtime marker, embeds those exact bytes in the host, then verifies the host.
Release host builds require that bundle workflow. Debug builds use an explicitly
synthetic embedded-worker fixture and cannot establish installation acceptance.

```console
cargo xtask dist --tag v0.1.0-preview.local --output artifacts/preview --allow-dirty
```

`dist` requires a fresh output directory and a tag matching the Cargo version.
It rejects dirty source by default. `--allow-dirty` is for local review only and
records dirty state. Packaging emits the entry executable, `SHA256SUMS`, `LICENSE`,
`build-info.json` and `RELEASE-NOTES.md`; it never ships the reference baseline.
Metadata records source revision, plan/runtime versions, static target, worker
hash and `installer_complete: false`. The release workflow creates only a draft
prerelease while full acceptance remains pending. It does not publish a stable
installation guarantee. Checksum verification establishes matching bytes, not
trust in an unauthenticated publisher.

`audit-sources` performs bounded read-only official metadata/HEAD requests. Keep
TLS, proxy/CA and signature requirements. Discovery/reachability is distinct from
payload authentication and from successful installation. Tests use recorded
metadata and mock commands; live audit failures must be reported explicitly.

## Maintenance conventions

Keep existing public configuration types/names unless a real protocol or safety
change requires otherwise. Change plan schema and matching runtime markers when
handoff encoding changes; reject incompatible data instead of guessing. Share
pure validators/renderers, use explicit command argv and the existing runner,
and validate all target outputs before publication. Keep credentials redacted.
No arbitrary command hooks or unrequested tuning defaults are supported.

Tests are organized by behavior and risk. Preserve disk identity, source trust,
private handoff, filesystem confinement, network correctness and no-reboot-on-
failure coverage. Add focused regressions for new behavior, not tests mirroring
implementation wording. Never execute installation against the development host;
actual partition/kexec/reboot tests require isolated disposable VMs and logs.
Before delivery run [the checks](TESTING.md), inspect the diff, fix links and avoid
unused dependencies, duplicate helpers and generated artifacts in source control.

## Provenance and license

Rustaller derives from [bin456789/reinstall](https://github.com/bin456789/reinstall)
and retains GNU GPLv3 (`GPL-3.0-only`); preserve the complete root `LICENSE` and
source notices. The original upstream implementation remains available at https://github.com/bin456789/reinstall/blob/98f1e21013233d42750cc2e7a298a0818461d686/reinstall.sh.
Baseline SHA-256: `b7ae1e185355d9e1ef0635828f9e7bbd0393475e38e1df72cba87fa415c7a255`.
Git history retains earlier revisions; no additional reference snapshots are kept.

The test `UTC`/`New_York` TZif fixtures come from Debian tzdata 2026b-0+deb13u1
(`Etc/UTC`, `America/New_York`); IANA timezone data is public domain.

The release builds both firmware binaries and the update helper. Install the
three Rust targets and build-machine binutils listed in [BOOT.md](BOOT.md).
The Linux EFI stub and x86 boot protocol are the target interfaces.
