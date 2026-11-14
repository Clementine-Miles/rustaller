# EFI Stub historical assessment

**The user has canceled this plan and it is not a task to be implemented.。** The current execution path is GRUB BIOS/UEFI + XFS，Alpine Live/target retention OpenRC。Only the program differences and acceptance requirements are retained below:。

## Compatibility prerequisites

- only x86_64 UEFI；KVM not equal to UEFI，SeaBIOS cannot be used EFI Stub，force-boot-mode Unable to convert firmware。
- BIOS Reject before going to liquidation, do not change silently GRUB/UKI Or reboot twice. Original system and kexec back Live Check separately EFI runtime、efivarfs and startup item management capabilities。
- efivarfs readable/mount rw No proof can be written. Detection must use the recoverable special item, it does not matter if it is not changed BootOrder/BootNext；Formal writing still checks the return value and readback, and does not restart if it fails.。
- target kernel/initramfs must include XFS、root disk controller and NIC driver；Live The currently loaded modules do not constitute a complete manifest。VMware/Hyper-V/Xen HVM must validate separately PVSCSI、storvsc/VMBus、Xen front end；PV/PVH、Containers, external kernels and restricted confidential VM Reject or otherwise adapt。
- Rooted in XFS UUID binding, kernel/initrd point to target ESP。The serial port is configured according to the provider and reserved. tty0；Guess parameters without branding。
- Update using versioned kernel/initramfs pair; switch managed boot entries after verification and preserve the previous version and recovery entry。Alpine kernel-hooks The error may not make apk Failure, you must independently check the update status and stable branch hook sequence。
- OVMF Must boot from installation disk；QEMU `-kernel` Not counting acceptance，TCG Not a replacement for the real platform/Provider testing。

## A. EFI Stub + NVRAM

Firmware loading enabled `CONFIG_EFI_STUB` The kernel, through LoadOptions Pass in command line and external initramfs。FAT32 ESP、XFS root, suggested layout：

```text
/boot/efi/EFI/rustaller/current/vmlinuz-virt.efi
/boot/efi/EFI/rustaller/current/initramfs-virt
/boot/efi/EFI/rustaller/previous/...
/etc/kernel/cmdline
```

in parameter example UUID must read actual target values：

```text
root=UUID=<XFS-UUID> rootfstype=xfs rw initrd=\EFI\rustaller\current\initramfs-virt console=tty0 console=ttyS0,115200n8
```

the last one console Main console. Keep native init，Not mandatory init= or dinit；Account、hostname、time zone、DNS、swap Not allowed cmdline，ordinary swapfile No need resume。

`/etc/kernel/cmdline` is the parameter source for the updater, it is not read by the firmware. Parameter modification/Pairing must be rebuilt after kernel upgrade initramfs，verify EFI PE Head and ESP mount/Capacity, then deploy and update startup items; retaining the old version does not mean automatic rollback。

Proof required before winding up kexec back Live can write EFI Variables cannot be replaced by writable ones from the original system. Copy only `EFI/BOOT/BOOTX64.EFI` Command line and external initramfs incoming。

## B. UKI + fallback

Will stub、Kernel、initramfs、The command line is encapsulated into a single EFI file, put in `EFI/BOOT/BOOTX64.EFI`，No need NVRAM Carrying parameters but still relying on firmware fallback。Parameter or kernel updates require rebuilding UKI，and keep the old version/Recovery method；FAT32 Replace no transaction or automatic rollback guarantee。systemd-stub Not required systemd PID 1，Available OpenRC；Different from firmware directly booting the original kernel。

## C. Lightweight startup manager

like systemd-boot，exist ESP management kernel/initramfs with text options，It is convenient for parameter updating and alternative version selection, but it adds a startup manager and is not a pure firmware direct startup.；OpenRC not affected。

## Original designed interface and acceptance scope

Keep existing independent goal drafts and Save/Discard/Cancel。swapfile according to MiB Enter, go to the size field when enabled, empty values ​​are required; turn off to retain but not submit. New installation hostname Leave blank for rustaller。

If you re-authorize in the future, you must separate the firmware mode/Guidance methods, restrictions root/initrd/init Wait for administrative parameters, deny credentials、Shell hooks, excessive lengths and conflicting duplicate keys; synchronize schema/Tags, package strategies、ESP、Boot verification, kernel update hooks and VM probe，Can’t just replace grub-install。

The minimum matrix is Alpine/OpenRC + OVMF + virtio-blk/scsi，Expand again SATA/NVMe and other distributions/NIC。verify kexec back EFI Variable, disk cold start、XFS、SSH、routing/DNS、swap、Kernel/Parameter update and alternate entry; override NVRAM read only/Full/lost、ESP Missing/Insufficient, missing driver and writing failure。Secure Boot Still refuse, sign UKI Design separately。
