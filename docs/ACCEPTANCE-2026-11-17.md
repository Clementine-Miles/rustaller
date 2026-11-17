# Rustaller QEMU acceptance report — 2026-11-17

## Scope and verdict

Real installations were exercised with the repository's `cargo xtask vm-acceptance` runner, not a substitute framework. **Debian + OVMF + ext4: PASS** after fixes, including native kernel transaction/regeneration and another reboot. **Alpine + SeaBIOS + ext4: FAIL** on the retained strict QMP reset-count check, despite passing installation/target/rebuild/reboot probes. The baseline pair is therefore **not accepted** and the broader matrix was not expanded. Multi-VPS coverage is blocked by missing access to any second host.

All paths in this report are relative to `/root/rustaller-acceptance` unless absolute. The checkout is `repo/`. No changes were pushed upstream. All host-side test data, extracted tools, Rust toolchains, keys and build output are confined to this directory. No host disk was attached to QEMU; no host partitioning, formatting, reboot, privileged mount, service/network/firewall/bootloader change was performed. Read-only diagnosis of the failed Debian filesystem was mounted **inside its disposable guest**, never on the VPS.

## Host inventory and roles

Only one VPS was accessible. `/root/.ssh` contained only inbound `authorized_keys`: no SSH configuration, known-host inventory or outbound private keys. Other VPS inventory/execution is **BLOCKED: endpoints and authorized access not supplied**. No address scanning or credential guessing was attempted.

| Host | Observed capabilities | Role |
|---|---|---|
| ESTake, 94.249.176.66 | Debian 13, kernel 6.12.117+deb13-amd64; 3 vCPUs, AMD Ryzen 9 9950X guest-visible CPU, SVM/NPT; 2.8 GiB RAM, no swap; initially 64 GiB available storage; KVM VPS; usable `/dev/kvm` | Sequential baseline KVM tests only |

A paused, disposable QEMU `q35,accel=kvm -cpu host -m 128` probe initialized successfully and was terminated without attaching any disk. Actual install guests also ran with explicit `-accel kvm`; there was no TCG fallback. Other hosts' KVM/TCG suitability is unknown. TCG was not executed here.

Installed tools: QEMU **10.0.13** (`1:10.0.13+ds-0+deb13u1`), SeaBIOS **1.16.3-2**, OVMF **2025.02-8+deb13u1**. xorriso and dependencies were downloaded using the existing APT package indexes and extracted under `tools/`, not installed on the host. Existing host QEMU/firmware files are referenced through this tool root. SHA-256 firmware hashes are in `evidence/input-hashes.txt`.

Networking: SLIRP user networking, one virtio NIC, MAC `52:54:00:12:34:56`; guest 10.0.2.15/24, gateway 10.0.2.2, DNS 10.0.2.3. Guest SSH forwarding binds **127.0.0.1 only**. QMP is a Unix socket inside mode-0700 case directories. No public QEMU management/SSH listener was created. Existing VPS SSH on port 22 and host networking were preserved. HTTPS package/image access worked without bypassing certificate verification.

Full inventory: `evidence/host-inventory.txt`. Final host checks: `evidence/host-final.txt`. All test QEMU processes and supplementary polling helpers had exited at final inspection; no guest SSH forwarding listener remained. Host SSH/networking remained operational. Approximately 12 GiB of private test artifacts are retained, with 53 GiB of host disk space still available.

## Source, provenance and guest configuration

