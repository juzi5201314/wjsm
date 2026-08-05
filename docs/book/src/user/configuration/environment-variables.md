# 环境变量

这一章列出面向使用者的环境变量及其解析规则。每个值都由某一层代码单独解析，取值判定各不相同，下面逐项给出实际接受的写法。

## 垃圾回收

| 变量 | 说明 |
| --- | --- |
| `WJSM_GC` | 取 `mark-sweep`、`g1`、`zgc` |
| `WJSM_TEST_GC` | 同上，优先级高于 `WJSM_GC` |
| `WJSM_GC_LOG` | 值恰好为 `1` 时输出 GC 日志，其他值一律视为关闭 |

命令行 `--gc` 覆盖这两个变量。非法值直接报错并列出合法名：

```bash
WJSM_GC=bogus wjsm run app.js
# Error: unknown GC algorithm `bogus`; expected one of: mark-sweep, g1, zgc
```

## 内存

`WJSM_SHADOW_STACK_MAX` 设置影子栈软上限，接受字节数或 `K`/`M`/`G` 后缀，默认 16MiB。`--shadow-stack-max` 优先。

## 启动与缓存

| 变量 | 说明 |
| --- | --- |
| `WJSM_STARTUP_SNAPSHOT` | `0`、`false`、`off` 关闭启动快照，其他值或未设置即启用 |
| `WJSM_STARTUP_SNAPSHOT_DEBUG` | `1`、`true`、`on` 打开快照诊断输出 |
| `WJSM_CACHE_DIR` | 编译缓存目录；非空值优先，为空或未设置时回落到 `$HOME/.cache/wjsm` |
| `WJSM_NO_BUILTIN_CACHE` | 非空时禁用 builtin IR 段缓存，lower 阶段每次完整重建 builtin 模块段 |

两个来源都不可用时（`WJSM_CACHE_DIR` 空且 `HOME` 空）缓存禁用，此时 `wjsm cache stats` 打印 `Cache disabled`。

## 编译器

| 变量 | 取值 |
| --- | --- |
| `WJSM_COMPILER` | `winch`（大小写不敏感）选择 Winch，其他值用 Cranelift |
| `WJSM_OPT_LEVEL` | `none`、`speed_and_size`，其他值用默认等级 |
| `WJSM_DISABLE_LICM` | `0`、`false`、`off`、空值或未设置时保持启用；其他值关闭循环不变量调用提升 |

`WJSM_DISABLE_LICM` 面向性能对照实验：wjsm 在编译期会把循环体内的纯函数调用提升到循环外只执行一次，关闭它可以让基准测到循环内的真实调用成本（`wjsm-bench` 就是这么用的）。日常使用没有理由关闭。

## 系统能力

| 变量 | 说明 |
| --- | --- |
| `WJSM_FS_ALLOW_READ` | 追加可读目录，用平台路径分隔符分隔 |
| `WJSM_FS_ALLOW_WRITE` | 值为 `1` 时解除写入目录限制 |
| `WJSM_CHILD_PROCESS_ALLOW` | 允许的命令名列表，或 `*` 允许全部；默认禁用子进程 |
| `WJSM_VM_MAX_REALMS` | `node:vm` 活跃 Realm 上限，默认 1024 |
| `WJSM_WORKER_THREADS_MAX` | Worker 线程数上限 |

## 终端颜色

`NO_COLOR` 非空关闭颜色，`CLICOLOR_FORCE` 非空且不为 `0` 强制开启，后者优先。`--color` / `--no-color` 优先于两者。

> <details><summary>为什么每个变量的取值判定都写得很死？</summary>
>
> 简单的字符串 `true`/`on`/`1`、`0`/`off`/`false` 这类判断看起来啰嗦，但能让用户**预测行为**：看到 `WJSM_GC_LOG=1` 就知道开启、`=0` 知道关闭、`=true` 也能猜对（多数 Unix 工具的惯例）。
>
> 反面例子是「任何非空字符串都视为 true」——这种宽松解析对用户友好，但对脚本不友好：`WJSM_GC_LOG=$SOME_VALUE` 在 `SOME_VALUE` 是空字符串时和「未设置」的行为一样，但用户可能想「明确设成空」表示「关闭」。wjsm 的设计偏向「明确值明确语义」，代价是每条规则要写清楚。
>
> </details>

## 深入了解

- [运行时配置与环境变量的解析 owner 索引](../../internals/reference/runtime-configuration-index.md)
- [文件系统与子进程能力的宿主实现](../../internals/runtime-features/fs-process-and-child-process.md)
