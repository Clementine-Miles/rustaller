# 安装源与校验边界

四个自动目标都从 Alpine Live 执行，但目标根系统的来源不同。当前不增加四套 ISO 下载框架；用户指定本地 ISO 的 GRUB loopback 接口仍独立保留。

| 目标/阶段 | 使用资源 | 官方 HTTPS 来源 | 应有的信任链 |
|---|---|---|---|
| 公共 Alpine Live | x86_64 netboot 内核、initramfs、modloop | `dl-cdn.alpinelinux.org/alpine` | 官方 netboot tar 的签名、SHA256，再从已验证 tar 提取 |
| Alpine 目标 | 稳定版 APK 仓库 | 同上 | APK 信任密钥及包签名 |
| Debian 目标 | debootstrap minbase、APT 仓库 | `deb.debian.org/debian`、`security.debian.org/debian-security` | Debian archive keyring → InRelease/Release.gpg → 包索引与包摘要 |
| Arch 目标 | pacstrap、core/extra 软件仓库 | `geo.mirror.pkgbuild.com` | 可信 `archlinux-keyring`，软件包签名必需，数据库签名可选 |
| Gentoo 目标 | amd64 systemd stage3、软件仓库 | `distfiles.gentoo.org` | 可信 Gentoo release 密钥 → 签名索引/DIGESTS → 选定 stage3 SHA512 |

静态 Rust 入口不包含发行版、完整安装介质、包管理工具或信任密钥。GitHub Release 是本项目程序的分发渠道；四个系统的安装资源继续来自各自官方来源。

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

该命令使用系统 `curl`，首参 `--disable` 禁用用户 `.curlrc`，保持代理/CA 环境和 TLS 校验；只允许 HTTPS 和 HTTPS 重定向，设置连接/总超时及重定向上限。只 GET 小型元数据和摘要文件（每次最多 1 MiB），大文件和签名附件只执行 HEAD。任何源失败都会报告并返回失败，不切换为未知镜像或旧版本。

输出严格区分：URL 可达、摘要已解析、摘要 sidecar 一致、签名尚未验证、完整文件尚未下载/计算摘要。ASCII armor 结构解析不执行 OpenPGP 验证；这个审计命令也不下载密钥、不安装软件、不进入 chroot、不运行分区或引导命令。

真实安装接入时，必须先获得并验证可信密钥、签名及完整文件摘要，才允许解压或使用资源。当前 Rust 主安装流程仍未接通，不能因只读审计通过而将版本标为可实际安装。

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

本次未下载或散列完整 tar/ISO，也未进行 OpenPGP 签名真实性验证。可达性和摘要文件的存在不能替代这些检查。

新增内核选项还检查了官方包页面：Alpine v3.24 `linux-virt`/`linux-lts`、Debian trixie `linux-image-cloud-amd64`/`linux-image-amd64`、Arch `linux-lts`、Gentoo `sys-kernel/gentoo-kernel-bin` 最终均返回 200。Debian generic 包页首次返回 503，一次有界重试后成功；网页可达不代替仓库中的包签名/摘要检查。

## 保留 Shell 基线中的待替换问题

Shell 文件为行为对拍基线，本轮不修改或实际运行其安装入口：

- `reinstall.sh` 的 Alpine netboot 地址仍使用 HTTP，注释说明 Live 校时晚于 initrd 启动。最终实现应区分 host 侧 HTTPS 下载和 Live 时钟初始化，不能用关闭 TLS 校验解决。
- `trans.sh` 的 Arch pacman 配置写有 `SigLevel = Never`。最终 Rust 安装链路不得继承它，需建立可信 keyring 并要求软件包签名；数据库签名缺失不构成关闭软件包签名的理由。
- Gentoo stage3 当前下载后直接解压，没有消费对应 DIGESTS/签名。最终实现必须先验证可信签名和完整 stage3 摘要。
- 旧 `test_url` 的可达性/格式检查不是密码学验证；Rust 稳定版文本解析本身也不是签名验证。

这些问题尚不能因裁剪基线、解析 sidecar 或只读在线审计而宣称修复。只有完整安装链路实际使用上述信任检查并通过验收后，才可删除旧实现。
