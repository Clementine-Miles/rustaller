# 安装源与校验边界

四个自动目标都从 Alpine Live 执行，但目标根系统的来源不同。当前不增加四套 ISO 下载框架；用户指定本地 ISO 的 GRUB loopback 接口仍独立保留。

| 目标/阶段 | 使用资源 | 官方 HTTPS 来源 | 应有的信任链 |
|---|---|---|---|
| 公共 Alpine Live | x86_64 netboot 内核、initramfs、modloop | `dl-cdn.alpinelinux.org/alpine` | 官方 netboot tar 的签名、SHA256，再从已验证 tar 提取 |
| Alpine 目标 | 稳定版 APK 仓库 | 同上 | APK 信任密钥及包签名 |
| Debian 目标 | debootstrap minbase、APT 仓库 | `deb.debian.org/debian`、`security.debian.org/debian-security` | Debian archive keyring → InRelease/Release.gpg → 包索引与包摘要 |
| Arch 目标 | pacstrap、core/extra 软件仓库 | `geo.mirror.pkgbuild.com` | 可信 `archlinux-keyring`，软件包签名必需，数据库签名可选 |
| Arch 可选 CachyOS | 官方仓库安装脚本、优化仓库及 linux-cachyos | `mirror.cachyos.org` | 固定归档 SHA256、完整主密钥指纹、原生 pacman 包签名 |
| Gentoo 目标 | amd64 systemd stage3、软件仓库 | `distfiles.gentoo.org` | 可信 Gentoo release 密钥 → 签名索引/DIGESTS → 选定 stage3 SHA512 |

GitHub Release 分发程序，发行版资源与包来自各自官方来源。静态入口不内含完整发行版或安装介质；认证所需密钥与工具由各安装阶段建立。

## 可选 CachyOS 仓库

