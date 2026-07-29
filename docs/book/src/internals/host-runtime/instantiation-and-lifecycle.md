# 实例化与执行生命周期

这一章说明从 WASM 字节到程序输出之间发生了什么。

## execute_with_writer_with_options

入口在 `crates/wjsm-host-wasm/src/lib.rs`：

```rust
pub async fn execute_with_writer_with_options_and_stats<W: Write>(
    wasm_bytes: &[u8],
    options: RuntimeOptions,
    writer: W,
) -> Result<(W, Vec<u8>)>
```

它返回 `(writer, diagnostics)`：writer 是传入的输出目标，diagnostics 是运行时累积的字节缓冲（如未处理 Promise rejection 警告）。

## 实例化步骤

1. **Engine 获取**：从 Engine 池按 `(compiler, opt_level, debug)` 取或创建。
2. **Module 编译**：`wasmtime::Module::new(engine, wasm_bytes)`。这一步是 Cranelift/Winch 编译 WASM 到本地代码，结果是可缓存的 `Module`。
3. **Linker 构造**：注册全部 host import（约 507 个 `env.*` 函数 + support module imports）。
4. **Store 创建**：`Store` 持有 `RuntimeState` 和 Engine。
5. **Support module 实例化**：从嵌入的 cwasm 加载，实例化到同一 Store。
6. **User module 实例化**：`linker.instantiate(store, user_module)`。
7. **入口调用**：`instance.get_typed_func(store, "main")` → `func.call(store, ())`。

## 启动快照恢复

实例化后、调用入口前，如果启动快照可用，运行时从快照恢复内置对象状态，跳过 bootstrap。快照禁用时（`WJSM_STARTUP_SNAPSHOT=0`）走完整 bootstrap 路径。

## 退出码

`process_exit_code(error)` 从运行时错误中提取 `process.exit` 的退出码：

- 错误携带 `EXIT_CODE_MARKER` → 返回该码
- 其他错误 → None，CLI 据此打印 `Runtime error:` 并返回 2

## 诊断缓冲

`console.*` 直接写 stdout（通过 `writer`），不走 diagnostics 缓冲。diagnostics 只承载运行时累积的警告类信息，在执行结束后写入 stderr。

## 深入了解

- [执行阶段在流水线中的位置](../pipeline/execute.md)
- [启动快照的恢复机制](../startup/startup-snapshot.md)
- [用户侧的退出码与流对应关系](../../user/output/process-io.md)
