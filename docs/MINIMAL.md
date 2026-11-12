# 四系统 minimal 设计与配置

minimal 保留启动、网络、SSH 与原生包维护能力，已实现包规划、配置和清理计划；**完整 minimal 安装/启动及系统体积尚未验收。**

| 系统 | 基础路线 | 精简范围与保留项 |
|---|---|---|
| Alpine | alpine-base / BusyBox / OpenRC；默认 linux-lts，可选 virt/LTS；OpenSSH、dhcpcd、CA、GRUB、所选文件系统工具 | 不主动选文档/语言/桌面/编译工具；保留必要固件、模块和包数据库 |
| Debian | debootstrap minbase，APT 不安装 recommends；默认 generic，可选 cloud；systemd/networkd/resolved、OpenSSH、CA、GRUB | 已是最小基础路线；dpkg 排除文档/man/info/翻译，保留 copyright |
| Arch | pacstrap base、archlinux-keyring、pacman、systemd/networkd、OpenSSH、CA、linux/linux-lts 或显式 CachyOS、GRUB | pacman NoExtract 排除文档/翻译并保留版权；保持签名验证，拒绝仓库级 SigLevel Never 等旁路 |
| Gentoo | systemd stage3、gentoo-kernel-bin/dracut、GRUB | 保持 Portage/Python/工具链；使用 FEATURES=nodoc noman noinfo，不改变 init/内核安装路线 |

容器 rootfs、内核元包和压缩下载均不代表可启动 VPS 的完整体积。2026-10-25 官方包页观察：Alpine linux-virt 约 49.4 MiB、Arch linux 约 161.6 MB；Gentoo stage3 下载已达数百 MB，均未含全部依赖/引导/文件系统开销。

参考：[Alpine](https://pkgs.alpinelinux.org/package/v3.24/main/x86_64/linux-virt)、[Debian](https://packages.debian.org/trixie/linux-image-cloud-amd64)、[Arch](https://archlinux.org/packages/core/x86_64/linux/)。这是历史包页观察，不是系统测量或载荷认证；信任边界见 [SOURCES.md](SOURCES.md)。

配置项：

| 选项 | 行为 |
|---|---|
| `--profile standard\|minimal` / `--minimal` | 默认 standard；minimal 开启裁剪计划 |
| `--with-tools` | 可选 curl/less，Gentoo 使用对应包 atom |
| `--keep-docs` | minimal 保留文档与翻译 |
| `--keep-package-cache` | minimal 保留下载缓存 |
| `--kernel` | 受支持的内核选择，见[映射](POSTINSTALL.md)；Debian 默认 standard/generic |
| `--filesystem xfs\|ext4` | 默认 XFS，minimal 不改变文件系统选择 |
| DNS、网络栈、swapfile | 见 [安装前配置](CONFIGURATION.md)；swapfile 会增加磁盘用量，默认关闭 |

TUI 按目标保存档位草稿，standard 不提交 minimal 隐藏项。共用 schema 6 计划；救援/ISO/netboot.xyz/reset 不接受目标配置。

`targets::packages::package_plan` 列出必要包/阶段，并非完整依赖闭包；`apply_package_policy` 写原生过滤配置并返回 `CleanupPlan`，不直接删除文件/缓存。debootstrap/stage3 已展开文档须经目标挂载/生命周期检查后清理，保留版权文件。

必须保留：启动文件、硬件所需固件与内核模块、CA、时区数据、许可证/版权、包数据库、软件仓库及正常更新能力。最低物理 RAM 仍为 Alpine/Debian 512 MB、Arch 1024 MB、Gentoo 2048 MB，不能根据下载大小或 swapfile 降低门槛。

验收分别记录压缩下载、根/boot 占用、缓存、启动 RAM、空闲空间；验证 SSH、IPv4/IPv6、续租 DNS、重启及包更新。分区容量、占用与下载体积分别报告。
