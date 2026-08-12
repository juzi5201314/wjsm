# 运行时配置与环境变量索引

这一章汇总所有运行时配置选项和环境变量。

## CLI 选项

| 选项 | 用途 | 默认值 |
| --- | --- | --- |
| `--gc` | GC 算法 | `zgc` |
| `--inspect` / `--inspect-brk` | 启用 CDP 调试器 | 关闭 |
| `--color` | 输出颜色 | auto |
| `--precompiled` | 加载预编译 WASM | 关闭 |
| `--max-realms` | 最大 realm 数 | 1024 |

## 环境变量

### GC

| 变量 | 用途 | 优先级 |
| --- | --- | --- |
| `WJSM_GC` | GC 算法 | 低于 `--gc` 和 `WJSM_TEST_GC` |
| `WJSM_TEST_GC` | 测试专用 GC 算法 | 高于 `WJSM_GC` |

选择优先级：`--gc` > `WJSM_TEST_GC` > `WJSM_GC` > 默认 `zgc`。

### 编译器

| 变量 | 用途 | 默认值 |
| --- | --- | --- |
| `WJSM_COMPILER` | 编译器（已移除，保留兼容） | cranelift |
| `WJSM_OPT_LEVEL` | Cranelift 优化等级（none / speed_and_size / default） | default |

`WJSM_COMPILER` 仅为兼容保留；当前唯一编译器是 Cranelift。

### 启动

| 变量 | 用途 | 默认值 |
| --- | --- | --- |
| `WJSM_STARTUP_SNAPSHOT` | 启动快照（0/false/off 禁用） | 开启 |
| `WJSM_STARTUP_SNAPSHOT_DEBUG` | 快照调试诊断 | 关闭 |

### 缓存

| 变量 | 用途 | 默认值 |
| --- | --- | --- |
| `WJSM_CACHE_DIR` | 编译缓存目录 | `$HOME/.cache/wjsm` |
| `WJSM_NO_BUILTIN_CACHE` | 非空时禁用 builtin IR 段缓存 | 未设置 |

`WJSM_CACHE_DIR=`（空值）不禁用缓存，回落到 `$HOME/.cache/wjsm`。

### 编译优化

| 变量 | 用途 | 默认值 |
| --- | --- | --- |
| `WJSM_DISABLE_LICM` | 关闭 IR 层循环不变量纯调用提升（`0`/`false`/`off`/空/未设置保持启用） | 启用 |

`WJSM_DISABLE_LICM` 的读取在 `compiler_module/module_compile.rs` 的 `licm_disabled_by_env`。bench runner 给 wjsm 子进程设 `WJSM_DISABLE_LICM=1`，避免循环内纯 `work()` 被提升出循环测不到真实开销。

### 测试

| 变量 | 用途 |
| --- | --- |
| `WJSM_UPDATE_FIXTURES` | 更新 fixture 期望输出 |
| `WJSM_UPDATE_SNAPSHOTS` | 更新 IR 快照 |

## 配置文件

`wjsm.toml` 或 `wjsm.json` 是项目配置文件。配置来源优先级：CLI > 环境变量 > 配置文件 > 默认值。

## 深入了解

- [用户侧的配置来源与优先级](../../user/configuration/sources-and-precedence.md)
- [用户侧的环境变量索引](../../user/reference/environment-variable-index.md)
- [Engine 配置与池化](../startup/engine-pool.md)
