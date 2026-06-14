# Checkpoint — 可插拔 GC 框架实施

**Date**: 2026-06-14
**Active slice**: P4 完成 ✅
**Branch**: gc-framework (HEAD @ 0b51d16)

## Current Todo
P4 全部完成。下一阶段 P5（删除旧 GC）+ P6（CLI flag）。

## Completed Todos (P4)
- T4.b1: sweeper 回收 resize-abandoned 区域 @ 477346e (abandoned_regions list)
- T4.b2: marker 标记 closure env_obj + native_callable @ 92feba3 (两阶段值解析)
- T4.b3: RootProvider fixed-point @ 81e8382 (移植 fixed-point tracer + collect_with_provider)
- T4.b4: 补全 safepoint 集 @ d7c3c29 (ObjectSpread/CollectRestArgs/NewPromise/...)
- T4.1+T4.2: host imports + gc_algorithm field @ 8bb9290 (3 imports + GcContext.collect_with_provider)
- T4.3: $obj_new/$arr_new 接通 GC @ 2c15631 (bump+slow, proactive, handle reuse, alloc_host_object grow)
- T4.4: gc() global 重接到框架 @ e520bc7
- T4.5: 长循环 + safepoint fixture + obj_table/内存扩容 + proto root @ 0b51d16
- T4.6: 集成验证 ✅

## Evidence Refs
- runtime_gc 20/20 tests green
- wjsm-ir 16/16 tests green
- streams_byob/async/class_super 43/43 green（含 streams_byob_gc_pending_view）
- gc_long_loop (200000 次) + gc_safepoint_local PASS
- workspace 889/893（4 个 fetch_http_streaming 是预先存在的 WSL 网络 flaky，
  git stash 回到 T4.2 baseline 同样失败）

## P4 实现期发现并修复的关键问题
1. alloc_slow 返回 ptr 而非 handle（handle 注册在 WASM $obj_new 中完成）。
2. proactive GC 移到分配前（原放分配后导致刚分配对象被 sweep 回收）。
3. obj_table 容量不足 → count 超 256 越界读垃圾（提高到 2048 + 内存 4 pages）。
4. prototype 对象漏 root → 原型链断裂（补充 array/object_proto_handle 作顶层 root）。
5. alloc_host_object（TypedArray/ArrayBuffer/...）OOM 不触发 GC/grow → 补 grow 路径。

## ResumeStateHint
- P0-P4 完成。P5（删旧 GC trigger_gc/gc_collect + grep 无残留）+ P6（CLI --gc-algorithm）待执行。
- 旧 GC 残留：runtime_builtins.rs trigger_gc（L2939+，P5 删）、host_imports/core.rs gc_collect（L1218+，P5 删）。
- gc() global 已重接到框架（T4.4），P5 删除 trigger_gc 无断档。
- fetch_http_streaming 网络 flaky 非 GC 引起（已确认）。

## DriftCheckDraft
- Scope: P4 per plan R4 ✅
- Compatibility: activity object layout/NaN-boxing/obj_table 不变（non-moving INV-C）✓
- obj_table 容量 + 初始内存扩容（2048 entries + 4 pages）是兼容性边界内的调整 ✓
- Retirement: legacy GC P5 删除（P4 已接管 gc() global）✓
- Decision: P4 complete, continue to P5

## Next Step
P5 T5.1: 删除 trigger_gc（runtime_builtins.rs L2939+）+ sweep_dead_promise_slots（已并入 sweeper）。
P5 T5.2: 删除 host_imports/core.rs gc_collect（L1218+）+ linker 注册。
P5 T5.3: grep 无残留 + 全 fixture。
