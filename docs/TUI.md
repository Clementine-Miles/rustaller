# English terminal interface

Run a build with the host-only `tui` feature without arguments, with stdin and stdout attached to a terminal:

```console
cargo xtask bundle
./target/x86_64-unknown-linux-musl/release/rustaller
```

The release host embeds its worker; the VPS needs no Rust toolchain. See [builds and downloads](GITHUB-RELEASE.md).

Menus and messages use English. Home shows a centered RUSTALLER title, compact on narrow terminals. Minimum size: 48 columns × 16 rows.

## Choose a route first

```text
RUSTALLER
Main menu
├── Install a system
│   └── Choose a system
│       ├── Alpine Linux
│       ├── Debian minimal
│       ├── Arch Linux
│       ├── Gentoo
│       ├── Local Linux ISO
│       ├── Rescue: Alpine Live
│       ├── Tool: netboot.xyz
│       ├── Tool: remove installation boot entry
│       └── Back to previous menu
├── Configure current system
└── Quit
```

Choose a target before configuring installation. Current-system settings use a separate draft without an installation plan or target disk.

## Installation directories

The four automatic targets share these directories. Choices are limited to those supported by the selected distribution.

```text
System configuration
├── Account and SSH
│   ├── Username
│   ├── Authentication
│   │   ├── Password / SSH public keys
│   │   ├── Password settings: password and confirmation
│   │   └── SSH public keys: add, edit, delete or generate
│   └── SSH port
├── Installation profile
│   ├── standard / minimal
│   ├── Add curl / less
│   └── minimal: keep documentation/translations / package cache
├── Host settings
│   ├── Hostname
│   ├── Timezone
│   └── Official kernel choice
├── Network and DNS
│   ├── DNS settings: automatic / static, servers, search domains
│   ├── TCP and congestion control
│   │   ├── Congestion control and queue discipline
│   │   ├── Buffer, listen, SYN and network-device queue limits
│   │   ├── MTU probing, Fast Open and SYN cookies
│   │   └── Fill reference values / clear draft overrides
│   ├── IPv6 and address preference
│   └── Interface MTU
├── Target swapfile
│   ├── Create target /swapfile
│   ├── Size in MiB
│   └── Optional swappiness
├── Disk and boot
│   ├── Whole target disk
│   ├── Boot mode
│   └── Hold behavior
├── Resources and logs
│   ├── Optional matching HTTPS resources
│   ├── Web log port
│   ├── frpc configuration
│   └── Debug output
├── Review installation
└── Back to previous menu
```

Kernel choices use official packages; BBR requires `fq` and target-kernel support, and Alpine/musl rejects glibc address preference. Ranges are in [CONFIGURATION.md](CONFIGURATION.md), profiles in [MINIMAL.md](MINIMAL.md).

Each target has a session draft. Hidden values are not submitted: stale static DNS in automatic mode, disabled swap size, or minimal-only flags in standard mode. Review uses the CLI's validated plan, then restores the native terminal for disk confirmation and progress. Errors return to a result page; navigating back never retries installation.

## Configure the current system

Supports Alpine, Debian-compatible, Arch and Gentoo configuration semantics. Apply requires root on Linux x86_64; opening, editing or saving drafts changes no system files.

Directories:

```text
Current system
├── Account and SSH
│   ├── Existing user
│   ├── SSH public keys: append, edit the draft or generate
│   └── SSH port: blank means unchanged
├── Host settings
│   ├── Hostname
│   └── Timezone
├── Network and DNS
│   ├── DNS settings
│   ├── TCP and congestion control
│   ├── IPv6 and address preference
│   └── Interface MTU
├── Review and apply configuration
└── Back to previous menu
```

Only explicit fields are applied. Existing-account public keys accept inline OpenSSH values or absolute local files, not remote sources. This route does not create accounts, set passwords, select disks, install kernels or create swap.

Apply validates paths and native SSH settings, prepares private backups and publishes files. The result lists changes, backups and notes; unsupported/conflicting layouts are rejected, existing access restrictions retained.

No services restart, sysctl load or swap activation occurs. Static DNS may affect lookups immediately. Reload SSH deliberately from a retained session; socket activation may need separate configuration. Keep backups for restoration. Apply establishes the saved draft baseline; discarding later edits does not undo applied changes. Inspect reported partial failures using backups and changed paths.

## Back, save, discard and cancel

Every configuration directory has an explicit **Back to previous menu** item. `Esc` or `Left` performs the same navigation. Leaving a directory with changes opens:

- **Save draft and go back**: keep its changes in memory and return to its parent.
- **Discard changes and go back**: restore that directory's entry snapshot and return.
- **Cancel - keep editing**: stay in the directory with its changes intact.

Unchanged directories return immediately. Discarding a parent restores its entry snapshot, including child edits; other directories remain intact.

Drafts last only for the session; Save never installs or applies settings. `Ctrl-C` exits immediately. Discarding a draft does not remove generated key files.

## Native SSH key generation

Open **Account and SSH → Authentication → SSH public keys → Generate key** for installation, or **Account and SSH → SSH public keys → Generate key** in current-system mode.

Configure the type, KDF rounds, cipher, comment, output path and optional RSA bit count consecutively. Defaults produce the equivalent of:

```console
ssh-keygen -o -a 512 -Z aes256-ctr -t ed25519 -C YYYYMMDD -f /absolute/output/path
```

The system date supplies the comment. Arguments are passed without Shell expansion. **Generate and add public key** restores the native terminal for ssh-keygen's passphrase prompt, then returns to the TUI. The passphrase never enters fields, argv, environment or plan; an empty value has no KDF protection.

Existing keys and unsafe paths are rejected. Private/public files use 0600/0644; only the public key enters the draft. This is a client login key, not a host key: back it up before erasing its disk.

## Editing and privacy

- `Up/Down`, `Tab/Shift-Tab`: select; long menus scroll.
- `Enter` or `Right`: open a directory, confirm a choice or commit a field.
- `Esc`: cancel the current editor/selector, or return with the directory save decision.
- `Left/Right`, `Home/End`, `Backspace/Delete`, `Ctrl-U`: edit text.
- `F2`: review the active configuration; it does not apply it.
- `Up/Down/PgUp/PgDn`, `Home/End`: scroll review and result pages.
- `Ctrl-C`: quit and restore the terminal.

Fields are single-line, at most 8192 bytes; control/newline or oversized pastes are rejected whole. Password spaces are preserved. Up to 128 public-key sources are supported. Sensitive values are masked; callback results redact configured secrets and terminal controls.

Below 48×16, hidden actions are blocked while drafts survive resizing. Navigation, privacy, terminal restoration and encrypted-key tests do not establish OS installation success.

## 2026-11-02 界面调整

新安装 hostname 留空使用 `rustaller`，当前系统留空不变。启用 swapfile 后焦点转至 MiB 大小，空值必填；关闭保留草稿但不提交。返回使用 Save/Discard/Cancel。EFI Stub 已取消，不接入执行菜单，方案见 [EFI Stub 评估](EFI-STUB-OPTIONS.md)。
