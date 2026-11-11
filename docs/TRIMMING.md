# 裁剪记录与边界

## 当前整理（2026-10-30）

- src Rust 文件 48→44：四个仅声明子模块的 mod.rs 合入 lib.rs，公开模块路径不变。
- 公共 Live 命令默认构造集中到 command::live_command；storage 和两个目标适配器复用。保留发行版环境、输入覆盖、错误和重试策略。
- 八份旧计划/设计/续接记录与旧审计目录说明合并为 HISTORY.md；避免旧“未接通安装”“全部禁止 swap”误导后续 agent。
- ARCHITECTURE.md 按当前 host→worker→targets/shared 结构重写；AGENT-HANDOFF.md 为统一续接入口。
- 保留 Rust 安装能力、安全检查、可选 TUI、测试及 Shell 对拍文件。未引入依赖和额外 unsafe；文件减少不代表二进制一定缩小。
- 使用已有命令对拍、单元与终端测试验证模块和构造器重构；完整 VM 验收状态见 ACCEPTANCE.md。

## 之前的裁剪

Shell 删除 Windows/ARM、Debian Installer、地理检测及多余部署资源；Rust 前次整合将 60 个 src 文件降到 48 个。原始决策见 [历史摘要](HISTORY.md)，2026-10-28 的源码学习、性能改动和二进制测量见 [重构记录](REFACTOR.md)。

## 继续裁剪的条件

尚未接通的 ISO、netboot.xyz、reset 组件不是可以随意删除的废代码。大文件应按真实职责拆分或消除重复，不以行数为唯一指标。不同发行版命令的 locale、stdin、重试和包签名语义不能因合并丢失。

Shell 基线及固定上游 Git 对象必须保留供对拍。源码包不带 .git，由固定 SHA 获取基准；未验收前不能删除兼容路径或改为稳定发布。
