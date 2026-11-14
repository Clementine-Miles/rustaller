# DMITBox 本地版：去遥测、去服务端与同类功能整合审查

**版本**：`2026.11.13-local-r3`  
**输入**：此前的 `dmitbox-network-secure.sh`（网络守护版）  
**输出**：`dmitbox-local.sh`（本地单文件脚本）  
**适用定位**：直接在 Linux VPS 上使用的本地配置、诊断与维护程序，不提供供其他主机调用的远端管理服务器。

## 1. 本次目标及“完全本地”的界限

- **已实现**：不主动发送遥测数据；不含操作统计上传端点；不部署 DMITBox 网站/反向代理管理服务；不通过 `curl | bash`、`wget` 或类似方式下载并执行第三方 shell 脚本；不提供第三方 DD 重装入口。
- **并不等于物理隔绝互联网**：主动选择软件安装和更新时，包管理器会连接发行版仓库；主动 Ping、tracepath、DNS 解析或证书无关的域名检查时也可能产生普通网络流量。这些不是操作遥测，且不再在脚本启动时自动发起 HTTP 报告。
- **未删除已部署在真实系统上的资源**：本次操作只修改交付的脚本文件，不会自动卸载既有 Nginx、站点、证书、反向代理、云初始化服务或删除历史备份。若曾在 VPS 上安装相关资源，必须单独审计和迁移。
- **未提供远程控制面**：启动入口为本地交互菜单和本地 CLI，无长期监听的控制端口或管理 API。

## 2. 量化结果

| 指标 | 原网络守护版 | 本地版 | 变化 |
|---|---:|---:|---|
| 源码行数（`splitlines`） | 12,647 | 7,374 | 减少 5,273，约 41.7% |
| 文件字节 | 508,361 | 287,448 | 减少 220,913，约 43.5% |
| 函数定义次数 | 525 | 337 | 减少 188 处 |
| 不同函数名 | 514 | 337 | 净减少 177 个 |
| 同名重复定义 | 11 | 0 | 已清零 |
| 主菜单一级入口 | 11 | 7 | 聚焦本地系统配置 |

函数集合相较原版**删除了 181 个函数、增加 4 个本地替换函数**：`external_tcp_tuning_active`、`vm_guest_status`、`vm_guest_install`、`vm_guest_menu`。删除的函数中相当一部分属于不再需要的独立服务端功能，并非简单重命名。

## 3. 删除/替换内容

| 原模块 | 处理方式 | 主要原因 |
|---|---|---|
| 社区宣传及原有广告变量 | 从菜单、状态输出和文案移除 | 本地工具不应隐含推广与外部链接 |
| 遥测相关外联能力 | 脚本中不再有统计上报或 HTTP 请求执行函数 | 本地优先，避免未经明确选择的信息外传；原脚本实际上未包含 KejiLion 的 `send_stats` 上报函数 |
| 网站静态部署 `secure_site_*` | 删除完整的建站/证书/防护配置入口 | 这是本地 VPS 管理的可选扩展，不属于配置内核 |
| 反向代理 `reverse_proxy_*` | 删除 | 去除网站服务端及其外部证书申请流程 |
| 第三方测速与网络脚本在线执行 | 删除 `remote_script_*` 及测试入口 | 不执行未经固定版本和审计的远程代码 |
| DD 重装下载执行 | 删除 `dd_reinstall` | 高危险且不可在普通本地配置事务中安全恢复 |
| tcpfit 下载、验证与动态调优 | 删除安装/执行/更新逻辑 | 本地版保留 TCP/BBR 固定参数和原始快照，不接管外部程序 |
| DMIT 专用云初始化与网络自动重配 | 删除旧 `cloudinit_qga_*` 等复杂流程 | 防止 cloud-init 与网络守护器争夺配置归属 |
| KVM 访客组件 | 用 `vm_guest_*` 只读检查及用户主动的软件仓库安装替代 | 仅检查/安装，不写 cloud-init 网络配置或自动重启网络 |
| 旧强制 IPv6 修复 | 删除写 GRUB、黑名单、重启网络的流程 | 与受保护 IPv6 sysctl 操作冲突且可能断线 |
| 旧恢复全部设置 `restore_all` | 移除旧入口 | 旧实现不能保证全面恢复，容易误导使用者 |

