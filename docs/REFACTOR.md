# Rust 结构精简与执行优化（2026-10-28）

本轮以重构开始时的工作树为基准，不与尚无 Rust 实现的上游 Git 提交比较。原始源码快照位于 `/workspace/scratch/refactor-20261028/before.tar.gz`。功能范围、CLI 参数、计划 schema 4、运行时标记和 host/worker 分工保持一致；内部 Rust 模块路径已更新，不额外保留一套转发 API。

## 实际阅读的参考源码

- [zoxide，f0b31631](https://github.com/ajeetdsouza/zoxide/tree/f0b31631d295b0623bc440016ed0aba3c7c49dde)：阅读 `src/main.rs`、`src/db/mod.rs`、`src/util.rs` 和 `Cargo.toml`。借鉴按职责聚合配置与工具能力、集中原子文件写入的结构；现有 `fsutil` 的独占临时文件、sync、rename 和失败清理继续复用。其单 codegen unit、LTO、strip 本项目此前已启用，不算本轮新增收益。
- [fd，b7a0a29d](https://github.com/sharkdp/fd/tree/b7a0a29d07c3d0e9e0a41155019acada08831a6c)：阅读 `src/exec/mod.rs`、`src/exec/command.rs`、`src/walk.rs`、`src/exit_codes.rs` 和 `Cargo.toml`。采用执行路径的 `Stdio::inherit()` 思路，仅在真正需要捕获输出时建立管道和读取线程。

参考仓库放在项目之外，没有复制源码、引入它们的 CLI 框架、分配器或依赖树。安装步骤有严格先后顺序，未套用 fd 的并行遍历模型。

## 合并与删除

| 旧模块 | 新归属 |
|---|---|
| system_config、network_config、swap_config | `src/config.rs` |
| targets 的 profile、Arch/Gentoo 包选择，shared/package_policy | `src/targets/packages.rs` |
| shared/disk、shared/mount、公共存储错误与命令工具 | `src/shared/storage.rs` |
| shared/services | `src/shared/configure.rs` |
| host/metadata | `src/host/sources.rs` |
| host/kexec 的只读策略检查 | `src/host/preflight.rs` |
| embedded | `src/lib.rs` |
| targets/install/rolling | `src/targets/rolling.rs`，去掉仅容纳单文件的目录 |

删除了没有生产调用者、已经被实际 Live 驱动替代的 Rust 函数：Alpine `run_setup_disk`、`cleanup_setup_disk_packages`、`add_openrc_service`、`kernel_flavor` 和配置转发包装；旧 Arch `pacstrap`；旧 Gentoo `run_emerge`；旧存储 `mount_pseudo_filesystems`、`unmount_previous`、`unmount_pseudo_filesystems` 及其两项内部匹配函数。对应旧包装器测试退出，仍在使用的内核/包选择断言并入 `target_config`。

原来的宽范围卸载匹配只用于早期 Shell 对拍，实际驱动采用身份绑定和仅卸载自身目标树的逻辑。保留 Shell 文件及其独立验证 crate，仍保留有效的分区/挂载命令对拍，不删除正式安装需要的身份、权限、签名或输入校验。

## 执行与内存

- `OutputMode::Stream` 直接继承父进程描述符。通常 stdout/stderr 都实时输出的命令从两个转发线程降为零，并省去两条额外管道和转发时的重复锁定/flush；子进程能够观察真实终端属性。
- `Capture` 保留并发读取，以免大输出和 stdin 回写相互堵塞。读取器采用泛型静态分派及标准库 `read_to_end`，移除 `Box<dyn Read + Send>` 和手写分块转发循环。
- kexec 与网络模块能力检查用迭代器检查唯一值，省去收集临时 `Vec`，仍拒绝重复或冲突的 Kconfig 项。
- 未新增运行时依赖。worker 继续不包含 TUI，主机 TUI 仍由可选 feature 控制。

`unsafe` 不是编译加速开关。zoxide 的环境变量修改、fd 的信号处理都有特定用途；本轮没有为优化添加 unchecked 索引或扩大裸指针使用。保留必需的 Linux FFI，并补充私密计划原子发布的 ABI、指针寿命和参数安全说明。

## 测量与验证

| 指标 | 重构前 | 重构后 |
|---|---:|---:|
| src 下 Rust 文件数 | 60 | 48 |
| src 下 Rust 行数，含内联测试 | 21,159 | 20,842 |
| 静态 TUI host，含匹配 worker | 2,011,600 字节 | 1,999,312 字节 |
| 静态 worker | 901,392 字节 | 893,200 字节 |

文件减少 20%，源码净减 317 行；host 减少 12 KiB，worker 减少 8 KiB。两次使用相同 Rust 1.99.1、x86_64 musl、LTO、单 codegen unit、opt-level=s、panic=abort 和 strip。未据此推断完整安装耗时或峰值内存下降；磁盘/下载/原生包管理仍决定大部分安装时间。

- `cargo xtask check`：263 项 Rust workspace 测试及 49 项 Shell 基线测试通过；Rustfmt、Clippy `-D warnings`、ShellCheck、Bash/ash、diff 检查通过。
- 新增直接继承输出/私密捕获测试，以及 2 MiB 管道输入输出测试；现有配置、磁盘身份、安全边界、四目标包选择和 TUI 测试继续通过。
- `v0.1.0-preview.7` 静态包的精确 worker 嵌入与 SHA256 校验通过；实际静态二进制通过 4 项伪终端测试，包括返回/丢弃当前配置、密码遮蔽、原生 ssh-keygen 和终端恢复。
- 输出：`artifacts/refactor-after-20261028/`；对照：`artifacts/refactor-before-20261028/`；日志：`/workspace/scratch/refactor-20261028/`。

此前 v5 实际 BIOS 测试通过 kexec 到达 Alpine Live，但未完成自动安装，最终超时。四目标 BIOS/UEFI 全流程仍未验收；本轮重构和静态检查不改变这一结论，详见 [验收记录](ACCEPTANCE.md)。保留开发预览和 `installer_complete: false`。未提交、推送或发布 GitHub Release。
