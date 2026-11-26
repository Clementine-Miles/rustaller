# Testing and installation evidence

## Safe local checks

```console
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-features
cargo xtask check
cargo xtask bundle
cargo xtask vm-acceptance --help
git diff --check
```

`xtask check` runs Rust formatting, Clippy, workspace tests and diff hygiene.
The bundle verifies static x86_64 musl ELF, the matching runtime marker and exact
worker embedding; the worker has no TUI dependencies. Both bundled `--help`
commands are safe smoke checks.

Tests use `MockRunner`, isolated temporary files and controlled subprocesses.
They cover plan/CLI validation and bounded serialization, public-key policy,
source authentication, host preflight/handoff, disk identity and destructive
confirmation, storage geometry/cleanup, distribution package/boot requirements,
complete target network/DNS rendering/publication, temporary Live networking,
first-boot MAC repair and TUI/terminal restoration. Failure cases must stop unsafe
continuation or reboot. Rendering tests are not native boot or connectivity tests.
Fixture timezone files are compiled IANA public-domain data; they do not read the
host timezone database. No test activates host swap or changes host networking.

## Integration test ownership

The root integration files are grouped by observable behavior. Common private
temporary directories live in `tests/support/`; the small Rust subprocess
fixture exercises real command I/O without a second installer.

| Files under `tests/` | Responsibility |
| --- | --- |
| `planning.rs`, `configuration.rs` | CLI/plan encoding, validation, target accounts/settings and safe publication |
| `host.rs`, `sources.rs` | Inspection, disk confirmation, authenticated handoff and source metadata |
| `network_config.rs`, `live_network.rs` | Complete target output and temporary Live connectivity |
| `storage.rs`, `installation.rs`, `targets.rs` | Storage guards, lifecycle and distribution requirements |
| `boot.rs`, `initrd.rs`, `firstboot.rs` | Target boot, private worker handoff and Alpine MAC repair |
| `runtime.rs`, `terminal.rs` | Commands, worker compatibility, CLI/TUI terminals and credential privacy |

Host module unit tests cover applied DHCPv6/RA policy separately from address
observations, unsupported owners/drop-ins, stale and unsafe files, routing rules
and tables, ambiguous/gateway-less defaults and incompatible source selection.
Systemd target tests cover MAC matching without name binding for Debian and
Arch. Alpine retains name-repair, idempotence and failure-path coverage.

## Disposable VM harness

The retained Rust harness supports `smoke`, `prepare-source` and `install`:

```console
cargo xtask vm-acceptance --help
cargo xtask vm-acceptance install \
  --tools /absolute/tools-root \
  --base /absolute/prepared-source.qcow2 --sha512 VERIFIED_SOURCE_SHA512 \
  --key /absolute/test-key --public-key /absolute/test-key.pub \
  --installer /absolute/rustaller-x86_64-linux-musl \
  --target all --firmware both --filesystem all --disk-bus virtio-blk \
  --timeout 3600 --output /absolute/new-results-directory
```

On memory-constrained hosts, use `--memory-mib 1536` (default 3072).
Use `--accel kvm` only after verifying nested KVM; the default remains TCG and
there is no silent fallback. Both settings are recorded in `hardware.txt`.
The source seed uses the fixed private SLIRP address 10.0.2.15/24 and networkd;
cloud-init reapplies its profile before preflight. Target plans explicitly select
SLIRP DNS `10.0.2.3`, since static addressing supplies no DHCP DNS lease. This
avoids DHCP-added host routes unsupported by the installer's strict source
routing contract.
`source-state.log` records initial disk/network state. Recovery SSH periodically
captures `live-install.log` and stops the case early on a reported worker failure.

Obtain publisher-authenticated input images and verify their supplied hashes.
Inputs must be regular files without qcow2 backing chains. Each case uses fresh
scratch images and private firmware variables, never host block devices. Preserve
serial/QMP/SSH/probe logs and explicit pass/fail/not-run status. The harness's
proxy relay preserves the configured proxy and public CA; it does not bypass TLS.

A full pass requires authenticated source SSH, Live and install completion,
exactly one OS-requested final firmware reboot, installed-disk boot, target SSH,
the required kernel, filesystem UUIDs in fstab and boot state, valid boot
payloads, routing, DNS and swap activation. Negative SSH checks cover password
and keyboard-interactive authentication and an unauthorized key. The non-root
case also checks direct root SSH denial and non-interactive sudo.

