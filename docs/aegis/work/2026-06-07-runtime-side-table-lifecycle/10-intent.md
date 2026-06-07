# TaskIntentDraft — 运行时侧表生命周期补全

**日期**: 2026-06-07  
**计划**: `docs/aegis/plans/2026-06-07-runtime-side-table-lifecycle.md`  
**规格**: `docs/aegis/specs/2026-06-07-runtime-side-table-lifecycle-design.md`

## 目标

补全 continuation/combinator free-list、GC 侧表边 trace、promise_table 清槽；修复 PromiseResolvingFunction 错误 recycle。

## 成功证据

- `cargo check -p wjsm-runtime`
- `cargo nextest run -p wjsm-runtime`（除既有 async_reentry_audit）
- fixture `promise_with_resolvers_second_resolve` stdout `1`

## 非目标

不 compaction；不把 registry 整表当 GC root。