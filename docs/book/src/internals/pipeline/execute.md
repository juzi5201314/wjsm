# 实例化与执行阶段

这一章说明 WASM 字节如何被交给 Wasmtime 执行，以及退出码从哪里产生。

## 调用链

```text
run_compile_then_execute (CLI)
  └─ block_on_wasm_execute
       └─ shared_execution_runtime().block_on(...)
            └─ runtime::execute_with_options
```

`shared_execution_runtime` 是进程级 `LazyLock<tokio::runtime::Runtime>`（multi-thread + `enable_all`）。复用同一个 runtime 是为了避免 in-process 测试反复重建，CLI 单次执行也走这条路。

## 执行前的两件事

1. **fork AOT handoff**：若 argv 中有脚本路径，raw WASM 被写入 `${WJSM_CACHE_DIR}/pipeline/<sha256>.wasm`，并把 `current_entry` 设为该路径。子进程可用隐藏命令 `__run-precompiled` 直接加载，跳过重编译。
2. **统计输出**：`--stats` 在执行前打印常量数、函数数、基本块数、指令数与 WASM 字节数。

> <details><summary>「fork AOT handoff」是什么？</summary>
>
> 是个优化：fork 出的子进程加载同一份 WASM 时，跳过重新编译。
>
> 场景是 CI 跑大量 fixture 测试。父进程编一次 WASM，把字节写到 `WJSM_CACHE_DIR/pipeline/<hash>.wasm`；fork 出的子进程看到 `WJSM_CURRENT_ENTRY` 指向这个文件，直接 `Module::deserialize_file` 加载，跳过整套编译流程。
>
> 这个机制让「一个项目跑 1000 个 fixture」的耗时从「1000 倍编译时间」降到「1 倍编译时间 + 1000 倍加载时间」。对短 fixture 收益巨大。
>
> </details>

## 退出码来源

`RuntimeOptions` 携带 argv、cwd、env 快照、fs 沙箱根、GC 算法、inspect 配置等。执行结果的错误分三类处理：

| 情况 | 处理 |
| --- | --- |
| `process.exit(n)` | `process_exit_code_from_error` 提取 n，直接作为退出码 |
| 其他运行时错误 | 打印 `Runtime error: ...` 到 stderr，退出码 2 |
| 编译期错误 | 在此之前失败，退出码 1 |

`process.exit` 通过错误通道回传，而不是直接终止进程——这样 diagnostics 缓冲区仍有机会刷出。

## 诊断缓冲

执行返回 `(writer, diagnostics)`。`diagnostics` 是运行时累积的字节缓冲（如未处理 Promise rejection 警告），执行结束后写入 stderr。`console.*` 不走这条路，它直接写 stdout。

## 深入了解

- [实例化与执行生命周期细节](../host-runtime/instantiation-and-lifecycle.md)
- [Engine 配置与实例池](../host-runtime/engine-configuration.md)
- [RuntimeState 与 Realm 的组织](../host-runtime/runtime-state-and-realms.md)
- [预编译执行与隐藏命令](../tooling/precompiled-execution.md)
- [用户侧的退出码与流对应关系](../../user/output/process-io.md)