**保留**：TCP/BBR 固定预设、系统诊断、IPv6 `/128` 池及 NAT66、DNS/MTU/SSH 受保护修改、包管理、Fail2Ban、Swap、系统清理与快照、KVM 来宾状态等本地功能。

## 4. 相似函数整合

| 职责 | 单一实际实现 | 调整 |
|---|---|---|
| 命令存在性 | `have_cmd` | 原 `netg_has` 合并至公共 helper，减少重复检测逻辑 |
| 默认出口网卡 | `default_iface` | 使用原网络守护层的路由感知方法；不再在找不到网卡时猜测 `eth0` |
| 网络接口 MTU 获取 | `netg_link_mtu` | 旧 `mtu_current` 保留为 UI 兼容适配器，不再独立读取/解析 MTU |
| DNS 设置及恢复 | `netg_dns_set_safe` / `netg_dns_restore_safe` | 移除旧 `dns_set`、`dns_apply_resolvconf`、`dns_apply_resolved` 等直接修改路径，UI 统一调用事务引擎 |
| DNS 首次基线备份 | `dns_backup_once` | **特意保留**：用于恢复脚本首次接管之前的状态，不可与每次独立事务快照混淆 |
| IPv6 基础开关 | `netg_ipv6_toggle_safe` | 移除原 `ipv6_enable/disable` 及直接 GRUB 修复，实现明确的定时回滚 |
| glibc 地址优先级 | `netg_gai_set_safe` | 移除多条直接修改 `/etc/gai.conf` 的旧入口；首次备份严格检查原始文件 |
| SSH 端口 | `netg_ssh_port_safe` | 统一走守护事务，再调用旧的受控 SSH 具体修改步骤 |
| 系统诊断和网络菜单 | `network_menu` + `netg_network_menu` | 合并重复入口和体检；主动链路测试只在显式选择时执行 |
| 旧防火墙规则修复 | 只读诊断，旧写入默认拒绝 | 解除对已删除建站模块的悬挂调用，避免偷偷变更其他服务的规则 |

虽然已消除同名函数覆盖，保留少量**有意的 UI 适配器/状态读取器**：它们不重复实现底层策略。代码尚不是全面的抽象重写，例如 SSH Key、Swap、NAT66 子系统仍有不少辅助函数。

## 5. 本地事务与 SHA-256

受保护写入的大致流程：

1. 读取内核/配置运行态并计算相关资源 SHA-256；防火墙只读指纹先规范化规则，排除计数器及 nft 动态 handle，减少无意义的变化。
2. 检查输入是否合法、当前配置能否备份、有无其他未完成的受保护事务。
3. 在仅 root 可访问的目录保存快照，写入独立回滚脚本，**先启动 systemd 超时回滚单元**。
4. 进行唯一的受控修改，再核验预期状态、记录变更后指纹。
5. SSH 用户必须从**新 SSH 会话**执行 `--net-confirm ID` 取消回滚；否则超时或主动操作将恢复快照。

当前默认回滚时间为 **180 秒**；不支持此守护单元的平台不执行对应高危写入。该 SHA-256 指纹是状态比较，不是带密钥的真实性证明，也不能替代来自 VPS 外部的连通性测试。独立定时器能降低断线风险，但尚未在真实 VPS 的网络中断和重启条件下验证。

## 6. 安全边界与仍需处理的问题

### P0 — 不要误认为所有路径均已事务化

- **旧 UFW/firewalld 防火墙写入**：默认禁止；仍可通过 `DMITBOX_ALLOW_UNGUARDED_FIREWALL=1` 显式开启遗留路径。它们尚无真正的原生 nftables 事务和超时恢复。除非有带外控制台，不建议开启。
- **SSH 端口变更**：SSH 配置文件可以回滚，但为新 SSH 端口增加的 UFW/firewalld 放行规则**尚未纳入完整逆向回滚**；服务商云防火墙也不受本脚本控制。
- **IPv6 `/64` 地址池 / NAT66**：保留旧的独立服务及 nftables 变更逻辑，不属于新的统一保护事务。其“本地自检”只检查规则与路由存在，不保证运营商实际将 `/64` 路由到该 VPS，也不能验证随机出站公网地址。
- **TCP/BBR 固定调优**：有原始参数快照和单独恢复路径，但不是 systemd 超时事务；不能承诺所有调优参数在不同内核上的最优性或总是可恢复。
- **非 systemd**：受保护写入刻意 fail-closed；只读诊断正常，旧模块某些高级写入也有独立的 systemd 依赖。不要将其宣传为支持全部 init 的安全配置工具。

