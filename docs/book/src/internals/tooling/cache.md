# 缓存实现

这一章说明 CLI 的 `cache` 命令和两套磁盘缓存的开关。

## cache 命令

`cache` subcommand 管理已经落盘的 native image 条目（`*.wnat`）：

| 子命令 | 行为 |
| --- | --- |
| `cache stats` | 显示缓存目录、条目数与总字节数 |
| `cache clear` | 清除所有 `.wnat` 并报告删除数量 |
| `cache prune --max-bytes N` | 按最旧修改时间删到字节上限 |

目录解析只认 `--dir` 或 `WJSM_CACHE_DIR`。两者都没有时直接报错，不回落 `$HOME/.cache/wjsm`。`--dir` 只影响这次管理命令，不会替后续 `run` 打开编译缓存。

公开统计来自 `NativeImageRepository::stats()`（`wjsm-backend-native`），经 `NativeExecution` 的 cache 字段暴露给 CLI `--stats`。没有 `module_cache_stats` / `clear_module_cache` / `module_cache_dir()`。

## 缓存目录

磁盘缓存是 opt-in，没有默认目录：

1. `WJSM_CACHE_DIR` 被设置 → 使用该路径。
2. 未设置或空值 → `NativeRuntimeConfig.cache_dir = None`，不读写磁盘。
3. `NativeRuntimeConfig::default()` / `RuntimeOptions::default()` / `child_config()` 都是 `None`。

`wjsm run`、in-process fixture 和普通测试套件都不设这个变量。测试里只有 `tests/cluster_ipc.rs`（`/tmp/wjsm-test-cache`）和 builtin cache e2e 会显式打开。

## Native image cache

`NativeImageRepository` 是进程内 image 与磁盘 cache 的唯一 owner。`cache_dir` 为 `None` 时只做内存 Weak 池和 in-flight gate，编译产物不落盘。

磁盘条目是 `${WJSM_CACHE_DIR}/<sha256>.wnat`。key 不是 SipHash，而是 `NativeCacheKey` 的 SHA-256：

| 维度 | 来源 |
| --- | --- |
| cache schema | `CACHE_SCHEMA` |
| portable artifact digest | `.wjsm` 内容 SHA-256 |
| native ABI hash | `native_abi_hash()` |
| native codegen source hash | `NATIVE_CODEGEN_HASH` |
| 当前 target | `{ARCH}-{OS}` |
| Cranelift 版本 | `CRANELIFT_VERSION` |
| codegen / ISA settings | `NativeCompiler::settings_key()` |

命中时加载 `.wnat` object 再 `CompiledImage::load`；header / object / hash / 权限校验失败计为 invalidated 并重编译。同 key 的并发 prepare 由 in-flight gate 合并。

## builtin IR 段缓存

多文件项目每次冷启动都要把入口依赖的 Node builtin 模块重新 lower 成 IR。`wjsm-module/src/builtin_cache.rs` 把这部分产物按依赖闭包序列化到磁盘。

| 条件 | 行为 |
| --- | --- |
| `WJSM_CACHE_DIR` 已设置且 `WJSM_NO_BUILTIN_CACHE` 未设 | 走缓存路径（`lower_bundle_cached`） |
| `WJSM_NO_BUILTIN_CACHE` 非空 | 整体跳过缓存，每次完整 lower builtin 段 |
| `WJSM_CACHE_DIR` 未设置或为空 | 仍可走分段 lower，但构建段不落盘 |

缓存键是 `sha256(BUILTIN_CACHE_VERSION ‖ emit_debug_checks ‖ 每个 builtin canonical 与其源码 SHA-256)`。`BUILTIN_CACHE_VERSION` 是语义版本号——builtin_js 源码、lowerer 或 IR 布局任一变化时必须手动 bump，否则会命中语义过期但结构合法的旧缓存。resolution options 与 root 有意不入键：builtin 源码是编译期常量、自包含。

段文件落盘在 `${WJSM_CACHE_DIR}/builtin_ir/<key>.bin`，原子写入（先写临时文件再 rename）。读取时任何失败（缺文件、反序列化错误、版本不匹配）都回落到重建。

> <details><summary>为什么不做 `program.verify()` 门禁？</summary>
>
> 部分 builtin 闭包（events/path/perf_hooks）在基线上就存在死块校验告警（block has instructions but terminator is unreachable），运行时与 native 编译均容忍。若把 verify 当命中条件，这些闭包的缓存永远不命中。段与 plain 路径同源（同一 lowerer），结构合法由 bincode 解码 + 版本号保证。
>
> </details>

## 与启动快照的关系

编译缓存和启动快照是两套独立的机制：

| 机制 | 缓存对象 | 触发时机 |
| --- | --- | --- |
| Native image cache | 用户代码的 native image | 设置了 `WJSM_CACHE_DIR` 且编译用户代码 |
| Builtin IR 段缓存 | builtin 模块的 IR 段 | 设置了 `WJSM_CACHE_DIR` 且首次冷 lower |
| 启动快照 | bootstrap 后的堆状态 | 进程启动时 |

三者都加速启动，但对象不同。native image cache 跳过 Cranelift 编译，builtin IR 段缓存跳过 builtin 模块 lower，startup snapshot 跳过 builtin JS 的执行。

## 深入了解

- [编译缓存](../startup/compilation-cache.md)
- [用户侧的 cache 命令](../../user/cli/cache.md)
- [用户侧的启动快照与嵌入工件配置](../../user/configuration/startup-snapshot.md)
