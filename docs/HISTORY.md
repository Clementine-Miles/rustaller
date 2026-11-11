# 历史决策摘要

此文替代旧 PLAN.md、RUST-CONTINUATION.md、六份 superpowers 计划/设计及 upstream-audit 说明。旧文档有过时的依赖提案、测试数、环境信息和尚未安装的状态；当前实施依据为 AGENTS.md、[架构](ARCHITECTURE.md)、[续接指南](AGENT-HANDOFF.md) 与最新用户指令。

## 保留的决策

- 上游基准 98f1e21013233d42750cc2e7a298a0818461d686；GPLv3 与署名不变。
- Shell 阶段删去 Windows/ARM 目标、地理检测、Debian Installer；统一四目标从 Alpine Live 安装，Debian 用 debootstrap minbase。
- Gentoo 保留 systemd stage3、原生内核/GRUB 路径；没有改用另一个迁移设计。
- Shell 资源从九个压缩到四个，辅助函数与服务模板内联；保留对拍基线。
- Rust 采用单静态 host + 嵌入式无界面 worker，原生发行版工具完成安装；未引入早期提议的 clap/serde/tokio。
- 2026-10-25/26：加入英文多级 TUI、目标草稿、原生 ssh-keygen、当前系统持久配置；接通自动安装调度，带实际清盘能力。
- 用户后来授权可选目标 swapfile 和隔离 VM 破坏性验收。这覆盖早期“任何自动路径不得创建 swap”“不执行任何真实分区”的旧计划措辞；开发宿主机和生产盘依旧不能作为测试对象。
- 2026-10-28：src Rust 文件 60→48，统一配置、包策略、存储和源元数据；子进程流输出直接继承。详细源码学习与测量保留在 [REFACTOR.md](REFACTOR.md)。
- 2026-10-29：增加 UEFI/虚拟化检查、FAT32 ESP、initramfs 驱动策略；VM 发现 OpenRC 日志后台设置及 RAM modloop 定位故障并修改源码。最终修复尚缺新一轮完整安装验证。
- 2026-10-30：src 文件 48→44，合并公共 Live 命令默认构造，压缩历史文档并建立统一续接入口；没有新增 unsafe 或运行时依赖。

## 历史资料与可复现性

本轮整理前的 src、docs、README、AGENTS 快照保存在原工作区 /workspace/scratch/trim-20261030/before.tar.gz，旧 2026-10-29 源码包也未覆盖。它们不属于新的精简源码包，新 agent 不应依赖这些本地路径存在。

更早的 upstream-audit/snapshot 曾被移到原交接仓旁的 archive/，并非当前代码或测试依赖。本次移除了它的过时入口说明。

行为基准由 tests/verify 从固定 Git 对象读取。源码包不带 .git；本地取得对象的方法与 GitHub CI 配置见 [GITHUB-RELEASE.md](GITHUB-RELEASE.md)。历史版本的大小/测试数只对应当时源码，不用于证明最新安装兼容性。
