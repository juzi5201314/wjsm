# 环境变量

环境变量是介于命令行和配置文件之间的配置层：比配置文件优先级高，比命令行低。

## GC

| 变量 | 用途 | 默认值 |
| --- | --- | --- |
| `WJSM_GC` | GC 算法（`mark-sweep`/`g1`/`zgc`） | `zgc` |
| `WJSM_TEST_GC` | 测试专用 GC，优先级高于 `WJSM_GC` | 未设置 |

GC 选择优先级：`--gc` > `WJSM_TEST_GC` > `WJSM_GC` > 默认 `zgc`。非法值在启动阶段被拒绝。

`WJSM_TEST_GC` 排在 `WJSM_GC` 之前，让测试能强制指定 GC 而不受用户配置干扰。详见 [垃圾回收器](gc.md)。

## 缓存

| 变量 | 用途 | 默认值 |
| --- | --- | --- |
| `WJSM_CACHE_DIR` | native image 与 builtin IR 段缓存目录 | 未设置（磁盘缓存关闭） |
| `WJSM_CACHE_MAX_BYTES` | 自动 LRU 上限（字节） | `268435456`（256 MiB） |
| `WJSM_NO_BUILTIN_CACHE` | 非空时禁用 builtin IR 段缓存 | 未设置 |

磁盘缓存是 opt-in。只有 `WJSM_CACHE_DIR` 被设置时，`wjsm run` 才会读写 `${WJSM_CACHE_DIR}/*.wnat`，多文件项目才会把 builtin IR 段落到 `${WJSM_CACHE_DIR}/builtin_ir/`。未设置或空值都不会回落到 `$HOME/.cache/wjsm`；`wjsm cache` 这时也需要 `--dir`。

打开磁盘缓存后，每次写入会节流扫描目录总字节（顶层 `*.wnat` 与 `builtin_ir/*.bin`）。超过上限按 mtime 删除最旧条目。`WJSM_CACHE_MAX_BYTES=0` 关闭自动淘汰；非法值回落到 256 MiB。手动管理走 `wjsm cache stats / clear / prune --max-bytes N`。

## 启动快照

启动快照在构建 `wjsm` 时嵌入，`NativeRuntime` 启动时始终恢复。当前没有环境变量可以关闭快照或打开快照诊断。

## 编译优化

| 变量 | 用途 | 默认值 |
| --- | --- | --- |
| `WJSM_OPT_LEVEL` | Cranelift 优化等级（`none`/`speed`/`speed_and_size`） | 未设置时为 `speed` |
| `WJSM_VERIFY_CLIF` | CLIF verifier（`0`/`false`/`FALSE` 关闭） | 开启 |
| `WJSM_DISABLE_SPECIALIZATION` | 关闭运行时类型反馈与热函数特化（设为 `1`） | 启用 |

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
