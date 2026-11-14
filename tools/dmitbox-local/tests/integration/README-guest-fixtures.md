# Minimal native-init fixtures

These development fixtures boot a real Linux kernel under QEMU TCG with a persistent ext4 disk. They exercise systemd and OpenRC services, the public CLI's identified serial-console origin, independent worker readiness, forced reboot while a file transaction is pending, boot-time rollback/content readback, and real ENOSPC/read-only rejection. They do **not** constitute full Debian, Arch or Alpine image acceptance, external Internet access proof, or KVM/performance evidence.

The runner performs no downloads or host mounts. Prepare reviewed inputs explicitly:

- An x86-64 Debian-compatible host with native Bash, jq, flock, coreutils, acl, systemctl/systemd and their shared libraries. The systemd fixture copies these native programs and records the input artifact hash.
- A reviewed static BusyBox executable, supplied using `DMITBOX_DEV_STATIC_BUSYBOX`.
- An official Linux `6.12.107+deb13-amd64` kernel package, including modules, extracted under `.cache/vm-validation/tools/`; the fixture uses its ext4/virtio dependencies.
- QEMU x86, SeaBIOS, e2fsprogs, attr and native dependencies extracted under that same tools directory. Preserve official signed repository metadata and verify package SHA-256 values before extraction. The fixture uses TCG, never `/dev/kvm`.
- A separate prepared Alpine root with native Bash, jq, GNU coreutils, acl, attr, util-linux, OpenRC and its dependencies. For this run, Alpine 3.22 minirootfs and OpenRC 0.62.6 were used. Verify minirootfs/repository signatures and install packages into that private root with native `apk`; do not alter the host.

Run against a stable built artifact:

```sh
python3 build/build.py
DMITBOX_DEV_STATIC_BUSYBOX=/absolute/path/to/static-busybox \
DMITBOX_DEV_ALPINE_ROOT=/absolute/path/to/prepared-alpine-root \
  bash tests/integration/guest-matrix.sh
```

`DMITBOX_DEV_VM_CACHE` may select a private development input cache. These variables affect only the development harness; they are absent from the distributed script's privileged configuration interface. Missing inputs produce exit 77, never an implicit installation or a passing result.

The builder writes fresh disposable initramfs files and the runner creates separate empty disks for both backends. No guest NIC is attached in the recovery tests. Each backend boots twice: first to apply the owned `recovery-probe` and force a kernel reset, then to prove a changed boot ID, `ROLLED_BACK`, and the original resource bytes. During the second boot, another probe is left pending while the isolated guest wall clock jumps forward and backward; the test then requires actual deadline rollback and original content readback using the unchanged monotonic clock. The host clock is never changed. A zero QEMU exit status alone does not pass. Fixture challenges are redacted from retained serial logs. Successful test disks are removed; failed disks remain in the private report directory for diagnosis.

The additional `ssh-guest.py` harness exercises real OpenSSH connection identity and fresh-authentication confirmation through a localhost-only QEMU host forward with a pinned ephemeral guest host key. Its separate report must be inspected independently. It validates confirmation primitives, not coordinated SSH port retirement or Internet routability.

Results and exact artifact/input hashes appear under `artifacts/guest-fixtures-*/`. Never publish private test disks, ephemeral private keys, or raw unsanitized debug traces. The strict release matrix must continue to mark unexecuted full-image, manager, external-connectivity and performance scenarios as unverified.
