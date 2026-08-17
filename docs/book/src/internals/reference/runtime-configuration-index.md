# 运行时配置与环境变量索引

这一章汇总当前 crate 真正读取的运行时配置。下列「已死」变量仍可能出现在 bench runner 或旧文档里，但宿主不再读取：`WJSM_STARTUP_SNAPSHOT`、`WJSM_STARTUP_SNAPSHOT_DEBUG`、`WJSM_DISABLE_LICM`、`WJSM_COMPILER`、`WJSM_GC_LOG`、`WJSM_VM_MAX_REALMS`、`WJSM_WORKER_THREADS_MAX`。

## CLI 选项

| 选项 | 用途 | 默认值 |
| --- | --- | --- |
| `--gc` | GC 算法 | `zgc` |
| `--inspect` / `--inspect-brk` | 启用 CDP 调试器 | 关闭 |
| `--color` | 输出颜色 | auto |
| `--max-heap-size` | ManagedHeap 上限 | 64 MiB |
| `--format native-executable` | 同宿主 ELF/PE（见下） | `wjsm` |
| `--stats` | 打印 IR 计数；有 artifact 时打印字节；执行后打印 native cache 行 | 关闭 |

`--format native-executable` 用预链 `wjsm-exec` stub + zstd overlay + 制品内源码快照打出同宿主可执行文件（ADR 0016–0019）。产物不可移植。失败时 fail-closed：不创建、不覆盖输出文件。

## 环境变量

### GC

| 变量 | 用途 | 优先级 |
| --- | --- | --- |
| `WJSM_GC` | GC 算法 | 低于 `--gc` 和 `WJSM_TEST_GC` |
| `WJSM_TEST_GC` | 测试专用 GC 算法 | 高于 `WJSM_GC` |

选择优先级：`--gc` > `WJSM_TEST_GC` > `WJSM_GC` > 默认 `zgc`。

### 编译与特化

| 变量 | 用途 | 默认值 |
| --- | --- | --- |
| `WJSM_OPT_LEVEL` | Cranelift 优化档：`none` / `speed` / `speed_and_size` | 未设置 = `speed` |
| `WJSM_VERIFY_CLIF` | `0` / `false` / `FALSE` 关闭 CLIF verifier | 开启 |
| `WJSM_DISABLE_SPECIALIZATION` | 设为 `1` 时关闭类型反馈与热函数特化 | 开启 |

### 缓存

| 变量 | 用途 | 默认值 |
| --- | --- | --- |
| `WJSM_CACHE_DIR` | native image 与 builtin IR 段缓存目录 | 未设置（磁盘缓存关闭） |
| `WJSM_CACHE_MAX_BYTES` | 自动 LRU 上限；`0` 禁用淘汰 | 256 MiB |
| `WJSM_NO_BUILTIN_CACHE` | 非空时禁用 builtin IR 段缓存 | 未设置 |

未设置或空的 `WJSM_CACHE_DIR` 都关闭磁盘缓存，不会回落到 `$HOME/.cache/wjsm`。`wjsm cache` 这时必须传 `--dir`。

### 打包可执行文件与进程

| 变量 | 用途 |
| --- | --- |
| `WJSM_EXEC_STUB` | 覆盖预链 `wjsm-exec` stub 路径 |
| `WJSM_INSPECT` / `WJSM_INSPECT_BRK` | packed exe 与 `NODE_OPTIONS` 共用的 inspector 开关 |
| `WJSM_CHILD_PROCESS_ALLOW` | 允许 `child_process` 执行的命令白名单 |
| `WJSM_EXEC_ENTRY` | 父进程写给 packed worker 的入口；不是对外承诺 |

启动快照始终从嵌入的 `startup_snapshot.bin` 恢复，没有关闭开关。

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
