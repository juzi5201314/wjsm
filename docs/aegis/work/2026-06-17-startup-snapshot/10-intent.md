# Startup Snapshot 任务意图

- Parent plan: `docs/aegis/plans/2026-06-17-startup-snapshot.md`
- Branch: `startup-snapshot-execution`
- Requested outcome: 完整实现 relocatable startup snapshot，使后续 wjsm 执行能跳过重复的 primordial bootstrap。
- Scope: 从 P3 继续到 P8（P0–P2 已在前序会话完成）。
- Non-goals: 不快照 Wasmtime Instance/Store；不捕获用户运行态 async/host/shared 状态；不重组 RuntimeState。
- Risk hints: 计划原写 `crates/wjsm-backend-wasm/src/constants.rs` 实际不存在，固定字符串常量改在 `crates/wjsm-ir/src/constants.rs` 维护；`runtime_host_helpers.rs` 已超 800 行，新增小 helper 应优先放到独立 owner 文件而非继续膨胀。

## 基线阅读清单

- `docs/aegis/plans/2026-06-17-startup-snapshot.md`（已读）
- `crates/wjsm-backend-wasm/src/compiler_module.rs`（已读，P1/P2 改动点）
- `crates/wjsm-ir/src/constants.rs`（已读，P3 目标文件）
- `crates/wjsm-runtime/src/lib.rs`（已读，启动路径与 bench）
- `crates/wjsm-runtime/src/wasm_env.rs`（已读）
- `crates/wjsm-runtime/src/runtime_gc/roots.rs`（已读，P2 已完成）
- `crates/wjsm-runtime/src/types.rs`（NativeCallable 枚举）
- `crates/wjsm-runtime/src/runtime_host_helpers.rs`（host post-bootstrap 相关入口）
