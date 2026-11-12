# EFI Stub 历史评估

**用户已取消此方案，不作为待实施任务。** 当前执行路径为 GRUB BIOS/UEFI + XFS，Alpine Live/目标保留 OpenRC。以下仅保留方案差异与验收要求。

## 兼容性前提

- 仅 x86_64 UEFI；KVM 不等于 UEFI，SeaBIOS 不能使用 EFI Stub，force-boot-mode 不能转换固件。
- BIOS 上须清盘前拒绝，不静默改用 GRUB/UKI 或两次重启。原系统与 kexec 后 Live 分别检查 EFI runtime、efivarfs 及启动项管理能力。
- efivarfs 可读/挂载 rw 不证明可写。探测须用可恢复专用项，不改无关 BootOrder/BootNext；正式写入仍检查返回值和读回，失败不重启。
- 目标内核/initramfs 须含 XFS、根盘控制器和 NIC 驱动；Live 当前加载模块不构成完整清单。VMware/Hyper-V/Xen HVM 须分别验证 PVSCSI、storvsc/VMBus、Xen 前端；PV/PVH、容器、外部内核及受限 confidential VM 拒绝或另行适配。
- 根以 XFS UUID 绑定，内核/initrd 指向目标 ESP。串口按提供商配置，保留 tty0；不按品牌猜参数。
- 更新采用版本化内核/initramfs 配对，验证后切换受管理启动项，保留旧版本和恢复入口。Alpine kernel-hooks 的错误可能不使 apk 失败，须独立检查更新状态及稳定分支钩子顺序。
- OVMF 必须从安装磁盘启动；QEMU `-kernel` 不算验收，TCG 不替代真实平台/提供商测试。

## A. EFI Stub + NVRAM

固件加载启用 `CONFIG_EFI_STUB` 的内核，通过 LoadOptions 传入命令行和外部 initramfs。FAT32 ESP、XFS 根，建议布局：

```text
/boot/efi/EFI/rustaller/current/vmlinuz-virt.efi
/boot/efi/EFI/rustaller/current/initramfs-virt
/boot/efi/EFI/rustaller/previous/...
/etc/kernel/cmdline
```

参数示例中的 UUID 须读取实际目标值：

```text
root=UUID=<XFS-UUID> rootfstype=xfs rw initrd=\EFI\rustaller\current\initramfs-virt console=tty0 console=ttyS0,115200n8
```

最后一个 console 为主控制台。保留原生 init，不强制 init= 或 dinit；账户、hostname、时区、DNS、swap 不入 cmdline，普通 swapfile 无需 resume。

`/etc/kernel/cmdline` 是更新器的参数源，固件不读取它。参数修改/内核升级后须重建配对 initramfs，验证 EFI PE 头和 ESP 挂载/容量，再部署、更新启动项；保留旧版本不等于自动回滚。

清盘前须证明 kexec 后 Live 能写 EFI 变量，不能以原系统可写代替。仅复制 `EFI/BOOT/BOOTX64.EFI` 不保证命令行和外部 initramfs 传入。

## B. UKI + fallback

将 stub、内核、initramfs、命令行封装为单个 EFI 文件，放入 `EFI/BOOT/BOOTX64.EFI`，无需 NVRAM 携带参数但仍依赖固件 fallback。参数或内核更新须重建 UKI，并保留旧版/恢复方法；FAT32 替换无事务或自动回滚保证。systemd-stub 不要求 systemd PID 1，可配 OpenRC；与固件直启原始内核不同。

## C. 轻量启动管理器

如 systemd-boot，在 ESP 上管理内核/initramfs 与文本 options，便于参数更新和备用版本选择，但增加了启动管理器，不是纯固件直启；OpenRC 不受影响。

## 原设计的界面与验收范围

保留现有独立目标草稿和 Save/Discard/Cancel。swapfile 按 MiB 输入，启用后转至大小字段，空值必填；关闭保留但不提交。新安装 hostname 留空为 rustaller，当前系统留空不变。

若未来重新授权，须分开固件模式/引导方式，限制 root/initrd/init 等管理参数，拒绝凭据、Shell 钩子、过长值和冲突重复键；同步 schema/标记、包策略、ESP、引导校验、内核更新钩子及 VM probe，不能只替换 grub-install。

最小矩阵为 Alpine/OpenRC + OVMF + virtio-blk/scsi，再扩展 SATA/NVMe 和其他发行版/NIC。验证 kexec 后 EFI 变量、磁盘冷启动、XFS、SSH、路由/DNS、swap、内核/参数更新与备用入口；覆盖 NVRAM 只读/满/丢失、ESP 缺失/不足、缺驱动及写项失败。Secure Boot 仍拒绝，签名 UKI 另行设计。
