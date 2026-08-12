# 异常与完成记录

这一章说明 IR 的 abrupt completion 在 native 后端的落地方式。

## 异常值

异常是 NaN-box 值，标签 `TAG_EXCEPTION`（`0x5`）。`encode_exception_handle` 把句柄编码进异常值，`is_exception` 判定一个值是否是异常。generated code 在每个可能抛出的操作后检查返回值。

这与 Wasmtime 异常处理无关——direct native 后端不使用 trap 或 exception 引用，而是通过返回值协议在调用链上传播。

## try / catch / finally 的展开

`lowerer_branching.rs` 的 `emit_unwind_for_abrupt` 在 lowering 阶段已经把 `break`/`continue`/`return` 跨 `try-finally` 或 `for-of` 的清理序列展开成线性 IR。后端拿到的是已经展开的 CFG——没有嵌套的异常表，没有运行时 unwinder。

catch 块通过检查异常值标签进入，finally 块是内联到各 abrupt 路径的普通 IR 指令。

## 异常传播路径

```text
caller                          callee
  |                               |
  |  call host_op(args)           |
  |------------------------------>|
  |                               | 执行操作
  |                               | 可能抛出 → 编码异常值
  |<------------------------------|
  |  %result = return value        |
  |  %is_exc = is_exception(%result) |
  |  brif %is_exc, exc_handler, continue |
  |                               |
  v                               v
```

异常传播是显式的返回值检查，不是隐式的栈展开。代价是每个可能抛出的调用点都有检查分支；收益是不依赖平台异常机制，跨后端一致。

## 退出码

运行时错误在执行结束后通过 `process_exit_code_from_error` 分类：

| 情况 | 退出码 |
| --- | --- |
| `process.exit(n)` | n |
| 未捕获异常 | 2 |
| 编译期错误 | 1（在执行前失败） |

`process.exit` 通过错误通道回传，而不是直接终止进程——这样 diagnostics 缓冲区仍有机会刷出。

## 深入了解

- [控制流与异常](../frontend/control-flow-and-exceptions.md)
- [基本块与控制流图](../ir/cfg.md)
- [控制流代码生成](control-flow-codegen.md)
