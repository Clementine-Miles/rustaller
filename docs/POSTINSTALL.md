# 安装末尾的基础配置

当前 Rust 预览支持在 CLI/TUI 中选择官方内核、主机名和时区，并将配置写入同一份安装计划。四系统的完整重装主流程仍未接通；此页面不代表当前二进制已经可以安装系统，也不是在现有生产系统上热切换内核的命令。

## 配置接口

普通自动安装的 TUI 表单显示以下字段；CLI 使用：

```console
./reinstall debian --confhome https://your.example/reinstall/pinned-version \
  --kernel generic --hostname my-vps --timezone Etc/UTC
```

当前执行该命令只会校验计划并运行主机预检，随后报告安装链路未完成。

| 目标 | `--kernel` 选择 | 对应官方包/方式 |
|---|---|---|
| Alpine | `auto`、`cloud`、`generic`、`lts` | cloud 为 linux-virt；generic/lts 为 linux-lts；setup-disk 使用对应 flavor |
| Debian | `auto`、`cloud`、`generic` | linux-image-cloud-amd64 或 linux-image-amd64 |
| Arch | `auto`、`standard`、`lts` | linux 或 linux-lts；保留原来独立于 pacstrap 的内核安装阶段 |
| Gentoo | `auto`、`standard` | sys-kernel/gentoo-kernel-bin；保留 systemd stage3/GRUB/dracut 路线 |

`auto` 保持旧默认：Alpine 虚拟机选 virt，其他情况选 lts；Debian 选 cloud-amd64；Arch 选 linux；Gentoo 选 gentoo-kernel-bin。旧 `--no-cloud-kernel` 继续使 Alpine/Debian 选择通用内核，与非 auto 的显式 `--kernel` 互斥。未请求配置时，不增加时区包或覆盖主机名/时区。

这些是受限的官方包选择，不接受任意包名、下载 URL 或 Shell 钩子。这里的 `lts` 仅对 Alpine/Arch 有定义，不把 Debian 的官方稳定内核另称为一个不存在的 LTS 包。Gentoo 不额外引入源码编译或其他 init 系统。

主机名只接受 1–63 位 ASCII 字母、数字和连字符，首尾必须是字母或数字。时区接受相对 IANA 名称，如 `Etc/UTC`、`Asia/Shanghai`、`America/New_York`；拒绝绝对路径、点路径段、空段和控制字符。解析阶段只检查格式，实际存在性必须以目标系统安装的 zoneinfo 为准。

本地 ISO、netboot.xyz、reset 和 `hold=1` 救援不能设置这些目标配置；用户自行进入 ISO 安装的系统由该介质管理。

## 代码边界与应用顺序

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

`src/postinstall.rs` 只验证和准备数据：官方内核包、Alpine flavor、所需时区包、主机名/时区文件内容以及目标内的 zoneinfo 相对路径。设置时区会要求安装 `tzdata`，Gentoo 对应 `sys-libs/timezone-data`。

`src/shared/configure.rs` 是目标目录配置组件。它接收已经过 Live 检查的计划，写入目标的 `/etc/hostname`、`/etc/timezone` 和 `/etc/localtime`。它不执行宿主机的 `hostnamectl`、`timedatectl`、包管理器或其他系统配置命令。

应用前验证目标根目录和父目录、拒绝宿主 `/`、符号链接根/`etc`、路径逃逸和救援计划。时区文件必须位于目标的 zoneinfo 树内、大小受限并具有 TZif 格式头；全部检查先于任何配置写入。合法的目标内部 `UTC → Etc/UTC` 链接可用，跨出目标 zoneinfo 的链接不可用。文件按临时文件加 rename 发布，失败清理自己的临时文件。各文件分别原子替换，不提供跨文件事务；调用方必须独占目标目录。

配置组件和目标包选择可在隔离目录及 mock 中验证，但它们仍需由后续完整 Live 安装调度按顺序调用。当前没有新增 worker 安装命令、自动重启或在线系统换核入口。未知/旧 schema 在读取头部后立即拒绝，不能将旧计划静默当成新配置。

## 网络、精简与 swapfile

2026-10-26 新增 `InstallPlan.system`：standard/minimal 档位、工具/文档/缓存选择、DNS、受限网络栈及可选目标 swapfile。`apply_basic_configuration` 也会应用目标 DNS；网络栈和 swapfile 由各自组件处理，并为 Alpine 准备原生启动服务链接。完整目录与参数见 [安装前配置](CONFIGURATION.md)，包集合与体积边界见 [minimal 设计](MINIMAL.md)。

原生密钥生成属于 host 工具，口令由 ssh-keygen 输入、私钥留在用户指定位置；计划只保存公钥。配置组件尚未接入完整 Live 安装器，也没有默认启用网络调优、第三方内核、任意 Shell 钩子或新增系统仓库。
