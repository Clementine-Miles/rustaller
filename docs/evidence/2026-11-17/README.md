# Reviewed VM acceptance evidence

Campaign started 2026-11-17; Arch follow-up completed 2026-11-19. See
[the acceptance follow-up](../../VERIFICATION-FOLLOWUP-2026-11-19.md) for exact
revisions, host-kernel conditions, hardware, scope and limitations.

Five installation cases passed, including native regeneration/reboot probes:

- `alpine-bios-09`: ext4 / OpenRC / linux-virt.
- `debian-efi-09`: ext4 / systemd / cloud kernel.
- `alpine-bios-xfs-01`: XFS / OpenRC / linux-virt.
- `debian-efi-btrfs-01`: Btrfs / systemd / cloud kernel.
- `arch-efi-ext4-07`: ext4 / systemd / standard kernel.

`alpine-recovery-01` and `debian-recovery-01` additionally booted a selected
previous generation, checked a sampled module's vermagic and reran the existing
installed-target probe. Their native generators also rejected unmounted `/boot`
without changing the active configuration or underlying files. This is not a
claim of a transition between different kernel versions.

`debian-guard-diag-02` is deliberately **failed evidence**, not a passing case:
it demonstrated that the old late hook permitted creation of a zero-byte output
file before aborting. The early build-only guard and its successful rerun are
explained in the follow-up report.

## What is included

Full QMP event records and firmware debug logs, source inspection/state,
installed-target/native-update probes, acceptance records, recorded timing,
recovery-action output and selected serial excerpts. Empty EFI debug logs are
expected: the SeaBIOS debug port is not an OVMF log interface.

`live-install-redacted.log` is a copy with its ephemeral guest web-log URL
redacted. `serial-excerpt.log` is explicitly an excerpt (terminal cursor controls
removed), not a full serial recording. `original-log-sha256.txt` hashes the
original private logs before redaction or extraction, so hashes need not match
a redacted/excerpt file. The raw originals are retained under the case paths in
`/root/rustaller-acceptance` on the test host.

Log attributes preserve captured bytes, padding and CRLF rather than rewriting
raw evidence to satisfy source-code whitespace rules. Source and Markdown
whitespace checks remain enabled.

No VM SSH key, known-hosts file, seed, transport file, guest image or credential
is included. Public proof is supplementary to, not a replacement for, the
recorded real QEMU acceptance executions. Unit/mock/build success alone is never
an installation PASS.
