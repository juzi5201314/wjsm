---
artifact: TodoCheckpointDraft
work: 2026-06-19-build-time-embedded-runtime
created: 2026-06-20
updated: 2026-06-20
---

# Todo Checkpoint Draft

## Current Todo

P1 runtime/client-side disk cache retirement + P2.3 回归修复完成：happy/errors fixture 520/520 通过，workspace 970/970 通过（1 skipped）。

## Active Task

P2.4 前置状态：P2.3 object helpers 已恢复基线；runtime startup snapshot cache 与 `WJSM_MODULE_CACHE` 已删除，无已知 fixture 回归。

## Completed Tasks

- P0: 工作区准备 ✅
- P1.0-P1.4: Embedded startup snapshot ✅
- P2.0: Support module ABI ✅
- P2.1: build.rs 产 cwasm ✅
- P2.2: 共享 memory/table/globals + 双 instance instantiate ✅
- P2.3: 切 object helpers (obj_new/obj_get/obj_set/obj_delete + string_eq + to_int32) ✅
- P3.0: builtin_js 框架 ✅
- P4.0: ADR 0004 ✅
- Client-side disk cache retirement: `startup_snapshot_cache.rs` / `WJSM_MODULE_CACHE` 删除 ✅

## Evidence Refs

- `cargo nextest run -E 'test(happy__) or test(errors__)'` → 520 passed, 52 skipped
- `cargo nextest run -p wjsm-backend-wasm` → 50 passed
- `cargo nextest run -p wjsm-runtime -E 'test(startup_snapshot) or test(embedded_startup_snapshot)'` → 4 passed, 69 skipped
- `cargo nextest run --workspace` → 970 passed, 1 skipped

## 关键设计决策

1. **共享 type section** (`shared_types.rs`)：wasmtime `call_indirect` 要求 type index 一致
2. **`__wjsm_init_globals` 函数**：globals import 后初始值在 runtime 调用，先于 host post-bootstrap
3. **无 support module element section**：helpers 通过 Call(import) 调用，不占 table slot
4. **Element section 从 table[0] 开始**：support module 不使用 element section，user wasm 独占 table
5. **全部 globals mutable**：import 后需 global.set 初始化

## 已修复回归根因

- **support-origin host callbacks 缺失 WasmEnv**：support module 只 export helper functions，未 re-export imported memory/table/globals；从 support helper 进入 `native_call` / proxy / stream 等 host import 时 `Caller::get_export` 取不到 env。修复：support module 重新 export 共享 env handles，ABI version bump 到 3。
- **compiled eval globals mutability / env export 不一致**：Normal user wasm 把 19 个 env globals 全部 mutable re-export，但 Eval wasm 仍按旧 const/mutable 混合导入，且未 export memory/table。修复：Eval wasm 导入 mutability 与父模块一致，并 export memory/table/全部 env globals。
- **data segment 在 heap_start 后继续增长**：P2.2 提前计算 imported global heap 布局后，`finalize_eval_var_map_data` 和函数 `.name` 字符串仍在后续追加，落进 object heap，被分配/GC 覆盖。修复：计算 heap_start 前固化 eval metadata 与函数属性名字符串。

## Drift Check Draft

### Decision

`continue` — P1 runtime/client-side disk cache 已退役，P2.3 回归已修复并通过 workspace 验证；下一步进入 P2.4 array/elem helpers 迁移。