仅 Arch 显式选择 CachyOS 内核时使用[官方仓库方案](https://wiki.cachyos.org/features/optimized_repos/)。资源为 `https://mirror.cachyos.org/cachyos-repo.tar.xz`，固定 SHA256 `a1e3a74bdd12c10ac7317079e3e8c8e7d5557d818e4d4021364005a336b6afdc`，签名主密钥指纹 `882DCFE48E2051D48E2562ABF3B607488DB35A47`。摘要不匹配时停止，须重新审计后更新固定值。

清盘前下载并验证归档/完整密钥，随后仅在隔离 Arch 目标执行审核过的官方脚本：将短密钥 ID 引导替换为固定完整指纹、使用非交互 pacman，并使配置写入错误正常失败。脚本依据 glibc loader 报告的可用 ISA 选择 v3/v4 或适用的 znver4 仓库；低于 v3 使用官方基线 `[cachyos]`。包签名保持开启，不允许任意脚本 URL。

首次开机的 `rustaller-cachyos-update.service` 在网卡修复、network-online 与 networkd-wait-online 后执行 `pacman -Syu --noconfirm`、`mkinitcpio -P`、`grub-mkconfig`。失败保留标记并重试；全部成功后才移除标记，不额外自动重启。standard/LTS 不启用该仓库。

## Rust 源解析与只读在线审计

`src/host/sources.rs` 集中维护官方地址和四种资源类型，复用已有 Alpine/Debian 稳定版解析。它仅接受 Linux x86_64 目标：

- Alpine：选择唯一的 `alpine-netboot` 记录，绑定 branch、完整版本、x86_64 文件名和 SHA256；检查官方 `.sha256` 与 YAML 一致。不会给单个内核文件编造摘要 URL。
- Debian：检查 Origin、stable、amd64 和唯一版本/codename，将 InRelease 固定到选定 codename；检查签名消息中的发行版身份与原元数据一致。
- Arch：明确输出软件仓库及 `Required DatabaseOptional`/`archlinux-keyring` 要求，不把它当作 ISO 或 stage3 下载。
- Gentoo：从 clearsigned 索引中选择唯一的时间戳目录和 `stage3-amd64-systemd` 文件；从对应 clearsigned DIGESTS 中选择该文件唯一的 SHA512。拒绝其他架构、OpenRC、目录穿越、外站 URL、重复记录和无效摘要。

在开发机执行：

```console
cargo xtask audit-sources
```

审计用系统 `curl --disable` 禁用 `.curlrc`，继承代理/CA、保持 TLS，仅允许 HTTPS/HTTPS 重定向并设超时及次数上限。只 GET 每次最多 1 MiB 的元数据/摘要，大文件和签名仅 HEAD；失败不回退未知镜像或旧版本。

输出区分可达性、摘要解析/sidecar 一致与尚未执行的签名/完整文件验证。ASCII armor 解析不验证 OpenPGP；审计不下载密钥、不安装软件或执行分区/引导。实际安装须先验证信任密钥、签名及完整载荷，再解压使用；当前驱动已接通，但只读审计不证明安装成功。

## 2026-10-25 在线观察

下面是本次查询得到的元数据，不是写死的安装版本。稳定/滚动指针会更新，源解析器每次重新解析，之后应绑定精确资源。

| 查询 | 观察结果 |
|---|---|
| Alpine latest-stable x86_64 YAML | 3.24.3、branch v3.24、2026-10-21；netboot tar SHA256 为 `0228553464c9588ffeb63ec04d26abb5f79405006b74574034394199af47dab4` |
| Alpine netboot tar `.sha256` / `.asc` | 均可获取；`.sha256` 与 YAML 一致 |
| Alpine HTTPS virt 内核/initramfs/modloop | 三个 HEAD 请求均成功 |
| Alpine 单文件 `netboot/vmlinuz-virt.sha256` | 404；不能假设存在这种 sidecar |
| Debian stable Release | 13.7、trixie；stable 与 trixie-security 的 InRelease 可达 |
| Arch core.db | 可达；core.db.sig 为 404，因此不能把数据库签名当作必有附件 |
| Arch 官方镜像 SHA256 列表 | 有 2026.10.01 ISO/bootstrap；这两个文件不是当前 pacstrap 路径所需资源 |
| Gentoo latest-stage3-amd64-systemd | `20261018T124555Z/stage3-amd64-systemd-20261018T124555Z.tar.xz`，297255400 字节 |
| Gentoo 对应 stage3、DIGESTS、`.asc` | HEAD/小文件请求成功；DIGESTS 为 clearsigned，包含 SHA512/BLAKE2B |
| 辅助 Fedora 44 GRUB EFI 文件 | 原 HTTPS CloudFront 地址 HEAD 成功；Fedora 不是安装目标 |

上述历史审计未下载/散列完整 tar/ISO，也未验证 OpenPGP。内核包页检查覆盖 Alpine virt/lts、Debian cloud/generic、Arch lts、Gentoo kernel-bin，最终均返回 200；Debian generic 首次 503，有界重试成功。网页可达不替代包签名/摘要。

## 保留 Shell 基线中的待替换问题

Shell 文件为行为对拍基线，本轮不修改或实际运行其安装入口：

- `reinstall.sh` 的 Alpine netboot 地址仍使用 HTTP，注释说明 Live 校时晚于 initrd 启动。最终实现应区分 host 侧 HTTPS 下载和 Live 时钟初始化，不能用关闭 TLS 校验解决。
- `trans.sh` 的 Arch pacman 配置写有 `SigLevel = Never`。最终 Rust 安装链路不得继承它，需建立可信 keyring 并要求软件包签名；数据库签名缺失不构成关闭软件包签名的理由。
- Gentoo stage3 当前下载后直接解压，没有消费对应 DIGESTS/签名。最终实现必须先验证可信签名和完整 stage3 摘要。
- 旧 `test_url` 的可达性/格式检查不是密码学验证；Rust 稳定版文本解析本身也不是签名验证。

这些是保留 Shell 的已知问题，不可复制到 Rust。解析或只读审计不能证明修复，完整信任链与安装须独立验收。
