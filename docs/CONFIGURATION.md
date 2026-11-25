# Target configuration

`InstallPlan.system: SystemConfig` holds Gentoo init selection, filesystem, boot
partition size, Btrfs compression, package profile, DNS,
network and optional swap policy. `InstallPlan.post_install: PostInstallConfig`
holds kernel, hostname and timezone choices. Credentials and SSH settings belong
to `InstallPlan`; `DeploymentManifest` carries source disk/network facts. These
settings configure the new target. The resolved Ethernet, MTU and resolver
policy also applies during Alpine Live. The source stays configured by its
existing manager.

Schema 11 adds the explicit Ethernet settings. Schema 10 can be read with its
encoded profile and Gentoo init selection intact. Earlier schemas require a
new plan. Handoff uses the matching embedded worker runtime.

## Unified pipeline

Plan and deployment validation → pure final target rendering → installed-kernel,
path/file/service validation → safe per-file publication/readback → target
verification. `shared::network` owns interface, DNS, IPv6, MTU, TCP, module and
GAI output. `shared::configure` supplies file publication. No later DNS/MTU
writer reparses interface files. Bootstrap DNS remains available until package
operations finish. File generation proves neither activation nor reachability.

## Source networking, addresses and routes

The host binds interfaces to MAC addresses and separates observed addresses from
source configuration policy. DHCPv6 and RA must come from an identifiable active
configuration; the IPv6 `dynamic` flag alone never selects either mechanism.
Automatic capture dispatches per Ethernet link.

| Source | Supported policy and evidence |
| --- | --- |
| systemd-networkd | Applied flat profile with explicit `DHCP=` and `IPv6AcceptRA=`, without drop-ins or global overrides |
| NetworkManager | Active wired connection UUID and matching manual, DHCP, RA or disabled policy, read via `nmcli` without secrets |
| ifupdown and ifupdown-ng | Applied interface state and unchanged flat static or supported DHCP configuration |
| dhcpcd | Active daemon and matching DHCPv4 lease with a supported address family configuration |
| iwd and NetworkManager Wi-Fi | Detected as wireless association, independently of IP ownership. Handoff stops before erasure because firmware and private credential reassociation are unavailable. |

Conflicting ownership, custom DHCP identities, VPNs, tunnels, bridges, bonds,
policy routing and unsupported address combinations require an explicit Ethernet
policy or a supported topology. Offline inspection remains available.

Use `--network-mac` to supply the complete address policy for one Ethernet
interface. Omitted families use no static or dynamic addresses. Specify either
a static address/prefix and optional same-family gateway or DHCP for each family.
IPv6 RA can be selected with or without DHCPv6. The installer matches the MAC
again in Live. `--dns` and `--mtu` retain their existing meaning.

```console
rustaller debian --inspect --network-mac=52:54:00:12:34:56 --dhcp4 \
  --ipv6-address=2001:db8:1::10/64 --ipv6-gateway=fe80::1 --dns=192.0.2.53
```

The TUI includes these address fields under **Network and DNS → Explicit
address policy**. Review the effective settings before confirming the disk.
Gentoo OpenRC targets use Gentoo's netifrc/dhcpcd/openresolv conventions, rather
than Alpine's ifupdown-ng files. Per-interface `/etc/conf.d/net` settings preserve
supported static addresses, DHCPv4/DHCPv6/RA and off-link default routes. MAC-bound
udev rules stabilize the target interface names; the matching `net.INTERFACE`
services and SSH are added to OpenRC runlevels. Static DNS suppresses client
resolver replacement. The systemd variant retains networkd and resolved.
DHCP/RA address, route and client-identity overrides outside the supported policy
are refused. DNS overrides are selected through the installation plan.
Only supported Ethernet-style links and simple main-table routing are inherited.
Policy rules, VRFs, unsupported link kinds, multiple default routes, gateway-less
defaults and nonrepresentable static routes are rejected instead of guessed.

The manifest retains independent `dhcp4`, `dhcp6` and `accept_ra` fields. Static
addresses require valid family-specific CIDR; gateways must match the family.
Duplicate canonical addresses/MACs/names and malformed prefixes fail validation.
Temporary IPv6 addresses are not made persistent; tentative/DAD-failed addresses
stop capture. Mixed static and automatic addressing within a family requires a
representation the current manifest does not provide and is refused.

