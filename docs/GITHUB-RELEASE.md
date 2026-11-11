# GitHub 发布与轻量 TUI

当前源码包含真实清盘安装操作，仍为开发预览：主机准备、受验证 Alpine Live、kexec 与四目标安装驱动已连接，四系统 BIOS/UEFI 完整安装验收尚未完成。工作流默认创建 draft/prerelease，不会把它作为正式安装版更新到 `latest`。

当前计划 schema 为 **5**，Rust 自动安装根为 XFS，可选目标 swapfile 使用 GNU dd 写零，仅在目标系统启动后激活。确认完整磁盘身份令牌后会进入真实安装流程；手动 ISO/netboot.xyz/reset 的 Rust 交接尚未连接。验收状态见 [ACCEPTANCE.md](ACCEPTANCE.md)。

历史记录（2026-10-26）：当时计划 schema 为 **4**，包含 minimal/DNS/网络栈与目标 swapfile 选项；SSH 子菜单可直接调用系统 ssh-keygen。详细菜单见 [TUI](TUI.md)，一次最终重启与安装流程见 [配置说明](CONFIGURATION.md)。下列 2026-10-25 体积和旧 schema 记录是历史测量。

2026-10-25 更新：新增统一官方源解析、只读 `cargo xtask audit-sources`，以及官方内核/主机名/时区配置。安装计划 schema 为 2；旧计划会拒绝。详见 [下载源审计](SOURCES.md) 和 [基础配置](POSTINSTALL.md)。

