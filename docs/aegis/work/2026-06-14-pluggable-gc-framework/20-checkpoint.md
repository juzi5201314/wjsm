# Checkpoint — 可插拔 GC 框架实施

**Date**: 2026-06-14
**Active slice**: P1 (IR liveness + ValueTy)
**Branch**: gc-framework (HEAD @ abc5e01)

## Current Todo
P1: T1.2 — ValueTy 类型推断

## Completed Todos
- P0: T0.1-T0.2 SIZE_CLASSES validated (no changes) @ analysis
- P1: T1.1 tag_needs_root @ abc5e01 (2 new tests, 8/8 wjsm-ir green)

## Evidence Refs
- spec @ 2fafc04, plan @ 2fafc04
- T0 evidence: 90-evidence.md (SIZE_CLASSES coverage table)
- T1.1: crates/wjsm-ir/src/value.rs:tag_needs_root, tests/liveness.rs

## ResumeStateHint
- Execution mode: executing-plans (inline, no write-capable subagent available)
- T1.1 done & committed. Next: T1.2 ValueTy (new file value_ty.rs + tests).
- T1.2 needs: read Instruction enum variants (lib.rs:318-488), Constant enum, BinaryOp/CompareOp/UnaryOp, and whether Function has module() access to Constants. Then implement infer_value_ty.
- API verified facts from T1.1 subagent: value.rs predicates all pub; encode fn names confirmed (encode_function_idx not encode_function, etc.)

## DriftCheckDraft
- Scope: P0-P6 per plan ✓
- Compatibility: activity object layout/NaN-boxing/obj_table unchanged ✓
- New owner: tag_needs_root in value.rs (appropriate, value.rs owns value classification) ✓
- Retirement: legacy GC deleted in P5 ✓
- Decision: continue

## Blocked On
(none)

## Next Step
T1.2 ValueTy type inference — read IR Instruction/Constant/Op enums, implement infer_value_ty, TDD
