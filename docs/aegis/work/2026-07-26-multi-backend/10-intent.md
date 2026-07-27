# 多后端完全支撑：任务意图

- 请求结果：执行 `local://multi-backend-complete-plan.md` 的 P0–P12，完成后端无关 JS 语义迁移、多后端契约与性能闭环。
- 当前范围：P5–P8；Streams、Fetch、Modules/CJS、残留 JS 语义迁入 `wjsm-builtins`，I/O、wasmtime 实例化、分配与 bootstrap 保持后端所有权。
- 非目标：不迁移 `fetch_http`、`streams_fetch_body`、模块 instantiate、`create_global_object`、再入/eval/timers 基础设施；不引入 dyn ExecContext 或兼容旧路径。
- 基线：`local://multi-backend-complete-plan.md`、根 `AGENTS.md`、`docs/aegis/work/2026-07-26-multi-backend/00-baseline.md`。
- 成功证据：每 phase 定向测试、workspace 全测、耦合断言、最终 release 性能对比。
- 风险：Streams/Fetch 状态机共享类型含 host-only 字段；Modules 冷路径动态 loader 必须留在后端；迁移必须维持单一语义 owner。
