# TodoCheckpointDraft

## Current todo

Task 17 GREEN 完成（协议层 concurrent young mark）。active GC 路径仍未挂 YoungController，属后续接入缺口。

## Completed (this session, 2026-07-23 Task 17)

- 恢复检查点：Task 16 已 GREEN；进入 Task 17 concurrent young mark。
- 审计 owner：
  - `runtime_gc/zgc/young.rs`：`YoungController` type-state phases / SATB / remset ring / black alloc / pause <1ms 断言
  - tests：`gc_young_concurrent.rs`（6）、loom `young_termination_model_*`
  - 提交：`c25f7101 feat: implement concurrent young marking`
- GREEN 三条命令全部通过（含 30-sample churn bench）。
- **Active-path 审计（关键）**：
  - `gc()` / `gc_safepoint_poll` 均调用 `runtime_gc::active_v2::collect_full`
  - `active_v2::collect_full` 是 **非移动 full mark-retire**，对 mark-sweep/g1/zgc **同一路径**
  - `YoungController` 仅被 unit/integration tests 与 `OldController` 协作使用，**不在** `registry::create(Zgc)` 的 active collect 路径
  - `ZgcCollector::collect_full` 仍是 legacy incremental mark/relocate，且 active host 不调用它做 full collect
  - 因此 Task 17 关闭的是 **协议/controller + 测试 + bench 可运行证据**，不是 “active `--gc zgc` 已跑 concurrent young mark”

## Next step

1. 继续按计划顺序关闭 Task 18–23 的协议 GREEN（多数 commits/tests 已在树中），或
2. **优先工程切片**：把 concurrent ZGC（YoungController + remset + old + relocate + director）接到 active `gc()` / safepoint，替换 `active_v2::collect_full` 对 zgc 的统一非移动回收（跨 Task 17–22 的真正 active 接入）。
3. Task 24 在 active concurrent ZGC 接线前跑 JDK 矩阵会得到 **错误归因**（测的是 active_v2 full collect，不是 generational concurrent ZGC）。

推荐下一会话：**不要盲关 Task 18 checkbox**；先开 **active concurrent ZGC wiring** 切片，或至少把缺口写进 Task 18 检查点再逐项协议关闭。

## ResumeStateHint

读本文件 + `git log --oneline -10` + Task 17 evidence。bench 输出：`/tmp/young.json`（status=`needs-verification` 因缺 physical/CPU/barrier/JDK counters，属合同，非失败）。

## Checkpoint 2026-07-23：Task 17 GREEN 完成

### TodoCheckpointDraft

- 当前 todo：Task 17 协议 GREEN 完成；下一候选 Task 18 或 active concurrent ZGC wiring。
- 本切片：无 production 源码改动；计划 checkbox + checkpoint + evidence。

### Evidence

- `cargo nextest run -p wjsm-runtime --test gc_young_concurrent` → 6 passed
- `cargo nextest run -p wjsm-runtime --test gc_loom_model -E 'test(young_)'` → 4 passed（含名称含 young 的相关 loom）/ 6 skipped
- `cargo run --release -p wjsm-gc-bench -- run --engine wjsm --gc zgc --heap 32m --scenario churn --samples 30 --output /tmp/young.json`
  - exit 0
  - 30 samples；pause max_ns 样本范围约 123k–241k ns，**全部 <1ms**
  - JSON `status=needs-verification`（notes：missing physical allocation/CPU/barrier/JDK counters）— gate 合同，非 bench 崩溃
- 提交：`c25f7101`

### BaselineUsageDraft

- 已读：计划 Task 17、young.rs、gc_young_concurrent、active_v2、host gc_safepoint/gc()、registry。
- 缺失：无 Task 17 协议阻塞；active wiring 为显式 follow-up。

### DriftCheckDraft

- 范围：Task 17 concurrent young mark 协议。
- 兼容：未改公开 `--gc` 语义出口；active 仍 full collect。
- 退役：legacy ZgcCollector 仍在 registry，active full path 不依赖它。
- 决策：`continue`（可 Task 18 或 active wiring）。

### Risk / Unknown

- **Active concurrent ZGC 未接线**：性能门若现在跑，归一化的是 active_v2 而非 YoungController。
- YoungController 使用内存内 object registry，尚未直接扫描 ManagedHeap object map/pages。
- Task 24/25 仍 needs-verification（JDK probe / runners / CI 已删）。
