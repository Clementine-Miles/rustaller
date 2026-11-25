# Security and trust boundaries

Rustaller performs irreversible disk installation. Its safeguards reduce known
failure modes; they are not complete installation or connectivity certification.
Keep backups and independent console access. Full runtime acceptance is recorded
in [testing](TESTING.md), not inferred from compilation or generated files.

## Sources and integrity

| Stage | Trust mechanism |
| --- | --- |
| Alpine Live | Official HTTPS netboot archive, pinned release primary key, signature and SHA-256; extract kernel/initramfs/modloop from authenticated data |
| Alpine target | Trusted APK keys and signed packages |
| Debian | Debian archive keyring, authenticated release metadata, package index hashes and APT signatures |
| Arch | Trusted archlinux-keyring and required Arch package signatures. CachyOS uses a pinned official archive SHA-256, full release-key fingerprint and native database and package signatures. |
| Gentoo | Trusted release primary/subkey binding for the latest-stage3 index, DIGESTS and archive signature. The worker verifies the SHA-512 and exact `amd64-nomultilib-openrc` archive identity, timestamp and size before disk erasure. |
| rustaller-boot | Firmware artifacts built from the release source, checked for the expected format and schema, hashed and embedded in the worker |
| Rustaller release | Matching static host/worker, SHA-256 checksums, source/build metadata and license |

Official source addresses/parsers are centralized in `host::sources`. Downloads
retain TLS and HTTPS-only redirects, bounded sizes/timeouts and existing proxy/CA
settings. Authentication failure stops installation; there is no unsigned fallback.
The read-only `xtask audit-sources` checks metadata and endpoint reachability; it
does not authenticate complete payloads/signatures or prove installation success.

## Disk and execution isolation

The host reports complete disk identity and an action-bound confirmation token.
Before destructive operations, Live rebinds stable identifiers, rejects ambiguity
and rechecks size, sectors, writable state, mounts, active swap and RAM. Device
names alone never authorize erasure. Confirmation is explicit, not a generic yes.
Only Linux x86_64 and supported firmware/kexec environments pass preflight.

Live must be the private Alpine memory environment. Target operations use an
isolated root, normally `/os`; configuration APIs reject `/`, path traversal,
unsafe roots, symlinked ancestors and unexpected file types. Disk cleanup and
unmounting are limited to the identity-bound target. Tests never attach production
block devices or partition/reboot the development host.

## Credentials and files

The host validates every resolved SSH public key before handoff. Only public
keys enter the bounded private plan. Native `ssh-keygen` handles private-key
passphrase entry. Root can connect over SSH only when explicitly selected as
the account. The default `admin` and other non-root administrators get
validated passwordless sudo with direct root SSH disabled. Before starting Live
SSH or booting the target, Rustaller checks the
native daemon's syntax and effective public-key-only policy. Authorized keys,
sudoers entries and plan files use restrictive ownership and modes.

Configuration reads are bounded and reject special files before open. Existing
outputs must be caller-owned regular files with one hard link and no group/world
write permission. Known resolver/localtime links may be replaced without following
their targets. Timezone link resolution must remain inside target zoneinfo.
Generated paths and all output/service prerequisites are validated before writes.

Exclusive temporary files are synced and renamed; directories are synced after
publication. Outputs are read back. Atomic replacement applies to each file, not
the entire configuration set. I/O failure may leave earlier files published.
Callers must exclusively own the target tree; checks do not defeat concurrent
privileged writers or guarantee hardware durability/ACL/security-label preservation.

## Network and boot

Source addresses are observations, not evidence of DHCPv6 or RA policy. Supported
active configuration ownership and routing must validate before handoff; unknown
managers, policy routes/VRFs, ambiguous defaults and nonrepresentable mixed policy
stop execution. Source inspection is read-only. Temporary Live networking and
future target networking have separate implementations and roots.

Static DNS has one policy owner and suppresses lease/RA overrides. Resolver links
are never followed into Live. IPv6 disable cannot strand an IPv6-only plan.
Kernel capability checks concern the installed target kernel; offline configuration
does not prove active settings, DNS reachability or external SSH access.

Kexec requires source kernel support, available tools and an admissible lockdown/
firmware state. EFI requires 64-bit firmware and disabled Secure Boot. rustaller-boot uses
the UEFI fallback path without NVRAM changes; firmware honoring it still needs
testing. BIOS embedding uses the reserved MBR gap or explicit GPT stage2 partition.
Package/initramfs/boot/SSH verification failures stop finalization. The final
reboot is requested only after successful verification, unmount and sync; failed
installation retains Alpine Live rescue. There is no automatic restoration of
an already-erased disk.


## Boot artifact publication

Release tooling builds the PE32+ EFI application and the BIOS bootstrap and
secondary loader. Alpine Live checks for matching embedded artifacts before
partitioning. BIOS embedding validates partition geometry, syncs the secondary
loader and reads it back before publishing the MBR code. EFI uses the standard
firmware image loader. Both paths read a bounded TOML policy on their boot volume.
The helper checks the exact FAT mount and filesystem UUID, serializes updates,
syncs and verifies staged payloads, and retains previous-generation files.
[Publication and recovery](BOOT.md)

## Diagnostic collection

Normal host inspection performs bounded reads of procfs/sysfs and bounded local
utility calls (two seconds and 64 KiB per captured command). It installs nothing,
does not escalate privileges, execute reference scripts, benchmark, upload data,
or contact external lookup APIs. Optional facts can be unavailable independently;
strict disk identity, virtualization, firmware and source checks still govern
installation. Diagnostic addresses are not classified as public or used to infer
NAT, provider, ASN or location. CPU model names never authorize an ISA level.
