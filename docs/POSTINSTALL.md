# 目标内核、主机名与时区

CLI/TUI 将这些设置保存到 `InstallPlan.post_install`，由 Alpine Live 安装调度应用。**当前入口确认后会执行真实安装；四目标 BIOS/UEFI 验收仍待完成。** 本页不提供现有系统在线换核功能。

## 配置接口

```console
./rustaller debian --confhome https://your.example/reinstall/pinned-version \
  --kernel generic --hostname my-vps --timezone Etc/UTC
```

示例需配置认证并完成磁盘身份确认；仅检查时添加 `--inspect`。

| 目标 | `--kernel` 选择 | 对应官方包/方式 |
|---|---|---|
| Alpine | `auto`、`cloud`、`generic`、`lts` | cloud 为 linux-virt；generic/lts 为 linux-lts；安装对应内核包 |
| Debian | `auto`、`cloud`、`generic` | linux-image-cloud-amd64 或 linux-image-amd64 |
| Arch | `auto`、`standard`、`lts` | linux 或 linux-lts；保留原来独立于 pacstrap 的内核安装阶段 |
| Gentoo | `auto`、`standard` | sys-kernel/gentoo-kernel-bin；保留 systemd stage3/GRUB/dracut 路线 |

`auto` 保持默认：Alpine 虚拟机用 virt、其他情况用 lts；Debian 用 cloud-amd64；Arch 用 linux；Gentoo 用 gentoo-kernel-bin。`--no-cloud-kernel` 为 Alpine/Debian 选择通用内核，与非 auto 的 `--kernel` 互斥。

仅接受上述官方包，不支持任意包名、URL、Shell 钩子或额外 init 系统。`lts` 仅对 Alpine/Arch 定义。未指定时区时不额外安装时区包或覆盖时区。

主机名为 1–63 位 ASCII 字母、数字、连字符，首尾须字母或数字。时区须为相对 IANA 名称，如 `Etc/UTC`；拒绝绝对路径、点/空路径段和控制字符，存在性以目标 zoneinfo 为准。ISO/netboot.xyz/reset/`hold=1` 不接受目标配置。

## 应用顺序与边界

下图保留早期 schema 4 的流程示意；当前 schema 为 5，拒绝旧计划，实际调用顺序以 `targets/install.rs` 为准：

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

`InstallPlan.system` 保存档位、工具/文档/缓存、DNS、网络和可选 swapfile。DNS 由基础配置应用，网络与 swapfile 由各自组件处理，Alpine 离线准备启动服务。见[安装前配置](CONFIGURATION.md)与 [minimal](MINIMAL.md)。

原生 ssh-keygen 属于 host：私钥留在指定路径，口令不进入计划，仅公钥交接。安装不默认调优网络，也不添加第三方内核、任意命令钩子或额外系统仓库。
