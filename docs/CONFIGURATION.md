# Target configuration

`InstallPlan.system: SystemConfig` holds filesystem, package profile, DNS,
network and optional swap policy. `InstallPlan.post_install: PostInstallConfig`
holds kernel, hostname and timezone choices. Credentials and SSH settings belong
to `InstallPlan`; `DeploymentManifest` carries source disk/network facts. These
settings configure the new target, not the running source or Live network.

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
Automatic capture currently requires active systemd-networkd, an applied flat
profile with explicit `DHCP=` and `IPv6AcceptRA=`, no profile/global drop-ins or
global overrides, and no active NetworkManager. Other source managers (including
ifupdown/OpenRC-managed networking), implicit defaults, changed profiles and
conflicting owners stop preparation with a diagnostic. Apply an explicit supported
profile before retrying; do not change a remote host without console access.
This constraint does not restrict Alpine target networking.
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
full upgrade/initramfs/GRUB refresh. Other kernels do not enable those repositories.
Native package/service policy preserves SSH, DNS, CA roots, modules, boot files,
licenses and package databases. Minimal profile trims documentation/cache unless
explicitly retained; it is not a measured final size guarantee.

## Optional swap

`--swapfile-mib 64..65536` creates a new `/swapfile` only on the checked XFS/ext4
target; `--swappiness 0..200` requires that option. 2048 MiB is 2 GiB. Capacity,
full allocation, 0600, regular-file identity and one fstab entry are checked.
Existing files/swap are not adopted, resized or activated in source/Live. Target
boot activates the file. Swap size is not reserved RAM and does not change RAM
preflight floors. Runtime activation still requires [acceptance](TESTING.md).

Media routes configure a one-shot host boot, not a future target system. They
reject account, target networking, swap and installation options. See
[installation](INSTALLATION.md) for ISO modes and boot confirmations.
