# Target configuration reference review

DMITBox Local was used as a reference for configuration validation and safe
publication. The reviewed source, tests and recorded evidence remain in Git at
`2cc3650774a9869e37bc3d75a7f4395867d0d0f1`; it is no longer a maintained product.

## Integration matrix

| Reference component | Actual mechanism and Rustaller counterpart | Decision |
| --- | --- | --- |
| core / validation | Trusted ancestors, bounded input, numeric IP/CIDR parsing, durable replacement; `fsutil`, `deployment`, `config` | Adapt canonical duplicate detection, reject unsafe names/paths and file aliases, sync publication. Use `std::net`, not shell parsers. |
| filesystem | Single-link files, expected metadata/content, ACL/xattr snapshots and conflict-aware inverse; target configuration writers | Retain target isolation and validate the complete output set before replacement. Reject unsafe existing files; no generic snapshots, ownership adoption or rollback engine. |
| DNS | Identify the authoritative resolver owner, reject unsupported generators/endpoints, suppress DHCP/RA overrides; `DnsConfig`, target network rendering | Generate native final configuration once. Static DNS owns a regular resolver and disables lease DNS; automatic systemd DNS uses resolved after package operations. Never follow a resolver link into Live. |
| routing | Explicit interface/namespace/VRF/table inventory and MTU limits; host handoff and deployment manifest | Preserve MAC identity, static on-link default routes and read-only capture. Reject ambiguous inherited gateways rather than guess. Keep the IPv6 MTU floor; no live MTU/probes/link administration. Complex policy routing remains unsupported. |
| IPv6 | Strict family parsing, unmanaged static addresses, route/sysctl inverses, DAD readback | Reuse validation ideas for inherited addresses and IPv6 policy. Preserve DHCPv6/RA and static routes in target output. No original-host address deletion or inverse machinery. |
| gai | Complete glibc precedence table, refuse foreign active precedence and malformed markers | Retain and test existing target implementation; Alpine/musl rejects the option. |
| runtime TCP | Explicit supported algorithm, detect competing persistence, distinguish requested/available/active/measured | Retain typed manual `NetworkConfig` and installed-kernel/module checks. Remove the TUI fixed tuning preset. File generation is not runtime activation or performance evidence. |
| runtime swap | Exclusive new ext4/XFS file, capacity/block-count/mode checks | Existing target swapfile already checks these and defers activation to boot. Preserve it; discard runtime swapon/swapoff, resizing and supervision. |
| SSH | Enrolled Port-only include, native sshd syntax/effective-policy checks and two fresh authenticated connections | Retain installation public-key preparation, target accounts, root-login policy and native sshd validation. Port migration, listener/firewall choreography and live account editing are out of scope. |
| confirmation | Kernel-proven SSH socket/process identity, console enrollment, one-use challenges | Discard live-mutation confirmation. Retain Rustaller's disk-identity erase confirmation, private credentials and Live revalidation. |
| transaction / recovery supervisor | Resource reservations, immutable plans, intent journal, monotonic timers, pinned independent worker, boot recovery | Discard. Installation failures retain Alpine Live and do not request the final reboot; multiple file replacements are explicitly not a transaction. |
| pool / NAT66 | Enrolled delegated /64, permanent /128 members, pinned external verifier, new-flow SNAT and flow drain | Discard; no installation requirement. |
| nft | Exclusive table/scoped child, native atomic batches, foreign generation checks | Discard; general firewall management is not installation configuration. |
| runtime maintenance | Reports, package/cache/clock/journal operations and local snapshots | Discard ongoing maintenance. Keep hostname/timezone, package/service selection and kernel modules in existing target modules. |
| main / build / tests | Separate CLI, concatenated Bash artifact, Rust xtask wrapper, mock/namespace/guest fixtures | Remove the independent implementation and build/CI dispatch. Transfer relevant regressions to Rust tests. Preserve Rustaller bundle/release/source-audit tooling and `sha2`. |

## Evidence boundary

The imported report records 26 passing checks and two skips (random NAT66 and
the full distribution/manager matrix). Minimal systemd/OpenRC recovery and SSH
guests establish only their fixture assertions. Pool verifier observations were
mocked; namespace tests use their own capability seams. None establishes
Rustaller installation, target network boot, external reachability or BIOS/UEFI
acceptance. Existing Rustaller acceptance limits still apply.

Baseline: `cargo test --locked --workspace --all-features` passed on the reviewed
revision after installing its pinned Rust 1.99.1 toolchain. No installation or
host-network mutation was run.

## Implemented flow and simplification pass

`InstallPlan` and the existing `DeploymentManifest` now feed a pure complete
network renderer. Final interface, DNS and MTU content is not subsequently
reparsed or patched. `shared::network` also prepares requested sysctl, installed
kernel modules and glibc GAI policy, validates the complete output/service set,
and publishes after native package operations finish using bootstrap DNS.
`shared::configure` owns shared safe reads/publication and hostname/timezone;
`targets::install` keeps package/kernel payload checks and final verification.
The standalone DNS writer and final-resolver helper are gone.

