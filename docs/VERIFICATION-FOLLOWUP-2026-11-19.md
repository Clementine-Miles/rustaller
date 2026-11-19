# Rustaller acceptance follow-up — 2026-11-17 to 2026-11-19

This supersedes the outstanding-baseline verdict in [the initial report](ACCEPTANCE-2026-11-17.md). Historical failures there remain evidence; they are not retrospectively relabelled as passes.

## Current verdict

- **PASS:** Alpine / SeaBIOS / ext4, including native regeneration and firmware reboot.
- **PASS:** Debian / OVMF / ext4, including native regeneration and firmware reboot.
- **PASS:** Alpine and Debian previous-generation recovery boots, sampled module compatibility, and safe native-generator rejection with `/boot` unmounted.
- **PASS:** gated expansion to Alpine / SeaBIOS / XFS and Debian / OVMF / Btrfs, including update/regeneration/reboot probes.
- **PASS:** Arch / OVMF / ext4, including native regeneration and firmware reboot after package/bootstrap, keyring-lifecycle and harness portability fixes.
- GitHub publication is separate from verification. The initial push was blocked by missing credentials; final delivery is attempted after the reviewed report/evidence commit, with the result recorded in the private campaign delivery log. This report does not infer push success from successful public remote reads.

These are real QEMU results using `cargo xtask vm-acceptance`, not conclusions from mocks or compilation. This is selected available-host coverage, **not** acceptance of the entire distribution/kernel/controller/provider matrix.

## Host and isolation

Host identifier: **ESTake / 94.249.176.66**. The 2026-11-17 tests used host kernel `6.12.117+deb13-amd64`; the resumed 2026-11-19 Arch tests used `6.12.117+deb13-cloud-amd64`, boot ID `37b0e56e-97f3-4e6a-ae51-03d82d705d61`. A restart/kernel change occurred between sessions; its cause is unverified. No host reboot or kernel installation was issued by this work. The environments are recorded separately, not treated as identical host-kernel evidence.

Both inventories showed 3 guest-visible Ryzen 9 9950X vCPUs, approximately 2.8 GiB RAM, no host swap, usable nested KVM and sufficient disk space. Test guests ran **sequentially**, q35/KVM, `-cpu host`, **2 vCPUs / 1536 MiB RAM / new 8 GiB qcow2 overlay / virtio-blk / 512-byte sectors**, with a 1024 MiB FAT32 `/boot`. SeaBIOS was **1.16.3-2** and OVMF **2025.02-8+deb13u1**, QEMU **10.0.13**. EFI cases had independent copied variable stores. The firmware hashes and original inventory remain in the initial evidence.

Only guest disk image files were attached. No host block device, host mount, host network/firewall/bootloader change or destructive host installation was used. QEMU SSH forwarding was loopback-only; QMP used private Unix sockets. No VM SSH private key, seed, transport file, guest image or known-hosts file is published in the Git evidence.

The same verified Debian source image/prepared-source SHA-512 values in the initial report were reused. Source image authentication remains verified HTTPS plus publisher checksum; no independent OpenPGP cloud-image signature is claimed. Installer payload/package authentication was not weakened.

## Executed follow-up matrix

Times are UTC; dates are explicit. All installation rows used the host/guest configuration above and `--kernel-update yes --keep-disks yes`. Alpine uses OpenRC/linux-virt; Debian uses systemd/cloud kernel; Arch uses systemd/standard kernel. ext4/XFS cases requested a 64 MiB swapfile, swappiness 10; Btrfs used `@`, zstd and no swapfile.

