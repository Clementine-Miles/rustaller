# rustaller — 个人 VPS 重装工具（Linux-only）

基于 [bin456789/reinstall](https://github.com/bin456789/reinstall)，上游基准提交：
`98f1e21013233d42750cc2e7a298a0818461d686`。rustaller 统一采用 GNU GPLv3（`GPL-3.0-only`），完整条款见 [LICENSE](LICENSE)；保留上游源码声明与署名。

Rust/musl 版本提供可选的轻量 ratatui 多级配置界面。**主机准备、受验证 Alpine Live、kexec 交接和四系统安装驱动已连接，包含真实清盘操作；四系统 BIOS/UEFI 完整安装验收仍在进行，当前保持开发预览。**
英文界面先选择系统，再配置账户/SSH、主机名、内核、网络、DNS、minimal 档位和可选目标 swapfile。另有独立的当前系统配置入口、原生 ssh-keygen，以及返回上级时的 Save/Discard/Cancel 草稿选择；详见 [TUI 使用说明](docs/TUI.md)。
完整参数与 kexec 切换后只进行一次最终重启的执行方式见 [安装前配置](docs/CONFIGURATION.md)，精简范围见 [minimal 档位设计](docs/MINIMAL.md)。
构建、GitHub Actions/Release、curl 启动和体积对比见 [GitHub 发布指南](docs/GITHUB-RELEASE.md)。
官方安装资源及只读在线核验见 [下载源审计](docs/SOURCES.md)；官方内核、主机名、时区选项见 [基础配置](docs/POSTINSTALL.md)。安装使用原生签名验证和受检目标目录；元数据审计通过不能代替安装验收。
新 agent 请先读 [续接指南](docs/AGENT-HANDOFF.md)；下文保留 Shell 基线接口说明。

2026-11-02：Rust 自动安装根文件系统改为 XFS（ESP 仍为 FAT32），保留 Alpine/OpenRC 和 GRUB；可选 swapfile 由 GNU dd 完整写零并在目标开机后激活。计划 schema=5，拒绝旧计划。XFS 启动与 swap 激活尚待 VM 验收，Shell 基线不变。
[English](README.en.md) · [项目框架与资源调用](docs/ARCHITECTURE.md) · [裁剪记录与验证边界](docs/TRIMMING.md) · [历史摘要](docs/HISTORY.md)

2026-10-28 重构：Rust 源文件 60 → 48，统一配置、包策略与存储模块，删除旧调度包装，优化子进程输出；参考源码、体积对照及验证结果见 [重构记录](docs/REFACTOR.md)。

2026-10-29 虚拟化审计：增加 Secure Boot/EFI 位数、Xen 模式和磁盘扇区检查；修复 ESP FAT32 布局、目标 initramfs 驱动裁剪和跨磁盘设备链接清理。KVM、VMware、Hyper-V 等平台的条件、限制、官方参考资料及实测结果见 [虚拟化验收记录](docs/ACCEPTANCE.md)。

> **自动重装会清空目标磁盘的全部分区和数据。请先备份，并准备商家控制台/VNC。**
> Rust 自动安装通过 kexec 离开原系统，随后重建目标磁盘和 GRUB；目标 UEFI 使用可移动介质回退路径。下文保留的 Shell 引导/reset 接口还可能写 NVRAM/BootNext。
> BIOS/UEFI 虚拟机验收正在进行，尚未完成四系统矩阵，不应直接用于唯一生产机。

## Rust 入口

```console
cargo xtask bundle
./target/x86_64-unknown-linux-musl/release/rustaller
```

无参数且连接交互终端时进入英文 TUI，首页显示居中的 ASCII RUSTALLER 标题。GitHub 固定版本下载命令见 [发布指南](docs/GITHUB-RELEASE.md)；目标 VPS 不需要 Rust 工具链。

CLI 可先执行 `./rustaller debian --inspect` 查看目标磁盘、固件模式和确认令牌；它不执行安装或 kexec。实际安装必须提供 SSH 公钥或密码，并在原生终端完整输入磁盘身份确认令牌；非交互执行需要显式 `--confirm-erase`。Rust 内嵌匹配 worker 和日志页面，默认使用官方签名资源，不要求自建 `confhome`。

原系统的实际 kexec 适配器目前支持 systemd 或 openrc-init 作为 PID 1；不支持时停止，不自动退回两次重启流程。Alpine Live 救援使用 `hold=1`，不自动分区。Rust 的本地 ISO、netboot.xyz 和 reset 菜单/校验组件保留，实际交接尚未连接；以下 Shell 基线仍提供这些接口。

`Configure current system` 可持久化现有账户公钥、SSH 端口、主机名、时区、DNS 和受支持的网络设置，并提供私有备份与变更清单。它不会自动重启服务或立即加载 sysctl；静态 DNS 文件变化可能立即影响新解析。草稿仅保存在本次会话，Save 不代表 Apply。

## 支持范围

- **主机和目标架构仅支持 Linux x86_64**。这不改变不同 x86_64 Linux 原系统的包管理器和引导方式兼容性。
- 自动安装目标：Alpine 最新稳定版、Debian 最新稳定版、Arch、Gentoo；**四者都从 Alpine Live 执行**。不支持 Arch Linux ARM 或其他 ARM 路径。
- Alpine、Debian 版本从官方稳定版元数据解析；不使用硬编码旧版本，也不接受版本号参数。
- Debian 使用 Alpine 的 `debootstrap --variant=minbase` 创建基础系统，配置 systemd-networkd、SSH、内核与 GRUB；不安装桌面/standard 任务或 recommends。
- Arch、Gentoo 保留上游滚动发行版安装流程；Gentoo 仍是上游 systemd stage3 + GRUB + gentoo-kernel-bin/dracut，不擅自改用 dinit、OpenRC、ugrd、efistub 或指定文件系统。
- Shell 基线保留 Alpine Live、独立直启的 netboot.xyz、本地 Linux ISO loopback 和 `reset`；Rust 的手动介质/reset 执行尚未连接。ISO/netboot 不经过自动安装器。
- 保留网络识别、静态/DHCP、IPv4/IPv6、多网卡、SSH 公钥/密码、端口、frpc 和磁盘 ID 识别。
- **保留的 Shell 基线和 Rust 默认路径不改变已有 swap 状态**；Rust 另提供显式启用的目标 `/swapfile` 配置，进入新系统时启用，不操作原系统/Live 的 swap；不提供 swap 分区。
- 删除 Windows 原系统/目标系统支持、Debian Installer、ARM 目标路径、其他发行版的目标安装器、相关文件和地域检测。不开放任意 RAW/DD 自动安装入口；手动 DD 可先进入 Alpine Live。

物理内存下限：Alpine 512 MB、Debian 512 MB、Arch 1024 MB、Gentoo 2048 MB。原系统准备阶段和 Alpine Live 都会检查；Alpine Live 在任何磁盘清理/分区操作前再次检查。不支持 OpenVZ/LXC 容器、启用 Secure Boot 的机器，也不支持从根目录为 tmpfs 的 Live OS 准备重装。这些限制不是所有 VPS 都能安装成功的保证。

## Shell 基线部署：必须使用自己的配套文件

`reinstall.sh` 会把 `trans.sh`、网络脚本和配套配置放进 initrd，并在后续阶段使用配套资源。
**不能只改本地的 reinstall.sh，却继续下载上游未裁剪的 trans.sh。**

将本仓库的配套文件保持原相对路径，发布到一个 VPS 能读取的 HTTPS 静态目录：

- `trans.sh`、`initrd-network.sh`、`fix-eth-name.sh`、`logviewer.html`

共 4 个运行时配套文件，不包括入口 `reinstall.sh`、`frpc-example.toml`、文档和测试。TTY 选择、frpc 下载地址解析以及 Alpine/systemd 服务模板已内联到 `trans.sh`，不再单独下载或执行。Debian Installer 删除后，`debian.cfg` 与 `get-xda.sh` 不再存在或部署。
当前兼容标记为 `PERSONAL-LINUX-ONLY-20261119-04`；升级时须同步全部文件。

推荐固定到自己的 Git 提交或不可变目录，避免执行期间不同文件版本变化。资源 URL 目前不支持认证信息、查询参数或 URL 中的特殊字符；不是私有 GitHub 下载令牌接口。

指定资源根地址：

```console
export REINSTALL_CONFHOME=https://your.example/reinstall/pinned-version
bash reinstall.sh alpine
```

也可通过 `--confhome https://your.example/reinstall/pinned-version` 指定。
自动安装和 Alpine Live 必须配置此地址；`reset`、netboot.xyz、本地 ISO 不需要这个配套资源地址。
默认不会回退到上游资源，且明确拒绝原上游的 raw GitHub/CNB 地址。
`trans.sh` 版本标记不匹配时终止；运行中的 `trans.sh update` 同样检查标记。

## Shell 基线：自动安装与救援

以下命令**仅展示接口，不会由文档自动执行**：

```console
bash reinstall.sh alpine
bash reinstall.sh debian
bash reinstall.sh arch
bash reinstall.sh gentoo
bash reinstall.sh alpine --hold 1
bash reinstall.sh netboot.xyz
bash reinstall.sh reset
```

- `alpine --hold 1`：进入内存系统并开启 SSH，不自动格式化目标盘；用户手动操作可能破坏数据。
- `netboot.xyz`：进入交互式网络引导菜单；不表示其菜单的全部目标都由本裁剪版自动安装器支持。
- `--hold 2`：安装结束后不自动重启，可手动检查目标系统；不用于 netboot.xyz/本地 ISO。
- 安装前运行 `reset` 可以取消计划中的重装。安装已开始后取消不能撤销格式化。

## Shell 基线：通用参数

- `--username USERNAME`：用户名，默认 root。
- `--password PASSWORD`：密码；未提供密码或公钥则交互输入，留空生成随机密码。注意命令行密码会留在 shell 历史中。
- `--ssh-key KEY`：SSH 公钥、文件、HTTP(S) URL、`github:用户名` 或 `gitlab:用户名`；不能与密码同时指定。
- `--ssh-port PORT`、`--web-port PORT`：SSH、安装日志网页端口。
- `--frpc-config PATH_OR_URL`：保留 frpc 配置接口，示例见 `frpc-example.toml`。示例仅提供 SSH 转发；必须换成自己的 frps 地址和认证 token，其他 Linux 服务可自行追加。
- `--no-cloud-kernel`：Alpine/Debian 使用通用内核；默认选择云内核。
- `--hold 0|1|2`：沿用安装阶段控制；0 在准备 initrd 后等待手动确认。
- `--target-disk /dev/DISK`：指定目标磁盘，**不是只安装到一个分区**。
- `--force-boot-mode bios|efi`：沿用上游目标启动模式设置。
- `--debug`、`--help`：调试、帮助；帮助不需要 root，也不访问网络或配置引导项。

netboot.xyz/本地 ISO 不接受账号、公钥、密码、frpc、SSH/Web 端口、hold 或目标磁盘参数。
此处的 Shell 基线不支持 `--ci`、`--installer`、`--img`、`--minimal`、Windows 选项或 `--force-cn`。Rust 预览的 `--minimal` 是新增加的受限档位，见上方设计文档。
原系统无需限定为上述四种目标系统，例如仍可从 Ubuntu 原系统重装到 Alpine。

## Shell 基线：本地 ISO 引导接口

这里的 ISO 是**已经下载到 VPS 本地的 Linux 安装介质**，不是 RAW 硬盘镜像。
通过 GRUB loopback 引导，使用控制台/VNC 交互安装；不会自动复制 ISO、重分区或将 ISO DD 到硬盘。

ISO 自带 loopback 配置时：

```console
bash reinstall.sh iso --iso /images/systemrescue.iso
```

默认配置路径为 ISO 内的 `/boot/grub/loopback.cfg`，可显式指定：

```console
bash reinstall.sh iso --iso /images/custom.iso --iso-config /boot/grub/loopback.cfg
```

没有该配置时，使用 ISO 内真实的内核/initrd 路径及发行版自己的介质发现参数。例如下面仅是 **Debian live 风格的参数示意，不适用于所有 ISO**：

```console
bash reinstall.sh iso --iso /images/custom-live.iso \
  --iso-kernel /live/vmlinuz --iso-initrd /live/initrd.img \
  --iso-cmdline 'boot=live findiso=%ISO_PATH%'
```

`%ISO_PATH%` 替换为 ISO 在其文件系统上的 GRUB 路径，`%ISO_UUID%` 替换为该文件系统 UUID。

### 必须了解的限制

- **不存在“任意 ISO 都能直接 loopback 启动”的保证。**ISO 的 initramfs 必须能够重新找到硬盘上的 ISO；GRUB 能读取内核不等于内核能找到安装介质。
- 需安装 `grub-probe`/`grub-mkrelpath` 或对应的 `grub2-*` 工具；例如 Debian 的 `grub-common` 包。
- BIOS 原系统仅使用 extlinux 时，本接口拒绝执行，不擅自替换其引导器。
- 不支持加密存储；其他存储布局仍需 GRUB 能读取 ISO 所在文件系统。
- 本地和 ISO 内部路径必须是绝对路径，不能含空格、点路径段或 Shell/GRUB 元字符。
- 显式启动参数暂不支持引号、换行、分号、反斜杠、`$`、括号或花括号；这不是运行任意 GRUB 命令的接口。
- 只读挂载 ISO 检查配置/内核/initrd 是否存在；检测到常见的非 Linux 安装介质布局时拒绝。
- 不允许 ISO 放在 `/reinstall-tmp/` 或受管理的引导文件路径里，避免准备时被清除。
- **安装器读取完 ISO 前，不要格式化/删除 ISO 所在分区。**如需清空同一块盘，必须由该 ISO 明确支持加载介质到内存，且内存足够，或把 ISO 放到独立保留盘。
- 仅配套资源地址不必要；UEFI 引导仍可能下载辅助 GRUB EFI 文件并写入 NVRAM。
- 本接口没有对通用 ISO 注入自动网络、SSH 或无人值守安装配置。ISO 自带环境决定这些能力。

## Shell 基线：海外下载策略

删除 `is_in_china`、地域请求、`--force-cn`、大陆镜像切换和 netconf 中的地域字段。
Alpine/Debian 使用官方源，Arch 使用上游海外镜像，Gentoo 使用 distfiles.gentoo.org。

保留两个容易误解的细节：

1. frpc 沿用上游**非大陆**策略中的 IPv6-only 回退：GitHub releases 缺少 IPv6 时仍用支持 IPv6 的南京镜像。这只是连通性兼容，不是地域判断。
2. EFI 引导仍下载 Fedora 44 的辅助 GRUB 文件；Fedora **不是安装目标**，其版本不再错误依赖目标系统版本列表。未来需维护该辅助文件来源。

日志网页使用 websocketd 和官方 cdnjs.cloudflare.com 的固定版本库；不再部署未使用的 Nginx/Lighttpd 日志后端。
目标系统的网卡修正只处理当前生成的 ifupdown/systemd-networkd 配置；这不缩减原 Linux 系统的兼容范围。

## 开发验证

测试编排使用 Rust，没有新增 Python/Node 测试脚本。开发机需要 Rust、Bash、BusyBox、Git 和 OpenSSH client（真实终端密钥生成测试）：

```console
cargo test --manifest-path tests/verify/Cargo.toml
shellcheck --severity=warning reinstall.sh trans.sh initrd-network.sh fix-eth-name.sh
shellcheck --severity=warning --shell=sh tests/fixtures/fix-eth-name-openrc.initd
```

上述 Shell 基线测试只做语法检查、隔离函数调用、临时文件夹内的模拟，以及原始上游输出逐字节对拍；覆盖 x86_64 路由、四个 RAM 下限、无 swap 操作、Debian debootstrap/networkd/fstab/GRUB 和资源清单；不调用真正重装入口（只允许安全的 `--help`）。完整 Rust 检查使用 `cargo xtask check`；真实安装验收另外在可丢弃 QEMU 客体中进行，不在开发宿主机上运行清盘或重启。
`TEST_BASH`、`TEST_BUSYBOX` 可指定 Bash、BusyBox 的绝对路径。上游对拍使用上述固定提交，须保留该提交的 Git 对象。

**测试通过不等于重装经过实机验证。**投入使用前，仍须在可丢弃的 BIOS/UEFI VPS 上验证网络、磁盘选择、引导取消和四个目标系统的完整安装。