- Upstream Git baseline: `eb58f8a19712f8dbc12354a7987958b85d082bfe`.
- Alpine 07 / Debian 02 checkout: `068c15de2b694f73c73c87a8d5aeafdc1ad30858`, installer SHA-256 `6aa47a2efa8e301a362c0af37cff5d19bbd641e330193a439ca21e12fa613a66`.
- Debian 03 checkout: `a9a2f77911c50d38d16bd8264efcdad3612d37c1`.
- Debian 04 checkout: `5181761236af1998ef63a380ec2b54e53422207d` (harness DNS selection only; production installer SHA-256 `586ef96c652e2bf549be354a534f2e002e608211f2baaab65e2290b61e6976e4`).
- Final Debian 05 checkout: `db5d0609bd35f5c5d10ecd234e2de8d9b8c1ac90`.
- Final installer SHA-256: `f74848a68c8c346a2e661ae1c5c7519bcb9c996710e6a01f6e16fb1828cd16ef`.
- Rust 1.99.1, static musl host/embedded worker, built with `cargo xtask bundle --no-tui`.
- Official Debian generic image: `https://cloud.debian.org/images/cloud/trixie/latest/debian-13-generic-amd64.qcow2`; metadata identifies version **20261116-2664**.
- Image and publisher SHA512SUMS were fetched with verified HTTPS; local image hash matched exactly. SHA-512: `e53a93ef33f828ceed0e7278c2cc2753bdc7d5098fbfae15bd64b6d878db350ea3744247763a1c8ecb6b4019a0f623c38caf6038ad6f9c357fb4e01493ddf1a3`.
- Detached `.sign`/`.asc` checksum URLs returned 404. **No independent OpenPGP authentication of the cloud image is claimed**; authentication was HTTPS plus the publisher's HTTPS checksum. The preserved image/hash, not a future mutable `latest`, is the reproduction input. An attempted dated image URL also returned 404.
- `source-03/` provisioning: **PASS**, 14:24:15–14:25:30 UTC (approximately 75 seconds from first base inventory to results publication). Authenticated source SSH, cloud-init completion, kexec-tool availability and clean guest shutdown were verified. `source-01`/`source-02` failed before guest creation because rustup supplied symlink `/usr/lib/ssl/cert.pem`; reproduction explicitly sets `SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt`.
- Standalone prepared source: `source-03/source-prepared.qcow2`, SHA-512 `d0d97cf16ff21094c539f6e316eabfb0c539c289031431118b2ad11f35ed5d12ab16245d963f11c11528e13d6ab16fd370d55dbfe169e74d4ff0f222797cfa6a`. No backing chain in either input; see image-info JSON evidence.
- Each case has a **new qcow2 overlay** and, for EFI, a **separate OVMF variable store**. No host block devices are attached.
- Actual case hardware: q35, KVM, `-cpu host`, **2 vCPUs, 1536 MiB RAM, 8 GiB virtio-blk disk**, 512-byte sectors, serial bound in preflight and revalidated in Live. Source serial display truncates to `rustaller-alpine-bio` or `rustaller-debian-efi`; exact tokens appear in `inspect.log`.
- Targets: Alpine **OpenRC/linux-virt**, Debian **systemd/linux-image-cloud-amd64**, ext4 root, **1024 MiB FAT32 `/boot`**, 64 MiB swapfile, swappiness 10, minimal profile, root public-key-only SSH, UTC timezone. Attempts through Debian 03 used automatic target DNS; Debian 04/05 explicitly select static SLIRP DNS 10.0.2.3.
- Final source seed uses an explicit flat systemd-networkd profile with static private SLIRP addressing. This avoids unsupported DHCP-generated gateway/DNS host routes. Cloud-init reapplies the profile before inspection. `source-state.log` captures initial disk, RAM, routes, DNS and networkd state; `inspect.log` captures firmware, disk identity and destructive-confirmation token.
- First five Alpine attempts used the original production installer plus evolving harness fixes. Attempt 06 and Debian 01 used the serial-fallback production change (`7b78293`, harness checkpoint `3057c5e`). Alpine 07 and Debian 02 use commit `068c15d`; Debian 03 uses `a9a2f77` (only Debian package requirements changed); Debian 04 uses `5181761` (explicit target DNS in the harness); Debian 05 uses `db5d060` (native initramfs guard adapter). Early harness work was uncommitted while diagnosing; do not interpret those attempts as pristine upstream acceptance.

## Executed matrix and timings

All times UTC on 2026-11-17. All installation attempts used ESTake/KVM/2 vCPU/1536 MiB/8 GiB virtio-blk/ext4 and the firmware/init/kernel selections above. Each output directory retains `results.txt`, `failure.txt`, serial/QMP logs and available stage logs. Source provisioning is not target installation acceptance.

