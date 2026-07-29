# 标准输出、标准错误与退出码

判断脚本成败要看退出码，不要靠 grep 输出流。这一章给出两者的确切规则。

## 流分配

程序自己的输出全部进 stdout，包括所有 `console` 方法。级别前缀由运行时添加：

```bash
wjsm run -e 'console.log("L"); console.error("E"); console.warn("W")'
```

```text
L
[error] E
[warn] W
```

`console.error` / `warn` / `info` / `debug` / `trace` 分别加 `[error]`、`[warn]`、`[info]`、`[debug]`、`[trace]` 前缀，但仍在 stdout。把 stdout 重定向到文件时，这些行会一起被重定向。

stderr 只承载 wjsm 自己的诊断：

- 编译错误（`Error: error: ...` 加源码片段）。
- 运行时错误摘要（`Runtime error: ...`）。
- `--verbose`、`--time`、`--stats` 的输出。
- `out.wasm` 覆盖警告、`--watch` 状态行。

未捕获异常会同时出现在两个流：stdout 收到运行时打印的 `Uncaught exception: ...`，stderr 收到 CLI 的 `Runtime error: ...` 摘要。

## 退出码

| 码 | 触发条件 |
| --- | --- |
| `0` | 正常结束 |
| `1` | 编译失败：解析、Lowering、codegen 错误，`validate` 校验不通过，`lint` 报出问题，`test` 有失败项 |
| `2` | 运行时未捕获异常 |
| `3` | 命令行用法错误：未知子命令、非法参数值 |
| 其他 | `process.exit(n)` 指定的值原样透出 |

`wjsm run -e 'process.exit(5)'` 退出码为 5。缺少输入参数（既无文件也无 `-e`）属于编译阶段错误，退出码为 1；子命令拼写错误由 Clap 拒绝，退出码为 3。

## 深入了解

- [标准流与退出码在 host 侧的 owner](../../internals/host-runtime/instantiation-and-lifecycle.md)
- [`process` 对象与退出诊断的实现](../../internals/runtime-features/fs-process-and-child-process.md)
