# 四系统 minimal 设计与配置

minimal 的目标是保留可启动、联网、SSH 登录和原生包管理更新能力的 VPS 系统。当前实现了包集合规划、CLI/TUI 选项、原生包管理器过滤配置和清理计划；**尚未构建或启动完整 minimal 镜像，没有实测完整系统体积。**

| 系统 | 基础路线 | 精简范围与保留项 |
|---|---|---|
| Alpine | alpine-base / BusyBox / OpenRC；VM 推荐 linux-virt；OpenSSH、dhcpcd、CA、GRUB、ext4 工具 | 不主动选文档/语言/桌面/编译工具；保留必要固件、模块和包数据库 |
| Debian | debootstrap minbase，APT 不安装 recommends；linux-image-cloud-amd64，systemd/networkd/resolved、OpenSSH、CA、GRUB | 已是最小基础路线；dpkg 排除文档/man/info/翻译，保留 copyright；云内核不支持设备时选 generic |
| Arch | pacstrap base、archlinux-keyring、pacman、systemd/networkd、OpenSSH、CA、官方 linux/linux-lts、GRUB | pacman NoExtract 排除文档/翻译并保留版权；保持签名验证，拒绝仓库级 SigLevel Never 等旁路 |
| Gentoo | systemd stage3、gentoo-kernel-bin/dracut、GRUB | 保持 Portage/Python/工具链；使用 FEATURES=nodoc noman noinfo，不改变 init/内核安装路线 |

不能把“几十 MB 的 Alpine 容器 rootfs”当作可直接启动的完整 VPS 系统大小。2026-10-25 官方包页面记录 linux-virt 单包安装体积约 49.4 MiB，尚未包括用户空间、引导、依赖及文件系统开销；Arch linux 单包约 161.6 MB。Debian 的 cloud 内核元包本身很小，但依赖真正的内核包，不能用元包体积代替内核。Gentoo systemd stage3 的压缩下载已是数百 MB，不适合作为几十 MB 的承诺。

参考：[Alpine linux-virt](https://pkgs.alpinelinux.org/package/v3.24/main/x86_64/linux-virt)、[Debian cloud 内核](https://packages.debian.org/trixie/linux-image-cloud-amd64)、[Arch linux](https://archlinux.org/packages/core/x86_64/linux/)。这些是带日期的包页面观察，不是完成系统的测量或下载有效载荷验证；官方源信任边界见 [SOURCES.md](SOURCES.md)。

配置项：

| 选项 | 行为 |
|---|---|
| `--profile standard|minimal` / `--minimal` | 默认 standard；minimal 开启裁剪计划 |
| `--with-tools` | 可选 curl/less，Gentoo 使用对应包 atom |
| `--keep-docs` | minimal 保留文档与翻译 |
| `--keep-package-cache` | minimal 保留下载缓存 |
| `--kernel` | 受支持的官方内核选择；Debian 默认 cloud |
| DNS、网络栈、swapfile | 见 [安装前配置](CONFIGURATION.md)；swapfile 会增加磁盘用量，默认关闭 |

TUI 中对应“安装档位与裁剪”目录，各目标草稿独立保存；standard 时不提交隐藏的 minimal 保留开关。所有配置进入同一 schema 4 计划，不复制一套安装逻辑。救援、ISO、netboot.xyz、reset 不接受这些目标系统配置。

`targets::profile::package_plan` 给出必要包及安装阶段，列表不是完整依赖闭包。`shared::package_policy::apply_package_policy` 写入原生过滤配置并返回 `CleanupPlan`；**它不删除文件或缓存**。已展开的 debootstrap/stage3 文档仍需在完成挂载/生命周期校验后安全清理，不能直接对整个 doc 目录 rm -rf 而丢失版权文件。

必须保留：启动文件、硬件所需固件与内核模块、CA、时区数据、许可证/版权、包数据库、软件仓库及正常更新能力。最低物理 RAM 仍为 Alpine/Debian 512 MB、Arch 1024 MB、Gentoo 2048 MB，不能根据下载大小或 swapfile 降低门槛。

完整安装验收时应分别记录：压缩下载字节、安装后根目录/boot 大小、包缓存、首次启动 RAM、磁盘空闲量，并验证 SSH、IPv4/IPv6、DHCP 续租后的 DNS、重启与包管理更新。文件系统分区容量、使用空间和压缩下载体积应分别报告。
