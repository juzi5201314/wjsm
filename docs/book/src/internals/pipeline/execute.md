# 实例化与执行阶段

这一章说明 portable artifact 如何交给 `NativeRuntime` 执行，以及退出码从哪里产生。

## 调用链

```text
run_compile_then_execute (CLI)
  └─ create_native_runtime
       └─ NativeRuntime::execute(artifact, module_root, cwd)
```

`create_native_runtime` 只在 `WJSM_CACHE_DIR` 有值时把该路径传给 `NativeRuntimeConfig.cache_dir`。未设置时 runtime 每次从 IR 编译，不读写磁盘。

in-process fixture（`run_file_in_process`）与 CLI 共用同一条路径，同样只认这个环境变量。测试套件默认不设置它。

## 执行前的两件事

1. **artifact 已验证**：`run` 从 source 编出 `.wjsm`，或直接解码已有 artifact；bounded decode / verifier 通过后才进入 `NativeImageRepository::prepare`。
2. **统计输出**：`--stats` 在执行前打印常量数、函数数、基本块数、指令数与 artifact 字节数；执行后再打印 native cache hits / misses（未打开磁盘缓存时 entries/bytes 为 0）。

没有 Wasmtime `deserialize_file`、`__run-precompiled` 或 `${WJSM_CACHE_DIR}/pipeline/<hash>.wasm` 这条 fork AOT handoff。重复执行要复用机器码，只能显式设置 `WJSM_CACHE_DIR`。

## 退出码来源

`NativeRuntime::execute` 返回 `NativeExecution`。错误分三类处理：

| 情况 | 处理 |
| --- | --- |
| `process.exit(n)` | 提取 n，直接作为退出码 |
| 其他运行时错误 | 打印 `Runtime error: ...` 到 stderr，退出码 2 |
| 编译期错误 / FatalJavaScript | 在此之前或作为退出码 1 失败 |

`process.exit` 通过错误通道回传，而不是直接终止进程——这样 diagnostics 缓冲区仍有机会刷出。

## 诊断缓冲

执行结果带 `stdout` / `stderr`。运行时累积的诊断（如未处理 Promise rejection 警告）在执行结束后写入 stderr。`console.*` 不走这条路，它直接写 stdout。

## 深入了解

- [实例化与执行生命周期细节](../host-runtime/instantiation-and-lifecycle.md)
- [Engine 配置与实例池](../startup/engine-pool.md)
- [RuntimeState 与 Realm 的组织](../host-runtime/runtime-state-and-realms.md)
- [编译缓存](../startup/compilation-cache.md)
- [用户侧的退出码与流对应关系](../../user/output/process-io.md)
