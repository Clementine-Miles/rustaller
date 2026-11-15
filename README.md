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
身份确认令牌；`--inspect` 只读。支持 XFS/ext4/Btrfs、发行版内核、SSH/账户、DNS/网络策略
及可选目标 swapfile。自动流程要求 kexec，最后仅重启一次；失败保留 Live 救援。
新系统统一使用 Limine 9.6.5，默认 FAT32（`/boot`）加根分区；BIOS 大盘使用带专用
引导分区的 GPT。Btrfs 使用 `@` 和可选 zstd，暂不支持 Btrfs swapfile。
TUI 对照展示检测到的当前系统与安装计划；保留 reset 基础设施，但执行仍不可用。

只读主机检查涵盖客户机可见的 CPU/ISA 级别、内存与 swap、存储、虚拟化、固件及网络，
不运行基准测试或第三方脚本。Arch 新增 hardened、Zen 内核；Gentoo 通过
`--gentoo-init` 选择经过认证的 `amd64-nomultilib-openrc`（新计划默认）或
`amd64-nomultilib-systemd` stage3。根子卷快照不包含 FAT32 `/boot`。
原生内核更新钩子及新增内核/init 组合仍需真实虚拟机重启、SSH 和网络验收。

当前原系统网络捕获要求已生效、显式配置的单文件 networkd 配置；不支持的网络管理器
或路由策略会在清盘前终止准备。

- [架构](docs/ARCHITECTURE.md)
- [安装、要求与使用方法](docs/INSTALLATION.md)
- [目标系统配置](docs/CONFIGURATION.md)
- [安全与信任边界](docs/SECURITY.md)
- [测试与实际验收限制](docs/TESTING.md)
- [开发、构建、发布与来源](docs/DEVELOPMENT.md)

[English](README.en.md) · [许可证](LICENSE)
