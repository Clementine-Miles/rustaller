# rustaller 续接指南

先读根目录 `AGENTS.md`、[架构](ARCHITECTURE.md)与[验收记录](ACCEPTANCE.md)。本文列开发优先级，[历史](HISTORY.md)解释旧决策；无需依赖旧会话或未随源码提供的本地文件。

## 当前状态

- Linux x86_64，英文多级 TUI；四目标经认证的 Alpine Live 和 kexec 安装，最后只重启一次。已有真实清盘能力，四系统 BIOS/UEFI 完整验收尚未通过，保持开发预览。
- 根为 XFS，保留 GRUB（SeaBIOS/UEFI）与 Alpine/OpenRC。EFI Stub 已取消，[评估](EFI-STUB-OPTIONS.md)仅为历史记录。
- Live 清盘前检查 XFS 模块及扣除 ESP/对齐后的根容量；mkfs 禁用 reflink/bigtime/inobtcount/nrext64，以 `-K` 跳过整卷 discard。swapfile 用 GNU dd 写零、核对长度/块数，0600、mkswap、fstab，仅目标开机激活；容量解析固定 `LC_ALL=C`。
- 计划 schema=5、manifest schema=1、Rust 标记 `PERSONAL-LINUX-XFS-20261102-05`，拒绝旧计划及混用 worker。Shell 四资源及旧标记保持不变。
- 仓库：https://github.com/Clementine-Miles/rustaller 。Cargo 主包、CLI 为 `rustaller`，worker 为 `rustaller-worker`；GNU GPLv3，保留上游声明与署名。
- 主 crate + xtask，`tests/verify` 独立。核心无第三方运行时依赖；tui 可选，libc 仅测试、sha2 仅开发工具。模块声明集中在 `lib.rs`，公共 Live 命令默认值在 `command::live_command`，环境与重试留在适配器。
- 源码包保留 Cargo.lock、公开密钥、fixture、`.github`/`.cargo`，排除 `.git`、构建产物、VM、日志与私钥。

历史 XFS 修改通过 273 workspace + 49 Shell 测试，参考项目修复后为 277 + 49；这些记录及静态构建不代表新的 VM 安装证据。最新检查以对应提交的 CI 为准。

## 恢复开发环境

工具链/musl target 见 `rust-toolchain.toml`。需要 Bash、Git、BusyBox、ShellCheck 0.11+、ssh-keygen；VM 另需 QEMU、SeaBIOS、OVMF、xorriso 与经过认证的可丢弃源镜像。

```console
git fetch --no-tags --depth=1 https://github.com/bin456789/reinstall.git 98f1e21013233d42750cc2e7a298a0818461d686:refs/rustaller/upstream-baseline
cargo xtask check
cargo xtask bundle
target/x86_64-unknown-linux-musl/release/rustaller --help
```

已有仓库先 clone 再复制源码，保留隐藏目录与历史；裸源码可先 git init。CI 自动获取固定上游对拍对象。

可通过 `TEST_BUSYBOX`、`TEST_BASH`、`TEST_SHELLCHECK` 指定工具。以下是旧工作区路径示例，换环境须替换：

```console
PATH=/home/agent/.cargo/bin:$PATH \
TEST_BUSYBOX=/workspace/scratch/rustaller-tools/usr/bin/busybox \
TEST_SHELLCHECK=/workspace/scratch/rustaller-tools-current/usr/bin/shellcheck \
cargo xtask check
```

检查/构建期间不要修改源码；仅针对失败或新变更追加验证。未跟踪源码不得当作垃圾用 git clean/reset 清除。

## 功能入口

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

## P0：复验 Live 入口

SeaBIOS/virtio-blk、OVMF/virtio-scsi 已实际 kexec 进入 Live，均未完成安装。UEFI 的 SecureBoot 变量误拦截已区分可读 efivarfs 中确实缺失与未挂载/不可读。

BIOS 发现两处启动故障，源码已修但缺少新一轮完整复验：

