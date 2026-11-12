# 目标内核、主机名与时区

CLI/TUI 将这些设置保存到 `InstallPlan.post_install`，由 Alpine Live 安装调度应用。**当前入口确认后会执行真实安装；四目标 BIOS/UEFI 验收仍待完成。** 本页不提供现有系统在线换核功能。

## 配置接口

```console
./rustaller debian --confhome https://your.example/reinstall/pinned-version \
  --kernel generic --hostname my-vps --timezone Etc/UTC
```

示例需配置认证并完成磁盘身份确认；仅检查时添加 `--inspect`。

| 目标 | `--kernel` 选择 | 包/方式 |
|---|---|---|
| Alpine | `standard`（默认）、`virt`、`lts` | standard/lts 为 linux-lts，virt 为 linux-virt；没有独立 linux-standard 包 |
| Debian | `standard`（默认）、`cloud`、`generic` | standard/generic 为 linux-image-amd64，cloud 为 linux-image-cloud-amd64 |
| Arch | `standard`（默认）、`lts`、`cachyos` | linux、linux-lts、linux-cachyos；CachyOS 需显式启用其仓库 |
| Gentoo | `auto`、`standard` | sys-kernel/gentoo-kernel-bin；保留 systemd stage3/GRUB/dracut 路线 |

`auto` 对前三者解析为 standard；Gentoo 仍为 gentoo-kernel-bin。Alpine 的旧 cloud/generic 分别作为 virt/lts 别名保留。`--no-cloud-kernel` 为 Alpine/Debian 选择通用内核，与非 auto 的 `--kernel` 互斥。

仅接受上述选项，不支持任意包名、URL、脚本钩子或额外 init 系统；CachyOS 是显式授权的仓库例外。未指定时区时不额外安装时区包或覆盖时区。

主机名为 1–63 位 ASCII 字母、数字、连字符，首尾须字母或数字。时区须为相对 IANA 名称，如 `Etc/UTC`；拒绝绝对路径、点/空路径段和控制字符，存在性以目标 zoneinfo 为准。ISO/netboot.xyz/reset/`hold=1` 不接受目标配置。

未指定 hostname 时，先读取原系统 `/etc/hostname`，再读 `/proc/sys/kernel/hostname`；有效 FQDN 取首标签，忽略 localhost/无效值，最后使用 `alpine`、`debian`、`arch` 或 `gentoo`。当前系统配置留空仍表示不修改。

## Arch CachyOS

`--kernel cachyos` 使用[官方仓库安装方案](https://wiki.cachyos.org/features/optimized_repos/)；官方脚本按 glibc 可用 ISA 检测选择优化仓库，避免只看 CPU 型号而忽略操作系统支持。安装器固定资源摘要与信任密钥，保留软件包签名检查，仅在隔离的 Arch 目标中执行。

目标首次开机执行完整包更新并重建 initramfs/GRUB；失败保留待执行状态供重试，成功后停止重复更新，不额外自动重启。standard/LTS 不启用 CachyOS。该路径与各 ISA 仓库仍待真实 VM 验收。

## 应用顺序与边界

下图保留早期 schema 4 的流程示意；当前 schema 为 6，拒绝旧计划，实际调用顺序以 `targets/install.rs` 为准：

```text
CLI / TUI
  → InstallPlan.post_install
  → schema 4 计划编码与私密文件交接
  → Live 校验计划/架构/RAM
  → 目标适配器选择官方内核及所需时区包
  → 完成 rootfs、内核/initramfs、GRUB、网络与 SSH
  → shared::configure::apply_basic_configuration
  → 首次启动目标系统
```

`src/postinstall.rs` 验证字段并准备包/文件数据；时区需要 `tzdata`，Gentoo 对应 `sys-libs/timezone-data`。`src/shared/configure.rs` 在受检目标中写 `/etc/hostname`、`/etc/timezone`、`/etc/localtime`，不调用宿主的 hostnamectl/timedatectl。

写入前拒绝宿主 `/`、符号链接根/父目录、路径逃逸及救援计划。时区文件须限于目标 zoneinfo 树、大小受限且含 TZif 头；允许目标内部 `UTC → Etc/UTC` 链接。临时文件加 rename 逐文件原子发布，失败清理自身临时文件；无跨文件事务，调用方须独占目标目录。

## 相关配置

`InstallPlan.system` 保存根文件系统、档位、工具/文档/缓存、DNS、网络和可选 swapfile。DNS 由基础配置应用，网络与 swapfile 由各自组件处理，Alpine 离线准备启动服务。见[安装前配置](CONFIGURATION.md)与 [minimal](MINIMAL.md)。

原生 ssh-keygen 属于 host：私钥留在指定路径，口令不进入计划，仅公钥交接。安装不默认调优网络；CachyOS 之外不增加内核仓库或任意命令钩子。