Systemd targets use networkd files matching MAC addresses, so interface renaming
does not affect matching. Static default gateways use on-link routing for provider
/32 and /128 layouts. DHCP/RA lease addresses and gateways are not pinned as static
boot routes. DHCPv6 without RA uses `WithoutRA=solicit`.

Alpine uses ifupdown-ng and dhcpcd. The renderer explicitly selects client address
families, RA and SLAAC; RA-only operation retains automatic DNS, and DHCPv6 without
RA requests IA_NA. Additional static addresses/default routes use bounded native
`ip` hooks. Its one-shot Rust first-boot repair follows MAC identity when interface
names change. IPv6 disable suppresses IPv6 addresses, routes, DHCPv6, RA and
link-local target configuration; at least one supported IPv4 path is required.

## DNS

| Option | Contract |
| --- | --- |
| `--dns-mode auto` | Default; explicit servers/search domains are not allowed |
| `--dns IP[,IP]` | Static numeric unicast upstreams; repeatable, 1–3 total |
| `--dns-search DOMAIN[,DOMAIN]` | At most six unique ASCII domains, 255 bytes total |

The TUI offers Automatic, Custom, Cloudflare, Google and Quad9. Only the selected
provider's values are submitted; stale custom drafts stay hidden. Loopback,
multicast, unspecified, broadcast, unscoped link-local IPv6, duplicates, URLs,
ports and invalid search domains are rejected. Private upstreams are allowed.
IPv4-mapped IPv6 DNS is rendered as canonical IPv4. IPv6 DNS is incompatible
with disabled IPv6.

Static DNS owns a regular `/etc/resolv.conf` and suppresses DHCPv4/DHCPv6/RA DNS
and search-domain overrides. Alpine disables both client resolver hooks. Automatic
systemd DNS uses the native resolved link after packages finish; automatic Alpine
DNS remains with its clients. Unknown resolver symlinks/owners fail explicitly.
With entirely static addressing, supply explicit `--dns` servers for predictable
resolution; there is no DHCP/RA source of automatic upstreams.

## MTU, TCP and address preference

All tuning is explicit; blank/default retains target defaults. No preset or
performance claim is implied.

| CLI option | Values |
| --- | --- |
| `--mtu` | 1280–9000, excluding loopback |
| `--tcp-congestion` | default, bbr, cubic; BBR requires explicit fq |
| `--qdisc` | default, fq, fq_codel |
| `--tcp-buffer-mib` | 1–64 MiB socket maxima and TCP buffer triplets |
| `--somaxconn`, `--syn-backlog`, `--netdev-backlog` | 128–262144 |
| `--tcp-mtu-probing` | 0–2 |
| `--tcp-fastopen` | 0–3 |
| `--tcp-syncookies` | 0 or 1 |
| `--ipv6` | inherit, enable, disable |
| `--address-preference` | default, ipv4; glibc targets only |

Requested kernel symbols and module payloads are checked against the installed
target kernel, not Live's kernel. Missing/ambiguous evidence fails. Arch's module
inventory can prove module capabilities but cannot prove SYN cookies without
Kconfig. Sysctl/module files and required OpenRC boot links are staged offline.
The glibc IPv4 preference writes a complete precedence table while preserving
unrelated policy; conflicting active precedence or malformed markers fail.
Alpine/musl rejects this option.

## Accounts, host settings, kernel and services

Root SSH is permitted only for an explicitly selected root installation account;
non-root installations disable it and require an independent root password.
Public keys are validated by OpenSSH before handoff; target SSH syntax is checked
before final reboot. SSH port is 1–65535 (default 22).

Hostname is one ASCII label, 1–63 characters, with alphanumeric ends. An explicit
value wins; otherwise the source hostname's valid first label is captured, falling
back to the distribution name. Timezone is a safe relative IANA name such as
`Etc/UTC`; the target's bounded TZif data must validate. No host hostname/timezone
command is invoked. No requested timezone means no timezone overwrite.

[Installation](INSTALLATION.md) lists kernel choices. Arch CachyOS is opt-in,
requires its authenticated official repositories and performs a native first-boot
full upgrade, initramfs rebuild and boot refresh. Other kernels do not enable those repositories.
Native package/service policy preserves SSH, DNS, CA roots, modules, boot files,
licenses and package databases. Minimal profile trims documentation/cache unless
explicitly retained; it is not a measured final size guarantee.