1. OpenRC 日志重定向缺少后台模式；`host/handoff.rs::LIVE_SERVICE` 已加 `command_background=yes`、pidfile、`need modloop`。
2. stock modloop 搜索排除 RAM 根；现由 `worker/install.rs::prepare_live_modules` 要求 root、Alpine、`rustaller_live=1` 和内存根，只读挂载认证 squashfs，核对运行内核后建立模块/固件链接。串口 console 放在 cmdline 最后。

下一步：

1. 从当前源码重新 bundle，先跑 Alpine BIOS/virtio-blk，再跑 UEFI/virtio-scsi。
2. 缺少 `RUSTALLER_LIVE_START` 时查 modloop/rustaller 服务、APK 覆盖、`/lib/modules`、findmnt、`/var/log/rustaller.log` 和串口。
3. 确认模块、存储/NIC、恢复 SSH 后定位首个失败命令；修复并复跑对应案例，添加针对实际故障的回归验证。

旧证据在原工作区 `/workspace/scratch/virt-audit-20261029/`（含 `alpine-bios-console.ppm`、BIOS/EFI 日志），不随源码分发；新环境须重建镜像与证据。

## P1：完整安装与虚拟硬件验收

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

认证源镜像后使用 prepare-source；每次创建独立目录、qcow2 overlay、OVMF 变量副本，不挂宿主块设备，继承代理/CA 并保持 TLS。详细接口见 [ACCEPTANCE.md](ACCEPTANCE.md)。

覆盖四系统 × BIOS/UEFI，再扩展 virtio-scsi、SATA、NVMe、PVSCSI；源内核也须支持控制器。TCG 设备模拟不替代真实 KVM/ESXi/Hyper-V/Xen HVM 验收。

每例须看到 Live/start 与 install-complete、dispatch 后 QMP 恰好一次 RESET、新磁盘启动、正确 distro/hostname/kernel、公钥 SSH、XFS/UUID fstab、GRUB/UEFI fallback、DNS/默认路由、原生包元数据更新和目标开机激活 swapfile。IPv6-only、多 NIC、特殊路由及真实 4Kn 另验。

故障覆盖身份改变/多盘同身份、挂载/swap 占用、只读盘、低 RAM、下载/签名失败、缺驱动、GRUB/SSH 校验失败。清盘前的拒绝条件不得延后，安装失败不得最终重启。

UEFI 为 64 位、Secure Boot 关闭，512 MiB FAT32 ESP、单扇区簇、removable/no-nvram、PE 头校验。拒绝 BIOS/4Kn、32-bit EFI、Xen PV/PVH/control domain、容器及已检测受限 confidential VM。外部内核启动和提供商固件回退策略须另行适配。

## P2：剩余功能

- ISO：接通只读预检、介质校验、GRUB loopback、安全交接与回滚；不承诺通用 ISO，不混入自动清盘。
- netboot.xyz/reset：补执行和失败恢复，防止误触自动安装。
- 当前系统配置：目前不创建账户、设密码、换核或建 swap；扩展前须明确恢复边界。
- minimal：测量根占用与启动 RAM，再按原生依赖裁剪，保留 SSH、网络、CA、模块、引导、包维护能力。
- 发布：固定版本下载和校验；完整安装验收前保持预览，未公开 draft 不作为匿名下载入口。

## 工程与交付要求

遵守 `AGENTS.md`：四目标统一 Live，RAM 下限 512/512/1024/2048 MB 双检，完整身份确认，无通用 `--yes`，计划 0600，凭据不写 cmdline，签名不可降级，失败不重启。TUI 仅 host，开发编排使用 Rust；保留 Shell 四资源和上游 Git 对拍对象。

更新验收记录中的版本/固件/控制器、结果、首个故障和证据位置，更新 P0/P1。打包源码及 SHA-256 时排除产物、镜像、日志和私钥；无用户明确要求不 commit/push/发布。

参考项目审查已修复 fstab 顺序、新目标 hosts、Hyper-V PCI 模块，并增加 GRUB XFS 识别/语法检查，见 [REFERENCE-REVIEW.md](REFERENCE-REVIEW.md)；没有新增 VM 安装通过证据。