`--filesystem all --target all --firmware both` runs 16 base cases (four
distributions, XFS and ext4, SeaBIOS and OVMF). Each distro uses the fixed
[kernel and init policy](INSTALLATION.md#requirements). `--account alternating`
is the default and exercises both account policies. `--account root` or
`--account admin` forces one policy across the selected cases.
`--boot-size-mib 1024` (512–4096) allows acceptance of larger images/generations.
`--disk-size-gib 3072 --firmware bios` expands only a fresh sparse overlay to
exercise the three-partition GPT case.

Raw QMP RESETs remain in `qmp.log`. For SeaBIOS, a soft reset may enter
`handle_resume32` and explicitly request a second hard reset. Two raw events are
accepted only with exactly one matching full soft-to-hard sequence in the
captured `firmware-debug.log`; extra resets, missing evidence and UEFI pairs fail.
The rule is also checked for the native-update reboot, with no time debouncing.
Use `--keep-disks yes` to retain successful disposable overlays for subsequent
recovery testing (the default still removes successful overlays).

`--kernel-update yes` runs a native required-package update, initramfs rebuild,
boot refresh, another reboot and target SSH/network probe. Logs preserve the
before/after kernel release: a repository with no newer candidate establishes
rebuild/reboot evidence only. Neither upgrade-hook correctness nor a new kernel
version is inferred from a mock command succeeding. The standalone probe also
verifies the required kernel package/family and Gentoo OpenRC profile and 64-bit ABI.

The fixed-policy refactor's actual boot, SSH, and update results are recorded in
the [key-only acceptance evidence](evidence/2026-11-25/key-only-policy.txt).

### Static dual-stack and reset acceptance

Use the existing disposable source image, checksum and test key from the
installation command above. Add `--ipv6-acceptance local --kernel-update yes
--reset-check yes` to verify held cancellation, install a static dual-stack
target and run the IPv6 checks again after native regeneration and reboot.
The case installs the optional native curl tool. QEMU/SLIRP creates an isolated
ULA subnet and a local OpenSSL HTTPS endpoint with a disposable certificate.
The guest validates the address, prefix, DAD state, default route and AAAA
resolution. HTTPS must report both an IPv6 source and peer and a verified
certificate. No proxy or IPv4 fallback participates in the probe.

The authenticated IPv4 control session starts a test-only Rust relay inside
the guest. A separate SSH session then authenticates against the target's
IPv6 socket and verifies the selected port. The relay keeps the installed
SSH policy intact. External inbound reachability is outside this SLIRP topology.

`--ipv6-acceptance external` requires direct, working IPv6 from the QEMU host to
the documented HTTPS endpoint `www.debian.org`. It tests the actual installed
guest path with IPv6 forced and proxies disabled. A local PASS records external
IPv6 as unverified. The case covers static IPv6, including a default route.
It does not establish SLAAC, DHCPv6 or IPv6 resolver transport.

`--source-network-manager networkmanager` installs NetworkManager on the
disposable Debian source and applies a static Ethernet profile before the
installer captures the network policy.

The `smoke --iso` input is the official Alpine Live test image. Its stock boot
files and source-image bootloader are harness inputs. Target installations use
rustaller-boot.

## Historical acceptance evidence

The archived results apply to the recorded revisions, packages and guest
hardware. Use fresh results to qualify the current fixed target and key policy.

- Migration audit and local checks. [2026-11-15 report](MIGRATION-REVIEW.md)
- Earlier Limine installations, including the SeaBIOS reset fix, native rebuild
  and reboot, and recovery boots. [Initial report](ACCEPTANCE-2026-11-17.md)
  [Follow-up report](VERIFICATION-FOLLOWUP-2026-11-19.md)
- Rust-native UEFI and BIOS loader, disposable installation, package hooks and
  partial Gentoo results. [Initial evidence](evidence/2026-11-21/native-boot.txt)
  [Follow-up evidence](evidence/2026-11-22/native-followup.txt)
- NetworkManager-to-Alpine BIOS installation, held reset, static IPv6 in an
  Alpine UEFI snapshot, and generation fallback. [2026-11-25 evidence](evidence/2026-11-25/network-reset-boot.txt)

Real 4Kn, physical microcode application, provider firmware and external IPv6
require the corresponding hardware or network environment. The harness records
TCG and KVM separately. Secure Boot, Xen PV/PVH/control domains and containers
are rejected during preflight.

## Regression boundaries

`boot.rs` table-tests plan/layout/render combinations, GPT stage2 commands,
UUID fstab/fsck rules, kernel version ordering and update generations. Regression
tests reject earlier plan schemas and unsupported target options. Mocked resource
tests reject digest mismatch before use. Native temporary-file tests exercise
missing initramfs, unsafe symlinks/hardlinks, invalid firmware payloads, unmounted
or wrong-UUID FAT, low capacity, interrupted copies, corrupted active config,
previous-generation recovery, numeric ordering, microcode and kernel removals.
Generator guard syntax and dracut wrapper staging/diagnostic deferral use controlled
local fixtures; they do not establish actual distribution package transaction behavior.

ISA tests cover every cumulative v1/v2/v3 feature, all required v4 features,
missing OSXSAVE, incomplete XCR0 state, heterogeneous/feature-masked CPUs, malformed
CPU fields, topology, and caller affinity preservation. Inspection tests exercise
missing utilities/proc/sys facts, malformed lsblk/routes/mounts, changing inventories,
memory/swap/zram, firmware unknowns and cached responsive panels. Controlled native
command tests enforce output bounds/deadlines, including descendant output pipes.

TUI tests preserve per-target drafts, fresh-key submission, cancellation, focused
validation and terminal recovery, and add current/target
panel placement at 120-, 60- and 48-column sizes. Set `RUSTALLER_CAPTURE_TUI=1`
when running these tests to save simulated terminal captures under `/tmp`.
They are deterministic renderer fixtures, not host inspection or VM screenshots.

Retained safety coverage includes disk identity/rebinding, exact destructive
confirmation, source authentication, private handoff, unsupported firmware,
read-only/4Kn geometry, mounted/active-swap rejection, unsafe paths, file
publication failures, network rendering/ownership, first boot and refusal to
reboot an incomplete installation. An offline or simulated test cannot establish
external IPv6, multiple NIC renewals, low-memory installations or hardware boot.

Official source audit reachability is environment-dependent and does not replace
payload authentication. Previous audits reached Debian metadata while other
endpoints returned HTTP 403; rerun and report actual results for each delivery.
