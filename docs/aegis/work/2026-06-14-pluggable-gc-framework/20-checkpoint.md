# Checkpoint — 可插拔 GC 框架实施

**Date**: 2026-06-14
**Active slice**: P4 T4.1-T4.6（接通 alloc 路径；4 blocker 已完成）
**Branch**: gc-framework (HEAD @ d7c3c29)

## Current Todo
P4 T4.1: host imports gc_alloc_slow + gc_maybe_collect + gc_take_freed_handle
**当前执行**: T4.1

## Completed Todos
- P0: T0.1-T0.2 SIZE_CLASSES validated
- P1: T1.1-T1.5 IR 层 liveness + ValueTy (16/16 tests)
- P2: T2.1-T2.4 + R-fix safepoint spill codegen (561/561 fixtures)
- P3: T3.1-T3.8 runtime_gc 框架 + MarkSweep (16/16 runtime_gc tests)
- **P4 T4.b1**: sweeper 回收 resize-abandoned 区域 @ 477346e (abandoned_regions list, sweeper +2 tests)
- **P4 T4.b2**: marker 标记 closure env_obj + native_callable @ 92feba3 (两阶段值解析, marker +2 tests)
- **P4 T4.b3**: RootProvider fixed-point @ 81e8382 (移植 fixed-point tracer, collect_with_provider)
- **P4 T4.b4**: 补全 safepoint 集 @ d7c3c29 (ObjectSpread/CollectRestArgs/NewPromise/PromiseResolve/Reject/StringConcatVa)

## Evidence Refs
- runtime_gc 20/20 tests green
- workspace 非 fetch_http 887/887 green（4 个 fetch_http_streaming 是预先存在的 WSL 网络 flaky，
  git stash 回到各 blocker 前同样失败）

## ResumeStateHint
- Execution mode: executing-plans (inline)
- P4 执行顺序（严格按计划 R4）: T4.b1 → T4.b2 → T4.b3 → T4.b4（修框架正确性），再 T4.1-T4.6（接通 alloc 路径）。
- 每个 blocker / 子任务独立 commit，修一个验证一个（fixture 全绿 + 相关单测）。
- 每阶段后 cargo nextest run --workspace 确认无回归，重点关注 streams_byob / async / class_super_constructor。
- P4 blocker 关键事实：
  - grow_array/runtime_values.rs:190 / grow_object:234 重写 obj_table 槽后旧 ptr 不可达 → abandoned_regions list 方案。
  - marker.rs:153 push_value_handle 只处理 object/array/function，closure/native_callable 漏标 → 需 ctx.with_state 查 closures/native_callables 表。
  - roots.rs for_each_host_table_root 只提供 function props；fixed-point tracer 在 runtime_builtins.rs:2590（trace_runtime_side_table_roots_fixed_point），需移植或注入 collect_with_roots 多轮。
  - is_safepoint() (compiler_instructions.rs:14) 缺 ObjectSpread/CollectRestArgs/NewPromise/PromiseResolve/PromiseReject/StringConcatVa/SetProto。
- RuntimeState 已有字段复用: handle_free_list(1072), gc_threshold(1054 u64), alloc_counter(1051), gc_mark_bits(1049), closures(1062), native_callables(1066), continuation_table(1092).

## DriftCheckDraft
- Scope: P4 per plan R4 ✓ (T4.b1-b4 + T4.1-T4.6)
- Compatibility: activity object layout/NaN-boxing/obj_table unchanged ✓ (non-moving INV-C)
- New owner: abandoned_regions 加到 RuntimeState（resize 注册）；marker/roots/sweeper 改在 runtime_gc/ 模块组 ✓
- Retirement: legacy GC deleted in P5（P4 先接管 gc() global）✓
- P4 risk: 接通真实 GC → 4 blocker 必须先解决否则 fixture 崩。Mitigation: blocker 顺序 + fixture-green gate。
- Decision: continue

## Blocked On
(none)

## Next Step
T4.b1: RuntimeState 加 abandoned_regions 字段 + grow_array/grow_object 注册旧 (ptr, size) + sweeper 读 abandoned_regions add_free_region + sweep 结束清空。
