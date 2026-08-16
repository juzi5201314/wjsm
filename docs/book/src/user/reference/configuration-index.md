# 配置项索引

所有配置入口的速查表。完整语义和取值见各配置主题页。配置来源优先级：CLI > 环境变量 > 配置文件 > 默认值。

## CLI 选项

| 选项 | 用途 | 默认值 |
| --- | --- | --- |
| `--config <PATH>` | 指定 `wjsm.toml` / `wjsm.json` | 自动发现 |
| `-q/--quiet` | 抑制非必要诊断 | 关闭 |
| `-v/--verbose` | 增加阶段诊断 | 0 级 |
| `--time` | 输出 pipeline timing | 关闭 |
| `--stats` | 输出 IR 与 artifact 统计 | 关闭 |
| `--verify-ir` | codegen 前验证 IR | 关闭 |
| `--color <auto\|always\|never>` | 控制颜色输出 | `auto` |
| `--browser` | 启用 browser 解析条件 | 关闭 |
| `--condition <NAME>` | 自定义解析条件 | 无 |
| `--gc <mark-sweep\|g1\|zgc>` | GC 算法 | `zgc` |
| `--max-heap-size <SIZE>` | 堆内存上限 | 无限制 |
| `--inspect[=HOST:PORT]` | CDP 调试器 | 关闭 |
| `--inspect-brk[=HOST:PORT]` | CDP 调试器，入口暂停 | 关闭 |
| `--root <DIR>` | 模块解析根目录 | 入口所在目录 |
| `--script` | 脚本模式解析 | module 模式 |
| `--watch` | 监听文件改动重新执行 | 关闭 |
| `--stage <parse\|lower\|compile\|execute>` | build 流水线阶段 | `compile` |
| `--format <wjsm\|native-executable>` | 制品格式 | `wjsm` |
| `--include <PATH>` | 补入 native-executable 快照的文件 | 无 |
| `-o/--output <PATH>` | build 输出路径 | `out.wjsm` |

`--gc` 不能写入配置文件，只接受命令行或环境变量。

## 环境变量

| 变量 | 用途 | 默认值 |
| --- | --- | --- |
| `WJSM_GC` | GC 算法 | `zgc` |
| `WJSM_TEST_GC` | 测试专用 GC 覆盖 | 未设置 |
| `WJSM_GC_LOG` | GC 回收日志（仅认 `1`） | 关闭 |
| `WJSM_CACHE_DIR` | native / builtin 缓存目录 | 未设置（关闭） |
| `WJSM_NO_BUILTIN_CACHE` | 禁用 builtin IR 段缓存 | 未设置 |
| `WJSM_STARTUP_SNAPSHOT` | 启动快照（`0`/`false`/`off` 禁用） | 开启 |
| `WJSM_STARTUP_SNAPSHOT_DEBUG` | 快照调试诊断 | 关闭 |
| `WJSM_OPT_LEVEL` | Cranelift 优化等级 | `default` |
| `WJSM_DISABLE_LICM` | 关闭循环不变量提升 | 启用 |
| `WJSM_DISABLE_SPECIALIZATION` | 关闭热函数特化 | 启用 |
| `WJSM_INSPECT` / `WJSM_INSPECT_BRK` | packed exe 启用 CDP | 关闭 |
| `WJSM_CHILD_PROCESS_ALLOW` | child_process 命令白名单 | 禁用 |
| `WJSM_WORKER_THREADS_MAX` | worker_threads 上限 | `32` |
| `WJSM_UPDATE_FIXTURES` | 更新 fixture 期望输出 | 未设置 |
| `WJSM_UPDATE_SNAPSHOTS` | 更新 IR 快照 | 未设置 |

完整环境变量说明见[环境变量索引](environment-variable-index.md)。

## 配置文件字段

`wjsm.toml` 或 `wjsm.json` 支持的字段：

| 字段 | 用途 |
| --- | --- |
| `browser` | 启用 browser 解析条件 |
| `condition` | 自定义解析条件列表 |
| `root` | 模块解析根目录 |

配置文件不能包含 `--gc`、`--max-heap-size`、`--inspect` 等运行时选项——这些只能通过命令行或环境变量设置。完整说明见[`wjsm.toml` 与 `wjsm.json`](../configuration/project-files.md)。

## GC 选择优先级

`--gc` > `WJSM_TEST_GC` > `WJSM_GC` > 默认 `zgc`。

## 深入了解

- [配置来源与优先级](../configuration/sources-and-precedence.md)
- [环境变量索引](environment-variable-index.md)
