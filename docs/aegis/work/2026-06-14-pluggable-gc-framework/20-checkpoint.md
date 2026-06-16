# Checkpoint — 可插拔 GC 框架实施

**Date**: 2026-06-14
**Active slice**: P4 + P5 + P6 全部完成 ✅
**Branch**: gc-framework (HEAD @ cc29eb4)

## Current Todo
P0-P6 全部完成。剩余：收尾文档同步（T-final.1/2，可选）+ finishing-a-development-branch。

## Completed Todos
### P4 (blocker + 接通 alloc 路径 + fixture)
- T4.b1: sweeper 回收 resize-abandoned 区域 @ 477346e
- T4.b2: marker 标记 closure env_obj + native_callable @ 92feba3
- T4.b3: RootProvider fixed-point @ 81e8382
- T4.b4: 补全 safepoint 集 @ d7c3c29
- T4.1+T4.2: host imports + gc_algorithm field @ 8bb9290
- T4.3: $obj_new/$arr_new 接通 GC @ 2c15631
- T4.4: gc() global 重接到框架 @ e520bc7
- T4.5: 长循环 + safepoint fixture + obj_table/内存扩容 + proto root @ 0b51d16
- T4.6: 集成验证 ✅

### P5 (删除旧 GC)
- T5.1: 删除 trigger_gc + sweep_dead_promise_slots + fixed-point tracer @ b873878
- T5.2: 删除 gc_collect host import @ ada84c2
- T5.3: grep 无残留 + 全 fixture ✅（唯一残留是 gc() builtin 注释引用历史）

### P6 (预留 hook + CLI)
- T6.1: 预留 hook 默认 impl 确认（已在 T4.1/T4.2 实现）@ 265a799
- T6.2: CLI --gc-algorithm flag @ cc29eb4

## Evidence Refs
- runtime_gc 20/20 tests green
- wjsm-ir 16/16 tests green
- streams_byob/async/class_super 43/43 green
- gc_long_loop (200000 次) + gc_safepoint_local PASS
- workspace 890/893（2-4 个 fetch_http_streaming 是预先存在的 WSL 网络 flaky，
  git stash 回到 T4.2 baseline 同样失败；1 个偶发 timeout 同网络套件）

## P4-P6 实现期发现并修复的关键问题
1. alloc_slow 返回 ptr 而非 handle（handle 注册在 WASM $obj_new 中完成）。
2. proactive GC 移到分配前（原放分配后导致刚分配对象被 sweep 回收）。
3. obj_table 容量不足 → count 超 256 越界读垃圾（提高到 2048 + 内存 4 pages）。
4. prototype 对象漏 root → 原型链断裂（补充 array/object_proto_handle 作顶层 root）。
5. alloc_host_object OOM 不触发 GC/grow → 补 grow 路径。

## DriftCheckDraft
- Scope: P4-P6 per plan R4 ✅
- Compatibility: non-moving INV-C ✓；obj_table 容量 + 内存扩容是兼容边界内调整 ✓
- Retirement: legacy GC 已删除（trigger_gc + gc_collect import 全清）✓
- Decision: P4-P6 complete。下一步 finishing-a-development-branch。

## Next Step
finishing-a-development-branch：验证测试、向用户呈现选项（merge/PR/branch cleanup）。
可选收尾：T-final.1（bug.md O2 → RESOLVED + AGENTS.md 更新）+ T-final.2（ADR 0002）。
