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
| Arch | Trusted archlinux-keyring, required package signatures; database signatures remain optional |
| Opt-in CachyOS | Pinned official archive SHA-256 and full release-key fingerprint; native package signatures remain required |
| Gentoo | Trusted release primary/subkey binding, signed DIGESTS, archive signature and SHA-512; exact amd64-nomultilib init/timestamp/size identity |
| Limine 9.6.5 | Fixed official commit and independent hard-coded SHA-256 values for utility source/header, BIOS payload, x86_64 EFI payload and license; checked before erasure |
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

Passwords are redacted in debug/errors/UI and transported in a bounded private
0600 plan, not the kernel command line. Non-root installations keep root SSH
disabled and require a separate root password. Native `ssh-keygen` owns private-key
passphrase entry; only the public key enters the plan. Private files/directories
use restrictive permissions. Command-line secrets can still be visible to the OS
and history; prefer interactive private entry.

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
firmware state. EFI requires 64-bit firmware and disabled Secure Boot. Limine uses
the UEFI fallback path without NVRAM changes; firmware honoring it still needs
testing. BIOS embedding uses the reserved MBR gap or explicit GPT stage2 partition.
Package/initramfs/Limine/SSH verification failures stop finalization. The final
reboot is requested only after successful verification, unmount and sync; failed
installation retains Alpine Live rescue. There is no automatic restoration of
an already-erased disk.


## Boot artifact publication

The pinned Limine C utility is compiled in the private Live work area before
partitioning; it is a native bootloader utility, not application-level Shell
logic. Download, hash or compiler failures leave disk preparation unstarted.
FAT boot files, boot state and native update hooks use existing target-path
checks. FAT mounts use file mask 0177 and directory mask 0077 so generated
0600 files/0700 directories agree with filesystem permission emulation; `quiet`
explicitly tolerates native chmod/chown requests that FAT cannot store; boot
executables are read by firmware, not Linux execution permissions. Refresh rejects unsafe links/hardlinks, malformed paths, missing pairs,
unmounted or wrong-UUID `/boot`, insufficient space and malformed EFI images. Native
update guards verify the mount before boot-file writes. Debian versionless kernel
links are disabled; Gentoo uses the legacy regular-file installkernel backend.
Complete new payloads are written and synced before the configuration rename.
Previous generation payloads and `limine.conf.previous` are retained. Capacity
failure preserves the published boot configuration; it requires freeing space
or increasing the partition rather than deleting the only recovery generation.
A power loss,
filesystem failure or concurrent privileged writer still requires recovery;
this is not a signed or transactional boot chain.

Actual firmware mode is read from the running kernel's EFI environment.
Firmware bitness and Secure Boot are checked independently of CPU architecture.
DMI/vendor/hypervisor information is supplementary. A forced mode cannot bypass
these checks. The same disk geometry and firmware assumptions are rechecked in
Live before destructive operations.

## Diagnostic collection

Normal host inspection performs bounded reads of procfs/sysfs and bounded local
utility calls (two seconds and 64 KiB per captured command). It installs nothing,
does not escalate privileges, execute reference scripts, benchmark, upload data,
or contact external lookup APIs. Optional facts can be unavailable independently;
strict disk identity, virtualization, firmware and source checks still govern
installation. Diagnostic addresses are not classified as public or used to infer
NAT, provider, ASN or location. CPU model names never authorize an ISA level.
