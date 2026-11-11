# rustaller：给下一位 agent 的续接指南

更新日期：2026-11-02。此文件随完整源码包提供，不要求获得旧会话、旧 Git 历史或本机构建产物。

本轮用户授权优先完成 XFS 根文件系统及 TUI swapfile，保留 Alpine/OpenRC 和现有 GRUB。Rust 四目标默认根已改为 XFS，Live 在清盘前 modprobe xfs；mkfs 禁用 reflink/bigtime/inobtcount/nrext64 以限制磁盘特性。swapfile 使用 Live GNU dd 写零、0600、mkswap、fstab，OpenRC 在目标开机激活。计划 schema=5，Rust 标记 PERSONAL-LINUX-XFS-20261102-05；Shell 四资源及其旧标记不变。未进行新的 VM 安装；P0 Live 复验及 P1 安装矩阵仍待完成，不能承诺 XFS 实机启动成功。

## 先读这段

最新用户范围：取消 EFI Stub，保留 GRUB 对 SeaBIOS/UEFI 的路径，只优化 XFS。
本轮新增清盘前根分区容量下限、mkfs.xfs -K 跳过整卷 discard、swapfile 分配
块数核对与固定 LC_ALL=C 的容量解析。EFI-STUB-OPTIONS.md 仅为历史记录。

用户目标是：在 x86_64 Linux VPS 上下载并运行一个静态入口，进入英文多级 TUI，先选 Alpine / Debian / Arch / Gentoo，再完成账户、SSH、网络、DNS、内核、minimal、swap 等配置；kexec 进入 Alpine Live，最后只进行一次固件重启进入新系统。另有独立当前系统配置和手动本地 ISO 接口需求。

当前代码已连接实际自动安装与清盘，绝不是无副作用的演示程序。但四系统 BIOS/UEFI 完整安装矩阵尚未通过，不能发布为稳定版。最高优先级是验证并修复 Live 入口，随后完成真实目标安装；不要重新从头实现 TUI 或仅增加界面占位功能。

先读根目录 AGENTS.md，再读 ARCHITECTURE.md、ACCEPTANCE.md。本文件列当前任务，HISTORY.md 仅解释历史。原先八份计划/续接文档已合并，旧文件名中的日期及早期约束不代表当前授权。

## 交付源码的状态

- 2026-11-02 XFS/swapfile 修改：273 项 workspace + 49 项 Shell 测试、格式/lint/语法检查、静态 musl bundle 通过。没有进行新 VM 验收，仍保留开发预览；P0/P1 未完成。

- 仓库地址：https://github.com/Clementine-Miles/rustaller 。本次没有 commit、push 或创建线上 Release。
- Rust workspace 为主 crate + xtask；tests/verify 是独立 crate。核心零第三方运行时依赖，tui 可选，libc 仅测试、sha2 仅开发工具。
- src 有 44 个 Rust 文件。host/shared/targets/worker 的模块声明在 lib.rs；目录下业务文件及公开模块路径不变。
- 公共命令默认值在 command::live_command，发行版环境与重试仍留在各适配器。
- 当前计划 schema=5，部署 manifest schema=1，兼容标记 PERSONAL-LINUX-XFS-20261102-05。不能随意接受旧计划或拼接不同构建的 worker。
- 源码包排除 .git、target、artifacts、dist、VM、日志和测试私钥；保留公开验证密钥、测试 fixture、Cargo.lock、.github 和 .cargo。
- 2026-10-30 本轮 271 项 workspace + 49 项 Shell 测试通过，Rustfmt、Clippy、ShellCheck、Bash/ash、diff 检查及静态 musl bundle 构建通过；没有重跑完整 VM 安装。静态检查和构建不等于成功安装。

## 如何恢复开发环境

Rust 版本和 musl target 以 rust-toolchain.toml 为准。需要 Bash、Git、BusyBox、ShellCheck 0.11+、ssh-keygen；VM 测试另需 QEMU、SeaBIOS、OVMF、xorriso 和可丢弃源镜像。

在有 Git 的项目根目录获取对拍对象，然后运行检查：

```console
git fetch --no-tags --depth=1 https://github.com/bin456789/reinstall.git 98f1e21013233d42750cc2e7a298a0818461d686:refs/rustaller/upstream-baseline
cargo xtask check
cargo xtask bundle
target/x86_64-unknown-linux-musl/release/rustaller --help
```

直接解压且没有 Git 元数据时，可以先在解压根目录 git init；若要上传已有仓库，优先 clone 该仓库再复制源码，保留隐藏目录。上游对象获取已写入 GitHub Check 工作流，并在前一轮独立空仓中实测成功。

