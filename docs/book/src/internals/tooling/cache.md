# 缓存实现

这一章说明 CLI 的 `cache` 命令和缓存管理。

## cache 命令

`cache` subcommand 管理编译缓存：

| 子命令 | 行为 |
| --- | --- |
| `cache stats` | 显示缓存目录、条目数与总字节数 |
| `cache clear` | 清除所有缓存文件并报告删除数量 |

`module_cache_stats` 和 `clear_module_cache` 是 `wjsm-host-wasm` 暴露的公开 API。

## 缓存目录

缓存目录由 `module_cache_dir()` 决定：

1. `WJSM_CACHE_DIR` 非空 → 使用该路径。
2. `WJSM_CACHE_DIR` 为空或未设置 → 回落 `$HOME/.cache/wjsm`。
3. 两者都不可用 → 缓存禁用。

`WJSM_CACHE_DIR=`（空值）不禁用缓存，仍回落到 `$HOME/.cache/wjsm`。

## 缓存 key

缓存 key 是 WASM 字节内容的 SipHash，前缀 `wasmtime-43`：

```rust
"wasmtime-43".hash(&mut hasher);
wasm_bytes.hash(&mut hasher);
```

key 不受文件 mtime 影响。命中时 `Module::deserialize_file` 走 mmap 零拷贝加载，跳过 Cranelift 编译。

## 与启动快照的关系

编译缓存和启动快照是两套独立的机制：

| 机制 | 缓存对象 | 触发时机 |
| --- | --- | --- |
| 编译缓存 | 用户 WASM 的 cwasm | 每次编译用户代码 |
| 启动快照 | bootstrap 后的堆状态 | 进程启动时 |

两者都加速启动，但对象不同。编译缓存跳过用户代码的编译，启动快照跳过 builtin JS 的执行。

## builtin IR 段缓存

多文件项目每次冷启动都要把入口依赖的 Node builtin 模块（`node:fs`、`node:path` 等）重新 lower 成 IR。`wjsm-module/src/builtin_cache.rs` 把这部分产物按依赖闭包序列化到磁盘，第二次启动直接反序列化，跳过 builtin 模块的重复 lower。

| 条件 | 行为 |
| --- | --- |
| `WJSM_CACHE_DIR` 可用且 `WJSM_NO_BUILTIN_CACHE` 未设 | 走缓存路径（`lower_bundle_cached_with_options`） |
| `WJSM_NO_BUILTIN_CACHE` 非空 | 整体跳过缓存，每次完整 lower builtin 段 |
| `WJSM_CACHE_DIR` 不可用 | 构建段但不落盘 |

缓存键是 `sha256(BUILTIN_CACHE_VERSION ‖ emit_debug_checks ‖ 每个 builtin canonical 与其源码 SHA-256)`。`BUILTIN_CACHE_VERSION` 是语义版本号——builtin_js 源码、lowerer 或 IR 布局任一变化时必须手动 bump，否则会命中语义过期但结构合法的旧缓存。resolution options 与 root 有意不入键：builtin 源码是编译期常量、自包含。

段文件落盘在 `${WJSM_CACHE_DIR}/builtin_ir/<key>.bin`，原子写入（先写临时文件再 rename）。读取时任何失败（缺文件、反序列化错误、版本不匹配）都回落到重建。

> <details><summary>为什么不做 `program.verify()` 门禁？</summary>
>
> 部分 builtin 闭包（events/path/perf_hooks）在基线上就存在死块校验告警（block has instructions but terminator is unreachable），运行时与 wasm 编译均容忍。若把 verify 当命中条件，这些闭包的缓存永远不命中。段与 plain 路径同源（同一 lowerer），结构合法由 bincode 解码 + 版本号保证。
>
> </details>

## 深入了解

- [编译缓存](../startup/compilation-cache.md)
- [用户侧的 cache 命令](../../user/cli/cache.md)
- [用户侧的启动快照与嵌入工件配置](../../user/configuration/startup-snapshot.md)
