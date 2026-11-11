# EFI Stub 引导方案评估

状态：用户已取消 EFI Stub，保留 GRUB 的 BIOS/UEFI 路径。以下仅为历史设计记录。
不再将本文作为待实施任务。
当前可执行安装路径仍为 GRUB + XFS；Alpine Live 和 Alpine 目标保留 OpenRC。

## 已选方案的 VPS 兼容性要求

- CPU 范围继续为 x86_64。KVM 是虚拟化平台，不代表 UEFI：SeaBIOS 客体
  不可使用 EFI Stub；OVMF 客体仍须通过 EFI 64 位、Secure Boot、kexec、
  NVRAM 与磁盘启动检查。TUI 不得把 force-boot-mode 当作固件转换能力。
- EFI Stub 安装请求在 BIOS 上清盘前拒绝；不自动改用 GRUB、UKI 或二次
  重启。保留原有独立 GRUB 实现，不将其宣称为 EFI Stub fallback。
- 原系统执行只读预检。Live 清盘前必须重新检查 EFI runtime services、
  efivarfs 与启动项管理能力；挂载 rw 和读取成功均不证明写入成功。
  写入探测需设计可恢复的专用启动项，不能改写无关 BootOrder/BootNext。
  即使探测通过，安装后的写入仍须检查返回值并读回验证，失败不最终重启。
- virtio-blk、virtio-scsi、AHCI/SATA、NVMe 的根设备驱动和 XFS 必须由
  目标内核提供并进入 initramfs；不能只打包 Live 当前用到的设备模块。
  网卡至少验收 virtio-net；e1000、vmxnet3、Hyper-V netvsc 另做对应测试。
- VMware UEFI、Hyper-V Gen2、Xen HVM/UEFI 作为条件支持目标，分别检查
  PVSCSI、storvsc/VMBus、Xen 前端驱动。Xen PV/PVH、容器、外部内核启动、
  已检测到的受限 confidential VM 保持拒绝或待提供商适配。
- 使用实际 XFS UUID 绑定根分区，NVRAM 内核和 initrd 路径均指向目标 ESP。
  串口名和速率不能按平台品牌猜测，保留可配置入口及 tty0 控制台。
- 更新必须生成配对的内核/initramfs，使用版本化 ESP 路径，验证后再切换
  受管理启动项，保留恢复入口和对应模块。不要覆盖发行版自带 mkinitfs。
  Alpine kernel-hooks 提供 /etc/kernel-hooks.d，但其 trigger 可能只报告
  钩子错误后返回成功；必须另设明确更新状态和失败提示，不能把 apk 成功
  当作 ESP 更新成功。核对目标稳定分支的钩子顺序，不能依赖 master 行为。
- EFI Stub 验收必须让 OVMF 从安装磁盘启动；QEMU -kernel 直接加载内核
  不算通过。TCG+OVMF 结果不能替代真实 KVM 或具体 VPS 提供商验收。

最低矩阵：Alpine/OpenRC + OVMF + virtio-blk/virtio-scsi，随后 SATA/NVMe；
每项验证 kexec 后变量可用性、首次磁盘启动、XFS、SSH、路由/DNS、swap、
内核升级、参数修改、冷启动及备用入口。故障用例包括 NVRAM 只读/满/丢失、
ESP 未挂载/空间不足、驱动缺失和目标启动项写入失败。

## A. Linux EFI Stub + NVRAM 启动项

UEFI 加载 ESP 上启用 CONFIG_EFI_STUB 的 Linux 内核，启动项的 LoadOptions
携带内核参数及外部 initramfs 的 ESP 路径。ESP 保持 FAT32，真实根为 XFS。

建议文件布局（Alpine virt 示例）：

```text
/boot/efi/EFI/rustaller/current/vmlinuz-virt.efi
/boot/efi/EFI/rustaller/current/initramfs-virt
/boot/efi/EFI/rustaller/previous/...
/etc/kernel/cmdline
```

示例参数（UUID 由安装器读取，不能照抄占位符）：

```text
root=UUID=<XFS-UUID> rootfstype=xfs rw initrd=\EFI\rustaller\current\initramfs-virt console=tty0 console=ttyS0,115200n8
```

最后一个 console 是主控制台；串口名称/速率应按提供商配置。保留发行版
默认 init，不强制 init=，不添加 dinit。hostname、账户、时区、DNS、swap
属于目标配置，不进入内核命令行。普通 swapfile 不需要 resume 参数。

`/etc/kernel/cmdline` 只是项目约定的参数源，固件不会读取它。修改后必须
由受控更新器刷新管理的 NVRAM 启动项。升级内核时重建 initramfs，验证
配对版本和 EFI PE 头，检查 ESP 挂载/容量，先部署新版本，再更新启动项。
保留上一对内核/initramfs 和恢复启动项；不能宣称自动回滚。

关键前提：kexec 后 Live 的 EFI runtime services 可用且 efivarfs 可写。
必须在清盘前验证；无法满足则停止。旧系统可写 NVRAM 不证明 Live 也可写。
只把内核复制为 EFI/BOOT/BOOTX64.EFI 不能保证外部 initramfs 和命令行被传入。

## B. UKI + fallback 路径

把 EFI stub、内核、initramfs、命令行封装成单个 EFI 文件，部署到
EFI/BOOT/BOOTX64.EFI。无需依靠 NVRAM 来携带参数，更贴近当前 removable/
no-nvram 安装策略。适合 NVRAM 行为不稳定的 VPS，但固件仍须支持 fallback。

参数源同为 /etc/kernel/cmdline；每次修改参数或升级内核后必须重建 UKI。
保留上一份 UKI 和恢复方法，FAT32 文件替换不等于事务或自动回滚。
常用 systemd-stub 属于 EFI 加载组件，不要求 systemd PID 1，兼容 OpenRC。
这不是“固件直接启动原始 Linux EFI Stub 内核”的相同实现，须明确选择。

## C. EFI Stub + 轻量启动管理器

使用支持 Linux EFI Stub 的启动管理器（例如 systemd-boot）管理 ESP 上的
内核/initramfs 和文本 options。参数修改通常无需写 NVRAM，方便选择当前/
备用内核；但增加一个启动管理器，不是纯固件直启。OpenRC 不受影响。

## TUI 与实现范围

- swapfile 大小由 TUI 输入 MiB；启用后焦点移到大小字段，关闭保留草稿但
  不提交。空大小是必填错误。Save/Discard/Cancel 和发行版独立草稿保留。
- New host settings 配置 hostname、时区和内核；账户/SSH、网络/DNS 独立。
  新安装 hostname 留空使用 rustaller，当前系统配置留空保持不变。
- 未来 Disk and boot 应区分固件模式和引导方式：EFI Stub/UKI 仅 UEFI，
  BIOS 明确报不支持或由用户选择现有 GRUB，不静默降级。
- 参数界面建议分为根设备（自动生成）、控制台（类型/端口/速率）、高级
  参数；拒绝覆盖安装器管理的 root/initrd/init 等关键参数，限制长度，
  不允许凭据、任意 Shell 钩子或未经校验的重复键。
- 接通时须同步计划 schema/兼容标记、包策略、ESP 布局、引导安装、最终
  校验、原生内核更新钩子与 VM probe。不能只替换 grub-install 一条命令。
- 先验收 Alpine/UEFI/XFS，再扩展其他发行版。测试首次启动、参数更新、
  内核升级、NVRAM 不可写/丢失、ESP 空间不足及失败不重启。Secure Boot
  仍按当前策略拒绝；签名 UKI 是另一项工作。
