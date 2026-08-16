# 环境变量索引

所有 `WJSM_` 前缀环境变量的速查表。优先级规则：CLI 选项 > 环境变量 > 配置文件 > 默认值。

## GC

| 变量 | 用途 | 默认值 |
| --- | --- | --- |
| `WJSM_GC` | GC 算法（`mark-sweep` / `g1` / `zgc`） | `zgc` |
| `WJSM_TEST_GC` | 测试专用 GC 覆盖，优先级高于 `WJSM_GC` | 未设置 |
| `WJSM_GC_LOG` | 输出 GC 回收日志（仅认字面值 `1`） | 关闭 |

GC 选择优先级：`--gc` > `WJSM_TEST_GC` > `WJSM_GC` > 默认 `zgc`。非法值在启动阶段被拒绝。

## 缓存

| 变量 | 用途 | 默认值 |
| --- | --- | --- |
| `WJSM_CACHE_DIR` | native image 与 builtin IR 段缓存目录 | 未设置（磁盘缓存关闭） |
| `WJSM_NO_BUILTIN_CACHE` | 非空时禁用 builtin IR 段缓存 | 未设置 |

未设置或空的 `WJSM_CACHE_DIR` 都关闭磁盘缓存，不会回落到 `$HOME/.cache/wjsm`。cache 是可重建的派生数据，损坏或 stale 的条目会被 invalidated 而非执行。

## 启动

| 变量 | 用途 | 默认值 |
| --- | --- | --- |
| `WJSM_STARTUP_SNAPSHOT` | 启动快照（`0` / `false` / `off` 禁用） | 开启 |
| `WJSM_STARTUP_SNAPSHOT_DEBUG` | 快照调试诊断输出 | 关闭 |

启动快照是 bootstrap 后的堆状态，加速启动。禁用后每次冷启动都要执行 builtin JS 构造 primordial 对象。

## 编译器

| 变量 | 用途 | 默认值 |
| --- | --- | --- |
| `WJSM_OPT_LEVEL` | Cranelift 优化等级（`none` / `speed_and_size` / `default`） | `default` |
| `WJSM_DISABLE_LICM` | 关闭 IR 层循环不变量纯调用提升 | 启用（设为 `1` 关闭） |
| `WJSM_EXEC_STUB` | `wjsm-exec` stub 路径，供 `--format native-executable` 打包 | 与 `wjsm` 同目录的 `wjsm-exec` |

`WJSM_DISABLE_LICM` 读取 `0` / `false` / `off` / 空 / 未设置时保持启用。bench runner 设 `1` 来测真实循环开销。

## 运行时能力

| 变量 | 用途 | 默认值 |
| --- | --- | --- |
| `WJSM_CHILD_PROCESS_ALLOW` | `child_process` 命令白名单，逗号分隔或 `*` | 禁用 |
| `WJSM_WORKER_THREADS_MAX` | `worker_threads` 最大线程数 | `32` |

`WJSM_CHILD_PROCESS_ALLOW` 未设置时 `node:child_process` 的 `spawn` / `exec` / `execFile` 全部被拒绝。

## 测试

| 变量 | 用途 |
| --- | --- |
| `WJSM_UPDATE_FIXTURES` | 更新 fixture 期望输出 |
| `WJSM_UPDATE_SNAPSHOTS` | 更新 IR 快照 |

这两个变量仅供仓库内部测试使用。

## 深入了解

- [环境变量](../configuration/environment-variables.md)
- [运行时配置与环境变量索引](../../internals/reference/runtime-configuration-index.md)