| Output directory | Firmware / target | Start–end | Seconds | Verdict / exact failed stage |
|---|---|---|---:|---|
| `alpine-bios-01` | SeaBIOS / Alpine virt | 14:25:45–14:26:08 | 23 | FAIL: source preflight; applied networkd profile became stale after cloud-init rewrote it |
| `alpine-bios-02` | SeaBIOS / Alpine virt | 14:26:59–14:27:20 | 21 | FAIL: cloud-init; initial reconfigure command lacked a device argument |
| `alpine-bios-03` | SeaBIOS / Alpine virt | 14:27:56–14:28:17 | 21 | FAIL: cloud-init; networkctl does not accept wildcard interface `*` here |
| `alpine-bios-04` | SeaBIOS / Alpine virt | 14:28:45–14:29:08 | 23 | FAIL: source preflight; extra DHCP gateway/DNS host routes unsupported |
| `alpine-bios-05` | SeaBIOS / Alpine virt | 14:29:29–14:34:07 | 278 | FAIL: Live stable-disk rebinding, before partitioning; lsblk serial absent without udev |
| `alpine-bios-06` | SeaBIOS / Alpine virt | 14:35:59–14:38:51 | 172 | FAIL: strict final firmware reset count; installed target hostname/SSH observed, two raw RESET events |
| `debian-efi-01` | OVMF / Debian cloud | 14:40:10–14:46:01 | 351 | FAIL: target resolver publication; relative resolved symlink rejected, no final reboot |
| `alpine-bios-07` | SeaBIOS / Alpine virt | 14:47:57–14:53:06 | 309 | FAIL: same strict final reset count; target and native rebuild/reboot probes both passed |
| `debian-efi-02` | OVMF / Debian cloud | 14:53:47–15:01:53 | 486 | FAIL: target network probe could not execute missing `ip`; target boot/SSH, storage and one-reset checks passed |
| `debian-efi-03` | OVMF / Debian cloud | 15:03:59–15:10:04 | 365 | FAIL: target DNS lookup; static network had no DHCP DNS lease and target plan still selected automatic DNS |
| `debian-efi-04` | OVMF / Debian cloud | 15:11:07–15:17:46 | 399 | FAIL on evidence review (raw runner PASS): native initramfs precheck hook rejected/skipped as invalid shell syntax; install/network/rebuild/reboot probes passed |
| `debian-efi-05` | OVMF / Debian cloud | 15:20:42–15:25:42 | 300 | PASS: full installation, target probe, native kernel transaction/regeneration and post-update firmware reboot/probe; native hook syntax verified |

Alpine 05 and Debian 01: after capturing recovery evidence, the guest was deliberately powered off over its recovery SSH to terminate an already-failed test. The runner's generic “did not become ready before deadline” message refers to its readiness loop, not an actual full deadline expiry. The earlier worker error in the preserved recovery log is the precise failure. The final harness detects `rustaller-worker:` errors directly.

## Verified stages

### Alpine / SeaBIOS — final attempt 07

- **PASS:** source preflight, exact confirmation token and source public-key authentication.
- **PASS:** authenticated Alpine resource preparation and kexec; serial shows source Debian kernel → Alpine Live **6.18.60-0-virt** without an intermediate firmware boot.
- **PASS:** Live identity rebinding with sysfs serial fallback; partitioning and installation completed; `RUSTALLER_INSTALL_COMPLETE` present.
- **PASS:** Limine **9.6.5 BIOS** booted installed Alpine **6.18.63-0-virt**; target public-key SSH authenticated.
- **PASS:** probe validated Alpine ID/hostname/kernel family, ext4 root, UUID fstab, mounted FAT32 `/boot` and its recorded UUID, Limine config/payloads, active protected swapfile and swappiness.
- **PASS:** IPv4 default route, real DNS lookup and native APK repository refresh, both before and after the update reboot.
- **FAIL:** strict raw QMP count: two `guest-reset` events for the installation reboot. Attempt 06 events were at 1794926311.650150 and .653267; attempt 07 also reproduced a closely spaced pair. Serial shows one Limine/target kernel boot for that installation reboot. Cause is **not established**; no event deduplication or relaxed acceptance was introduced.

The QMP failure remains the case verdict despite guest probe lines containing `RUSTALLER_VM_ACCEPTANCE_PASS`. Those lines cover guest checks only, not full host-side acceptance.

### Debian / OVMF

Initial attempt 01 **passed** source preflight, EFI detection, authenticated Live handoff, stable disk identity, partitioning, debootstrap Release signature verification and package/kernel/initramfs installation. It **failed** at target resolver configuration. Recovery SSH confirmed `/etc/resolv.conf -> ../run/systemd/resolve/stub-resolv.conf` on the installed guest filesystem. No final reboot, Limine target boot or target SSH pass is claimed for that attempt.

Attempt 02 verified the resolver fix: it installed, booted through OVMF/Limine, authenticated target SSH, passed the exactly-one-reset check, validated the selected cloud kernel **6.12.117+deb13-cloud-amd64**, mounted ext4 root and FAT32 `/boot` with matching UUID, Limine payloads and active swap. It then **failed** before route/DNS testing because Debian's minimal package set lacked `ip`. Native update was not reached. Initramfs logs additionally warned about missing `fsck.ext4`.

