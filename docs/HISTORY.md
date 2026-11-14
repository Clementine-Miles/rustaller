# Summary of historical decisions

This article summarizes old plans, designs, and continuity records. The current implementation basis is `AGENTS.md`、[Architecture](ARCHITECTURE.md)、[Continuation Guide](AGENT-HANDOFF.md)and latest user instructions。

- upstream benchmark `98f1e21013233d42750cc2e7a298a0818461d686`；reserve GPLv3 with signature。
- Shell delete Windows/ARM、Region detection、Debian Installer；Four goals unified Alpine Live，Debian use debootstrap minbase，Gentoo reserve systemd stage3、native kernel vs. GRUB。
- Shell reduce deployment assets from nine to four; inline helpers and service templates; retain the pinned Git Object byte-by-byte comparison。
- Rust Use static host + Embedded without interface worker，Installed using the distribution’s native tools; not using earlier clap/serde/tokio proposal。
- 2026-10-25/26：Add English multi-level TUI、Target draft, native ssh-keygen、The current system is configured and connected to real liquidation scheduling; subsequent authorization targets swapfile and isolation VM Destructive acceptance, development host and production disk are still prohibited as test objects。
- 2026-10-28：src document 60→48，Integrated configuration/package policy/storage/Metadata, child process output is directly inherited. Measurements see [REFACTOR.md](REFACTOR.md)。
- 2026-10-29：Increase UEFI/Virtualization check、FAT32 ESP、initramfs driver；VM Discover and fix OpenRC Backend logs and RAM modloop Positioning, lack of complete installation acceptance after repair。
- 2026-10-30：src document 48→44，unified Live Command default value and document continuation entry, no new runtime dependencies or unsafe。
- 2026-11-02：use XFS root、GNU dd Target swapfile，reserve GRUB/Alpine OpenRC；EFI Stub Canceled, the plan remains only as history。
- 2026-11-11：Source code import `Clementine-Miles/rustaller`，Unify project naming and GPLv3，Keep upstream claims。

Historical source code/Audit snapshot、VM images and logs are not distributed with source; do not assume previous workspace paths exist. The behavior baseline is `tests/verify` from fixed Git Object reading, see the obtaining method[Publishing Guidelines](GITHUB-RELEASE.md)。The old volume and test number only represent the corresponding version。
