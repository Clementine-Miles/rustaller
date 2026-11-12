# 安装前配置与一次重启目标

配置通过 TUI/CLI 写入 schema 5 计划。**确认磁盘身份后会执行真实安装；仅检查须加 `--inspect`。** 四目标 BIOS/UEFI 完整验收仍待完成，见[验收记录](ACCEPTANCE.md)。原生 ssh-keygen 会立即写入指定本机文件。

## 执行顺序

四种自动安装的目标流程调整为：

```text
原系统中的 TUI：完成全部配置、公钥准备、最终审核
  → 校验磁盘身份、现有网络、RAM、kexec 权限和内核能力
  → 验证官方资源签名/摘要，准备 Alpine Live 与匹配 worker
  → 将私密计划放入 initrd，不把口令塞入内核参数
  → kexec 切换到 Alpine 内存环境（SSH 会断开，不经过固件/GRUB 重启）
  → Live 再验证磁盘/RAM，安装 rootfs、账户、SSH、网络、内核与引导
  → 写入目标 DNS/网络配置，可选创建目标 swapfile，完成检查
  → 最后重启一次，进入配置完成的新系统
```

Rust 通过 kexec 进入 Live，仅在安装验证成功后最终重启；不静默退回 Shell 的两次重启流程。原系统 PID 1 须为 systemd/openrc-init；预检要求内核 kexec 配置、未禁用/lockdown 及可用工具，实际加载和平台兼容性仍需验收。ISO/netboot.xyz 为独立交互介质；`hold=2` 时由用户最终重启。

## 网络参考脚本取舍

2026-10-26 仅阅读 `https://box.dmitstock.com`：534595 字节，SHA-256 `b70844acb181b9b6c82f0904b682bdfb5908d188c3b73328568edc21e6766fda`。未执行、复制或作为运行时依赖；远程内容可能变化。

保留可映射到四目标原生配置的选项：

| 配置 | 默认 / 范围 | 作用 |
|---|---|---|
| DNS | 自动，或 1–3 个上游 IPv4/IPv6 | 静态解析器及 DHCP/RA DNS 覆盖控制 |
| 搜索域 | 无；最多 6 个 ASCII 域，总计 255 字节 | DNS 搜索路径 |
| 拥塞控制 | default / bbr / cubic | BBR 必须配 fq，并校验目标内核配置 |
| 默认队列 | default / fq / fq_codel | 校验目标内核配置，模块形式时准备原生加载配置 |
| IPv6 | inherit / enable / disable | 只控制新系统的 IPv6 禁用标志；不强行修改 RA/转发/地址 |
| 地址优先级 | default / ipv4 | Debian/Arch/Gentoo 的 glibc gai.conf；Alpine/musl 隐藏并拒绝 ipv4 选项 |
| 网卡 MTU | 留空沿用；1280–9000 | 只修改已生成目标网络文件中的非 lo 接口，不运行测速/探测 |
| TCP/socket 缓冲上限 | 留空沿用；1–64 MiB | 设置 socket 最大值和 TCP 三元组，不预留固定大小内存 |
| somaxconn / SYN / netdev 队列 | 留空沿用；128–262144 | 对应监听/SYN/收包积压队列 |
| TCP MTU probing | 留空沿用；0 / 1 / 2 | 关闭 / 黑洞时探测 / 始终探测 |
| TCP Fast Open | 留空沿用；0–3 | 关闭 / 客户端 / 服务端 / 两者，应用仍须支持 |
| SYN cookies | 留空沿用；0 / 1 | 显式关闭或开启 |

“填入参考脚本固定 TCP 参数”只填充草稿：BBR/fq、64 MiB 最大缓冲、somaxconn/SYN 队列 8192、netdev 队列 16384、MTU probing=1、Fast Open=3、cookies=1。可逐项修改或清除，不在当前主机执行 sysctl，也不表示这些参数一定提高性能。低内存 VPS 可保留内核默认或选较小上限。

不纳入安装配置：动态测速/BDP/HTB、第三方内核、NAT66 随机出网、IPv6 地址池、全量防火墙规则、服务商专用“默认值”、已过时的 `tcp_adv_win_scale`、`kernel.panic=-1`、强制 RA=2、自动重启当前网络服务等。它们不能作为四系统通用安装默认。

配置仅用于目标，Live 沿用原系统网络。拒绝禁用 IPv6 与 IPv6 DNS 同选，并须证明 IPv4 路径可用。TUI 尚不提供手动 IP/路由替换；IPv6-only 与提供商特殊网络另行验收。

### 原生落盘组件