检查需要的可执行文件可用 TEST_BUSYBOX、TEST_BASH、TEST_SHELLCHECK 指定。当前旧工作区中可用的路径仅供参考，换环境不能假定存在：

```console
PATH=/home/agent/.cargo/bin:$PATH \
TEST_BUSYBOX=/workspace/scratch/rustaller-tools/usr/bin/busybox \
TEST_SHELLCHECK=/workspace/scratch/rustaller-tools-current/usr/bin/shellcheck \
cargo xtask check
```

不要在项目检查/构建过程中修改源码；只针对失败或新变更追加验证。原有大量未跟踪 Rust 文件是实际项目内容，不是临时垃圾，不能 git clean/reset 清除。

## 已实现的功能及入口

| 功能 | 代码入口 | 当前边界 |
|---|---|---|
| 英文多级 TUI、ASCII 标题、按目标草稿、返回保存/丢弃 | src/tui/；bin/rustaller.rs | 共用 CLI 校验，凭据遮蔽，结果返回不能自动重试 |
| 主机检查与 kexec | host/preflight.rs、handoff.rs | systemd/openrc-init PID 1；无两次重启回退 |
| Live 模块、安装调度 | worker/install.rs | 最新模块/服务修复待完整 VM 复验 |
| 四目标安装与 minimal 包策略 | targets/install.rs、packages.rs、debian.rs、rolling.rs | 尚无完整安装成功验收；不能承诺最终几十 MB |
| 当前系统持久配置 | host/current.rs | 备份、原生 SSH 校验；不自动重启服务/在线加载 sysctl |
| 原生 ssh-keygen | host/ssh_keygen.rs、TUI | 调用系统工具，可配置 rounds/cipher/type/comment，仅公钥序列化 |
| DNS/网络与可选 swapfile | shared/dns.rs、network.rs、swapfile.rs | swap 仅新目标 XFS，源/Live 不 swapon/swapoff |
| ISO/netboot.xyz/reset | host/iso.rs、boot.rs、args.rs、TUI | Rust 实际交接未接通，不得描述为可用 |
| 开发与 VM 验收 | xtask/src/ | 不随目标运行时分发 |

## P0：先复验并修复 Live 入口

前轮真实 QEMU 结果：SeaBIOS/virtio-blk 和 OVMF/virtio-scsi 均已通过原系统 kexec 进入 Alpine Live，未完成目标安装。UEFI 首次在 SecureBoot 变量缺失处误拦截；已按 Linux 策略区分“可访问 efivarfs 上确实不存在变量”与“变量文件系统未挂载/不可读”，第二轮通过源检查。

BIOS VGA 截图明确记录两处错误，源码已修复，但还没有完整重跑证明修复成功：

1. OpenRC 设置 output_log/error_log 却没启用后台执行，start-stop-daemon 拒绝启动。host/handoff.rs 的 LIVE_SERVICE 现有 command_background=yes、pidfile，并 need modloop。
2. stock Alpine modloop 服务遍历挂载介质时排除 RAM 根，找不到私密 overlay 内 /modloop-virt。现在覆盖该服务，调用 Rust worker 的 live-modules。

live-modules 在 worker/install.rs::prepare_live_modules：要求 root、Alpine、rustaller_live=1、内存根；只读挂载已验证 archive 中的本地 squashfs，核对 uname -r 对应目录，再建立 Live 模块/固件链接。Live 命令行最后指定串口 console，让 OpenRC 错误可见。

下一步具体操作：

1. 从当前源码重新 cargo xtask bundle，不复用 preview.8/9 或旧打包二进制。
2. 首先用 Alpine + BIOS/virtio-blk，再用 Alpine + UEFI/virtio-scsi 运行全链路。
3. 若没有 RUSTALLER_LIVE_START，先检查 OpenRC modloop/rustaller 服务、APK 是否保留私密覆盖服务、/lib/modules、findmnt、/var/log/rustaller.log 与串口日志。
4. 确认模块版本、存储/NIC 可见、恢复 SSH 可用后，定位后续首次失败的原生命令，修复后再跑对应案例。
5. 对实际故障加有意义的回归验证；不要以字符串包含某个参数的测试代替原生服务/安装验收。

旧原工作区证据位于 /workspace/scratch/virt-audit-20261029/，包括 alpine-bios-console.ppm、alpine-bios-blk/、alpine-efi-scsi-v9/。它们不在源码包；换环境时以本文和 ACCEPTANCE.md 恢复问题上下文，重新生成镜像和证据。

## P1：完成四系统与虚拟硬件验收

```console
cargo xtask vm-acceptance --help
cargo xtask vm-acceptance install \
  --tools /absolute/tools-root \
  --base /absolute/source-prepared.qcow2 --sha512 VERIFIED_SOURCE_SHA512 \
  --key /absolute/test-key --public-key /absolute/test-key.pub \
  --installer /absolute/rustaller \
  --target alpine --firmware both --disk-bus virtio-blk \
  --timeout 1800 --output /absolute/new-results-directory
```

