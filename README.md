# Rustaller

轻量、以 Rust 为主的 Linux x86_64 重装工具，源自
[bin456789/reinstall](https://github.com/bin456789/reinstall)，采用 GPL-3.0-only。
程序检查原系统、认证安装资源，经 kexec 进入 Alpine Live，安装 Alpine、Debian、
Arch 或 Gentoo，并配置新系统。

**当前为具有真实清盘能力的开发预览，四发行版 BIOS/UEFI 完整验收尚未完成。
请备份数据并保留独立控制台/VNC，不要用于唯一生产机。**

| 目标发行版 | 固定内核包 | init |
| --- | --- | --- |
| Alpine | `linux-virt` | OpenRC |
| Debian | `linux-image-cloud-amd64` | systemd |
| Arch | `linux-cachyos-server` | systemd |
| Gentoo | `sys-kernel/gentoo-kernel-bin` | OpenRC |

```console
cargo xtask bundle
./target/x86_64-unknown-linux-musl/release/rustaller --help
./target/x86_64-unknown-linux-musl/release/rustaller debian --inspect
```

在交互终端无参数运行时打开英文 TUI。实际安装需要 root、通过预检并输入完整磁盘
身份确认令牌。安装或通过 SSH 进入 Live 救援需要至少一个有效的 `--ssh-key`。
SSH 仅允许公钥认证。默认账户 `admin` 具备免密码 sudo，且禁止 root 直接 SSH 登录。
如需使用 root 账户，须明确选择 `--username root`。
`--inspect` 只读，无需提供公钥。根文件系统默认为 XFS，也可选择 ext4，支持 DNS/网络策略
及可选目标 swapfile。自动流程要求 kexec，最后仅重启一次；失败保留 Live 救援。
新系统统一使用 [rustaller-boot](docs/BOOT.md)，默认 FAT32（`/boot`）加根分区；BIOS 大盘使用带专用
引导分区的 GPT。
TUI 对照展示检测到的当前系统与安装计划。安装默认采用 Minimal 配置。
在切换内核前可以运行 `rustaller reset`，取消已准备的安装并保留原系统运行。

只读主机检查涵盖客户机可见的 CPU/ISA 级别、内存与 swap、存储、虚拟化、固件及网络，
不运行基准测试或第三方脚本。Arch 需要经过认证的 CachyOS 软件源，并按虚拟机可用的
CPU 指令集选择仓库。Gentoo 使用经过认证的 `amd64-nomultilib-openrc` stage3。
测试文档区分当前验收与历史结果。

支持从已验证的 networkd、NetworkManager、ifupdown 和 dhcpcd 以太网配置继承策略。
也可以按 MAC 地址在 CLI 或 TUI 设置静态地址、DHCP 和 IPv6 RA。
Wi-Fi 重连当前需要另行支持。[网络配置与限制](docs/CONFIGURATION.md)

- [架构](docs/ARCHITECTURE.md)
- [安装、要求与使用方法](docs/INSTALLATION.md)
- [目标系统配置](docs/CONFIGURATION.md)
- [安全与信任边界](docs/SECURITY.md)
- [测试与实际验收限制](docs/TESTING.md)
- [历史 QEMU 验收报告与证据](docs/VERIFICATION-FOLLOWUP-2026-11-19.md)
- [开发、构建、发布与来源](docs/DEVELOPMENT.md)

[English](README.en.md) · [许可证](LICENSE)
