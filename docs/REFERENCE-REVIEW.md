# debi / reinstall 对照审查

本轮读取用户指定的 bohanwood/debi 与 bin456789/reinstall 的 README 和安装
脚本，通过 GitHub connector 获取；未执行参考项目安装入口，也未修改保留的
Shell 四资源或固定上游对拍基线。

## 参考材料与取舍

- [bohanwood/debi README](https://github.com/bohanwood/debi/blob/master/README.md)
  与 [debi.sh](https://github.com/bohanwood/debi/blob/master/debi.sh)：围绕最小
  Debian、显式网络/账户/主机名配置、原生安装器与可检查的安装配置组织流程。
  本项目借鉴配置完整性与原生工具原则，继续保留 debootstrap 和 Alpine Live；
  不重新引入 Debian Installer 或额外固件重启。
- [reinstall README](https://github.com/bin456789/reinstall/blob/main/README.md)
  与 [trans.sh](https://github.com/bin456789/reinstall/blob/main/trans.sh)：强调
  VPS 特殊路由、磁盘身份、BIOS/EFI、设备驱动及安装环境可恢复性。本项目已有
  MAC 绑定/networkd on-link 路由、磁盘身份检查、恢复 SSH；本轮补齐一个上游
  明确处理的 Hyper-V PCI 驱动路径，不按平台名称推断兼容性。
- 不复制 README 的“全支持”表述，不扩大到 Windows/ARM，不降低签名校验，
  不启用任意脚本钩子，不移植地域探测。EFI Stub 已取消。

本次读取的 Git blob SHA（不是仓库 commit）：

| 文件 | blob SHA |
|---|---|
| debi README.md | 129dcfd2e52c9ada725ba7bbc62f05f37229b43c |
| debi debi.sh | 10ebcd2141f23271f59eb46e4174539f74fc9293 |
| reinstall README.md | b22b3a2ea9ec92315424bf3864695be3e8832aa5 |
| reinstall trans.sh | 345718c2e6e6277fd694f102cd547f3fc15e6bc6 |

## 已修复

1. 原来最终 initramfs 重建完成后才生成目标 fstab。现在在 configure 的内核
   配置阶段之前写入根分区/ESP UUID，供该阶段原生工具读取；bootstrap 中已
   运行的钩子不因此被追溯保证，最终重建仍然必须保留。
2. 原来 hostname 仅写 /etc/hostname，没有为新目标生成匹配的本地解析记录。
   现在为新目标写 localhost、127.0.1.1 主机名及 IPv6 loopback；仅限全新
   安装路径，不改当前系统配置入口的 hosts 文件。
3. Alpine initramfs 自定义模块增加 pci-hyperv 路径，补充已有 VMBus 路径。
   驱动若未包含在目标内核中仍不能凭配置生成，Hyper-V 实机验收仍待完成。
4. 最终验证原先只检查 GRUB 配置文件存在且非空。现在增加 chroot 内的
   grub-probe --target=fs /，要求识别为 xfs，以及 grub-script-check；错误
   阻止最终重启。这些检查不证明固件或内核实际能够启动。
5. 纠正文档中“示例只执行预检”的过时陈述。实际安装命令已包含清盘动作；
   只读检查必须显式使用 --inspect。

## 验证及剩余边界

277 workspace + 49 Shell 测试通过；含错误文件系统、无效 GRUB 配置、hostname
解析与重复写入的回归测试。Rustfmt、Clippy、ShellCheck、Bash/ash 和 diff 检查
通过。日志在 /workspace/scratch/reference-check.log。

未运行新的完整 VM 安装。P0 Live 修复复验、四发行版 BIOS/UEFI 安装矩阵、
XFS swap 实际激活、提供商特殊网络及真实 KVM/Hyper-V/VMware 验收仍待完成。
本轮是配置和安装收尾路径的针对性审查，不是全项目无缺陷认证。
