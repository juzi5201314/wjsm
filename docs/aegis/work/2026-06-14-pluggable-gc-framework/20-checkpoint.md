# Checkpoint — 可插拔 GC 框架实施

**Date**: 2026-06-14
**Active slice**: P2 (backend safepoint spill)
**Branch**: gc-framework (HEAD @ c308bbd)

## Current Todo
P2: T2.1-T2.2 — safepoint spill 代码生成

## Completed Todos
- P0: T0.1-T0.2 SIZE_CLASSES validated (no changes) @ analysis
- P1: T1.1 tag_needs_root @ abc5e01 (2 tests)
- P1: T1.2 ValueTy @ f0aa8bc (4 tests)
- P1: T1.3-T1.4 liveness @ c308bbd (4 tests, Phi edge distribution correct)
- P1: T1.5 export (modules registered in lib.rs)

**P1 complete**: wjsm-ir 16/16 tests green. All IR-layer GC prep done.

## Evidence Refs
- T0 evidence: 90-evidence.md
- T1.x: crates/wjsm-ir/src/{value.rs,liveness.rs,value_ty.rs}, tests/liveness.rs
- liveness Phi edge correctness verified (if/else join + loop backedge tests pass)

## ResumeStateHint
- Execution mode: executing-plans (inline)
- P1 fully done. Next: P2 backend safepoint spill — highest-risk task.
- P2 needs: Compiler struct fields (shadow_sp_global_idx=4, shadow_sp_scratch_idx, shadow_stack_end_global_idx=8, local_idx via compiler_module.rs:6), instruction emit points (compiler_instructions.rs NewObject L373, NewArray L532, Call), global_idx tracking for safepoint alignment.
- Compiler fields read: lib.rs:122-136 (scratch idxs, globals). compile_function at compiler_module.rs:529.

## DriftCheckDraft
- Scope: P0-P6 per plan ✓
- Compatibility: activity object layout/NaN-boxing/obj_table unchanged ✓
- New owner: liveness.rs/value_ty.rs in wjsm-ir (appropriate) ✓
- Retirement: legacy GC deleted in P5 ✓
- P2 risk: spill codegen touches hot path; must not break 470+ fixtures. Mitigation: P2 不接 GC, only spill+restore (no-op semantically), fixture-green gate.
- Decision: continue

## Blocked On
(none — but P2 is high-risk, proceeding carefully)

## Next Step
P2 T2.1: compute_spill_plan (Compiler field + pass in compile_function). Then T2.2 emit spill at safepoints. Verify fixtures green (no GC yet).
