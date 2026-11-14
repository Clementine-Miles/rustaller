# Unsupported and unverified behavior

This is a development refactor until the release acceptance matrix in [TESTING.md](TESTING.md) passes. Do not equate a completed build or unit suite with a safe production release.

## Implemented boundaries

| Area | Implemented operation | Unsupported or unverified cases |
| --- | --- | --- |
| Recovery | Pinned artifact, explicit states, bounded process-group cancellation, systemd monotonic timer/watcher and OpenRC supervisor/boot hooks | Unknown init systems; full distribution/manager matrix and every reboot-dependent network resource remain unverified unless named in the evidence |
| SSH | Enrolled native service, existing explicitly included Port-only fragment, staged listeners and two fresh authentications | Socket/inetd activation, ambiguous Include syntax/ownership, DNS-derived Host contexts/UseDNS yes, active Fail2Ban coordination and authentication-policy editing |
| Firewall | Exclusive owned table or scoped same-table child/jump, atomic native batches, parent-generation pinning; narrowly authorized exclusive-table recreation after a boot reset | Persistence, active/enabled competing loaders/managers, unsafe legacy INPUT policy and unknown hooks. Scoped foreign parents are not recreated on boot. Foreign parent edits produce conflict rather than being overwritten |
| DNS | Explicitly enrolled static resolver; NetworkManager UUID and resolved per-link numeric DNS; an existing owned networkd DNS drop-in | Named/URL/scoped server endpoints, Netplan/cloud-init/resolvconf, unknown DHCP hooks, full Alpine lease-owner adapter, broad reload/reapply. A persistent manager profile takes effect on native activation/reboot; a separate resolved override supplies current runtime DNS |
| MTU/routing | Explicit runtime interface and context; source-table/VRF inventory; IPv6 minimum and declared encapsulation bounds | Persistent MTU and generic link up/down, automatic interface selection, unmeasured end-to-end PMTU and asymmetric-route reachability |
| IPv6 | Enrolled unmanaged forever-static addresses and supported routes, granular inverse with DAD/full inventory checks; narrowly eligible empty links | DHCP/RA, unknown/native-manager ownership, complex multipath/VRF route restoration and changed interface identity after reboot |
| Pools | Explicit authorized /64 and permanent runtime /128 members with DAD and a pinned external verifier | Persistence, dynamic renumbering and externally verified delegated-prefix operation without an actual administrator-supplied verifier |
| NAT66 | Mark-free runtime policy for new original flows from the enrolled anchor, using one to four still-owned pool members; scoped disable waits for natural flow drain without deleting addresses | Persistence, dynamic renumbering, generic forwarded flows and real external verification. The development namespace kernel lacks the random generator expression, so full native random-policy qualification is skipped |
| TCP | Already available BBR/cubic congestion control, explicit iperf3 baseline/after request; new owned persistence file when no competing key owner exists | Loading modules implicitly, fixed global buffer/qdisc presets, generalized speed/latency improvement claims |
| Swap | New ext4/XFS file, atomic no-overwrite creation, runtime activation/priority, inode-bound inverse | Existing-file adoption, resize, fstab persistence, other filesystems and guaranteed swapoff under memory pressure |
| Maintenance | Native read-only reports; explicitly requested native update/cache/clock/journal operations and local configuration snapshots | Transactional reversal of package upgrades, automatic general snapshot restore, cloud-init installation and Fail2Ban writes |

The NAT66 namespace test exercises native fixed-source IPv6 SNAT batches, inverse and preservation of unrelated IPv4 NAT before recording the missing random-expression capability. Its verifier observations are mocked; this is partial evidence. No legacy mark-overwriting implementation is used as a fallback.

## Verification limits

The development environment has user/network namespaces. User-local official native nftables and QEMU TCG tools were obtained for isolated tests; `/dev/kvm` is unavailable. Namespace tests use the host kernel. Minimal native systemd/OpenRC guests, when reported as passing, establish only their named fixture assertions and are not full Debian, Arch or Alpine configuration matrices. There is no KVM or TCP tuning-performance evidence.

Read-only NFT queries require an observable `/sys/module/nf_tables` facility; otherwise they refuse before a netlink query can autoload a module. Exclusive boot-reset reconstruction is guarded by the changed boot ID, raw snapshot, pinned unchanged authority, exact current absence and ownership checks. Its namespace test mocks the boot ID; full native firewall/SSH port-migration reboot behavior remains unverified.

This development container hides the kernel module inventory. The native nft namespace fixture explicitly probes its isolated API and overrides only that visibility prerequisite in the test library; the distributed script retains its strict check. Protected application also refuses mount/network/PID/user namespaces that differ from init, because its worker cannot independently recover them.

No user-controlled external verifier or real delegated IPv6 prefix was supplied. Native pool tests exercise DAD and ownership locally while explicitly mocking the external verifier. Upstream routability, real external NAT66 source observations, renumbering and retained connection behavior remain unverified. Activation cannot treat a local-only test or a blocked probe as that missing proof.

## Fail-closed boundaries

- Unknown init systems and recovery mechanisms without an independently verified readiness/boot path.
- Mixed UFW/firewalld or other firewall ownership, unknown nftables input-hook pipelines and unenrolled foreign anchors.
- Generator-owned DNS or network configuration without a dedicated adapter, including unsupported netplan/cloud-init/resolvconf cases; symlink rewriting is not a fallback.
- Ambiguous interface, source/VRF/table or authentication context; a guessed default route is insufficient.
- Destructive IPv6 transitions whose addresses, routes and native owner state cannot be restored and read back.
- Unsupported Swap filesystems/layouts, foreign pre-existing files, unavailable metadata tools and unsafe ancestors.
- Filesystem/hardware failures, permanent read-only storage, uninterruptible kernel operations and arbitrary simultaneous root writes cannot carry an unconditional restoration guarantee.

The program must return a clear nonzero status and preserve evidence when an accepted postcondition cannot be established. `CONFLICT` and `ROLLBACK_FAILED` require administrator review through an independently working access path; neither means that connectivity has been restored.
