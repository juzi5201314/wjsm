# 环境变量

环境变量是介于命令行和配置文件之间的配置层：比配置文件优先级高，比命令行低。

## 缓存

| 变量 | 用途 | 默认值 |
| --- | --- | --- |
| `WJSM_CACHE_DIR` | 磁盘缓存目录（native image、builtin IR 段、artifact 缓存） | 未设置（回落 XDG/HOME） |
| `WJSM_CACHE_MAX_BYTES` | 自动 LRU 上限（字节） | `268435456`（256 MiB） |
| `WJSM_NO_BUILTIN_CACHE` | 非空时禁用 builtin IR 段与 artifact 缓存 | 未设置 |

磁盘缓存默认可用。`WJSM_CACHE_DIR` 未设置时回落 `${XDG_CACHE_HOME}/wjsm`，再回落 `${HOME}/.cache/wjsm`；设为空串则显式禁用磁盘缓存（此时 `wjsm cache` 需要 `--dir`）。缓存目录下有三类条目：native image（`*.wnat`）、builtin IR 段（`builtin_ir/*.bin`）与输入寻址 artifact 缓存（`artifact/*.{wjsm,dep}`，同源二次运行跳过 parse/lower）。

每次写入会节流扫描目录总字节（三类条目全部计入）。超过上限按 mtime 删除最旧条目。`WJSM_CACHE_MAX_BYTES=0` 关闭自动淘汰；非法值回落到 256 MiB。手动管理走 `wjsm cache stats / clear / prune --max-bytes N`。

## 启动快照

启动快照在构建 `wjsm` 时嵌入，`NativeRuntime` 启动时始终恢复。当前没有环境变量可以关闭快照或打开快照诊断。

## 编译优化

| 变量 | 用途 | 默认值 |
| --- | --- | --- |
| `WJSM_OPT_LEVEL` | Cranelift 优化等级（`none`/`speed`/`speed_and_size`） | 未设置时为 `speed` |
| `WJSM_VERIFY_CLIF` | CLIF verifier（`0`/`false`/`FALSE` 关闭） | 开启 |
| `WJSM_DISABLE_SPECIALIZATION` | 关闭运行时类型反馈与热函数特化（设为 `1`） | 启用 |
| `WJSM_OVERLAY_MAX_BYTES` | overlay 代码体积上限（字节）；`0` 不限 | `clamp(32MiB, 12.5% RSS, 256MiB)` |
| `WJSM_OVERLAY_MAX_COUNT` | overlay 份数上限；`0` 不限 | `4096` |

`WJSM_OPT_LEVEL` 进入 native cache key，不同档位的 `.wnat` 互不复用。非法值在 native compiler 初始化时被拒绝。`WJSM_VERIFY_CLIF` 不进入 cache key。

## packed 可执行文件

| 变量 | 用途 | 默认值 |
| --- | --- | --- |
| `WJSM_EXEC_STUB` | `wjsm-exec` stub 路径 | 与 `wjsm` 同目录的 `wjsm-exec` |
| `WJSM_INSPECT` / `WJSM_INSPECT_BRK` | packed exe 启用 CDP inspector（可带 `HOST:PORT`） | 关闭 |

`wjsm run` 仍用 `--inspect` / `--inspect-brk`。packed exe 没有 clap，只认这两个变量以及 `NODE_OPTIONS` 里的 `--inspect` / `--inspect-brk`。

## 运行时能力

| 变量 | 用途 | 默认值 |
| --- | --- | --- |
| `WJSM_CHILD_PROCESS_ALLOW` | `child_process` 白名单（空或未设置=禁用） | 未设置 |

不设置时 `child_process.exec`/`spawn` 会报错：

```text
child_process execution is disabled for '<command>'; set WJSM_CHILD_PROCESS_ALLOW
```

设置后只允许匹配的命令执行。`*` 表示允许全部。详见 [系统、网络与进程能力](../runtime/system-capabilities.md)。

## 测试

| 变量 | 用途 |
| --- | --- |
| `WJSM_UPDATE_FIXTURES` | 更新 fixture 期望输出 |
| `WJSM_UPDATE_SNAPSHOTS` | 更新 IR 快照 |

这两个变量仅供仓库内部测试使用。

## 深入了解

- [环境变量索引](../reference/environment-variable-index.md)
- [运行时配置与环境变量索引](../../internals/reference/runtime-configuration-index.md)
- [配置来源与优先级](sources-and-precedence.md)
