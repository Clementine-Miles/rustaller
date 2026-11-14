# rustaller — 个人 VPS 重装工具（Linux-only）

基于 [bin456789/reinstall](https://github.com/bin456789/reinstall)，上游基准为 `98f1e21013233d42750cc2e7a298a0818461d686`。采用 GNU GPLv3（`GPL-3.0-only`），见 [LICENSE](LICENSE)；保留上游源码声明与署名。

静态 Rust/musl 入口提供英文多级 TUI，支持 Alpine、Debian、Arch、Gentoo，经认证的 Alpine Live 和 kexec 执行安装。**当前为含真实清盘操作的开发预览，四系统 BIOS/UEFI、文件系统启动与 swap 激活尚未完成验收。**

> 自动安装会清空目标磁盘的全部分区。请先备份并准备控制台/VNC，不要直接用于唯一生产机。
> Rust 通过 kexec 进入 Live，最后重启进入目标系统；UEFI 使用 GRUB 回退路径。保留的 Shell 引导/reset 接口还可能写 NVRAM/BootNext。

[English](README.en.md) · [架构](docs/ARCHITECTURE.md) · [配置](docs/CONFIGURATION.md) · [TUI](docs/TUI.md) · [构建与发布](docs/GITHUB-RELEASE.md) · [验收状态](docs/ACCEPTANCE.md)

## Rust 入口

```console
cargo xtask bundle
./target/x86_64-unknown-linux-musl/release/rustaller
```

无参数且连接交互终端时打开英文 TUI。预编译程序的下载方式见[发布指南](docs/GITHUB-RELEASE.md)，VPS 无需 Rust 工具链。

先用 `./rustaller debian --inspect` 只读查看磁盘、固件和确认令牌。实际安装需要 SSH 公钥或密码及完整磁盘身份确认；非交互模式须传入 `--confirm-erase`。程序内嵌匹配 worker 和日志页面，认证官方资源，无需自建 `confhome`。

可选内核、XFS/ext4 根和公共 DNS/自定义 DNS。创建非 root 用户须另设 root 密码，且不开放 root SSH；TUI 与交互 CLI 提供私密输入。主机名优先继承原系统，检测不到时用发行版名称；手动设置优先。内核映射和 CachyOS 首次开机更新见[目标配置](docs/POSTINSTALL.md)。

原系统 PID 1 须为 systemd 或 openrc-init；kexec 不可用时停止，不退回两次重启。`hold=1` 进入 Alpine Live 救援，不自动分区。Rust 的 ISO/netboot.xyz/reset 尚未接通执行；下文 Shell 基线保留这些接口。

`Configure current system` 可持久化现有账户公钥、SSH 端口、主机名、时区、DNS 和受支持的网络设置，提供私有备份与变更清单。不自动重启服务或加载 sysctl；静态 DNS 变化可能立即影响解析。Save 仅保存本次会话草稿，Apply 才写入系统。

## 支持范围

- 原系统与目标均为 Linux x86_64；支持不同原系统发行版，拒绝容器、启用 Secure Boot 及 tmpfs 根的原系统。
- 四目标均从 Alpine Live 安装：Alpine/Debian 解析官方最新稳定版，Arch/Gentoo 使用滚动版；不接受版本号参数。
- Debian 使用 `debootstrap --variant=minbase`、systemd-networkd、SSH、内核和 GRUB，不安装桌面/standard 任务或 recommends。Gentoo 保留 systemd stage3、gentoo-kernel-bin/dracut 和 GRUB。
- Rust 根可选 XFS（默认）或 ext4，ESP 为 FAT32，Alpine 保留 OpenRC；计划 schema=6，拒绝旧计划。可选[目标 swapfile](docs/CONFIGURATION.md#目标-swapfile)由 GNU dd 写零，仅在新系统启动后激活；不操作原系统/Live swap，也不创建 swap 分区。
- 保留静态/DHCP、IPv4/IPv6、多网卡、SSH 公钥/密码、端口、frpc 和磁盘 ID 识别。默认及 Shell 路径不改变已有 swap。
- 不支持 Windows、ARM、Debian Installer、其他自动目标或任意 RAW/DD 自动写盘；手动恢复/DD 可进入 Alpine Live。

物理内存下限为 Alpine/Debian 512 MB、Arch 1024 MB、Gentoo 2048 MB，原系统预检及 Live 清盘前均检查。平台限制见[验收记录](docs/ACCEPTANCE.md)，包裁剪见 [minimal](docs/MINIMAL.md)，资源认证见[源审计](docs/SOURCES.md)。

## Shell 基线部署

`reinstall.sh` 必须配合本仓库的匹配资源，不能下载未裁剪的上游 `trans.sh`。将以下 4 个运行时文件按原路径部署到 VPS 可读取的 HTTPS 目录：

- `trans.sh`、`initrd-network.sh`、`fix-eth-name.sh`、`logviewer.html`

TTY/frpc 辅助函数和服务模板已内联；入口脚本、`frpc-example.toml`、文档和测试不属于这 4 项。兼容标记为 `PERSONAL-LINUX-ONLY-20261119-04`，升级时同步全部文件。

使用固定 Git 提交或不可变目录；资源 URL 不支持认证信息、查询参数或特殊字符。指定资源根：

```console
export REINSTALL_CONFHOME=https://your.example/reinstall/pinned-version
bash reinstall.sh alpine
```

也可用 `--confhome https://your.example/reinstall/pinned-version`。自动安装和 Alpine Live 必须配置，reset/netboot.xyz/本地 ISO 无需配置。不回退上游，拒绝原上游 raw GitHub/CNB 地址；版本不匹配时停止，`trans.sh update` 同样检查。

## Shell 基线：自动安装与救援

接口示例：

```console
bash reinstall.sh alpine
bash reinstall.sh debian
bash reinstall.sh arch
bash reinstall.sh gentoo
bash reinstall.sh alpine --hold 1
bash reinstall.sh netboot.xyz
bash reinstall.sh reset
```

- `alpine --hold 1`：进入带 SSH 的内存救援系统，不自动格式化。
- `netboot.xyz`：交互式网络引导菜单，菜单目标不等于自动安装支持范围。
- `--hold 2`：安装后保留现场，由用户重启；不用于 netboot.xyz/ISO。
- `reset`：取消尚未执行的重装计划，不能撤销已发生的格式化。

## Shell 基线：通用参数

| 参数 | 行为 |
|---|---|
| `--username USERNAME` | 默认 root |
| `--password PASSWORD` | 与公钥互斥；未提供凭据时交互输入，留空生成随机密码；命令行会暴露于历史 |
| `--ssh-key KEY` | 公钥、文件、HTTP(S) URL、`github:用户名` 或 `gitlab:用户名` |
| `--ssh-port PORT` / `--web-port PORT` | SSH / 安装日志网页端口 |
| `--frpc-config PATH_OR_URL` | 示例见 `frpc-example.toml`，须替换 frps 地址和 token |
| `--no-cloud-kernel` | Alpine/Debian 使用通用内核 |
| `--hold 0\|1\|2` | 0 在准备 initrd 后等待确认 |
| `--target-disk /dev/DISK` | 选择整盘，不是单个分区 |
| `--force-boot-mode bios\|efi` | 指定目标启动模式 |
| `--debug` / `--help` | 调试 / 帮助；帮助无需 root，不联网或修改引导 |

netboot.xyz/ISO 不接受账户、认证、frpc、SSH/Web 端口、hold 或目标盘参数。Shell 不支持 `--ci`、`--installer`、`--img`、`--minimal`、Windows 或 `--force-cn`；Rust 的 `--minimal` 见[设计说明](docs/MINIMAL.md)。原系统不限于四种安装目标，例如可从 Ubuntu 准备重装。

## Shell 基线：本地 ISO

本接口以 GRUB loopback 启动已下载的 Linux ISO，由控制台/VNC 交互安装；不复制 ISO、自动分区或 DD 写盘。

ISO 内含 `/boot/grub/loopback.cfg` 时：

```console
bash reinstall.sh iso --iso /images/systemrescue.iso
```

配置路径可覆盖：

```console
bash reinstall.sh iso --iso /images/custom.iso --iso-config /boot/grub/loopback.cfg
```

无 loopback 配置时指定真实内核/initrd 和介质发现参数。以下仅为 Debian live 风格示例：

```console
bash reinstall.sh iso --iso /images/custom-live.iso \
  --iso-kernel /live/vmlinuz --iso-initrd /live/initrd.img \
  --iso-cmdline 'boot=live findiso=%ISO_PATH%'
```

`%ISO_PATH%` 和 `%ISO_UUID%` 分别替换为 ISO 的 GRUB 文件系统路径和文件系统 UUID。

### 限制

- ISO 的 initramfs 必须支持 loopback 介质发现；GRUB 能读内核不代表安装器能找到介质。
- 需 `grub-probe`/`grub-mkrelpath` 或 `grub2-*`；拒绝仅 extlinux 的 BIOS 原系统及加密存储。
- 本地/ISO 内路径须绝对，不含空格、点路径段或 Shell/GRUB 元字符；显式 cmdline 不支持引号、换行、分号、反斜杠、`$`、括号或花括号。
- 只读挂载检查配置/内核/initrd，拒绝常见非 Linux 介质及 `/reinstall-tmp/`、受管理引导文件路径下的 ISO。
- **安装器读完前不能删除或格式化 ISO 所在分区。**同盘清空须有介质支持的 RAM 加载及足够内存，或改用独立保留盘。
- 不注入通用网络、SSH 或无人值守配置；UEFI 仍可能下载辅助 GRUB 并写 NVRAM。

## Shell 基线：下载与网络

无地域检测、`is_in_china`、`--force-cn` 或地域 netconf 字段。Alpine/Debian 使用官方源，Arch 使用上游海外镜像，Gentoo 使用 distfiles.gentoo.org。

frpc 保留 IPv6-only 时的南京镜像回退，用于弥补 GitHub releases 的 IPv6 连通性；Fedora 44 仅提供辅助 GRUB EFI 文件，不是安装目标。日志使用 websocketd 和 cdnjs.cloudflare.com 的固定版本库；网卡修正仅处理生成的 ifupdown/networkd 配置。

## 开发验证

需要 Rust、Bash、BusyBox、Git、OpenSSH client；验证编排使用 Rust：

```console
cargo test --manifest-path tests/verify/Cargo.toml
shellcheck --severity=warning reinstall.sh trans.sh initrd-network.sh fix-eth-name.sh
shellcheck --severity=warning --shell=sh tests/fixtures/fix-eth-name-openrc.initd
```

Shell 测试仅做语法检查、隔离调用、临时目录 mock 和上游逐字节对拍，仅允许入口 `--help`。`TEST_BASH`、`TEST_BUSYBOX` 可指定工具路径；须保留固定上游 Git 对象。完整检查用 `cargo xtask check`，破坏性验收仅在可丢弃 QEMU 客体进行。

静态测试不证明安装成功。四目标 BIOS/UEFI 全流程仍待验收；开发续接见[指南](docs/AGENT-HANDOFF.md)，改动记录见[裁剪](docs/TRIMMING.md)、[重构](docs/REFACTOR.md)和[历史](docs/HISTORY.md)。

## 本机管理工具

仓库集成了独立的 [DMITBox 本机管理组件](tools/dmitbox-local/README.md)，提供受事务保护的 SSH/nftables、DNS、MTU、静态 IPv6、TCP 与新建 Swap 管理。它保持单文件 Bash 分发，使用独立状态目录，不由重装流程自动执行。

```console
cargo xtask local-tool build
bash tools/dmitbox-local/dist/dmitbox-local.sh --help
cargo xtask local-tool check
```

修改现有系统前须按[配置说明](tools/dmitbox-local/docs/CONFIGURATION.md)登记资源所有权和恢复后端。该组件仍是开发候选：[原生恢复证据](tools/dmitbox-local/docs/NATIVE-EVIDENCE.md)已包含重启与定时恢复；[完整发行版矩阵及随机/外部 NAT66 验证](tools/dmitbox-local/docs/LIMITATIONS.md)尚未完成。
