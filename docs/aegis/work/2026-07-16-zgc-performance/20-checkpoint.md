# TodoCheckpointDraft

## Current todo

Task 16 GREEN 完成。colored load/store barrier、SATB/remset ring、backend barrier emit helpers 与 loom 模型均已验证通过。

## Completed (this session, 2026-07-23 Task 16)

- 恢复检查点：Task 15 已 GREEN；下一任务为 Phase D Task 16。
- 审计既有实现：`98dc81cc feat: add colored GC barriers` 已落地
  - backend：`compiler_helpers/barrier.rs`
  - runtime：`runtime_gc/zgc/barrier.rs`
  - tests：`gc_barrier_protocol.rs` + `gc_loom_model` 的 `satb_` / `remembered_`
- 复跑 Task 16 GREEN 三条命令全部通过；补充验证 young/old/relocation/host-roots 相关后续任务测试也已存在并通过。

## Historical

Task 15 cutover 证据见同目录先前 checkpoint 段与 `90-evidence.md` Task 15 段。

## Next step

1. **正式下一任务：Task 24（JDK 25 归一化矩阵硬门）** 或先审计 Task 17–23 的 active 接入缺口。
2. 代码树中 Task 17–23 的 controller/tests/commits 已存在且单元 GREEN，但：
   - barrier emit helpers **尚未**接入 support/object helpers 热路径（`emit_*` 目前主要是 contract/unit）；
   - ADR 0010 仍写 Task 15/24/25 为 open；
   - 计划 checkbox 多数未同步（仅本切片关闭 Task 16）；
   - Task 24/25 仍缺具名 JDK 25 probe / large-heap / capability runner 证据；
   - Task 26 源码级退役（legacy `GcContext` collector 体、`HANDLE_TABLE_ENTRY_SIZE=4` 残留字符串、旧 path）仍 open；
   - `.github/workflows` 已按用户要求删除，不得回灌 `managed-heap-v2`。
3. 用户若要求“按计划顺序一个 task 一个 task”，下一会话从 **Task 17 接入审计** 或直接 **Task 24 preflight/compare RED** 二选一；默认推荐先做 Task 17–23 的 active-path 接入缺口审计，避免性能门在未接线 barrier 上假跑。

## ResumeStateHint

读本文件 + `git log --oneline -15` + Task 16 GREEN 证据段即可恢复。
环境噪声：load>10 时 3s 门禁会假超时（先 `uptime`）。

## Checkpoint 2026-07-23：Task 16 GREEN 完成

### TodoCheckpointDraft

- 当前 todo：Task 16 已完成；下一候选 Task 17 active 接入审计 或 Task 24 JDK 归一化门。
- 已完成：Task 16 三条 GREEN 命令、loom satb/remembered、barrier protocol 8 tests、backend `gc_barrier` 5 tests。
- 本切片代码改动：无 production 源码改动；更新计划 checkbox、checkpoint、evidence。

### Evidence

- `cargo nextest run -p wjsm-backend-wasm -E 'test(gc_barrier)'` → 5 passed / 59 skipped
- `cargo nextest run -p wjsm-runtime --test gc_barrier_protocol` → 8 passed
- `cargo nextest run -p wjsm-runtime --test gc_loom_model -E 'test(satb_) | test(remembered_)'` → 2 passed / 8 skipped
- 提交：`98dc81cc feat: add colored GC barriers`
- 旁证（后续任务已落地，不在本切片关闭范围内）：
  - `gc_young_concurrent` 6 passed
  - `gc_old_concurrent` 2 passed
  - `gc_relocation_concurrent` 5 passed
  - `gc_host_roots_concurrent` 2 passed
  - director/platform unit tests 存在于 `director.rs` / `heap/platform/mod.rs`

### BaselineUsageDraft

- 已读取：主计划 Task 16、设计 §10 Barrier、检查点、`barrier.rs` backend/runtime、`gc_barrier_protocol`、`gc_loom_model`、commit `98dc81cc`。
- 缺失：无 Task 16 阻塞项。

### DriftCheckDraft

- 范围：Task 16 colored barriers + verifier；未扩展到 Task 24 性能门。
- 兼容边界：未改 ECMAScript / 公开 `--gc`；reference-only color bits 38–43；SeqCst shared words。
- 退役轨迹：旧 bump/4-byte 主路径已由 Task 15 删除 private gate；源码级残留仍属 Task 26。
- 决策：`continue`（可进入 Task 17 接入审计或 Task 24）。

### Risk / Unknown

- backend `emit_store_color_*` / `emit_atomic_*` 目前是 contract 级 helper，support 热路径仍主要靠既有 `emit_reference_barrier_event`；真正把 colored barrier 编进所有 object load/store 热路径可能要在 Task 17+ 或单独接线切片补齐。
- Task 24/25 仍 `needs-verification`（JDK 25 probe / 大堆 runner / CI workflows 已删）。
- ADR 0010 状态文案仍滞后于 Task 15 GREEN。
