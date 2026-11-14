# Rust Structural simplification and execution optimization（2026-10-28）

Historical reconstruction is based on the current working tree, snapshot `/workspace/scratch/refactor-20261028/before.tar.gz` Not provided with source code. Keep current functionality、CLI、schema 4、mark and host/worker Division of labor, updating internal module paths, without retaining forwarding API。

## Reference source code for actual reading

- [zoxide，f0b31631](https://github.com/ajeetdsouza/zoxide/tree/f0b31631d295b0623bc440016ed0aba3c7c49dde)：read `src/main.rs`、`src/db/mod.rs`、`src/util.rs` and `Cargo.toml`。Draw on the structure of aggregating configuration and tool capabilities by responsibility and centralizing atomic file writing; existing `fsutil` Exclusive temporary files、sync、rename And continue to reuse failed cleanup. Its single codegen unit、LTO、strip This project has been activated before and is not included in the new income of this round.。
- [fd，b7a0a29d](https://github.com/sharkdp/fd/tree/b7a0a29d07c3d0e9e0a41155019acada08831a6c)：read `src/exec/mod.rs`、`src/exec/command.rs`、`src/walk.rs`、`src/exit_codes.rs` and `Cargo.toml`。use the execution path’s `Stdio::inherit()` Idea, only set up pipes and reading threads when you really need to capture output。

The reference repository is placed outside the project, and there is no need to copy the source code or introduce it. CLI Framework, allocator or dependency tree. The installation steps are in strict order and are not applied. fd Parallel traversal model of。

## Merge and delete

| old module | new ownership |
|---|---|
| system_config、network_config、swap_config | `src/config.rs` |
| targets of profile、Arch/Gentoo package selection，shared/package_policy | `src/targets/packages.rs` |
| shared/disk、shared/mount、Public storage errors and command tools | `src/shared/storage.rs` |
| shared/services | `src/shared/configure.rs` |
| host/metadata | `src/host/sources.rs` |
| host/kexec read-only policy check | `src/host/preflight.rs` |
| embedded | `src/lib.rs` |
| targets/install/rolling | `src/targets/rolling.rs`，Remove directories that only hold single files |

Deleted by Live replaced by the driver Alpine/Arch/Gentoo Dispatch wrapping, configuration forwarding, and wide-range offloading functions; valid assertions incorporated `target_config`。The actual driver is bound to the disk identity and only uninstalls its own target tree; retained Shell Independent verification and partitioning/Mount comparison without compromising identity, permissions, signatures or input checks。

## Execution and memory

- `OutputMode::Stream` Directly inherit the parent process descriptor. generally stdout/stderr commands with live output use no forwarding threads instead of two, avoiding two pipes and repeated forwarding locks/flush；Child processes can observe real terminal properties。
- `Capture` Preserve concurrent reads to avoid large output and stdin Writebacks block each other. The reader uses generic static dispatch and the standard library `read_to_end`，Remove `Box<dyn Read + Send>` and handwritten chunked forwarding loops。
- kexec Capability checks with network modules use iterators to check for unique values, eliminating the need to collect temporary `Vec`，Still reject duplicates or conflicts Kconfig items。
- No new runtime dependencies have been added。worker continue not to include TUI，Host TUI still optional feature control。

Not added unchecked Index or raw pointer range; reserved as necessary Linux FFI，Supplementary plan for atomic release ABI、Pointer Life and Parameter Safety Instructions。

## Measurement and Verification

| index | before refactoring | after refactoring |
|---|---:|---:|
| src Down Rust Number of files | 60 | 48 |
| src Down Rust Number of lines, including inline tests | 21,159 | 20,842 |
| static TUI host，Contains matches worker | 2,011,600 byte | 1,999,312 byte |
| static worker | 901,392 byte | 893,200 byte |

File reduction 20%，Source code net reduction 317 OK；host reduce 12 KiB，worker reduce 8 KiB。Use the same twice Rust 1.99.1、x86_64 musl、LTO、one codegen unit、opt-level=s、panic=abort and strip。Not inferred from this, full installation time or peak memory drop; disk/download/Native package management still determines most of the installation time。

- `cargo xtask check`：263 items Rust workspace testing and 49 items Shell Baseline test passed；Rustfmt、Clippy `-D warnings`、ShellCheck、Bash/ash、diff Check passed。
- Added direct inheritance output/private capture testing, and 2 MiB Pipeline input and output testing; existing configuration, disk identity, security boundaries, four-target package selection and TUI Test continues to pass。
- `v0.1.0-preview.7` exact static-package worker Embed with SHA256 Verification passed; actual static binary passed 4 pseudo-terminal tests, including return navigation/Discard current configuration, password masking, native ssh-keygen and terminal recovery。
- output：`artifacts/refactor-after-20261028/`；control：`artifacts/refactor-before-20261028/`；log：`/workspace/scratch/refactor-20261028/`。

previously v5 actual BIOS Test passed kexec arrive Alpine Live，But the automatic installation did not complete and eventually timed out. four goals BIOS/UEFI The entire process has not yet been accepted; this round of refactoring and static inspection does not change this conclusion. For details, see [acceptance records](ACCEPTANCE.md)。Keep development preview and `installer_complete: false`。Not submitted, pushed or published GitHub Release。
