# 缓存实现

这一章说明 CLI 的 `cache` 命令和缓存管理。

## cache 命令

`cache` subcommand 管理编译缓存：

| 子命令 | 行为 |
| --- | --- |
| `cache stats` | 显示缓存统计（命中数、未命中数、缓存大小） |
| `cache clear` | 清除所有缓存文件 |
| `cache path` | 显示缓存目录路径 |

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

## 深入了解

- [编译缓存](../startup/compilation-cache.md)
- [用户侧的 cache 命令](../../user/cli/cache.md)
- [用户侧的启动快照与嵌入工件配置](../../user/configuration/startup-snapshot.md)