Gentoo uses `--gentoo-init=openrc|systemd` with OpenRC as the new-plan default.
Both select only the matching official `amd64-nomultilib` stage3. The extracted
matching stable no-multilib profile is preserved when possible; any replacement
must contain the exact no-multilib profile segment and match the selected init.
The final selected profile and `ABI_X86="64"` are checked. OpenRC enables native
udev, hostname, hwclock, modules, sysctl, local and SSH services; systemd enables
its existing native units. Kernel configuration, dracut and rustaller-boot remain shared.

## Optional swap

`--swapfile-mib 64..65536` creates a new `/swapfile` only on the checked XFS/ext4
target; `--swappiness 0..200` requires that option. 2048 MiB is 2 GiB. Capacity,
full allocation, 0600, regular-file identity and one fstab entry are checked.
Existing files/swap are not adopted, resized or activated in source/Live. Target
boot activates the file. Swap size is not reserved RAM and does not change RAM
preflight floors. Runtime activation still requires [acceptance](TESTING.md).

## Storage and kernel updates

`--filesystem xfs|ext4|btrfs` selects the root filesystem. `--boot-size-mib`
sets FAT32 size (512–4096 MiB, default 512) at `/boot`. Explicit partition roles
determine device numbering, mount paths and UUID fstab bindings. ext4 fsck pass
is 1; XFS/Btrfs use 0; FAT uses 2. BIOS/UEFI layouts are described in
[installation](INSTALLATION.md#disk-layout-and-boot).

Btrfs creates and mounts `@` and writes `subvol=@,compress=zstd:3` in fstab.
`--btrfs-compression 0` disables compression; 1–15 selects zstd. Compression
options require Btrfs. rustaller-boot passes `rootflags=subvol=@`. Each distribution
installs its filesystem tools and includes Btrfs support through mkinitfs,
initramfs-tools, mkinitcpio or dracut. Swapfiles on Btrfs fail plan validation;
there is no snapshot or Btrfs swap management.
FAT32 `/boot` is outside root-subvolume snapshots. Restoring a Btrfs root snapshot
does not restore its matching kernel/modules/initramfs; restore and verify matching
boot artifacts separately before rebooting.

Rustaller boots Linux from the FAT volume on BIOS and UEFI. The versioned
TOML, timeout, serial settings, kernel arguments and recovery procedure are
described in [BOOT.md](BOOT.md). Set `--root-size-mib` to leave free space
after root. Its default fills the remaining disk. The TUI can copy sizes from
a directly mounted root and FAT boot partition on the selected disk.

| Target | Native update integration |
| --- | --- |
| Alpine | APK pre/post-commit hook and successful mkinitfs completion |
| Debian | kernel postinst/postrm and initramfs post-update hooks |
| Arch | ALPM post-transaction hook on kernel pkgbase/initcpio paths, after mkinitcpio |
| Gentoo | traditional installkernel postinst, dracut; alternate boot-generator USE flags disabled |
| Opt-in CachyOS | native full upgrade, initramfs rebuild, Rust boot refresh |

Hook entry points link to the installed matching static Rust worker. Generated
minimal generator guards and a dracut PATH wrapper call that worker; no separate
boot-management implementation is maintained. The refresh requires FAT `/boot` mounted with the recorded boot UUID,
valid pinned boot state, safe sources and space for a complete new generation.
It scans only top-level native files, avoiding recursive copies of existing
generation directories. Debian's kernel symlink updates are disabled; Gentoo
uses installkernel's compatible traditional backend. FAT mount options explicitly
tolerate unsupported chmod/chown, while Rustaller publishes ordinary files without
symlinks, hard links or per-file Unix permission changes.

Before writing a new generation, free space must hold all new payloads plus a
1 MiB reserve while preserving native files and currently referenced generations.
Consider 1024–2048 MiB for multiple kernels or large initramfs; 512 MiB can fail
this check. The helper syncs all payloads before replacing
`/boot/rustaller/boot.toml`. Updates and recovery use ordinary FAT files.
Keep the matched helper on existing installations. Direct Gentoo invocations
of `/usr/bin/dracut` that bypass the PATH wrapper require an explicit
`/usr/local/libexec/rustaller-boot-update refresh` after output is complete.
