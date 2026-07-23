# TodoCheckpointDraft

## Current todo

**Task 24 之前的准备已完成。** Active concurrent ZGC 已接到 `gc()`；Task 16–23 协议 GREEN 已关闭。下一任务是 Task 24（JDK 25 归一化矩阵硬门）。

## Completed (this session, 2026-07-23)

### Active concurrent ZGC wiring（计划外但用户指定优先）
- 新增 `runtime_gc/active_zgc.rs`：`--gc zgc` 在唯一 `HeapAccessV2` 上
  构图 → YoungController + OldController phase machine → director evaluate →
  promote_to_old → heap mark 保险闭包 → retire dead → weak/realm cleanup。
- `collect_dispatch(algorithm)`：`zgc` → active_zgc；其它 → active_v2。
- 接线：
  - `runtime_builtins` `NativeCallable::GcCollect`
  - `host_imports/core` `gc_safepoint_poll`
  - `runtime_heap` host allocation pressure
- `HeapAccessV2` 增补：`handle_generation` / `promote_to_old` / `object_size_public`。
- 非 zgc 路径保持 active_v2；无第二 ManagedHeap。

### Task 18–23 协议 GREEN 关闭
- Task 18 remset/promotion：young remset/promotion + loom 通过
- Task 19 old mark：gc_old_concurrent + loom 通过
- Task 20 relocation：gc_relocation_concurrent + loom 通过
- Task 21 host roots：gc_host_roots_concurrent + integration + WJSM_TEST_GC=zgc happy weak/finalization/async 通过
- Task 22 director：lib `gc_director` 7 passed
- Task 23 platform：lib `heap_platform|bitmap_simd` 4 passed；`wjsm-gc-bench capabilities` 写出 JSON

## Next step — Task 24

```bash
cargo build --release -p wjsm-gc-bench
target/release/wjsm-gc-bench preflight --heap 1g --profile pr --output /tmp/zgc-pr-resources.json
target/release/wjsm-gc-bench compare --jdk-home "$JDK25_HOME" --jdk-probe-home "$JDK25_PROBE_HOME" \
  --heaps 32m,256m,1024m --live-sets 10,50,80 \
  --scenarios churn,request,chain,cycle,wide,mutation,humongous,idle-uncommit \
  --samples 30 --output /tmp/zgc-compare
target/release/wjsm-gc-bench gate --manifest /tmp/zgc-compare/manifest.json
```

## ResumeStateHint

读本文件 + 提交 `active concurrent ZGC wiring` + Task 16–23 勾选状态。
环境：本机约 16 GiB；4/16 GiB nightly 仍 exit 78。CI workflows 已删。

## Pre-Task24 readiness

| 项 | 状态 |
|---|---|
| Active `--gc zgc` 走 young/old/director 路径 | **已接线**（`active_zgc`） |
| mark-sweep/g1 仍 active_v2 | **是** |
| Task 16–23 协议测试 | **GREEN** |
| JDK 25 probe / instrumented counters | **needs-verification**（Task 24 本身） |
| physical/CPU/barrier raw numerators | **needs-verification**（telemetry 合同） |
| 真正 concurrent worker 后台 mark | **未完成**（active_zgc 是 safepoint 内 phase drain，非跨线程 worker） |
| ConcurrentRelocator 热路径 copy | **未挂到 active full collect**（phase 握手有，对象 copy 仍协议层） |
| Task 26 源码退役 | open |
| ADR 0010 状态文案 | 滞后，Task 27 闭环 |

## DriftCheckDraft

- 范围：用户要求的 active wiring + Task 18–23 协议关闭；未开始 Task 24 矩阵。
- 兼容：公开 `--gc` 不变；zgc full collect 语义变为 generational phase + 非移动 retire（与 active_v2 同为 non-moving reclaim，但 stats 为 ZgcCycle）。
- 退役：zgc 不再经 active_v2；legacy ZgcCollector 仍在 registry 供 alloc_slow/barrier_slow。
- 决策：`continue` → Task 24。
