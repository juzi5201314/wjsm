# TodoCheckpointDraft

## Current todo

**Task 26 GREEN 完成。** 下一任务是 Task 27（最终全量验证与 ADR/AGENTS 闭环）。

## Completed (this session, 2026-07-23)

### Task 26 — 删除旧 benchmark / scheduler 残余 owner + 负检查
- 旧 bench/example 入口此前已删；本切片完成**源码级**退役：
  - 删除 V1 `MarkSweepCollector` / `G1Collector` / `ZgcCollector` 与
    `mark_sweep/{allocator,marker,sweeper}`、`g1/{concurrent_mark,mixed,young}`。
  - 删除 `registry::create`、`GcAlgorithm` trait、`AllocRequest`/`StepOutcome`。
  - `RuntimeState.gc_algorithm` 改为 `GcAlgorithmKind`（Copy），去掉
    `Arc<Mutex<Box<dyn GcAlgorithm>>>`。
  - `gc_alloc_slow` → `allocate_v2_object_bytes` / `HeapAccessV2.reserve_nlab`；
    `gc_barrier_flush` / `gc_load_barrier_slow` 无 dyn lock。
  - support 双轨折叠为 V2-only；`embedded_support_cwasm` 与 `_v2` 同字节。
  - `HANDLE_TABLE_ENTRY_SIZE = 8`；startup snapshot 走 V2 object region capture/restore。
  - `heap_access` 转发 `HeapAccessV2`；移除 criterion。
- 负检查：`alloc_from_bump` / V1 collector 结构体 / `registry::create` /
  `Arc<Mutex<Box<dyn` / `HANDLE_TABLE_ENTRY_SIZE=4` / `emit_support_module_with_heap_mode` /
  Cargo `managed-heap-v2` / `cfg(feature = "managed-heap-v2")` / 旧 bench 源文件 → **0 实命中**。
- GREEN：`cargo nextest run --workspace` → **1796 passed, 17 skipped**。
- Smoke：`--gc mark-sweep|g1|zgc` 均输出 `2`。

### 此前已关闭
- Active concurrent ZGC wiring（`active_zgc`）
- Task 16–23 协议 GREEN
- Task 15 V2 cutover + feature 删除
- 旧 `gc_stress` / `zgc_autoresearch` / `zgc_barrier_pressure` 入口删除

## Next step — Task 27

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo nextest run --workspace
WJSM_TEST_GC=mark-sweep cargo nextest run -E 'test(happy__)'
WJSM_TEST_GC=g1 cargo nextest run -E 'test(happy__)'
WJSM_TEST_GC=zgc cargo nextest run -E 'test(happy__)'
# + Miri/TSan when available; superseding ADR / AGENTS / evidence closure
```

## ResumeStateHint

读本文件 + 提交 `refactor: retire legacy GC paths and benchmarks` + plan Task 26 勾选。
Task 24/25 性能/大堆矩阵仍 `needs-verification`（缺 JDK diagnostic / 具名 runner）。
Task 27 负责 ADR 0010 状态文案与全量闭环。

## DriftCheckDraft

- 范围：Task 26 退役；未改 Task 24 阈值/场景，未重开 CI workflows。
- 兼容：公开 `--gc` / `WJSM_GC` / `gc()` 不变；内部去掉 dyn collector 与 memory32 bump。
- 退役：V1 collectors、support dual path、4-byte entry 常量、criterion、collector 全局 mutex。
- 已知残余（非阻塞 Task 26）：
  - `compile_object_helpers` / `compile_array_helpers` 定义仍在但无调用方（Eval/Normal 走 support）。
  - zgc 协议单测仍有 4-byte *colored payload* 布局 + 8-byte stride（非 active 热路径）。
- 决策：`continue` → Task 27。
