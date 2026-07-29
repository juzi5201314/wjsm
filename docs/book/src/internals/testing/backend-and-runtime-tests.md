# Backend 与 Runtime 定向测试

这一章说明 `wjsm-backend-wasm` 和 `wjsm-host-wasm` 的定向测试。

## Backend 测试

`wjsm-backend-wasm` 的测试验证 WASM 代码生成的正确性：

- 特定 IR 指令序列生成正确的 WASM 指令。
- 函数表、import、export 的 ABI 正确性。
- 数据段、字符串常量的布局。

这些测试直接构造 IR，编译为 WASM，用 `dump-wat` 或 `disasm` 检查输出。

## Runtime 测试

`wjsm-host-wasm` 的测试验证运行时行为：

- host import 注册的正确性。
- Promise、微任务、async 调度的时序。
- GC 触发和回收行为。
- 快照捕获与恢复。

`crates/wjsm-host-wasm/tests/` 包含集成测试，如 `embedded_support_cwasm.rs` 验证嵌入工件的正确性。

## Runtime crate 测试

`crates/wjsm-runtime/tests/` 包含 facade 层的测试：

- `startup_snapshot.rs`：启动快照行为。
- `embedded_startup_snapshot.rs`：嵌入式快照。

这些测试通过 `wjsm-runtime` facade 调用，验证公开 API 的端到端行为。

## 运行方式

```bash
cargo nextest run -p wjsm-semantic
cargo nextest run -p wjsm-backend-wasm
cargo nextest run -p wjsm-host-wasm
cargo nextest run --workspace
```

窄测试先跑，确认改动正确后再跑全 workspace。

## 深入了解

- [Fixture 测试框架](fixtures.md)
- [test262 一致性测试](test262.md)
- [分层调试流程](debugging-workflow.md)
