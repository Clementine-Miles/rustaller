# Prioritized change log

This development refactor implements the following changes. Verification applies only to assertions recorded in [test evidence](TEST-EVIDENCE.md); [limitations](LIMITATIONS.md) remain release requirements.

| Priority | Change | Result |
| --- | --- | --- |
| P0 | Durable eight-state engine, canonical target reservations, ordered flock locks and per-step INTENT/DONE/inverse records | Cooperating administrators cannot claim the same resource; unexpected versions become CONFLICT |
| P0 | Pinned independent worker, bounded process groups, systemd monotonic timer, OpenRC supervision and boot recovery | Recovery executes independently after actor termination or reboot; failures retain evidence |
| P0 | Port-only SSH ownership, native effective-policy simulation, scoped nftables batches and two fresh authentications | User, peer/local address, port and transport age are checked; hand-edited plans cannot bypass coordination |
| P0 | Recovery-file ownership registry and pre-open lock validation | Interrupted publication/cleanup is retryable; foreign files, edits, symlinks and FIFOs are refused |
| P1 | Native DNS ownership, separate persistent/runtime steps and strict address parsing | Domain/security settings are preserved; probes distinguish DNS replies from blocked observations |
| P1 | Explicit MTU context and granular static IPv6 address/route inverses with DAD | Unsupported ownership/layouts fail before mutation; generic interface shutdown is refused |
| P1 | Authorized runtime IPv6 pools and mark-free NAT66 with a pinned external verifier | Retained connections and address-removal guards are explicit; no local-only accessibility claim |
| P1 | Measured named-key TCP changes and inode-bound new Swap files | Unrelated sysctls and pre-existing files are not adopted; missing capability or activation evidence fails |
| P1 | Checked atomic publication, private temporary files and metadata readback | Content, ownership, mode, ACLs and xattrs are verified before supported file replacement |
| P2 | Testable modules and deterministic standalone build/archive | Existing CLI names and seven menu families remain available within supported ownership boundaries |
| P2 | Local structured audit, mock/concurrency/failure tests, native namespace and init/SSH fixtures | Source hashes, logs, skipped cases and uncertainty accompany the artifact |

The supplied r3 implementation is archived unchanged for provenance. Production has no fallback to its unsafe mutators or environment bypass flags.
