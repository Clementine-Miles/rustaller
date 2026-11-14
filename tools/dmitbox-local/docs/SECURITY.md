# Resource ownership and recovery

DMITBox is a local Bash tool. It does not deploy a management server, dashboard, telemetry client or downloaded third-party script. Treat root access and the operating system kernel as trusted. A second administrator with root access can bypass locks; expected-state checks detect supported observable changes but cannot make arbitrary root writers participate in atomic compare-and-swap.

| Resource | Accepted owner | Mutation boundary | Risk and required evidence |
| --- | --- | --- | --- |
| Transaction records and snapshots | Private root-owned DMITBox state tree | Immutable plans, journal records and versioned state | Durable barriers can fail; preserve the failed record and report uncertainty |
| Recovery worker | Version-pinned standalone artifact | One pending transaction and its declared resources | A real supervisor readiness handshake and boot recovery are required; a scheduled timer alone is insufficient |
| SSH configuration | Explicitly enrolled systemd/OpenRC service and existing Port-only fragment | Preserve foreign files, Include/Match and effective context | Access loss; require newly authenticated transport or enrolled console, then a second fresh transport after retirement. Socket/inetd activation and active Fail2Ban are refused |
| nftables | Exclusive DMITBox table or explicitly enrolled same-table anchor | Owned rules/objects only, validated atomic batches; pin the foreign parent pipeline | Mixed ownership, active firewall loaders, legacy INPUT policy and changed parent generations must refuse; never flush the global ruleset. Runtime policy only |
| DNS | Identified native manager/profile or unmanaged regular resolver file | Only selected DNS settings with full preserved owner state | Runtime and persistent effects differ; readback is separate from optional resolution tests |
| Links, routing and IPv6 | Explicit interface plus namespace/VRF/table/source context | Declared fields only | Disabling IPv6 can destroy addresses/routes; sysctl reversal is insufficient |
| IPv6 pool | Explicit authorized global /64 on an enrolled unmanaged link | Runtime permanent /128 members, protocol ownership and DAD | Requires pinned separate-host verifier before/after mutation; local DAD does not establish upstream routability |
| NAT66 | Separate explicitly owned runtime policy | See the supported backend and limits in LIMITATIONS.md | External observed-source evidence and existing-flow handling are required; no global conntrack flush or mark overwrite |
| TCP profiles | Supported, explicitly selected sysctl keys | Capture and restore each owned value | Capability and measurement evidence required; unrelated sysctls remain outside ownership |
| Swap | New uniquely owned file on local ext4/XFS, same filesystem as transaction storage | Exact file identity and runtime activation/priority | Never truncate an existing file. Resize and fstab persistence are unsupported; swapoff failure retains evidence |
| Maintenance/package operations | Explicit administrator invocation | Report irreversible effects honestly | Package upgrades are not presented as fully reversible transactions |

Transactions use the authoritative states `PREPARED`, `ARMED`, `APPLIED`, `VERIFIED`, `COMMITTED`, `ROLLED_BACK`, `CONFLICT` and `ROLLBACK_FAILED`. Journal entries describe individual substeps. Rollback restores an owned after-state, skips an already restored before-state, and stops with conflict on an unrelated third state. Lock files coordinate cooperating invocations; they do not authorize resource adoption.

All high-risk mutation must refuse if independent recovery cannot be armed and verified ready. Read-only status, plan and diagnostic commands do not create durable state, install packages, load modules or start network probes. Native SSH parsing may use a private disposable hypothetical configuration. Probe-dependent tests require explicit invocation and distinguish blocked, timed-out and failed resolution.

Raw file/nft snapshots, metadata, logical state and semantic SHA-256 fingerprints serve different purposes. File restoration verifies content, mode, numeric ownership, ACLs and extended attributes using native tools; SELinux deployments and hardware durability remain unverified. nft fingerprints retain rule/expression order while removing demonstrated runtime counters and handles. They do not prove packet reachability.

Reservations include canonical targets derived from adapter payloads, so choosing a different caller resource label cannot bypass another transaction's lock. Human input is collected without holding transaction/resource locks, then state generation and the original monotonic deadline are revalidated. Reboot recovery can still reach CONFLICT when an interface identity or a volatile kernel resource no longer matches either recorded version; the tool does not overwrite that uncertainty.

Protected application must share init's mount, network, PID and user namespaces: the independent worker cannot recover a different resource universe. Actual SSH confirmation is bound to the intended local account and tested peer/local-address/listener context. Directory-only user identities and additional connection contexts are not certified by one successful login.