目标仓库：[Clementine-Miles/rustaller](https://github.com/Clementine-Miles/rustaller)。以下命令已使用该地址；完整模块职责、实际执行边界及待接通流程见 [架构文档](ARCHITECTURE.md)。仓库地址配置不表示已经提交、推送或完成在线发布。

## 当前结构与裁剪

```text
src/bin/rustaller.rs        CLI / 可选 TUI → 共用预检、磁盘确认与安装入口
src/tui/                   目录/字段/导航/计划分离；安装预检与独立本机密钥生成工具
src/host/                  主机检查、引导、ISO 组件
src/worker/                initrd、Live 检查、首启修正、日志组件
src/targets/               Alpine / Debian / Arch / Gentoo helpers
src/shared/                分区、挂载、清理
xtask/src/bundle.rs        先构建无 TUI worker，再嵌入 host
xtask/src/check.rs         Rust 编排的静态/隔离检查
xtask/src/dist.rs          发布包、SHA-256、构建状态
.github/workflows/         GitHub 检查与 Release 草稿流程
```

TUI 是 host 的可选 feature，未放进共享库或 worker。它复用 CLI 参数解析与模型校验，提供多级配置目录、密码确认、多公钥管理和配置汇总；密码/公钥/frpc 数据遮蔽，不增加第二套安装逻辑。原生密钥生成工具临时恢复终端供 ssh-keygen 询问口令，安装计划只保存公钥。界面由按键事件驱动，没有后台定时刷新。Alpine Live 救援固定 `hold=1`，不显示目标安装选项；ISO 只显示路径、引导参数及启动模式。菜单清单与快捷键见 [TUI 使用说明](TUI.md)。

旧审计快照的 59 个文件（含隐藏文件，约 1.1 MiB）已完整移到仓库旁 `../archive/upstream-audit-snapshot/` 保留，不进入 GitHub 源码树；它们全部通过原交接清单校验。必要的 Git 基准对象、Shell 对拍基线、测试和四个旧配套资源继续保留。`target/`、`artifacts/`、`dist/` 均被 Git 忽略。

## 当前体积与验证（2026-10-28）

重构后的 `v0.1.0-preview.7` 静态 TUI host 为 **1,999,312 字节（约 1.91 MiB）**，嵌入 worker 为 **893,200 字节**。同配置重构前分别为 2,011,600 和 901,392 字节。本轮未测量 CLI-only 包，不能以旧版本体积推算当前 TUI 增量。

263 项 Rust + 49 项 Shell 测试、格式/Clippy/ShellCheck/语法检查通过；静态包校验和、匹配 worker 嵌入及 4 项发布二进制伪终端测试通过。产物在 `artifacts/refactor-after-20261028/`，仍为未提交源码的本地开发预览。此前实际 BIOS 测试到达 Alpine Live 后安装未完成，完整矩阵仍待验收。模块合并和参考源码见 [重构记录](REFACTOR.md)。

## 历史配置预览体积与验证（2026-10-26，preview.4）

| 静态 musl 产物 | 字节 | 约合 |
|---|---:|---:|
| 多级 TUI host，含嵌入 worker | 1,393,072 | 1.33 MiB |
| CLI-only host，含嵌入 worker | 1,093,968 | 1.04 MiB |
| worker，无 TUI | 565,520 | 0.54 MiB |

TUI 增量为 299,104 字节（约 292 KiB）；TUI host 比 2026-10-25 多级版本增加 60 KiB。无新增运行依赖。两种构建中的 worker SHA256 均为 `1524cebe6ff8b6ba242ab64e23878b369556ad1866711155c4cd1a87da1caca7`；大小相同不表示与旧版本字节相同。

- `cargo xtask check`：216 项 workspace + 49 项 Shell 基线测试，格式、Clippy、ShellCheck、Bash/ash、diff 检查通过。
- 发布 TUI 二进制另通过 3 项真实终端测试，包含原生加密密钥生成、不回显口令、返回界面和退出终端恢复。
- 本地包 `artifacts/config-preview-20261026/` 与 CLI-only 对照 `artifacts/config-preview-cli-20261026/`：`v0.1.0-preview.4`、schema 4、未提交源码、安装未完成；两包 SHA256 校验通过。
- 没有执行真实安装、格式化、swapon/swapoff、kexec、重启或 NVRAM 写入；未提交、推送或在线发布。

## 历史体积（2026-10-25）

同一 Rust 1.99.1 工具链、静态 musl、LTO、单 codegen unit、去符号条件下，将发布优化从 `opt-level=3` 调整为 `s`：

| 产物 | 字节数 | 约合 |
|---|---:|---:|
| 旧 CLI host，优化前 | 1,143,120 | 1.09 MiB |
| 新 CLI host，无 TUI | 1,077,584 | 1.03 MiB |
| 新 host，多级 TUI（2026-10-25） | 1,331,632 | 1.27 MiB |
| 新 worker，始终无 TUI | 565,520 | 0.54 MiB |

当前 TUI 相对新 CLI 入口增加 254,048 字节，约 248 KiB。两个构建中的 worker 哈希一致；旧 worker 为 610,576 字节，尺寸优化后减少 44 KiB。相对上一轮平面配置预览（1,286,576 字节），本次多级菜单增加 44 KiB；CLI 和 worker 字节数未变。这些数据是当前配置预览界面的文件大小，没有据此推断运行内存、速度或未来完整安装器的最终体积。

## 本地使用

在项目根目录执行（需要 Rust/rustup，仓库已固定工具链及 musl target）：

```console
cargo xtask bundle
./target/x86_64-unknown-linux-musl/release/rustaller
```

无参数且 stdin/stdout 都连接终端时进入 TUI；`--help` 和现有 CLI 参数继续可用。CLI-only 构建使用 `cargo xtask bundle --no-tui`。四个自动目标的匹配 HTTPS 资源根可通过 `REINSTALL_CONFHOME` 预填；配置草稿本身不执行安装；提交后先预检，要求完整磁盘身份确认，再下载并认证资源、kexec 进入 Live 和安装。

全部本地检查：

```console
cargo xtask check
```

需要 Bash、BusyBox、OpenSSH client（真实终端密钥生成测试）、ShellCheck 0.11+；可用 `TEST_BASH`、`TEST_BUSYBOX`、`TEST_SHELLCHECK` 指定可执行文件。CI 安装 openssh-client，显式用 `check --skip-shellcheck`，随后通过固定版本容器执行同样的 ShellCheck，避免重复运行。

生成本地发布预览：

```console
cargo xtask dist --tag v0.1.0-preview.1 --output artifacts/github-preview --allow-dirty
```

`--allow-dirty` 仅用于未提交的本地预览，构建信息会记录 `source_dirty: true`；GitHub 工作流不使用该选项。输出目录必须不存在，不覆盖已有发布包。Tag 必须匹配根 Cargo package 版本：`0.1.0` 对应 `v0.1.0` 或 `v0.1.0-preview.1` 等。

输出包含：

- `rustaller-x86_64-linux-musl`：唯一入口，嵌入匹配 worker。
- `SHA256SUMS`：入口、许可证和构建元数据的 SHA-256。
- `LICENSE`：上游 GPL-3.0 许可证。
- `build-info.json`：版本、计划 schema、源码提交、是否有未提交修改、TUI、worker 哈希，以及 `installer_complete: false`。
- `RELEASE-NOTES.md`：用于 Release 正文，不重复作为下载附件。

## 第一次放到 GitHub

你已创建 `Clementine-Miles/rustaller`。希望 VPS 免登录 curl 下载时，应使用公开仓库的公开 Release；私有仓库下载需要认证。GitHub Pages 不参与二进制分发。以下首次推送示例以远程仓库为空为前提；若已有 README/许可证等提交，先核对历史并整合，不强制覆盖。

源码压缩包不包含 `.git`、编译产物或虚拟机测试资料，可以上传到你的仓库。解压后，将 `rustaller/` **里面的内容**放到仓库根目录，保留 `.github`、`.cargo`、`.gitignore` 等隐藏文件；不要再套一层 `rustaller/`。已有远程仓库时先 clone，再复制并审阅差异，不强制覆盖历史。

对拍测试依赖固定上游提交 `98f1e21013233d42750cc2e7a298a0818461d686`。GitHub Check 工作流现在会自动获取这个对象，新仓库无需包含上游全部历史。源码包解压后，本地测试前在该仓库执行：

```console
git fetch --no-tags --depth=1 https://github.com/bin456789/reinstall.git 98f1e21013233d42750cc2e7a298a0818461d686:refs/rustaller/upstream-baseline
cargo xtask check
```

若使用当前带 `.git` 的工作区而不是源码包，可以保留原历史，按下面步骤处理。

本交接仓是浅克隆；在更改 origin 到新仓库前，先补齐原上游历史：

```console
git fetch --unshallow origin
```

已经不是浅克隆时跳过。随后审阅本地变更，再由你执行首次提交和推送（本轮 Codex 未执行）：

```console
git status --short
git add -A
git commit -m "Prepare Rust installer preview and GitHub releases"
git remote set-url origin https://github.com/Clementine-Miles/rustaller.git
git push -u origin main
```

保留上游署名和许可证。推送后在 Actions 查看 Check：它运行格式、Clippy、单元/伪终端/隔离 Shell 测试、ShellCheck 和 musl 构建；不会运行真实安装。

## 发布预览版本

Check 通过后，推送版本 tag：

```console
git tag v0.1.0-preview.1
git push origin v0.1.0-preview.1
```

Release preview 工作流对该 tag 重新检查和打包，再创建一个 draft/prerelease。也可从 Actions 手动运行，填写已有 tag。仅最终创建草稿的 job 拥有 `contents: write`，使用仓库自带 `GITHUB_TOKEN`，无需另放个人 token。

到 Releases 审阅草稿与附件后，可公开为开发预览。未公开的草稿不能匿名下载；prerelease 使用固定 tag 地址，`latest` 不会自动指向它。已有版本不会被覆盖，后续修订使用 `v0.1.0-preview.2` 等新 tag。

## VPS 的 curl 启动

公开 `v0.1.0-preview.1` 预览 Release 后，在 VPS 终端执行；该地址在版本和附件公开前不可用：

```sh
curl -fL 'https://github.com/Clementine-Miles/rustaller/releases/download/v0.1.0-preview.1/rustaller-x86_64-linux-musl' -o rustaller && chmod 700 rustaller && ./rustaller
```

这会打开英文 TUI；提交自动安装配置并确认磁盘身份后会执行真实清盘安装。下载到文件后执行可以保留交互终端；Rust ELF 不通过 `curl | sh` 执行。安装需要 root，完整安装矩阵尚未验收，当前仅为开发预览。

完整安装流程验收通过并发布正式版本后，稳定入口可以固定为：

```sh
curl -fL 'https://github.com/Clementine-Miles/rustaller/releases/latest/download/rustaller-x86_64-linux-musl' -o rustaller && chmod 700 rustaller && ./rustaller
```

若要检查完整性，按原附件名下载入口及 `SHA256SUMS`，再运行 `sha256sum --check --ignore-missing SHA256SUMS`。源码、许可证和构建信息同时保留在 GitHub。

## 未完成项

GitHub 流程已有本地配置，尚未在用户仓库在线运行。仍需验收已连接的 host → 受验证 initrd → kexec → Alpine Live → 四系统安装 → 最后一次重启的完整链路，并接通本地 ISO 的实际引导。ISO 按绝对路径启动依赖介质的 loopback 支持，必要时需内核/initrd/启动参数。

当前自动安装入口已绑定设备身份，并要求完整磁盘确认令牌；确认后的操作会清盘。四目标 BIOS/UEFI VPS 验收通过后，再调整正式发布策略并移除旧 Shell 运行时。

## 历史验证：多级 TUI（2026-10-25）

- `cargo xtask check`：193 项 workspace + 49 项独立 Shell 基线测试通过；含 16 项 TUI 单元测试及 2 项伪终端测试，格式、Clippy、ShellCheck、Bash/ash 和 diff 检查通过。
- 多级菜单、输入/取消、密码确认、多公钥、各目标 CLI 计划一致性、目录草稿、结果页返回不重试、秘密遮蔽与滚动/尺寸边界均有隔离验证。
- TUI/CLI-only 静态 musl 构建通过，worker SHA256 保持 `5712d480fd5dc35f04ca95bc0f811b948a72d18d419166d586e7f3fdd075b617`。发布二进制另通过初始取消及进入密码编辑器后取消的实际终端检查。
- 本地包 `artifacts/tui-preview-20261025/`：`v0.1.0-preview.3`、`source_dirty: true`、`plan_schema_version: 2`、`installer_complete: false`；SHA256 校验通过。默认 target 目录最终保留带 TUI 的入口。
- 未提交、推送、在线发布或运行真实安装。

## 此前验证：基础配置与源审计（2026-10-25）

- `cargo xtask check`：181 项 workspace 测试和 49 项独立 Shell 基线测试通过；Clippy、Rustfmt、ShellCheck、Bash/ash 语法和 diff 检查通过。
- `cargo xtask audit-sources`：四目标源审计通过。只读取小型元数据和 HEAD，不代表完整文件摘要或 OpenPGP 签名已验证。
- TUI/CLI-only 静态 musl 打包通过，嵌入 worker 精确匹配；两个构建的 worker SHA256 均为 `5712d480fd5dc35f04ca95bc0f811b948a72d18d419166d586e7f3fdd075b617`。
- 本地包 `artifacts/refactor-preview-20261025/` 标记 `v0.1.0-preview.2`、`source_dirty: true`、`plan_schema_version: 2`、`installer_complete: false`。GNU SHA256 校验通过；实际发布二进制的伪终端取消/状态恢复测试通过。
- 未提交、推送或在线运行 Actions；未执行真实安装、分区、挂载、重启或 NVRAM 写入。

## 此前验证（2026-10-21）

- `cargo xtask check`：143 项 workspace 测试、49 项独立 Shell 基线测试通过；含 TUI 表单/凭据遮蔽、计划一致性、伪终端退出与发布包负例。Clippy、Rustfmt、ShellCheck、Bash/ash 语法与 diff 检查通过。
- actionlint 1.7.12：两个 GitHub workflow 的配置和内联命令通过本地检查；尚未实际推送或运行 GitHub Actions。
- TUI 与 CLI-only musl 打包成功；两个 worker 字节一致，host/worker 无 PT_INTERP/PT_DYNAMIC。发布包三个文件的 SHA-256 检查通过。
- 实际发布的 musl TUI 二进制另经伪终端验证：取消成功退出、恢复终端模式，不进入主机预检。未提交源码的正式打包请求按预期拒绝。
- 59 个归档文件匹配原清单，LICENSE 与上游一致；未暂存、commit、push，未执行真实安装、分区、挂载、重启或 NVRAM 写入。
