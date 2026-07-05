# TodoCheckpointDraft

## Current todo

- Active: `T6.4 执行全矩阵终验`（in progress）。
- Next: complete。

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
  - `T5.4 添加 footprint 指标`
  - `T5.5 审计 GC 回归矩阵`
  - `T5.6 验证 P5 阶段`
- P6:
  - `T6.1 执行删除清单`
  - `T6.2 同步文档描述`
  - `T6.3 新增 ADR 0005`

## Active slice card

- Goal: P6 T6.4，执行全矩阵终验并将计划状态头更新为完成：默认全量、G1 全量、ZGC 全量、startup snapshot 双路、`WJSM_GC_BENCH=1` 定量、零 warning。
- Parent plan/spec: `docs/aegis/plans/2026-07-03-pluggable-gc-v2.md` T6.4。
- Files: 计划/evidence/checkpoint；如验证失败则回到对应 owner 修复。
- Boundary: 本 slice 不新增功能，只做最终验收、状态头更新与提交。
- Verification: `cargo nextest run --workspace`；`WJSM_TEST_GC=g1 cargo nextest run --workspace`；`WJSM_TEST_GC=zgc cargo nextest run --workspace`；startup snapshot on/off happy subset；`WJSM_GC_BENCH=1` pause+footprint；`cargo build` 零 warning。
- Stop: T6.4 验证通过，更新 plan status 为完成并提交最终收口。

## Evidence refs

详见 `90-evidence.md`。P0/P1/P2/P3/P4/P5、T6.1、T6.2、T6.3 已完成；T6.4 正在进行。

## Blocked-on items

无。

## ResumeStateHint

恢复时先执行：

1. 执行全矩阵终验（默认/G1/ZGC workspace，snapshot on/off，bench，build）。
2. 更新计划状态头为完成，记录最终 evidence。
3. 提交 T6.4 最终收口。

# DriftCheckDraft

- Does current work still serve original task intent? 是，T6.3 已完成 ADR，当前进入最终验收。
- Does current work still serve goal and stop condition? 是，T6.4 只执行终验与状态更新。
- Compatibility boundary: 默认/G1/ZGC、snapshot 双路、bench gate 均需通过。
- New owner/fallback/adapter/branch: 无新增 owner。
- Retirement track: ADR 已记录 v2 边界；T6.4 确认所有阶段无未退休路径。
- Evidence sufficiency: T6.3 sufficient；T6.4 pending。
- Decision: continue。
