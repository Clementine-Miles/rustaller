# Rustaller

轻量、以 Rust 为主的 Linux x86_64 重装工具，源自
[bin456789/reinstall](https://github.com/bin456789/reinstall)，采用 GPL-3.0-only。
程序检查原系统、认证安装资源，经 kexec 进入 Alpine Live，安装 Alpine、Debian、
Arch 或 Gentoo，并配置新系统。

**当前为具有真实清盘能力的开发预览，四发行版 BIOS/UEFI 完整验收尚未完成。
请备份数据并保留独立控制台/VNC，不要用于唯一生产机。**

```console
cargo xtask bundle
./target/x86_64-unknown-linux-musl/release/rustaller --help
./target/x86_64-unknown-linux-musl/release/rustaller debian --inspect
```

在交互终端无参数运行时打开英文 TUI。实际安装需要 root、通过预检并输入完整磁盘
身份确认令牌；`--inspect` 只读。支持 XFS/ext4、发行版内核、SSH/账户、DNS/网络策略
及可选目标 swapfile。自动流程要求 kexec，最后仅重启一次；失败保留 Live 救援。
netboot.xyz 和本地 ISO 已接入需明确确认的 GRUB 单次启动流程；reset 不可执行。
请先阅读安装文档中的限制；实际介质启动验收仍未完成。

当前原系统网络捕获要求已生效、显式配置的单文件 networkd 配置；不支持的网络管理器
或路由策略会在清盘前终止准备。

- [架构](docs/ARCHITECTURE.md)
- [安装、要求与使用方法](docs/INSTALLATION.md)
- [目标系统配置](docs/CONFIGURATION.md)
- [安全与信任边界](docs/SECURITY.md)
- [测试与实际验收限制](docs/TESTING.md)
- [开发、构建、发布与来源](docs/DEVELOPMENT.md)

[English](README.en.md) · [许可证](LICENSE)
