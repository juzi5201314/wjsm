# TodoCheckpointDraft

## Current todo

- Active: `T4.6 验证 P4 阶段`（in progress）。
- Next: `T5.1 实现三入口选择`。

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
## Active slice card

- Goal: P4 T4.6，执行 ZGC 阶段矩阵终验，确认默认 mark-sweep、`WJSM_TEST_GC=g1` 与 `WJSM_TEST_GC=zgc` 关键路径均绿。
- Parent plan/spec: `docs/aegis/plans/2026-07-03-pluggable-gc-v2.md` T4.6。
- Files: 仅更新 evidence/checkpoint；如验证失败则回到对应 owner 修复。
- Boundary: 本 slice 不新增功能，只做 P4 阶段验收与漂移检查。
- Verification: `WJSM_TEST_GC=zgc cargo nextest run --workspace` 全量绿 + GC 子集绿；默认/G1 回归冒烟；build 零 warning。
- Stop: T4.6 验证通过，checkpoint/evidence 更新，然后进入 P5/T5.1。

## Evidence refs

详见 `90-evidence.md`。P0/P1/P2/P3、T4.1、T4.2、T4.3、T4.4、T4.5 已完成；T4.6 正在进行。

## Blocked-on items

无。

## ResumeStateHint

恢复时先执行：

1. 读取本文件、`90-evidence.md` 与父计划 T4.6。
2. 执行 P4 矩阵终验：ZGC workspace + GC 子集、默认 workspace、G1 happy、build 零 warning。
3. 完成后记录 P4 closure 并进入 T5.1。

# DriftCheckDraft

- Does current work still serve original task intent? 是，T4.5 已完成 ZGC 组装收口，当前进入 P4 验收。
- Does current work still serve goal and stop condition? 是，T4.6 只验证 P4，不提前实现 P5 observability。
- Compatibility boundary: 默认 mark-sweep、G1 与 ZGC 三路径均需保持绿。
- New owner/fallback/adapter/branch: 无新增 owner。
- Retirement track: ZGC 残余组装缺口已清理；T4.6 确认 P4 无未退休路径。
- Evidence sufficiency: T4.5 sufficient；T4.6 pending。
- Decision: continue。