Attempt 03 confirmed the package fixes and matched the running cloud kernel's `virtio_net` module vermagic; dry-run module resolution succeeded. Target SSH, storage, Limine and the single-reset check passed. The IPv4 default route was present. DNS lookup then **failed**: the harness had changed the source to static addressing without selecting explicit target DNS. Rustaller's documented static-address policy requires explicit DNS for predictable resolution (`docs/CONFIGURATION.md`). This was a harness setup defect, not proof of a target DNS renderer defect. Native update was not reached.

Attempt 04 fixed DNS and the raw harness reported **PASS**, including package refresh, native kernel transaction, initramfs/Limine regeneration and the post-update reboot/probe. However, `target-probe.log` contained `/etc/initramfs-tools/hooks/zz-rustaller-limine-check: 3: Syntax error: Unterminated quoted string`. Inspection of native `/usr/share/initramfs-tools/hook-functions` confirmed its `set_initlist` runs `sh -n` and silently skips hooks that fail. Rustaller had installed that hook as a symlink to an ELF worker. Therefore this report rates the run **FAIL on manual safety-hook review**, not an unqualified acceptance PASS. The guard was not executing even though regeneration succeeded.

Attempt 05: **PASS** at commit `db5d0609bd35f5c5d10ecd234e2de8d9b8c1ac90`. Source preflight, authenticated resources, kexec into Alpine Live, exact target identity, GPT partitioning/installation and Limine **9.6.5 UEFI** boot completed. Exactly one raw QMP RESET occurred for the installation reboot, then one additional RESET for the requested native-update reboot. Target public-key SSH, Debian/systemd/cloud-kernel selection, ext4 root, UUID fstab, FAT32 `/boot` UUID **1690-BB8B**, Limine artifacts, swap policy, static IPv4 route, actual DNS lookup and APT repository refresh all passed, before and after the update reboot. Guest hook syntax checks passed both times; the earlier invalid-shell-hook and missing-fsck warnings did not recur. `accepted.txt`, `target-probe.log`, `post-update-probe.log`, `serial.log` and `qmp.log` are the primary acceptance evidence.

Remaining non-fatal Debian warnings: en_US.UTF-8 locale was unavailable so Perl fell back to C; zstd was absent so initramfs-tools used gzip. These do not establish locale completeness or zstd support and were not silently treated as fixed.

## Native update and recovery evidence

Alpine attempt 07 ran the native `apk upgrade --available linux-virt` transaction, `mkinitfs 6.18.63-0-virt`, Limine refresh hooks and `limine-check`, followed by a new boot ID and another SSH/network probe. Active and previous Limine configurations/payloads were checked by the existing probe. **PASS for regeneration/generation validation and reboot; NOT a kernel-version-upgrade pass:** the running release stayed 6.18.63-0-virt because no newer candidate was installed.

Debian attempt 05 ran `apt-get install -y --only-upgrade linux-image-cloud-amd64` (already newest **6.12.117-1**), `update-initramfs -u -k all`, Limine generation refresh/checks, reboot and the second target/SSH/network probe. The running release remained **6.12.117+deb13-cloud-amd64**. **PASS for native transaction/regeneration, active/previous generation reference validation and reboot; NOT RUN for a transition to a newer kernel version.**

Supplementary read-only Debian 03 evidence (`evidence/debian-efi-03-module-state.log`) records `/lib/modules/6.12.117+deb13-cloud-amd64`, `virtio_net` vermagic `6.12.117+deb13-cloud-amd64 SMP preempt mod_unload modversions`, and successful modprobe dry-run. This verifies that sampled current module, not every module/recovery generation.

Automatic boot of the previous recovery generation, deliberate partial-package-update fault injection and independent vermagic validation of every recovery-generation module were **NOT RUN**. No intentional corruption was introduced. Successful current boot/networking establishes practical current-kernel driver operation, not exhaustive module ABI compatibility. Failed Debian resolver publication is preserved as an actual partial-install failure with refusal to perform a final reboot.

## Fixes made

Seven small local commits (full hashes in `evidence/commits.txt`, combined diff in `evidence/fixes.patch`):

1. `7b78293`: recover a missing lsblk serial from the kernel's exact sysfs serial paths; retain mandatory stable identity matching and bounded/single-line checks. Added regression coverage. Rerun reached installation and target boot.
2. `3057c5e`: configurable explicit KVM/TCG and guest RAM (defaults unchanged), hardware recording, preflight-ready static SLIRP/networkd seed and reapplication, initial-state capture, private recovery-log capture and more useful regular-file diagnostics. Host RAM would not safely accommodate the previous hardcoded 3072 MiB guest.
3. `4ee1069`: allow only the two exact Debian relative systemd-resolved symlink forms, still replacing rather than following them; added sentinel-content regression tests.
4. `068c15d`: correct standalone EFI probe path from `/sys/firmware/boot` to `/sys/firmware/efi` (the actual EFI directory was observed through recovery SSH); retain command stdout/stderr; defer but do not suppress a reset-count failure while collecting target/update diagnostics; recognize actual worker error lines.

