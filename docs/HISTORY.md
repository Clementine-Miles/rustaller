# 历史决策摘要

本文汇总旧计划、设计和续接记录。当前实施依据为 `AGENTS.md`、[架构](ARCHITECTURE.md)、[续接指南](AGENT-HANDOFF.md)和最新用户指令。

- 上游基准 `98f1e21013233d42750cc2e7a298a0818461d686`；保留 GPLv3 与署名。
- Shell 删除 Windows/ARM、地域检测、Debian Installer；四目标统一 Alpine Live，Debian 用 debootstrap minbase，Gentoo 保留 systemd stage3、原生内核与 GRUB。
- Shell 部署资源九减四，辅助函数与服务模板内联；保留固定 Git 对象作逐字节对拍。
- Rust 使用静态 host + 内嵌无界面 worker，以发行版原生工具安装；未采用早期 clap/serde/tokio 提案。
- 2026-10-25/26：加入英文多级 TUI、目标草稿、原生 ssh-keygen、当前系统配置并接通真实清盘调度；后续授权目标 swapfile 与隔离 VM 破坏性验收，开发宿主和生产盘仍禁作测试对象。
- 2026-10-28：src 文件 60→48，整合配置/包策略/存储/元数据，子进程输出直接继承。测量见 [REFACTOR.md](REFACTOR.md)。
- 2026-10-29：增加 UEFI/虚拟化检查、FAT32 ESP、initramfs 驱动；VM 发现并修复 OpenRC 后台日志及 RAM modloop 定位，缺少修复后的完整安装验收。
- 2026-10-30：src 文件 48→44，统一 Live 命令默认值和文档续接入口，无新增运行时依赖或 unsafe。
- 2026-11-02：采用 XFS 根、GNU dd 目标 swapfile，保留 GRUB/Alpine OpenRC；EFI Stub 取消，方案仅存历史。
- 2026-11-11：源码导入 `Clementine-Miles/rustaller`，统一项目命名与 GPLv3，保留上游声明。

历史源码/审计快照、VM 镜像和日志未随源码分发，不应假定旧工作区路径存在。行为对拍基准由 `tests/verify` 从固定 Git 对象读取，获取方法见[发布指南](GITHUB-RELEASE.md)。旧体积和测试数仅代表对应版本。
