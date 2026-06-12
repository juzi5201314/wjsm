# TodoCheckpointDraft

**日期**: 2026-06-07  
**状态**: continue

## 已完成

- Task 0–5：`wjsm-runtime` 侧表生命周期（resolver 不 recycle、free-list、GC trace、sweep、combinator settlement、单元测试、fixture）
- `cargo check -p wjsm-runtime` 通过
- `cargo nextest run -p wjsm-runtime -E 'not test(async_reentry_audit)'`：**35 passed**
- `cargo nextest run -E 'test(happy__promise)'`：**22 passed**（含 `promise_with_resolvers_second_resolve`）

## 阻塞 / 范围外

- `async_reentry_audit`：基线失败（agent_cluster sync callback），非本任务引入
- `wjsm-semantic` 若干 promise 相关 `.ir` snapshot：Lowering 输出与快照不一致（**未改 semantic crate**；全量 workspace 时 4 项失败）

## 下一步

- 若需严格满足计划「`cargo nextest run --workspace`」：需单独处理 semantic 快照或确认是否环境基线问题
- 建议提交：`feat(runtime): complete side-table lifecycle (Task 0-5)`