### P1 — 本地、但不等于全离线

- `pkg_install` 仍支持 `apt-get`、`dnf`、`yum` 和 `apk`，会访问系统软件源；用户主动选择更新/安装才能触发。某些平台功能需要工具已存在，不会偷偷下载安装脚本。
- DNS 体检可能查询 `example.com`；路由测试可能向 1.1.1.1 或其他指定 IP 发送 ICMP/tracepath 探测。没有 HTTP 远端上报。
- 曾由 DMITBox 管理的文件保留兼容性命名（例如 `SCRIPT_NAME="dmitbox.sh"`、`/etc/dmitbox-*`），是为了保留旧配置所有权和迁移基础，并不代表仍搭建 DMITBox 服务器。
- 对 `/etc/gai.conf` 的原始备份采用安全拒绝策略：当该文件不存在时，首轮优先级变更会拒绝，而不是猜测原始基线。
- `write_file` 使用同目录临时文件 + rename 并拒绝覆盖符号链接；它不等同于全面保留原属主、SELinux 上下文、ACL 的通用文件事务。

## 7. 验证记录

执行过以下操作：

```bash
bash -n dmitbox-local.sh
bash dmitbox-local.sh --help
bash dmitbox-local.sh --version
bash dmitbox-local-tests/test-local-integration.sh
bash dmitbox-local-tests/test-network-guard.sh
```

**静态与本地模拟测试均通过**：包含函数名去重、入口调用检查、远端执行/遥测模式扫描、受管文件原子写入、符号链接拒绝、DNS 首次基线备份、DNS/MTU/IPv6/网卡/SSH 回滚的模拟测试，以及新 SSH 会话确认。

完整文件由两步源码重构脚本重新生成后，SHA-256 与此前交付版本**完全一致**；这属于构建可重复性检查，并非证明所有运行时路径正确。

**未做**：真实 VPS 上改变活动 SSH、接口、DNS、nftables、RA、DHCP、云防火墙，也没有进行真实断网、定时器触发或重启后恢复测试。因此这仍是经静态和模拟验证的开发版本；在正式 VPS 上应用高危配置前应先准备服务商带外控制台和完整备份。

## 8. 使用示例

仅只读诊断：

```bash
bash dmitbox-local.sh --help
bash dmitbox-local.sh --version
bash dmitbox-local.sh --net-report
bash dmitbox-local.sh --net-route-test
sudo bash dmitbox-local.sh --net-firewall-hash
```

受保护 DNS 示例（仅适用于 systemd 且已准备带外控制台的测试 VPS，网卡名称以实测为准）：

```bash
sudo bash dmitbox-local.sh --net-dns-set cloudflare ens3
sudo bash dmitbox-local.sh --net-list
# 新开独立 SSH 连接后：
sudo bash dmitbox-local.sh --net-confirm tx-XXXXXXXX
```

不要直接将示例中的 `ens3` 套用到任意主机。若希望只在菜单中操作，运行 `sudo bash dmitbox-local.sh`。

## 9. 交付说明

- `dmitbox-local.sh`：最终单文件，运行不依赖本重构包的 Python 脚本。
- `dmitbox-local.patch`：相对于上一版 `dmitbox-network-secure.sh` 的完整 diff，供逐行代码审查。
- `dmitbox-local-refactor.py` + `dmitbox-local-finalize.py`：本次重构的源代码变换过程，路径默认是 `/mnt/data`，适合在该工作环境中重建，不是给普通 VPS 使用的安装器。
- `test-network-guard.sh`、`test-local-integration.sh`：不触碰真实生产网络的模拟/静态验证。
- `SHA256SUMS.txt`：完整压缩包内主要文件的 SHA-256 摘要。