源镜像须验证发行者来源/摘要；prepare-source 的接口与镜像认证细节见 ACCEPTANCE.md。为每轮创建新目录、磁盘 overlay 和 UEFI 变量副本；不得挂宿主机块设备。代理与 CA 按开发环境策略继承，不绕过 TLS。

依次覆盖 Alpine、Debian、Arch、Gentoo × BIOS/UEFI，再扩展 virtio-scsi、SATA、NVMe、PVSCSI。测试工具支持这些磁盘总线，但源镜像内核也必须支持控制器。当前工具使用 TCG；真实 KVM、ESXi、Hyper-V、Xen HVM 结果必须另记，不能用 QEMU 设备模拟替代平台验收。

每例通过要求：Live/start 与 install-complete 标记、dispatch 后 QMP 恰好一次 RESET、从新磁盘启动、正确 distro/hostname/kernel、SSH 公钥登录、XFS 与 UUID fstab、GRUB/UEFI fallback、DNS 和默认路由、原生包元数据更新、显式测试 swapfile 在目标启动后激活。IPv6-only、多 NIC、提供商特殊路由、真实 4Kn 启动另验。

故障场景至少检查：身份改变/同身份多盘、目标盘被挂载或用于 swap、只读盘、低 RAM、下载/签名失败、目标内核缺驱动、GRUB/SSH 校验失败。清盘前应停止的故障不能延后到 wipefs；安装失败不能请求最终重启。

UEFI 当前使用 512 MiB FAT32 ESP、显式 FAT32/单扇区簇、removable/no-nvram、EFI loader PE 头校验；Secure Boot 开启、32-bit EFI、Xen PV/PVH/control domain、容器、已检测的受限 confidential VM 都有拒绝路径。BIOS/4Kn 明确不支持。外部内核启动型 VPS 和固件回退策略仍需提供商适配，不可按品牌自动承诺兼容。

## P2：接通剩余用户功能

- 手动 ISO：绝对路径只读预检、介质支持检查、GRUB loopback/安全交接及回滚，不能宣称任意 ISO 均支持；此路径由用户进入介质自主安装，不混入自动清盘调度。
- netboot.xyz/reset：沿用已验证的模型和引导组件，补真正执行及失败恢复，并测试不会误触自动目标路径。
- 当前系统配置：如扩展创建账户、密码、内核或 swap，先明确现有能力边界并补安全的持久配置/恢复；现在这些不属于 current 入口已实现能力。
- minimal：先测实际目标根占用和启动内存，再按各发行版官方包依赖精简。保留 SSH、网络、证书、内核模块、引导及原生包维护能力。
- curl/GitHub：保持固定版本下载和校验、英文 TUI；发布前完成安装矩阵。不能把未公开 draft 当成已可下载资产，也不能擅自 push。

## 必须保留的工程边界

Linux x86_64；四目标统一 Alpine Live；RAM floor 为 512/512/1024/2048 MB，源/Live 双重检查；完整磁盘身份确认，无通用 --yes；私密计划 0600，凭据不写 cmdline；签名不能降级为“只下载成功”；失败不最终重启；不能跳过校验换取精简。

核心零第三方运行时依赖、TUI host-only；不为文件数减少而混合职责。不新增 Shell/Python/Node 编排工具，开发验证用 Rust；原生服务配置模板保留。unsafe 仅在必要系统接口边界使用，并有明确安全前提，不为“优化”无证据引入。

Shell 基线四资源及上游 Git 对象继续用于对拍。原 Shell 的弱验证不能复制到 Rust 新实现。源码裁剪范围和历史决策见 TRIMMING.md/HISTORY.md。

## 下一轮完成时请交付

更新 ACCEPTANCE.md：每例版本/固件/控制器、通过或失败、首个故障、证据位置；更新本文件 P0/P1 状态。检查通过后重新打包源码及 SHA-256，排除构建产物、.git、镜像、日志和私钥。明确未完成项，保持预览标记；没有用户明确要求时不要 commit/push/发布。

## 最新参考项目审查

已对照用户指定 bohanwood/debi 与 bin456789/reinstall 的 README/安装脚本，
记录在 REFERENCE-REVIEW.md。修复目标 fstab 写入顺序、全新目标 hosts、Alpine
Hyper-V PCI 模块；最终增加 GRUB XFS 识别和语法检查。277 workspace + 49 Shell
测试与静态 bundle 通过；没有新增 VM 安装验收。保持 GRUB + XFS，EFI Stub 已取消。