The dedicated second pass consolidated kernel symbol/module requirements,
account-name validation, MAC lookup, interface-name/CIDR parsing, directory
walking, effective UID calls and atomic replacement finalization. Alpine
bootstrap now uses the existing package policy instead of a second package
list. DNS is formatted once per networkd batch; sysctl output is reused when
deciding OpenRC service prerequisites. Obsolete current-system draft
transformations and helpers were removed; all eight installation selection
indices and their independent drafts are covered.

Alpine explicitly installs ifupdown-ng and uses its native IPv6 vocabulary and
dhcpcd protocol flags. Bare inet/inet6 compatibility hints do not constrain
ifupdown-ng's DHCP executor. Explicit family/RA/SLAAC settings therefore prevent
unrequested DHCP families, support RA-only DNS and request IA_NA for DHCPv6
without RA. These choices were checked against upstream
[ifupdown-ng](https://github.com/ifupdown-ng/ifupdown-ng/tree/60777d0f33453335761e8ab94359e6b31a4c8c1a)
`interface-file.c`, DHCP/IPv6 executors and native manuals, and dhcpcd's option
parser and DHCPv6 startup. They are covered by Rust rendering tests; this is
source review, not an Alpine boot acceptance result.

The public installation policy types and schema are unchanged. Internal target
configuration APIs deliberately no longer expose separate DNS/MTU mutation
paths: `apply_network_configuration` takes the manifest as well as the plan.
Shared file helpers moved from `shared::dns` into `shared::configure`.

Native external programs remain for Linux inventory, authentication, kexec,
filesystem creation, mounts, fully allocated swapfiles, package managers, SSH,
initramfs/GRUB and native service management. The application logic uses Rust;
no replacement Bash tool, registry, crate, transaction journal or recovery
executable was introduced. Native ifupdown configuration retains its existing
bounded `post-up ip` commands for additional static addresses/on-link routes.
`sha2` remains an xtask dependency for release checksums.

## Remaining boundaries

- Publication is per-file atomic with file/directory sync and readback, not a
  multi-file transaction. I/O failures can leave earlier files published; the
  installation fails and retains Live. Callers must exclusively own the target
  tree; these checks do not defeat concurrent privileged filesystem writers.
- Original-network capture still infers dynamic IPv6 from native address flags.
  Complex policy routing, VRFs, gateway-less default routes and multiple default
  routes per link cannot be represented in the existing manifest and are not
  silently approximated by the new default-gateway parser. Mixed static/dynamic
  addresses within one family still use the inherited per-family policy.
- Requested kernel settings require available evidence, not the Live kernel's
  capabilities. Ambiguous target Kconfig is rejected. Arch module inventory can
  establish module capabilities but cannot prove SYN cookies without Kconfig.
- No new full installation, BIOS/UEFI, provider network, DHCP/RA renewal,
  first-boot DNS, runtime sysctl activation, or swap activation acceptance is
  claimed. The existing acceptance matrix remains open.

## Verification — 2026-11-14

| Command | Final result |
| --- | --- |
| `cargo fmt --all -- --check` | PASS |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` | PASS |
| `cargo test --locked --workspace --all-features` | PASS: 295 tests |
| `cargo test --locked --manifest-path tests/verify/Cargo.toml` | PASS: 49 tests |
| `cargo xtask check` | PASS, including both crates' format/lint/tests, Bash/BusyBox ash syntax, ShellCheck warning severity and `git diff --check` |
| `cargo xtask bundle` | PASS: static musl worker without TUI, TUI host, matching runtime and exact embedded worker bytes |
| Bundled host and worker `--help` | PASS; no installation actions executed |

Rust 1.99.1 was installed in the workspace; BusyBox 1.37.0 and ShellCheck 0.11.0
were supplied through `TEST_BUSYBOX` and `TEST_SHELLCHECK`. The first full check
exposed two pre-existing formatting issues in the standalone Shell verifier;
only formatting changed there. The final check passed without skips. The
permission-denied publication fixture ran as an unprivileged user; that fixture
cannot exercise DAC denial when run as root.

The 303-test baseline included removed current-system and superseded incremental
network-writer tests. Replacement coverage tests final native files, DHCP-only,
static, IPv4-only, IPv6-only, dual stack, multiple MAC identities, RA/SLAAC and
DHCPv6 combinations, on-link routes, DNS overrides/canonicalization, manual TCP
values, glibc/musl, idempotence, unsafe paths/hardlinks/symlinks, kernel/service
prerequisites, publication failures, first-boot rename failures and merged-/usr
cleanup, plus all eight TUI selection indices.

QEMU and `/dev/kvm` are unavailable in this environment. No VM acceptance,
destructive installation, host networking change, service reload, swapon/swapoff,
firmware/NVRAM write or reboot was attempted. Existing BIOS/UEFI and distribution
acceptance remains unverified.
