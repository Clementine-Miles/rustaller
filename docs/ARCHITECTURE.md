# Architecture

Rustaller is a Linux x86_64 reinstallation tool. One Rust workspace contains the
primary crate and `xtask`. The `rustaller` host executable embeds the exact
matching `rustaller-worker`; the optional `tui` feature belongs to the host.
There is no current-system administration mode or separate recovery executable.

```text
src/
├── bin/                 host CLI/TUI and internal worker entry points
├── model.rs, config.rs  typed InstallPlan and target configuration policy
├── postinstall.rs       kernel, hostname and timezone validation
├── codec.rs, plan_file.rs, deployment.rs
│                        bounded private plan, disk identity and network facts
├── command.rs, fsutil.rs typed native commands and safe per-file replacement
├── host/                preflight, sources, disk/network discovery and kexec
├── worker/              Alpine Live execution, temporary networking and first boot
├── targets/             Alpine/Debian/Arch/Gentoo packages, kernels and GRUB
├── shared/              target storage, configuration, networking and swapfile
└── tui/                 installation drafts, editing and review
xtask/src/               checks, bundle, preview release, source audit and VM harness
tests/                   isolated Rust integration tests and fixtures
upstream/install.sh      passive reference; never executed or shipped
```

## Lifecycle

```mermaid
flowchart TD
    A[CLI/TUI validated plan] --> B[Read-only host, disk, network and kexec preflight]
    B --> C[Exact disk-identity confirmation]
    C --> D[Authenticate resources and create private initrd handoff]
    D --> E[Native PID 1 shutdown and kexec]
    E --> F[Alpine Live: modules, temporary network and rescue SSH]
    F --> G[Revalidate disk identity, RAM, sectors, mounts and swap]
    G --> H[Prepare filesystems and install selected distribution]
    H --> I[Configure accounts, kernel, GRUB, network and optional swap]
    I --> J[Verify target, unmount, sync and one final reboot]
```

`host` observes the original machine and prepares the handoff; it does not
partition its disks directly. Source PID 1 must be systemd or openrc-init.
Automatic network policy capture currently requires an active, explicit flat
systemd-networkd profile; other source network managers are refused. The OpenRC
shutdown adapter remains tested in isolation, but does not by itself make OpenRC
source networking supported. Kexec is required; failure does not select a
different reboot workflow.
`worker` admits only the private Alpine Live environment and binds the selected
disk to its stable recorded identity before destructive actions. The target is
mounted separately, normally at `/os`.

`targets` owns native distribution bootstrap, package trust, accounts, kernel
payloads, initramfs and GRUB. Debian uses debootstrap minbase and systemd;
Alpine uses OpenRC; Arch and Gentoo use systemd. GRUB supports the implemented
BIOS and 64-bit EFI layouts; EFI uses removable/no-nvram fallback.

## Configuration boundaries

`InstallPlan` holds credentials and typed `SystemConfig`/`PostInstallConfig`.
`DeploymentManifest` separately records disk identity and validated source
network facts. Handoff encoding is bounded and rejects incompatible versions.

- `host/handoff.rs`: observe source addresses, authoritative network policy and
  supported routing; refuse ambiguity before handoff.
- `worker/initrd_network.rs`: restore connectivity in temporary Alpine Live.
- `shared/network.rs`: pure final target rendering, kernel prerequisites, and
  publication of native interface/DNS/MTU/TCP/module/GAI configuration.
- `shared/configure.rs`: target isolation, safe bounded reads, complete output
  validation, publication, hostname/timezone and offline service links.
- `fsutil.rs`: exclusive temporary files, sync, rename and failure cleanup.

Package operations use bootstrap DNS. Final network publication happens after
those operations. Replacements are atomic per file, not across the batch;
installation failure retains Live and prevents the final reboot.

## First boot and limitations

Alpine's generated ifupdown configuration uses interface names, so a one-shot
Rust worker repairs them from captured MAC addresses before networking starts.
Systemd target files match MAC addresses directly and need no filename repair.
Opt-in Arch CachyOS has a separate native first-boot full upgrade and
initramfs/GRUB refresh; failure retains its pending marker for retry.

ISO/netboot.xyz/reset models and validation components remain internal work in
progress; their Rust execution paths are not connected. Unsupported source
network managers, complex routing, containers, ARM/Windows, Secure Boot and
provider-controlled boot contracts are not assumed compatible. See
[installation](INSTALLATION.md), [configuration](CONFIGURATION.md),
[security](SECURITY.md), [testing](TESTING.md) and [development](DEVELOPMENT.md).
