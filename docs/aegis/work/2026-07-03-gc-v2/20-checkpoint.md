# TodoCheckpointDraft

## Current todo

- Active: `T5.3 添加 pause benchmark`（in progress）。
- Next: `T5.4 添加 footprint 指标`。

## Completed todos

- P0:
  - `T0.1 创建 api.rs v2 trait`
  - `T0.2 实现 MarkSweepCollector`
  - `T0.3 切换调用方`
  - `T0.4 实现 registry`
  - `T0.5 新增 GC scheduler`
  - `T0.6 集成 heap governance`
- P1:
  - `T1.1 升级 immortal 边界`
  - `T1.2 新增八个 globals`
  - `T1.3 重构分配 fast-path`
  - `T1.4 换代 host imports`
  - `T1.5 参数化 support emitter`
  - `T1.6 验证布局阶段`
- P2:
  - `T2.1 新增 heap_access 断言`
  - `T2.2 建立裸写审计清单`
  - `T2.3 替换核心裸写点`
  - `T2.4 替换对象集合裸写点`
  - `T2.5 替换其余裸写点`
  - `T2.6 修复 WASM resize re-resolve`
  - `T2.7 验证 P2 阶段`
- P3:
  - `T3.0 接入测试矩阵`
  - `T3.1 实现 G1 region`
  - `T3.2 实现 G1 rset barrier`
  - `T3.3 生成 G1 support 变体`
  - `T3.4 实现 G1 young GC`
  - `T3.5 实现 G1 concurrent mark`
  - `T3.6 实现 G1 mixed GC`
  - `T3.7 组装 G1 registry`
  - `T3.8 验证 P3 阶段`
- P4:
  - `T4.1 实现 ZGC color page`
  - `T4.2 生成 ZGC support 变体`
  - `T4.3 实现 ZGC mark`
  - `T4.4 实现 ZGC relocate`
  - `T4.5 组装 ZGC registry`
  - `T4.6 验证 P4 阶段`
- P5:
  - `T5.1 实现三入口选择`
  - `T5.2 完善 GcStats v2`

## Active slice card

- Goal: P5 T5.3，新增门控 pause benchmark，执行三算法定量基准并记录/调参直到 spec §21.2 pause/吞吐/碎片验收达标。
- Parent plan/spec: `docs/aegis/plans/2026-07-03-pluggable-gc-v2.md` T5.3；`docs/aegis/specs/2026-07-03-pluggable-gc-v2-design.md` §21.2。
- Files: 新建 `crates/wjsm-runtime/tests/gc_pause_bench.rs`，必要时小幅调整 scheduler/算法 step budget。
- Boundary: 本 slice 只新增 `WJSM_GC_BENCH=1` 门控 benchmark 与必要参数调优；footprint 长运行指标留给 T5.4。
- Verification: `WJSM_GC_BENCH=1 cargo nextest run -E 'test(gc_pause_bench)'` 达标；不启用 env 时 bench skipped。
- Stop: T5.3 验证通过，checkpoint/evidence 更新，然后进入 T5.4。

## Evidence refs

详见 `90-evidence.md`。P0/P1/P2/P3/P4、T5.1、T5.2 已完成；T5.3 正在进行。

## Blocked-on items

无。

## ResumeStateHint

恢复时先执行：

1. 读取本文件、`90-evidence.md` 与父计划 T5.3 / spec §21.2。
2. 新增 `gc_pause_bench.rs` 门控基准并运行三算法；若不达标，调度/步进 owner 内修正并记录。
3. 完成后运行 bench、workspace/build 必要验证。

# DriftCheckDraft

- Does current work still serve original task intent? 是，T5.2 已完成 stats/log，当前进入 pause benchmark。
- Does current work still serve goal and stop condition? 是，T5.3 只交付 pause benchmark 与达标调参，不提前做 footprint。
- Compatibility boundary: bench 默认 skipped，只有 `WJSM_GC_BENCH=1` 执行。
- New owner/fallback/adapter/branch: `gc_pause_bench.rs` 成为定量 pause 验收 owner。
- Retirement track: 定量 pause 未验收限制开始退休；T5.4 后续消费 footprint 字段。
- Evidence sufficiency: T5.2 sufficient；T5.3 pending。
- Decision: continue。
