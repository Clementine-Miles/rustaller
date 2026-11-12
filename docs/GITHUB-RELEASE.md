# 构建与 GitHub 发布

仓库：[Clementine-Miles/rustaller](https://github.com/Clementine-Miles/rustaller)。当前为含真实清盘操作的开发预览：schema=5，XFS 根，GRUB，目标启动后激活可选 swapfile。四目标 BIOS/UEFI 完整验收见 [ACCEPTANCE.md](ACCEPTANCE.md)；Rust ISO/netboot.xyz/reset 交接仍未连接。

## 结构与界面

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

TUI 仅属于 host，共用 CLI 模型和安装回调；原生 ssh-keygen 临时恢复终端，仅公钥进入计划。菜单、草稿及凭据遮蔽见 [TUI.md](TUI.md)，模块职责见[架构](ARCHITECTURE.md)。

## 历史体积

以下测量只对应当时源码，不代表当前包大小、内存或安装速度。2026-10-28 的 preview.7 host 为 1,999,312 字节，worker 为 893,200 字节；重构前为 2,011,600 / 901,392 字节，详见 [REFACTOR.md](REFACTOR.md)。

| 版本 | TUI host（含 worker） | CLI host（含 worker） | worker |
|---|---:|---:|---:|
| 2026-10-26 preview.4 | 1,393,072 | 1,093,968 | 565,520 |
| 2026-10-25 多级 TUI | 1,331,632 | 1,077,584 | 565,520 |

上述配置使用 Rust 1.99.1、静态 musl、LTO、单 codegen unit、`opt-level=s`、strip。历史静态检查通过不代表完整安装验收。

## 本地使用

在项目根目录使用 `rust-toolchain.toml` 固定的工具链和 musl target：

```console
cargo xtask bundle
./target/x86_64-unknown-linux-musl/release/rustaller
```

无参数且 stdin/stdout 连接终端时打开 TUI；CLI-only 用 `cargo xtask bundle --no-tui`。草稿不执行安装，提交后须通过预检和完整磁盘确认，随后认证资源、kexec 进入 Live 并清盘安装。`REINSTALL_CONFHOME` 可预填匹配资源地址。

完整检查：

```console
cargo xtask check
```

需 Bash、BusyBox、OpenSSH client、ShellCheck 0.11+；可用 `TEST_BASH`、`TEST_BUSYBOX`、`TEST_SHELLCHECK` 指定工具。CI 用 `--skip-shellcheck` 后通过固定版本容器执行 ShellCheck。

本地预览打包：

```console
cargo xtask dist --tag v0.1.0-preview.1 --output artifacts/github-preview --allow-dirty
```

`--allow-dirty` 仅用于本地预览，记录 `source_dirty: true`，CI 不使用。输出目录须不存在，tag 须匹配 Cargo 版本，如 `0.1.0` 对应 `v0.1.0` 或其 prerelease。

| 文件 | 内容 |
|---|---|
| `rustaller-x86_64-linux-musl` | 唯一入口，内嵌精确匹配的 worker |
| `SHA256SUMS` | 入口、许可证、构建元数据摘要 |
| `LICENSE` | GNU GPLv3（`GPL-3.0-only`）完整条款；保留上游声明与署名 |
| `build-info.json` | 版本、schema、源码提交/脏状态、TUI、worker 哈希、`installer_complete: false` |
| `RELEASE-NOTES.md` | Release 正文，不作为下载附件 |

## 源码导入与对拍基准

已有仓库先 clone 再复制源码到根目录，保留隐藏目录和已有历史，不强制覆盖。源码包排除 `.git`、构建产物、VM、日志及私钥；`target/`、`artifacts/`、`dist/` 由 Git 忽略。

Shell 对拍依赖固定上游对象；CI 自动获取，本地执行：

```console
git fetch --no-tags --depth=1 https://github.com/bin456789/reinstall.git 98f1e21013233d42750cc2e7a298a0818461d686:refs/rustaller/upstream-baseline
cargo xtask check
```

以下是历史浅克隆迁移命令，仅在仓库确为浅克隆且需补齐原历史时使用：

```console
git fetch --unshallow origin
```

首次导入的提交/推送示例（已有远程历史须先整合）：

```console
git status --short
git add -A
git commit -m "Prepare Rust installer preview and GitHub releases"
git remote set-url origin https://github.com/Clementine-Miles/rustaller.git
git push -u origin main
```

保留上游声明与 GPLv3。Check 执行格式、Clippy、隔离/终端/Shell 测试、ShellCheck 和 musl 构建，不运行真实安装。

## 发布预览版本

Check 通过后，对待发布提交创建新 tag；以下仅为示例，勿复用已有版本：

```console
git tag v0.1.0-preview.1
git push origin v0.1.0-preview.1
```

Release preview 对 tag 重新检查、打包并创建 draft/prerelease；也可从 Actions 手动指定已有 tag。仅创建草稿的 job 有 `contents: write`，使用仓库 `GITHUB_TOKEN`。

审阅草稿与附件后才公开为开发预览。未公开草稿不能匿名下载；prerelease 须使用固定 tag，`latest` 不自动指向它。已有版本不覆盖。完整安装验收前保持 `installer_complete: false` 和预览状态。

## VPS 下载启动

以下固定版本命令仅在该 tag 与附件已公开后有效：

```sh
curl -fL 'https://github.com/Clementine-Miles/rustaller/releases/download/v0.1.0-preview.1/rustaller-x86_64-linux-musl' -o rustaller && chmod 700 rustaller && ./rustaller
```

下载为文件再运行可保留交互终端。安装需要 root；TUI 提交并确认磁盘身份后会清盘。Rust ELF 不使用 `curl | sh`。

完整验收并发布稳定版后，才使用以下稳定入口：

```sh
curl -fL 'https://github.com/Clementine-Miles/rustaller/releases/latest/download/rustaller-x86_64-linux-musl' -o rustaller && chmod 700 rustaller && ./rustaller
```

同时按附件原名下载 `SHA256SUMS`，运行 `sha256sum --check --ignore-missing SHA256SUMS` 核对完整性。

## 待完成

验收 host → kexec → Alpine Live → 四目标安装 → 一次最终重启全链路，接通 Rust 手动介质/reset。固定下载链接、静态检查和构建通过均不替代安装验收。
