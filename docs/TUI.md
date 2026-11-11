# English terminal interface

Run a build with the host-only `tui` feature without arguments, with stdin and stdout attached to a terminal:

```console
cargo xtask bundle
./target/x86_64-unknown-linux-musl/release/rustaller
```

The Release binary embeds its matching worker. The VPS does not need a Rust toolchain. Build and download instructions are in [GITHUB-RELEASE.md](GITHUB-RELEASE.md).

All menus, hints and generated messages use English. The Home screen displays a horizontally centered ASCII RUSTALLER title; narrower terminals use a compact ASCII title. The minimum terminal size is 48 columns by 16 rows.

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

An installation target is selected before its configuration directories appear. Current-system configuration has its own route and draft; selecting it does not create an installation plan or select a target disk.

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

The kernel choices map to official distribution packages. Alpine/musl does not expose glibc address preference. BBR requires `fq` and target-kernel support. DNS and network values use the common typed model rather than arbitrary Shell commands. Full ranges and behavior are described in [CONFIGURATION.md](CONFIGURATION.md); package profiles are described in [MINIMAL.md](MINIMAL.md).

Hidden options are not submitted: automatic DNS does not submit stale static DNS entries, disabled swap does not submit its size, and standard profile does not submit hidden minimal retention flags. Each target keeps an independent draft for this session.

The installation review submits the same validated plan used by the CLI. Before proceeding, the installation callback temporarily restores the native terminal for disk-identity confirmation and progress output. An identity or preparation failure returns to a result page; returning from that page does not retry automatically. Real BIOS/UEFI acceptance is recorded separately and must not be inferred from TUI or unit-test success.

## Configure the current system

This route detects Alpine, Debian-compatible, Arch or Gentoo configuration semantics. Applying changes requires root on Linux x86_64. Opening the route and editing or saving its draft do not change system configuration.

Available directories are:

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

The default draft is a no-op. Only explicitly configured fields are submitted. Public keys are appended for an existing account; this mode accepts inline OpenSSH public keys or absolute local public-key files, and rejects remote key sources. It does not create accounts, set passwords, install a system, choose a disk, replace a kernel or create a swapfile.

Applying validates the requested files and native SSH configuration, prepares private backups, and then publishes configuration files. The result page lists the changed relative paths, backup directory and relevant notes. Existing access restrictions are preserved; unsupported or conflicting network/SSH layouts are rejected rather than guessed.

The mode does not restart SSH or networking, apply sysctl values to the running kernel, or activate swap. Static resolver changes can affect new DNS lookups immediately. SSH changes require a deliberate reload from a retained working session; socket-activated SSH listeners may need separate socket configuration. Keep the backup directory if manual restoration is needed.

Successful application establishes the saved baseline for the draft. Discarding later draft edits does not undo already applied system changes. If publication reports a partial failure, use the reported backups and changed paths to inspect the system; draft navigation is not a rollback mechanism.

## Back, save, discard and cancel

Every configuration directory has an explicit **Back to previous menu** item. `Esc` or `Left` performs the same navigation. Leaving a directory with changes opens:

- **Save draft and go back**: keep its changes in memory and return to its parent.
- **Discard changes and go back**: restore that directory's entry snapshot and return.
- **Cancel - keep editing**: stay in the directory with its changes intact.

An unchanged directory returns immediately. Parent snapshots include changes made in child directories: saving DNS settings and later discarding the parent network directory restores the network draft as it was when that parent was entered. Other previously saved directories remain intact.

Drafts are session-only; they are not automatically written to a configuration file or credential store. `Ctrl-C` exits immediately. Generated SSH key files remain on disk even if their associated draft/public-key entry is discarded. Saving a draft never runs an installation or applies current-system settings.

## Native SSH key generation

Open **Account and SSH → Authentication → SSH public keys → Generate key** for installation, or **Account and SSH → SSH public keys → Generate key** in current-system mode.

Configure the type, KDF rounds, cipher, comment, output path and optional RSA bit count consecutively. Defaults produce the equivalent of:

```console
ssh-keygen -o -a 512 -Z aes256-ctr -t ed25519 -C YYYYMMDD -f /absolute/output/path
```

The date is obtained from the system date tool. Arguments are passed separately; no Shell expansion is used. Selecting **Generate and add public key** leaves raw/alternate-screen mode while the real system `ssh-keygen` asks for and confirms the passphrase. The TUI does not collect it or pass it through an argument, environment variable or installation plan. An empty passphrase has no KDF protection.

The tool rejects overwriting existing keys and unsafe output paths. It publishes the private key with mode 0600 and public key with mode 0644, then adds only the public key to the active draft. Back up the private key to your client before erasing its disk. This is a client login key, not a server host key.

## Editing and privacy

- `Up/Down`, `Tab/Shift-Tab`: select; long menus scroll.
- `Enter` or `Right`: open a directory, confirm a choice or commit a field.
- `Esc`: cancel the current editor/selector, or return with the directory save decision.
- `Left/Right`, `Home/End`, `Backspace/Delete`, `Ctrl-U`: edit text.
- `F2`: review the active configuration; it does not apply it.
- `Up/Down/PgUp/PgDn`, `Home/End`: scroll review and result pages.
- `Ctrl-C`: quit and restore the terminal.

Text input is one line and at most 8192 bytes. A paste containing controls/newlines or exceeding the limit is rejected as a whole. Password confirmation preserves exact leading/trailing spaces. At most 128 public-key sources are supported. Passwords, public keys, frpc input, matching-resource credentials and ISO arguments are masked where appropriate; callback results redact configured secrets and remove terminal control characters.

Resizing below 48x16 blocks hidden actions until the terminal is usable again. Input and drafts survive resizing. Tests cover English navigation, independent routes, save/discard/cancel snapshots, selected-field submission, privacy, scrolling, terminal restoration and native encrypted-key generation. These tests do not establish successful OS installation.
# 2026-11-02 操作调整

新安装入口显示 `New host settings`：hostname 留空使用 `rustaller`，当前系统
配置留空保持不变。启用 Target swapfile 后光标移到大小字段，Enter 编辑 MiB；
关闭时保留大小/swappiness 草稿但不提交。空大小显示必填，不再显示 Kernel default。
返回仍使用 Save/Discard/Cancel，草稿按发行版独立保存。EFI Stub 选项尚未接入
执行菜单，方案见 [EFI Stub 评估](EFI-STUB-OPTIONS.md)。
