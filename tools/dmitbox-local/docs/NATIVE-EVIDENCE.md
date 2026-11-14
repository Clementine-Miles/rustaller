# Native verification

The standalone artifact SHA-256 is `7cb505a6b2921170cf135cb397078836585fb25d7bce5e8d353da4f6f25c5544`. The [main report](TEST-EVIDENCE.md) records 26 passing checks, no failed checks and two skipped acceptance areas. Strict release acceptance remains blocked by those skips.

| Fixture | Observed postconditions | Limits |
| --- | --- | --- |
| [systemd and OpenRC recovery](native-evidence/recovery/results.json) | Public CLI identifies an enrolled serial console; independent READY precedes application; a forced kernel reset changes boot ID; boot recovery restores original file bytes and ROLLED_BACK; a second pending transaction expires despite forward/backward guest clock changes | Minimal native-init guests under QEMU TCG, file-resource recovery only; no full distribution/manager or network-reboot certification |
| [Independent systemd timer](native-evidence/timer-alone/results.json) | Polling watcher is deliberately SIGSTOPped; the actual monotonic timer still restores original bytes and ROLLED_BACK after guest clock changes | Separate file-resource fixture; no general claim that an active timer guarantees network restoration |
| [Real SSH confirmation](native-evidence/ssh/results.json) | Native OpenSSH login identity and socket are proved; original/reused and already-existing connections are rejected; a new authenticated connection completes the one-use challenge | Direct root login and confirmation primitives; complete port migration, socket activation and non-root/sudo integration remain unverified |

Both init fixtures also prove rejection of actual filesystem exhaustion and read-only writes while preserving existing bytes. Native namespace tests separately exercise MTU, static IPv6/DAD, multiple routing tables, nft rule order/counters, atomic rejection and preservation of foreign objects. Their ownership/facility fixture seams are stated in the logs.

Random NAT66 is skipped because the development kernel rejects its generator expression. Fixed-source native SNAT evidence does not qualify random selection or external connectivity. No real delegated prefix or administrator-operated external verifier was provided. The full Debian/Arch/Alpine manager matrix is also unverified.

Sanitized native logs, exact input/source hashes and [checksums](native-evidence/SHA256SUMS) accompany these records. Private disks, ephemeral private keys and unsanitized traces are excluded. The suite never changes the host clock or host network namespace. See [limitations](LIMITATIONS.md) before applying the development candidate.