5. `a9a2f77`: add `iproute2` and `e2fsprogs` to Debian's mandatory package set, preserving the selected kernel; exact package-list regression tests updated. This addresses the observed missing-route-tool probe failure and missing ext4 fsck/initramfs recovery utility. The affected Debian case was rerun.

6. `5181761`: explicitly select static SLIRP DNS 10.0.2.3 in target plans, consistent with the already-static source addressing. The DNS-failing Debian case was rerun; TLS/authentication and routing safety checks were not bypassed.

7. `db5d060`: replace only Debian's initramfs-tools precheck ELF symlink with a shell-parseable adapter that handles `prereqs` and `exec`s the same Rust `limine-check`; add native syntax/prerequisite regression checks and require real guest syntax validation. Other hooks remain unchanged. The affected Debian install/update case was rerun.

Final supporting checks: **347 workspace tests passed**, standalone guest-probe test passed, Clippy with `-D warnings` passed, static bundle validation passed, formatting and `git diff --check` passed. These are supporting checks, **not installation acceptance substitutes**. Logs are under `evidence/`.

## Evidence map and reproduction

Per-case subdirectory names: `alpine-bios-ext4-virt-native/` and `debian-efi-ext4-cloud-native/`.

- Firmware/boot evidence: `serial.log`, `qmp.log`, `qemu.stderr`, `hardware.txt`.
- Source evidence: `source-state.log`, `inspect.log`, `cloud-init.log`, source SSH known-host record and copy logs.
- Installation evidence: `install.stdout`, `install.stderr`, periodic `live-install.log`.
- Target/update evidence: `firmware-reset-check.log`, `target-probe.log`, `post-update-probe.log`, target SSH/copy logs.
- Extra failed-case diagnosis: `evidence/alpine-bios-05-live-state.log`, `evidence/alpine-bios-05-live-disk.log`, `evidence/debian-efi-01-recovery.log`.
- Start/end/elapsed/exit: `evidence/CASE.start`, `.end`, `.status`; full invocations: `evidence/CASE.log`.

Reproduce on this host using a **fresh output name** (the harness refuses existing directories):

```sh
cd /root/rustaller-acceptance
source env.sh
cd repo
cargo xtask bundle --no-tui
cd ..
bash run-case.sh alpine bios alpine-bios-repro > evidence/alpine-bios-repro.log 2>&1
# Only after the first guest exits:
bash run-case.sh debian efi debian-efi-repro > evidence/debian-efi-repro.log 2>&1
```

`run-case.sh` contains the full pinned-image/hash, static-installer, key, firmware, storage, timeout and update options; it invokes only the existing cargo acceptance infrastructure. For a fresh source image, repeat the full `prepare-source` invocation in `evidence/prepare-03.log`, with a fresh output directory and the same verified input hash. Generate new private test keys for another host; do not publish or copy the private key into a public artifact bundle. Results and raw-failed overlays were preserved locally; keys and QMP sockets are private. The existing runner deletes overlays for raw-PASS cases, so Debian 04/05 disks are not retained (04 was downgraded only during subsequent log review). Further recovery-generation boot testing requires a new installation or an explicit harness preserve-success option.

## Remaining limits and next tests

- **BLOCKED:** multi-VPS coverage until explicit endpoints/authorized access and resource budgets are provided.
- **FAIL / investigate first:** reproduce the KVM/q35 double-QMP-reset sequence with minimal guest reboot and correlate raw events, firmware entry and boot IDs. Do not silently count a pair as one without establishing its cause and adding a precise regression test.
- **NOT RUN:** TCG; Arch; Gentoo OpenRC/systemd; XFS; Btrfs; alternate kernels/controllers; BIOS GPT >2 TiB; 4Kn; Secure Boot; provider/hardware boot contracts. Expansion remains gated on baseline acceptance.
- **NOT RUN:** IPv6 Internet connectivity, DNS failover/renewal, multi-NIC, actual newer kernel-version transition, previous-generation recovery boot and controlled guest-only partial-update injection.
- After resolving baseline failures, rerun these exact two baselines on this host and a second independent KVM host, then add TCG on a suitable host. Only then expand distributions/filesystems/controllers, and exercise an actual two-version kernel/recovery transition with independent module-vermagic checks.
