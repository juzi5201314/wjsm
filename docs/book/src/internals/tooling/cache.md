# 缓存实现

这一章说明 CLI 的 `cache` 命令和三套磁盘缓存的开关。

## cache 命令

`cache` subcommand 管理已经落盘的缓存条目（`*.wnat`、`builtin_ir/*.bin`、`artifact/*.{wjsm,dep}`）：

| 子命令 | 行为 |
| --- | --- |
| `cache stats` | 显示缓存目录、条目数与总字节数 |
| `cache clear` | 清除所有缓存条目并报告删除数量 |
| `cache prune --max-bytes N` | 按最旧修改时间删到字节上限 |

目录解析顺序：`--dir` > `WJSM_CACHE_DIR` > XDG/HOME 回落（见下）。全部不可用时直接报错。`--dir` 只影响这次管理命令，不会改变后续 `run` 的缓存目录。

公开统计来自 `NativeImageRepository::stats()`（`wjsm-backend-native`），经 `NativeExecution` 的 cache 字段暴露给 CLI `--stats`。没有 `module_cache_stats` / `clear_module_cache` / `module_cache_dir()`。

## 缓存目录

所有磁盘缓存共用同一根目录，唯一 owner 是 `wjsm-module` 的 `resolve_cache_dir()`（issue #376）：

1. `WJSM_CACHE_DIR` 非空 → 使用该路径。
2. `WJSM_CACHE_DIR` 设为空串 → 显式禁用磁盘缓存（只剩进程内缓存）。
3. 未设置 → 回落 `${XDG_CACHE_HOME}/wjsm`（相对路径的 `XDG_CACHE_HOME` 按 XDG 规范忽略），否则 `${HOME}/.cache/wjsm`；两者都不可用则禁用磁盘缓存。

缓存内容全部按内容哈希校验（native cache key / builtin ABI 指纹 / artifact 读集回放），目录被污染只会 miss 重建，不会执行脏数据，因此默认落盘安全。缓存写入是 best-effort：目录只读或写入失败不会让编译失败。

测试套件通过 `tests/support/test_env.rs` 把 `WJSM_CACHE_DIR` 重定向到 `/tmp` 下的进程隔离目录，不污染用户缓存。

## 自动 LRU

写入后按全局计数节流扫描（每 32 次写入一次）。统计对象是目录顶层 `*.wnat`、`builtin_ir/*.bin` 与 `artifact/*.{wjsm,dep}`。总字节超过上限时按 mtime 删最旧文件，直到低于上限。

| 变量 | 行为 |
| --- | --- |
| 未设置 `WJSM_CACHE_MAX_BYTES` | 上限 256 MiB |
| `WJSM_CACHE_MAX_BYTES=<正整数>` | 使用该上限 |
| `WJSM_CACHE_MAX_BYTES=0` | 禁用自动淘汰 |
| 非法值 | 回落 256 MiB |

`wjsm cache prune` 是手动裁剪，与自动 LRU 独立。

## Native image cache

`NativeImageRepository` 是进程内 image 与磁盘 cache 的唯一 owner。`cache_dir` 为 `None` 时只做内存 Weak 池和 in-flight gate，编译产物不落盘。

磁盘条目是 `${WJSM_CACHE_DIR}/<sha256>.wnat`。key 是 `NativeCacheKey` 的 SHA-256：

| 维度 | 来源 |
| --- | --- |
| cache schema | `CACHE_SCHEMA` |
| program digest | 编码后的 IR `Program` SHA-256 |
| native ABI hash | `native_abi_hash()` |
| native codegen source hash | `NATIVE_CODEGEN_HASH` |
| 当前 target | `{ARCH}-{OS}` |
| Cranelift 版本 | `CRANELIFT_VERSION` |
| codegen / ISA settings | `NativeCompiler::settings_key()`（含 `WJSM_OPT_LEVEL`） |

命中时加载 `.wnat` object 再 `CompiledImage::load`；header / object / hash / 权限校验失败计为 invalidated 并重编译。同 key 的并发 prepare 由 in-flight gate 合并。

## builtin IR 段缓存

多文件项目每次冷启动都要把入口依赖的 Node builtin 模块重新 lower 成 IR。`wjsm-module/src/builtin_cache.rs` 把这部分产物按依赖闭包序列化到磁盘。

| 条件 | 行为 |
| --- | --- |
| 缓存目录可解析且 `WJSM_NO_BUILTIN_CACHE` 未设 | 走缓存路径（`lower_bundle_cached`） |
| `WJSM_NO_BUILTIN_CACHE` 非空 | 整体跳过缓存，每次完整 lower builtin 段 |
| 缓存目录不可解析（`WJSM_CACHE_DIR` 为空且无 XDG/HOME） | 仍可走分段 lower，但构建段不落盘 |

