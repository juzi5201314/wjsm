# 环境变量索引

所有面向用户的 `WJSM_` 前缀环境变量速查表。优先级规则：CLI 选项 > 环境变量 > 配置文件 > 默认值。

## 缓存

| 变量 | 用途 | 默认值 |
| --- | --- | --- |
| `WJSM_CACHE_DIR` | native image 与 builtin IR 段缓存目录 | 未设置（磁盘缓存关闭） |
| `WJSM_CACHE_MAX_BYTES` | 自动 LRU 上限（字节）；`0` 关闭自动淘汰 | `268435456` |
| `WJSM_NO_BUILTIN_CACHE` | 非空时禁用 builtin IR 段缓存 | 未设置 |

未设置或空的 `WJSM_CACHE_DIR` 都关闭磁盘缓存，不会回落到 `$HOME/.cache/wjsm`。cache 是可重建的派生数据，损坏或 stale 的条目会被 invalidated 而非执行。

## 编译器

| 变量 | 用途 | 默认值 |
| --- | --- | --- |
| `WJSM_OPT_LEVEL` | Cranelift 优化等级（`none` / `speed` / `speed_and_size`） | `speed` |
| `WJSM_VERIFY_CLIF` | CLIF verifier（`0` / `false` / `FALSE` 关闭） | 开启 |
| `WJSM_DISABLE_SPECIALIZATION` | 关闭运行时类型反馈与热函数特化 | 启用（设为 `1` 关闭） |
| `WJSM_EXEC_STUB` | `wjsm-exec` stub 路径，供 `--format native-executable` 打包 | 与 `wjsm` 同目录的 `wjsm-exec` |
| `WJSM_INSPECT` | packed exe / 环境启用 CDP（`HOST:PORT` 或端口） | 关闭 |
| `WJSM_INSPECT_BRK` | 同上，并在入口暂停 | 关闭 |

## 运行时能力

| 变量 | 用途 | 默认值 |
| --- | --- | --- |
| `WJSM_CHILD_PROCESS_ALLOW` | `child_process` 命令白名单，逗号分隔或 `*` | 禁用 |

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
