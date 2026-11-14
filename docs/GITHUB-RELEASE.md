# Build with GitHub release

storehouse：[Clementine-Miles/rustaller](https://github.com/Clementine-Miles/rustaller)。Currently a development preview with real liquidation operations：schema=6，XFS/ext4 root，GRUB，Optional activation after target launch swapfile。four goals BIOS/UEFI See complete acceptance [ACCEPTANCE.md](ACCEPTANCE.md)；Rust ISO/netboot.xyz/reset Handoff still not connected。

## Structure and interface

```text
src/bin/rustaller.rs        CLI / Optional TUI → Shared preflight, disk confirmation and installation portals
src/tui/                   Table of contents/Field/navigation/Plan separation; install preflight vs. standalone native key generation tool
src/host/                  Host check and boot、ISO components
src/worker/                initrd、Live Inspections, first boot fixes, log components
src/targets/               Alpine / Debian / Arch / Gentoo helpers
src/shared/                Partition, mount, clean
xtask/src/bundle.rs        Build none first TUI worker，Embed again host
xtask/src/check.rs         Rust choreographed static/isolated inspection
xtask/src/dist.rs          release package、SHA-256、Build status
.github/workflows/         GitHub Check with Release draft process
```

TUI only belongs to host，share CLI Model and install callbacks; native ssh-keygen To temporarily restore the terminal, only the public key enters the plan. Menus, drafts and credentials masking [TUI.md](TUI.md)，See module responsibilities[Architecture](ARCHITECTURE.md)。

## historical volume

The following measurements only correspond to the source code at that time and do not represent the current package size, memory or installation speed.。2026-10-28 of preview.7 host for 1,999,312 byte，worker for 893,200 Bytes; before reconstruction was 2,011,600 / 901,392 Bytes, see [REFACTOR.md](REFACTOR.md)。

| Version | TUI host（Contains worker） | CLI host（Contains worker） | worker |
|---|---:|---:|---:|
| 2026-10-26 preview.4 | 1,393,072 | 1,093,968 | 565,520 |
| 2026-10-25 multi-level TUI | 1,331,632 | 1,077,584 | 565,520 |

The above configuration uses Rust 1.99.1、static musl、LTO、one codegen unit、`opt-level=s`、strip。Passing the historical static check does not mean complete installation acceptance。

## local use

Use it in the project root directory `rust-toolchain.toml` fixed toolchain and musl target：

```console
cargo xtask bundle
./target/x86_64-unknown-linux-musl/release/rustaller
```

no parameters and stdin/stdout opens when attached to a terminal TUI；CLI-only use `cargo xtask bundle --no-tui`。The draft does not perform installation. After submission, it must pass preflight and full disk confirmation, and then authenticate the resource.、kexec Enter Live And clean up the installation。`REINSTALL_CONFHOME` Matching resource addresses can be prefilled。

Complete check：

```console
cargo xtask check
```

requires Bash、BusyBox、OpenSSH client、ShellCheck 0.11+；Available `TEST_BASH`、`TEST_BUSYBOX`、`TEST_SHELLCHECK` Specify tools。CI use `--skip-shellcheck` Then execute it through the fixed version container ShellCheck。

Local preview packaging：

```console
cargo xtask dist --tag v0.1.0-preview.1 --output artifacts/github-preview --allow-dirty
```

`--allow-dirty` Only for local preview, record `source_dirty: true`，CI Not used. The output directory must not exist，tag must match Cargo version, such as `0.1.0` correspond `v0.1.0` or its prerelease。

| document | content |
|---|---|
| `rustaller-x86_64-linux-musl` | The only entrance, embedded with exact matching worker |
| `SHA256SUMS` | Portal, license, build metadata summary |
| `LICENSE` | GNU GPLv3（`GPL-3.0-only`）Full terms; upstream claims and attribution retained |
| `build-info.json` | Version、schema、Source code submission/dirty state、TUI、worker Hash、`installer_complete: false` |
| `RELEASE-NOTES.md` | Release Main text, not as download attachment |

## Source code import and comparison benchmark

Already have a warehouse? clone Then copy the source code to the root directory, retain the hidden directory and existing history, and do not force overwriting. Source code package exclusion `.git`、Build product、VM、Logs and private keys；`target/`、`artifacts/`、`dist/` Depend on Git neglect。

Shell Dependency on fixed upstream objects；CI Obtain automatically and execute locally：

```console
git fetch --no-tags --depth=1 https://github.com/bin456789/reinstall.git 98f1e21013233d42750cc2e7a298a0818461d686:refs/rustaller/upstream-baseline
cargo xtask check
```

The following is a historical shallow clone migration command, which can only be used when the warehouse is indeed a shallow clone and the original history needs to be supplemented.：

```console
git fetch --unshallow origin
```

initial imported commit/Push example (existing remote history must be integrated first）：

```console
git status --short
git add -A
git commit -m "Prepare Rust installer preview and GitHub releases"
git remote set-url origin https://github.com/Clementine-Miles/rustaller.git
git push -u origin main
```

Preserve the upstream declaration with GPLv3。Check Execution format、Clippy、isolation/terminal/Shell test、ShellCheck and musl Build without running the real installation。

## Release Preview

Check After passing, create a new tag；The following are examples only, do not reuse existing versions.：

```console
git tag v0.1.0-preview.1
git push origin v0.1.0-preview.1
```

Release preview right tag recheck, package and create draft/prerelease；Also available from Actions Manually specify existing tag。Create draft only job have `contents: write`，Use warehouse `GITHUB_TOKEN`。

The draft and attachments are reviewed before being released as a development preview. Unpublished drafts cannot be downloaded anonymously；prerelease must use pinned tag，`latest` Does not automatically point to it. Existing versions will not be covered. Maintain until complete installation acceptance `installer_complete: false` and preview status。

## VPS Download starts

The following fixed version commands are only available in this tag Effective after the attachment has been made public：

```sh
curl -fL 'https://github.com/Clementine-Miles/rustaller/releases/download/v0.1.0-preview.1/rustaller-x86_64-linux-musl' -o rustaller && chmod 700 rustaller && ./rustaller
```

Download it as a file and run it to keep the interactive terminal. Installation required root；TUI After submitting and confirming the disk identity, the disk will be liquidated.。Rust ELF Not used `curl | sh`。

Only use the following stable entrance after complete acceptance and release of the stable version.：

```sh
curl -fL 'https://github.com/Clementine-Miles/rustaller/releases/latest/download/rustaller-x86_64-linux-musl' -o rustaller && chmod 700 rustaller && ./rustaller
```

At the same time, download according to the original name of the attachment. `SHA256SUMS`，run `sha256sum --check --ignore-missing SHA256SUMS` Check completeness。

## To be completed

acceptance host → kexec → Alpine Live → Quad target installation → Once the entire link is finally restarted, the Rust manual media/reset。Fixed download links, static checks and build passes do not replace installation acceptance。