| Case directory | Date | Start–end UTC | Seconds | Commit | Result / exact stage |
|---|---|---|---:|---|---|
| alpine-bios-08 | Nov 17 | 16:12:23–16:15:44 | 201 | 20423ee | Cargo acceptance PASS; wrapper exit 127 after an in-flight wrapper edit. Not used as the clean final baseline. |
| debian-efi-06 | Nov 17 | 16:17:05–16:21:44 | 279 | 20423ee | PASS: install, target/network, native regeneration and second reboot |
| alpine-bios-09 | Nov 17 | 16:22:45–16:26:22 | 217 | 20423ee | PASS: clean-wrapper baseline rerun, including update reboot |
| debian-efi-07 | Nov 17 | 16:29:40–16:38:08 | 508 | d443b70 | FAIL: harness discarded the entire non-UTF8 console, so did not recognize completion; installed-target SSH independently worked. Guest powered off after diagnosis. Update not reached. |
| debian-efi-08 | Nov 17 | 16:38:21–16:48:34 | 613 | 145c915 | FAIL: update reboot panicked after an intermediate guard change leaked build-only code into the runtime initramfs. Failed guest stopped after evidence capture. |
| debian-efi-09 | Nov 17 | 16:50:54–16:55:53 | 299 | e0e4d89 | PASS: corrected build-only guard, full install/update/reboot |
| debian-recovery-01 | Nov 17 | 16:57:09–16:57:28 | 19 | e0e4d89 target | PASS: guard abort/no hidden writes, previous-generation boot, module and existing target/network probe |
| alpine-recovery-01 | Nov 17 | 16:57:38–16:57:59 | 21 | 20423ee target | PASS: guard abort/no hidden writes, previous-generation boot, module and existing target/network probe |
| alpine-bios-xfs-01 | Nov 17 | 16:58:13–17:01:40 | 207 | e0e4d89 | PASS: XFS installation, swap, update/regeneration/reboot |
| debian-efi-btrfs-01 | Nov 17 | 17:03:28–17:08:51 | 323 | e0e4d89 | PASS: Btrfs installation, @/zstd/no-swap, update/regeneration/reboot |
| arch-efi-ext4-01 | Nov 17 | see saved start; no end | — | e0e4d89 | NOT RUN: interrupted before input validation/guest creation; empty output directory. Resumed with fresh directory on Nov 19. |
| arch-efi-ext4-02 | Nov 19 | 00:04:08–00:05:50 | 102 | e0e4d89 | FAIL before partitioning: signed Alpine packages conflict on `cmd:genfstab` (alpine-conf vs arch-install-scripts) |
| arch-efi-ext4-03 | Nov 19 | 00:10:31–00:12:06 | 95 | 823a6dc | FAIL: world-only fix insufficient; stock initrd still forcibly appended alpine-base |
| arch-efi-ext4-04 | Nov 19 | 00:20:29–00:23:13 | 164 | 91a3574 | FAIL: final target unmount busy, no final reboot |
| arch-efi-ext4-05 | Nov 19 | 00:28:23–00:31:18 | 175 | 4b54f0c | FAIL: same unmount; added diagnostic captures GPG agent with homedir under `/os` |
| arch-efi-ext4-06 | Nov 19 | 00:34:34–00:44:06 | 572 | 4e80080 | FAIL: harness readiness required absent `hostname` utility; installed Arch boot/SSH independently verified. Guest powered off after diagnosis; update not reached. |
| arch-efi-ext4-07 | Nov 19 | 00:44:20–00:47:41 | 201 | cfb79a2 | PASS: full install, selected kernel, storage, SSH/network/DNS, native regeneration and second firmware reboot |

The recovery rows used fresh child overlays of the retained successful VM disks, not the originals. They are supplementary native recovery tests, not substitute installation acceptance. One evidence-filter mismatch in `debian-guard-diag-01` was corrected before `debian-guard-diag-02` reproduced the actual premature-output-file defect. Diagnostic artifact timestamps, commands and logs are retained locally; no invented elapsed timing is assigned to those preliminary diagnostics.

### Verified stage meaning

A full PASS required source public-key SSH/preflight, authenticated preparation, kexec into verified Alpine Live, exact guest disk identity, installation completion, installed Limine/firmware boot, target public-key SSH, selected kernel/distribution, correct root filesystem, UUID fstab, mounted FAT32 `/boot` UUID against boot state, actual IPv4 route/DNS lookup and native package refresh. Native update cases additionally required regeneration/new Limine generations, another boot ID and repeated target/network checks. Source and target serial/QMP/probe logs are retained.

Observed successful kernel releases: Alpine **6.18.63-0-virt**, Debian **6.12.117+deb13-cloud-amd64**, and Arch **7.3.4-arch1-2**. Their native transactions found no newer selected kernel during these runs (Arch reported nothing to do); this establishes native transaction/regeneration/reboot evidence, **not a new-kernel-version transition**.

Final tested code revision: `cfb79a22449f369aa65a0db7c6719eacc33e1629`; final static installer SHA-256: `ba16be2134dafdf4af842479d62731eb264ede4a3f06185753991b488da440be`. Earlier rows are explicitly for their recorded revisions. Subsequent Arch-specific changes did not modify Alpine/Debian target paths; this is not a claim that every combination was rerun at the final revision.

## Defects diagnosed and fixes

