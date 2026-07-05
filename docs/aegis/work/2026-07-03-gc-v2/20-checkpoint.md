# TodoCheckpointDraft

## Current todo

- Active: `T3.8 验证 P3 阶段`（in progress）。
- Next: `T4.1 实现 ZGC color page`。

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

## Active slice card

- Goal: P3 T3.8，执行 G1 阶段矩阵终验，确认默认 mark-sweep 与 `WJSM_TEST_GC=g1` 全量路径均绿。
- Parent plan/spec: `docs/aegis/plans/2026-07-03-pluggable-gc-v2.md` T3.8。
- Files: 仅更新 evidence/checkpoint；如验证失败则回到对应 owner 修复。
- Boundary: 本 slice 不新增功能，只做 P3 阶段验收与漂移检查。
- Verification: `cargo nextest run -E 'test(gc_)'`（默认）+ `WJSM_TEST_GC=g1 cargo nextest run --workspace` + 必要 build/check 零 warning。
- Stop: T3.8 验证通过，checkpoint/evidence 更新，然后进入 P4/T4.1。

## Evidence refs

详见 `90-evidence.md`。P0/P1/P2、T3.0、T3.1、T3.2、T3.3、T3.4、T3.5、T3.6、T3.7 已完成；T3.8 正在进行。

## Blocked-on items

无。

## ResumeStateHint

恢复时先执行：

1. 读取本文件、`90-evidence.md` 与父计划 T3.8。
2. 执行 P3 矩阵终验：默认 `test(gc_)`、G1 workspace，全量零 warning。
3. 完成后记录 P3 closure 并进入 T4.1。

# DriftCheckDraft

- Does current work still serve original task intent? 是，T3.7 已完成 G1 组装收口，当前进入 P3 验收。
- Does current work still serve goal and stop condition? 是，T3.8 只验证 P3，不提前实现 ZGC。
- Compatibility boundary: 默认 mark-sweep 与 `WJSM_TEST_GC=g1` 都必须保持全量绿；ZGC 仍未实现。
- New owner/fallback/adapter/branch: 无新增 owner。
- Retirement track: G1 残余组装缺口已清理；T3.8 确认 P3 无未退休路径。
- Evidence sufficiency: T3.7 sufficient；T3.8 pending。
- Decision: continue。
