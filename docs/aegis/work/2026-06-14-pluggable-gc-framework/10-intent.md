# Task Intent — 可插拔 GC 框架实施

## Requested Outcome
用 non-moving mark-sweep + segregated free list 替代当前 `memory.grow` 无限扩容，建立 `GcAlgorithm` trait 框架，恢复自动 GC。同步长循环不 OOM，safepoint 安全。

## Scope
- `wjsm-ir`: liveness + ValueTy pass（P1）
- `wjsm-backend-wasm`: safepoint spill + 分配路径改造（P2/P4）
- `wjsm-runtime`: `runtime_gc/` 框架 + MarkSweep（P3/P4/P5）
- `wjsm-cli`: `--gc-algorithm`（P6）

## Non-goals
- generational/incremental/parallel GC（仅留 trait）
- WASM GC proposal
- 活动对象布局/NaN-boxing/obj_table 变更
- 分代 write barrier 真实实现（defer 到 generational）

## Stop Condition
- P0-P6 全部任务通过 spec + code-quality 双 review
- fixture 全绿
- 长循环/safepoint/深链表 fixture 通过
- 旧 GC 删除无残留

## Risk Hints
- liveness Phi 边分发错误 → safepoint 误回收（P1 单测守）
- grow 借用 UB → GcContext 不持 slice（#9）
- mark 栈溢出 → worklist（#11）
- async reentry → sync Func::wrap（§12.3）

## Baseline Refs
- `docs/aegis/specs/2026-06-14-pluggable-gc-framework-design.md`（§18 硬约束）
- `docs/aegis/plans/2026-06-14-pluggable-gc-framework.md`（P0-P6 任务）
- `bug.md` O2
- `AGENTS.md`

## ImpactStatementDraft
- wjsm-ir: 新增 liveness.rs/value_ty.rs + tag_needs_root
- wjsm-backend-wasm: compiler spill pass + $obj_new/$arr_new 改造
- wjsm-runtime: runtime_gc/ 新模块组；删除 trigger_gc + core.rs gc_collect
- 兼容性：活动对象布局/NaN-boxing/obj_table 不变；fixture 全绿