### SeaBIOS resets: established cause, not time-based deduplication

A focused reboot of a retained Alpine installation reproduced two raw QMP RESETs for one requested OS reboot. Firmware debug output showed the exact sequence:

```
In resume (status=0)
In 32bit resume
Attempting a hard reboot
SeaBIOS (version 1.16.3-debian-1.16.3-2)
```

SeaBIOS `resume.c:handle_resume32/tryReboot` and `fw/shadow.c:qemu_reboot` explicitly convert a soft reset to a hard reset. Source examined: upstream tag `rel-1.16.3`, commit `a6ed6b701f0a57db0569ab98b0661c12a6ec3ff8`. The original raw-QMP-only criterion was therefore a proven harness false failure.

Commit `20423ee42779dd142d5df76b8d827473713b06b3` captures firmware debug output and permits **two raw resets only for BIOS with exactly one complete matching soft-to-hard sequence**. Extra resets, absent evidence, incomplete sequences and UEFI pairs still fail. No timing window/debounce is used; raw QMP records remain intact. Both installation and native-update reboot checks are enforced. Regression tests cover rejection cases. `--keep-disks yes` retains successful overlays for recovery testing.

### Debian early guard and regression caught by real reboot

The guest-only unmounted-`/boot` test first demonstrated a real failure: native `mkinitramfs` created a zero-byte hidden `/boot/rustaller-guard-test.img` before its ordinary hooks ran. The hook aborted generation, but too late to prevent all writes. `debian-guard-diag-02/guest-action.log` preserves the before/after hashes and failed comparison.

Intermediate commit `d443b70` moved the guard earlier, but used ordinary initramfs configuration. That configuration is copied into the boot image. The real native-update reboot in Debian 08 caught the resulting PID-1 exit/kernel panic; it was **not accepted** despite the generator command succeeding.

Final fix `e0e4d890a8c9dc82d32569450a81c722d8bd059b` uses **`/usr/share/initramfs-tools/conf-hooks.d/00-rustaller-mount`**, sourced before output creation but not copied into runtime configuration. The regular hook remains a backstop. Tests ensure normal `initramfs.conf` is unchanged; the real guest probe checks both hook syntax and absence of build code in runtime configuration. Debian 09 passed installation, native rebuild and reboot; Debian recovery then confirmed abort-before-output and successful previous-generation boot. The intermediate regression is explicitly preserved in history and evidence.

### Serial decoding

Commit `145c915994dde85acabeb0d2508709d0c2ed8506` fixes discarded console evidence when systemd/kernel writes interleave within a UTF-8 sequence. Debian 07's raw serial contains an invalid sequence but intact ASCII completion markers. Raw bytes are preserved; only in-memory marker scanning uses lossy decoding. Regression coverage verifies markers survive without modifying the file. Update-reboot kernel panics now also fail immediately rather than waiting for SSH timeout.

### Arch package conflict

`alpine-conf` and signed `arch-install-scripts` both provide `cmd:genfstab`. Rustaller does not need Alpine setup tools in the Arch Live environment. Commit `823a6dc` selects Alpine's runtime base dependencies without that setup package, retaining OpenRC, mdev, release/trust metadata and normal APK resolution/signatures.

Real rerun showed the verified stock initrd unconditionally appends `alpine-base`, overriding the world-only fix. Commit `91a3574` replaces only that exact assignment for Arch with the same runtime dependency list. Missing/ambiguous anchors fail closed before handoff. No `--force`, broken-world or untrusted-package option was introduced. Other targets retain their original Live base. Real rerun passed APK preparation and installed Arch, exposing a separate final-unmount problem rather than that package conflict.

The retained failure state in case 05 showed `gpg-agent --homedir /os//etc/pacman.d/gnupg --use-standard-socket --daemon` still running while only the target root remained mounted. Commit `4e800803c3f94e0d3f60bd8683e1dd58c0d78253` shuts down the target keyring's agent contexts through native `gpgconf --homedir ... --kill all`, once inside the chroot and once from Live. It neither broadly kills processes nor uses forced/lazy unmount. Case 06 then completed installation, unmounted, rebooted and authenticated SSH.

That case exposed a harness portability failure: minimal Arch lacks the optional `hostname` executable. Commit `cfb79a22449f369aa65a0db7c6719eacc33e1629` reads the actual kernel hostname through `/proc/sys/kernel/hostname` instead, both in readiness and the standalone probe. Case 07 passed every full acceptance stage, including native `pacman -Syu`, `mkinitcpio` regeneration, Limine refresh and the second boot/probe. Native command/signature failures were not ignored.

