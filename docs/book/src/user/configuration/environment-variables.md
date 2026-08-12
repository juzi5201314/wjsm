# 环境变量

环境变量是介于命令行和配置文件之间的配置层：比配置文件优先级高，比命令行低。

## GC

| 变量 | 用途 | 默认值 |
| --- | --- | --- |
| `WJSM_GC` | GC 算法（`mark-sweep`/`g1`/`zgc`） | `zgc` |
| `WJSM_TEST_GC` | 测试专用 GC，优先级高于 `WJSM_GC` | 未设置 |
| `WJSM_GC_LOG` | 输出 GC 回收日志（仅认字面值 `1`） | 未设置 |

GC 选择优先级：`--gc` > `WJSM_TEST_GC` > `WJSM_GC` > 默认 `zgc`。

`WJSM_TEST_GC` 排在 `WJSM_GC` 之前，让测试能强制指定 GC 而不受用户配置干扰。详见 [垃圾回收器](gc.md)。

`WJSM_GC_LOG` 只认字面值 `1`，`true` 或 `on` 都不会启用。

## 缓存

| 变量 | 用途 | 默认值 |
| --- | --- | --- |
| `WJSM_CACHE_DIR` | 编译缓存目录 | `$HOME/.cache/wjsm` |
| `WJSM_NO_BUILTIN_CACHE` | 非空时禁用 builtin IR 段缓存 | 未设置 |

`WJSM_CACHE_DIR=`（空值）不禁用缓存，回落到默认目录。

## 启动快照

| 变量 | 用途 | 默认值 |
| --- | --- | --- |
| `WJSM_STARTUP_SNAPSHOT` | 启动快照开关（`0`/`false`/`off` 禁用） | 开启 |
| `WJSM_STARTUP_SNAPSHOT_DEBUG` | 快照调试诊断输出 | 关闭 |

默认开启，跳过 builtin JS 的 cold bootstrap 加速冷启动。详见 [启动快照与嵌入工件](startup-snapshot.md)。

## 编译优化

| 变量 | 用途 | 默认值 |
| --- | --- | --- |
| `WJSM_OPT_LEVEL` | Cranelift 优化等级（`none`/`speed_and_size`/`default`） | `default` |
| `WJSM_DISABLE_LICM` | 关闭 IR 层循环不变量纯调用提升（`0`/`false`/`off`/空/未设置保持启用） | 启用 |

`WJSM_DISABLE_LICM` 主要用于 benchmark：避免循环内纯 `work()` 被提升出循环，测不到真实开销。

## 运行时资源上限

| 变量 | 用途 | 默认值 |
| --- | --- | --- |
| `WJSM_VM_MAX_REALMS` | 最大 Realm 数（`vm.createContext`） | `1024` |
| `WJSM_WORKER_THREADS_MAX` | Worker 线程上限 | `32` |

超限时抛运行时错误，防止句柄/内存失控。

## 进程能力

| 变量 | 用途 | 默认值 |
| --- | --- | --- |
| `WJSM_CHILD_PROCESS_ALLOW` | `child_process` 白名单（空或未设置=禁用） | 未设置 |

不设置时 `child_process.exec`/`spawn` 会报错：

```text
child_process execution is disabled for '<command>'; set WJSM_CHILD_PROCESS_ALLOW
```

设置后只允许匹配的命令执行。详见 [系统、网络与进程能力](../runtime/system-capabilities.md)。

## 测试

| 变量 | 用途 |
| --- | --- |
| `WJSM_UPDATE_FIXTURES` | 更新 fixture 期望输出 |
| `WJSM_UPDATE_SNAPSHOTS` | 更新 IR 快照 |

## 深入了解

- [环境变量索引](../reference/environment-variable-index.md)
- [运行时配置与环境变量索引](../../internals/reference/runtime-configuration-index.md)
- [配置来源与优先级](sources-and-precedence.md)
