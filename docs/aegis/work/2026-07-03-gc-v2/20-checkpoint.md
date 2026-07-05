# TodoCheckpointDraft

## Current todo

- Active: `T5.4 添加 footprint 指标`（in progress）。
- Next: `T5.5 审计 GC 回归矩阵`。

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
  - `T5.3 添加 pause benchmark`

## Active slice card

- Goal: P5 T5.4，新增 `memory_footprint_hist` 与长运行 footprint 治理测试，验证对象存活量下降后后续分配优先复用空闲区域而非持续 grow。
- Parent plan/spec: `docs/aegis/plans/2026-07-03-pluggable-gc-v2.md` T5.4；`docs/aegis/specs/2026-07-03-pluggable-gc-v2-design.md` §17、§21.2。
- Files: `runtime_gc/api.rs` 已有 committed/reusable 字段，需补 RuntimeState footprint hist 与 `tests/gc_footprint_long_run.rs`。
- Boundary: 本 slice 只做 footprint hist 与 `WJSM_GC_BENCH=1` 门控长运行；回归矩阵覆盖审计留给 T5.5。
- Verification: `WJSM_GC_BENCH=1 cargo nextest run -E 'test(gc_footprint_long_run)'` 绿；无 env 时 skipped；三算法字段含 committed/reusable。
- Stop: T5.4 验证通过，checkpoint/evidence 更新，然后进入 T5.5。

## Evidence refs

详见 `90-evidence.md`。P0/P1/P2/P3/P4、T5.1、T5.2、T5.3 已完成；T5.4 正在进行。

## Blocked-on items

无。

## ResumeStateHint

恢复时先执行：

1. 读取本文件、`90-evidence.md` 与父计划 T5.4。
2. 新增 footprint hist ring buffer 与 long-run reuse benchmark。
3. 完成后运行 footprint bench、workspace/build 必要验证。

# DriftCheckDraft

- Does current work still serve original task intent? 是，T5.3 已完成 pause benchmark，当前进入 footprint 指标。
- Does current work still serve goal and stop condition? 是，T5.4 只交付 footprint hist/long-run，不提前做回归矩阵审计。
- Compatibility boundary: footprint bench 默认 skipped，只有 `WJSM_GC_BENCH=1` 执行。
- New owner/fallback/adapter/branch: runtime footprint hist + `gc_footprint_long_run.rs` 成为 memory footprint 治理验收 owner。
- Retirement track: footprint 未验收限制开始退休；T5.5 后续审计 fixture 覆盖面。
- Evidence sufficiency: T5.3 sufficient；T5.4 pending。
- Decision: continue。
