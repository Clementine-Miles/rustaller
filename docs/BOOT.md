# rustaller-boot

Rustaller builds its x86-64 Linux bootloader with the release. Alpine Live receives the firmware images and the static update helper inside the verified worker. Both boot paths use the FAT32 partition mounted at `/boot`. A release records the SHA-256 of each embedded artifact alongside the source revision and runtime version.

## Firmware

UEFI starts `EFI/BOOT/BOOTX64.EFI` from the ESP fallback path. The loader opens the filesystem of its own image handle, checks the kernel's x86 Linux setup and PE32+ EFI application headers and requires the Linux EFI stub's initrd media support. Firmware loads the kernel from its full device path with `LoadImage`. The loader supplies NUL-terminated UTF-16 load options and starts its normal EFI entry point with `StartImage`. The Linux stub requests the ordered initrd stream through `EFI_LOAD_FILE2_PROTOCOL` on the Linux initrd vendor media device path. Rustaller requires disabled Secure Boot. Installation leaves boot selection to the firmware fallback path.

SeaBIOS and compatible PC BIOS firmware enter a 440-byte bootstrap. It reads the secondary loader using INT 13h EDD on the firmware boot drive. BIOS installation requires the existing 512-byte logical-sector geometry. The secondary loader queries E820, reads FAT32 including long filenames, parses the shared configuration, loads a relocatable x86-64 bzImage and constructs Linux's zero page. It reserves nonoverlapping RAM for the kernel's full decompressor workspace, initrd, command line and parser. It enters the 32-bit protected-mode Linux entry with paging and interrupts disabled. The kernel transitions to long mode.

On BIOS/MBR, stage two occupies the gap before partition 1. On BIOS/GPT, it occupies the dedicated BIOS Boot partition. The installer checks the on-disk partition table against the plan and reads back the written stage before publishing stage one. The disk signature, partition entries and MBR signature remain intact. EFI supports the existing 512-byte and 4096-byte logical-sector layouts.

## Boot configuration

The active file is `/boot/rustaller/boot.toml`. Paths in it start at the root of the FAT volume. The example uses illustrative generation names. The installer supplies the actual root UUID and immutable generation paths.

```toml
schema_version = 1

[boot]
default = "current"
timeout_seconds = 3
serial = true
serial_baudrate = 115200

[linux]
root = "UUID=01234567-89ab-cdef-0123-456789abcdef"
rootfstype = "btrfs"
rootflags = "subvol=@"
mode = "rw"
consoles = ["ttyS0,115200n8", "tty0"]
extra_cmdline = ""

[entries.current]
version = "6.12.107-cloud-amd64"
kernel = "/rustaller-42-100/kernel-0"
initrds = ["/rustaller-42-100/microcode-0", "/rustaller-42-100/initrd-0"]

[entries.previous]
version = "6.12.106-cloud-amd64"
kernel = "/rustaller-41-999/kernel-0"
initrds = ["/rustaller-41-999/initrd-0"]
```

The default Linux command line is `root=UUID=<root-uuid> rootfstype=<type> rw console=ttyS0,115200n8 console=tty0`. Btrfs adds `rootflags=subvol=@`. Edit `[linux]` to change the generated arguments. For example, `extra_cmdline = "panic=15"` appends one argument. Managed arguments such as `root=`, `console=`, `ro` and `rw` have dedicated fields. An explicit `cmdline_override` string replaces the entire generated line.

Both loaders use the same maintained TOML parser and policy code. Limits are 32 KiB for the configuration, 2047 printable ASCII bytes for the Linux command line, 240 bytes for a FAT path, three initrd files per entry and 16 entries across two generations. The timeout ranges from 0 to 60 seconds. Serial rates are 9600, 19200, 38400, 57600 and 115200. These settings control the loader's optional serial interface. Firmware console redirection and the kernel's `consoles` setting are independent. The displayed key selects an entry once. Enter or an expired timeout selects `boot.default`.

## Kernel updates and recovery

The distribution owns its normal kernel and initramfs files in `/boot`. Apk/mkinitfs, Debian kernel hooks/initramfs-tools, pacman/mkinitcpio and Gentoo installkernel/dracut invoke `/usr/local/libexec/rustaller-boot-update`. Native hooks verify the mount point, filesystem root, FAT32 boot record and recorded volume serial before writing. The helper selects the newest complete kernel from the recorded installation package family for `current`. Additional complete kernels remain available by name. A legacy installation can infer its family from the existing current entry if recognizable. Microcode precedes the initramfs. Identical payloads already in a managed generation are reused after byte comparison.

Refresh reasserts the root UUID and filesystem from install state. It preserves
Btrfs settings, user arguments, comments and explicit command-line overrides.
It validates and syncs the complete generation before publishing its entries.

Allow room for the native images, the retained generations and the incoming generation plus 1 MiB of metadata reserve. A 512 MiB boot partition can run out of space during an upgrade. The exact requirement depends on the actual kernel and initramfs sizes. Publication fails if a native pair is incomplete or there is insufficient space.

Select `previous` in the loader menu to use the last independently published
generation. Both loaders can also try it once when the selected entry cannot be
prepared, for example when its kernel or initramfs is missing. UEFI stops at
`StartImage` and BIOS stops at the Linux entry point. A kernel that starts and
hangs or panics requires manual recovery.

A validated recovery document, `/boot/rustaller/boot.toml.previous`, accompanies
a published update. An unreadable primary configuration uses this recovery
document. A malformed user edit stops automatic publication. From rescue,
mount the expected boot filesystem, verify the saved configuration and its
referenced payloads, then restore the recovery file as `boot.toml`. After
repairing the native kernel or initramfs, run these commands.

```console
/usr/local/libexec/rustaller-boot-update check
/usr/local/libexec/rustaller-boot-update refresh
/usr/local/libexec/rustaller-boot-update verify
```

FAT power loss can damage multiple files. Keep an independent recovery console and backup. A previous kernel can require its matching module tree for drivers loaded after the initramfs. A Btrfs root snapshot excludes this partition. Restore a matching kernel, modules and initramfs when rolling back the root.

## Build

```console
rustup target add x86_64-unknown-linux-musl x86_64-unknown-uefi i686-unknown-linux-gnu
cargo xtask check
cargo xtask bundle
```

The build machine needs a Linux x86 GNU linker driver and binutils (`as`, `ld`, `objcopy`). Linux release executables use static musl. The BIOS build links freestanding Rust and x86 assembly at physical address 0x10000. Ordinary host builds use the workspace's default members. Only the release build embeds deployable firmware. Existing installations keep the helper matched to their boot state.

The firmware interfaces follow the [Linux x86 boot protocol](https://docs.kernel.org/arch/x86/boot.html) and [Linux EFI stub documentation](https://docs.kernel.org/admin-guide/efi-stub.html).