- `shared::dns`：静态 `/etc/resolv.conf`；Alpine 全局关闭 dhcpcd/udhcpc 的 DNS 覆盖；networkd 文件设置 DNS/Domains 并禁止 DHCPv4/v6/RA 替换。自动模式不写文件。拒绝回环、多播、未指定及无作用域的链路本地 DNS，允许内网 DNS；不测试地址可达性。
- `shared::network`：目标 `/etc/sysctl.d/90-rustaller-network.conf`；模块加载配置；glibc 完整 precedence 表；MTU 写入已有 networkd 文件或 Alpine ifupdown stanza。修改在网络文件内部，首启按网卡名重命名文件不会丢失选项。
- OpenRC 的 sysctl/modules 服务以目标 runlevel 链接离线启用；服务脚本必须已存在且可执行。systemd 通过原生 modules-load/sysctl 读取配置。
- BBR/qdisc 使用**目标已安装内核**的 Kconfig，须与版本绑定、核对模块并在启动后验证。

这些函数只接收受检独立根目录，拒绝宿主 `/`、路径逃逸、符号链接父目录和不安全配置文件。resolver 最终链接通过 rename 替换而不跟随。所有可预检项先检查，各文件分别原子替换；不提供跨文件事务，调用方必须独占目标树。

## SSH 密钥生成

菜单：`账户与 SSH → 认证设置 → SSH 公钥 → 公钥管理 → 生成密钥`。

| 选项 | 默认 / 支持值 |
|---|---|
| 类型 | ed25519；也可选 RSA |
| KDF 轮数 `-a` | 512；1–65536 |
| 加密算法 `-Z` | aes256-ctr；另有 aes256-gcm@openssh.com、chacha20-poly1305@openssh.com，需系统工具支持 |
| 注释 `-C` | 系统 `date +%Y%m%d` 的结果；可编辑 |
| RSA 位数 `-b` | 4096；2048/3072/4096/8192，仅 RSA 显示 |
| 保存位置 `-f` | 本机绝对路径；默认当前用户 `.ssh/id_rustaller_ed25519` |

参数独立传入系统 ssh-keygen，不经过 Shell。生成时暂退 raw/备用屏幕，由原生工具询问口令，完成后恢复 TUI；口令不经过字段、计划、环境或 argv，空口令无 KDF 保护。不自动安装缺失工具。

私有暂存目录为 0700；拒绝覆盖、符号链接和他人可写父目录。私钥 0600、公钥 0644，仅公钥加入认证计划。**这是客户端登录密钥，清盘前须备份到自己的客户端。**普通添加公钥支持文件、URL、github:/gitlab:。

## 目标 swapfile

保留 GRUB BIOS/UEFI。清盘前扣除 ESP/对齐，根至少 512 MiB；这只是格式化下限。`mkfs.xfs -K` 跳过整卷 discard，不修改挂载/TRIM 策略。dd 同步后核对长度与分配块数，仍需目标实际激活验收。

菜单：`目标 swapfile → 启用 → 大小 MiB / swappiness`。默认不创建；这是本次用户明确要求后加入的 Rust 选项，保留 Shell 基线的无 swap 行为。

- 固定目标 `/swapfile`，64–65536 MiB。`bs=1M count=2048` 是 **2048 MiB = 2 GiB**；1 GiB 使用 1024 MiB。
- 只接受独立挂载的 XFS 目标根；先检查空闲空间并至少留下 512 MiB。该余量是最低保护，不是后续更新空间足够的保证；应在软件包安装完成后执行。
- Live 中调用 GNU dd（bs=1M、count=所选 MiB、conv=notrunc,fsync、status=progress）完整写零，避免稀疏文件；0600；只对这个新建、尚未启用的目标文件调用系统 `mkswap`。
- 不覆盖已有 swapfile、不改已有 swap 条目；成功后为目标 fstab 添加一次 `/swapfile none swap defaults 0 0`。
- 可选 swappiness 为 0–200，写入目标 sysctl 文件。systemd 通过 fstab generator 启用；Alpine 离线启用目标 OpenRC swap 服务（及必要的 sysctl）。
- **原系统/Live 不执行 swapon、swapoff 或修改其 fstab**。进入新系统的正常启动过程自动启用，不需要额外重启。安装最低 RAM 不因 swapfile 改变。

`CommandRunner` 的全局 swap 拦截保持有效；只有内部目标文件分配器产生的特定 mkswap 调用具有精确例外。测试以 mock 拦截格式化，不实际调用 mkswap/swapon。多个文件发布发生 I/O 错误时仍可能留下部分目标配置，安装驱动必须停止并提供检查信息。

## CLI 示例与执行边界

```console
./rustaller debian --confhome https://your.example/reinstall/pinned-version \
  --minimal --kernel cloud --hostname my-vps --timezone Etc/UTC \
  --ssh-key /root/client.pub --ssh-port 2222 \
  --dns 1.1.1.1 --dns 2606:4700:4700::1111 \
  --tcp-congestion bbr --qdisc fq --tcp-buffer-mib 16 \
  --tcp-mtu-probing 1 --tcp-fastopen 3 --tcp-syncookies 1 \
  --mtu 1450 --swapfile-mib 2048 --swappiness 10
```

示例确认后会进入 Live 清盘；仅检查须加 `--inspect`。schema 5 拒绝旧计划；密钥生成是立即执行的本机工具，不交给 worker。