缓存键是 `sha256(BUILTIN_CACHE_ABI_HASH ‖ u8(emit_debug_checks) ‖ 每个 canonical 名)`。`BUILTIN_CACHE_ABI_HASH` 由 `wjsm-module` 的 `build.rs` 在构建期对 builtin_js 源码及 module/parser/semantic/IR 输入做摘要，**不是**手工 bump 的 `BUILTIN_CACHE_VERSION`。源码或 lower 输入变化会自动换命名空间，并拒绝 `cache_abi_hash` 不匹配的旧载荷。resolution options 与 root 有意不入键：builtin 源码自包含。

段文件落盘在 `${WJSM_CACHE_DIR}/builtin_ir/<key>.bin`，原子写入（先写临时文件再 rename）。读取时任何失败（缺文件、反序列化错误、ABI hash 不匹配）都回落到重建。

> <details><summary>为什么不做 `program.verify()` 门禁？</summary>
>
> 部分 builtin 闭包（events/path/perf_hooks）在基线上就存在死块校验告警（block has instructions but terminator is unreachable），运行时与 native 编译均容忍。若把 verify 当命中条件，这些闭包的缓存永远不命中。段与 plain 路径同源（同一 lowerer），结构合法由 bincode 解码 + `BUILTIN_CACHE_ABI_HASH` 保证。
>
> </details>

## 输入寻址 artifact 缓存

文件入口的 `run` / `build` 每次都要全额重付 parse + lower。`wjsm-module/src/artifact_cache.rs` 提供正向缓存 `sha256(源码闭包读集 ‖ 编译选项 ‖ 语义 ABI 指纹) → portable .wjsm`（issue #376），命中时 parse/lower 完全跳过，降到读盘量级。

`${cache_dir}/artifact/` 下两类文件：

| 文件 | 键 | 内容 |
| --- | --- | --- |
| `<content_key>.wjsm` | `sha256(语义 ABI ‖ 选项指纹 ‖ 读集事实)` | 编码后的 portable artifact 原始字节 |
| `<index_key>.dep` | `sha256(语义 ABI ‖ 选项指纹 ‖ 入口/root 身份)` | 读集事实、module root 与 content key（bincode） |

选项指纹覆盖 CLI 管线源码指纹（`wjsm-cli` 的 `build.rs` 生成）、`script` / `verify-ir` / `debug` 开关、resolution options（browser、conditions）与 `WJSM_DISABLE_LICM`。语义 ABI 指纹复用 builtin 缓存的 `BUILTIN_CACHE_ABI_HASH`（覆盖 module/parser/semantic/IR/artifact-format 源码与 `Cargo.lock`），语义版本 bump 自动作废全部旧条目。

命中流程：由入口 canonical 身份算 index key 读 `.dep`，校验 ABI 与选项指纹后逐条回放读集事实（文件内容 SHA-256、存在性正/负探测、canonicalize 结果），再由回放通过的事实重算 content key 读 `.wjsm`。任一步失败即 miss，冷路径重编译并覆盖写入（tmp + rename 原子写，失败静默）。读集由 `SourceReadTrace` 在 `DiskSourceStore` 收口记录；builtin 虚拟路径不入读集（由 ABI 指纹覆盖），出现相对路径或非 UTF-8 路径时放弃缓存该入口。

`WJSM_NO_BUILTIN_CACHE` 非空时 artifact 缓存一并停用：该开关强制非分段 lower 调试路径，artifact 缓存作为 lower 产物缓存必须让位。`-e` / stdin 输入不走该缓存（无稳定入口身份）。

## 与启动快照的关系

编译缓存和启动快照是两套独立的机制：

| 机制 | 缓存对象 | 触发时机 |
| --- | --- | --- |
| 输入寻址 artifact 缓存 | 文件入口的 portable artifact | 缓存目录可解析且文件入口编译 |
| Native image cache | 用户代码的 native image | 缓存目录可解析且编译用户代码 |
| Builtin IR 段缓存 | builtin 模块的 IR 段 | 缓存目录可解析且首次冷 lower |
| 启动种子 | 嵌入的 global/EvalIndirect 种子 | `NativeRuntime::new_*` 始终恢复 |

四者对象不同。artifact 缓存跳过 parse + lower，native image cache 跳过 Cranelift 编译，builtin IR 段缓存跳过 builtin 模块 lower。启动种子不包含 builtin JS。

## 深入了解

- [编译缓存](../startup/compilation-cache.md)
- [用户侧的 cache 命令](../../user/cli/cache.md)
- [用户侧的启动快照与嵌入工件配置](../../user/configuration/startup-snapshot.md)
