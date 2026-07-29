# 环境变量索引

面向使用者的环境变量。同名命令行选项存在时，命令行优先。

## 执行与内存

| 变量 | 取值 | 默认 | 说明 |
| --- | --- | --- | --- |
| `WJSM_GC` | `mark-sweep` \| `g1` \| `zgc` | `zgc` | 选择垃圾回收器；被 `--gc` 覆盖 |
| `WJSM_TEST_GC` | 同上 | 未设置 | 优先于 `WJSM_GC`，两者都被 `--gc` 覆盖 |
| `WJSM_GC_LOG` | 仅 `1` 生效 | 关闭 | 输出 GC 周期日志 |
| `WJSM_SHADOW_STACK_MAX` | 字节或 `K`/`M`/`G` | `16M` | 影子栈软上限；被 `--shadow-stack-max` 覆盖 |
| `WJSM_COMPILER` | `winch`（大小写不敏感），其他值按 Cranelift | Cranelift | 选择 Wasmtime 编译器；启用 inspector 时强制 Cranelift |
| `WJSM_OPT_LEVEL` | `none` \| `speed_and_size` \| 其他 | 默认等级 | Cranelift 优化等级 |

## 缓存与启动

| 变量 | 取值 | 默认 | 说明 |
| --- | --- | --- | --- |
| `WJSM_CACHE_DIR` | 目录路径 | `$HOME/.cache/wjsm` | 编译缓存位置；空值等同未设置。`HOME` 也不可用时缓存禁用 |
| `WJSM_STARTUP_SNAPSHOT` | `0`/`false`/`off` 关闭 | 启用 | 启动快照开关 |
| `WJSM_STARTUP_SNAPSHOT_DEBUG` | `1`/`true`/`on` 启用 | 关闭 | 输出快照恢复诊断 |

## 能力边界

| 变量 | 取值 | 默认 | 说明 |
| --- | --- | --- | --- |
| `WJSM_FS_ALLOW_READ` | 平台路径分隔符分隔的路径列表 | 未设置 | 追加可读根目录 |
| `WJSM_FS_ALLOW_WRITE` | 仅 `1` 生效 | 关闭 | 解除写入根目录限制 |
| `WJSM_CHILD_PROCESS_ALLOW` | 命令名列表或 `*` | 未设置（禁止） | `node:child_process` 允许执行的命令 |
| `WJSM_VM_MAX_REALMS` | 正整数 | `1024` | `node:vm` 活跃 realm 上限 |
| `WJSM_WORKER_THREADS_MAX` | 正整数 | `32` | worker 线程上限 |

## 终端颜色

| 变量 | 行为 |
| --- | --- |
| `CLICOLOR_FORCE` | 非空且不为 `0` 时强制启用颜色，优先级最高 |
| `NO_COLOR` | 非空时禁用颜色 |

`--color` / `--no-color` 显式传入时覆盖以上两者。

程序自身通过 `process.env` 读取的变量不受此表约束；wjsm 会把启动时的完整环境快照传给脚本。

逐项语义见[环境变量](../configuration/environment-variables.md)。开发与测试专用变量不在此表，见[开发工作流与代码约定](../../internals/development/workflow-and-conventions.md)。