## Recovery and partial-failure verification

For Alpine and Debian, the successful recovery test:

1. Booted a new disposable overlay and authenticated SSH.
2. Recorded active/previous configuration hashes.
3. Unmounted only the guest's `/boot`, ran the native generator, and required failure from the Rust boot-mount guard.
4. Remounted the expected FAT filesystem and verified the active config hash and hidden `/boot` file list/hashes were unchanged.
5. Selected the existing valid previous Limine config, synced, and requested a guest reboot.
6. Required a new boot ID, authenticated SSH, the expected previous configuration, matching `virtio_net` vermagic/dry-run module resolution, and the repository's existing static target/network/package probe.

Both passed. This is a real previous-**generation** boot with the same kernel release, not proof of fallback across different kernel versions. Every operation was inside a disposable guest. No intentionally corrupted VPS host, host block attachment or host boot change was used.

## Evidence, reproduction and GitHub delivery

The private campaign root is `/root/rustaller-acceptance`; complete logs, images, keys and wrappers remain there. Reviewed key-free evidence is published under `docs/evidence/2026-11-17/`: raw QMP, firmware debug, source inspection/state, target/update probes, accepted records, recovery logs, serial excerpts and hashes of the original full logs. Installation-log copies redact the ephemeral guest web-log URL. Serial excerpts are explicitly excerpts; full raw serial remains private. No SSH keys, seed images, transport settings or qcow2 images are included in Git.

Reproduce using a fresh output name and the existing runner only:

```sh
cd /root/rustaller-acceptance
source env.sh
bash run-case.sh alpine bios alpine-bios-repro-NEW
bash run-case.sh debian efi debian-efi-repro-NEW
FILESYSTEM=xfs bash run-case.sh alpine bios alpine-xfs-repro-NEW
FILESYSTEM=btrfs bash run-case.sh debian efi debian-btrfs-repro-NEW
bash run-case.sh arch efi arch-repro-NEW
```

Run sequentially, not concurrently. `run-case.sh` records complete commands, start/end/elapsed and exit status, invoking `cargo xtask vm-acceptance install` with the preserved source hash, private keys, static installer, explicit KVM/RAM and update options. Recovery uses the existing installed probe and native guest commands; its supplementary scripts/command records are preserved locally. The initial report contains upstream cloud-image provenance and exact source hashes.

Supporting validation: **352 workspace tests passed**, the standalone guest-probe test passed, Clippy with all targets/features and `-D warnings` passed, both formatting checks and `git diff --check` passed, and the matching static host/worker bundle passed validation. Logs end in `final-2026-11-19.log`. These checks do not replace the real guest results.

The requested GitHub delivery is attempted only after verification and the reviewed evidence/report commit, using a normal fast-forward push. Earlier `git push origin main` failed with `could not read Username ... terminal prompts disabled`; no credentials are guessed, printed or published. The actual final push/remote verification and any offline bundle are recorded under `/root/rustaller-acceptance/evidence/` and reported separately to the requester. Publication requires authorized credentials; a verified local commit alone is not called a successful push.

## Remaining limitations and next tests

- Other VPS inventories and multi-host acceptance remain **BLOCKED**: no endpoints/authorized access supplied.
- Gentoo OpenRC/systemd remains **BLOCKED on this role's RAM budget**: its minimum 2048 MiB exceeds the conservative 1536 MiB guest allocation used to preserve VPS headroom. No host swap or unsafe overcommit workaround was used.
- **NOT RUN:** TCG installation, alternate controllers/kernels, remaining filesystem/firmware combinations, BIOS GPT >2 TiB, real 4Kn, Secure Boot/provider-specific boot contracts, IPv6 Internet and multiple-NIC renewal/failover.
- A genuine newer-kernel transition and recovery across two different releases remain unverified. Current-version transactions and previous-generation boots must not be represented as those tests.
- Non-fatal Debian locale fallback-to-C and gzip fallback when zstd is absent remain recorded, not silently claimed fixed. Arch reported missing optional vconsole defaults and disabled an unreliable RDSEED32 CPUID feature; these warnings were not hidden by changing the guest CPU model.
- Next: reproduce accepted baselines on a separately inventoried authorized host, then expand the full matrix on adequately provisioned guests and exercise a two-version kernel/module/recovery transition.
