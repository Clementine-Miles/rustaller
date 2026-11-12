# 裁剪记录与边界

2026-10-30 整理：src Rust 文件 48→44，将四个纯模块声明文件合入 `lib.rs`，公开路径不变；公共 Live 命令默认构造集中到 `command::live_command`，保留适配器环境、输入、错误和重试语义。未新增依赖或 unsafe。

旧计划/续接材料并入 [HISTORY.md](HISTORY.md)，以[架构](ARCHITECTURE.md)和[续接指南](AGENT-HANDOFF.md)说明当前实现。此前 Shell 删除 Windows/ARM、Debian Installer、地域检测及冗余部署资源；Rust 的 60→48 文件重构与测量见 [REFACTOR.md](REFACTOR.md)。

继续裁剪须保留：

- 安装与配置安全检查、TUI、测试、四个 Shell 配套资源及固定上游对拍对象。
- ISO/netboot.xyz/reset 的待接通组件，以及发行版 locale、stdin、重试、签名语义。
- 清晰职责；不以文件数推断体积或性能收益，不以静态检查替代[VM 验收](ACCEPTANCE.md)。

源码包不带 `.git`，须按固定 SHA 获取对拍基准；完整验收前保持开发预览。
