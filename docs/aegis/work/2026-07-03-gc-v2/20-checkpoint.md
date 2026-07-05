# TodoCheckpointDraft

## Current todo

- Active: `T3.6 实现 G1 mixed GC`（in progress）。
- Next: `T3.7 组装 G1 registry`。

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

## Active slice card

- Goal: P3 T3.6，实现 G1 mixed GC：按 live bytes/收益选择 CSet，STW old→old evacuation，更新 obj_table 并回收/压缩 region。
- Parent plan/spec: `docs/aegis/plans/2026-07-03-pluggable-gc-v2.md`；`docs/aegis/specs/2026-07-03-pluggable-gc-v2-design.md` §10.5。
- Files: 新建 `g1/mixed.rs`，更新 `g1/mod.rs`、`g1/region.rs`、必要的 stats/fragmentation 可观测 helper。
- Boundary: 本 slice 只实现 mixed old evacuation；dead handle cleanup 已由 concurrent mark 负责，mixed 不再次发布 dead handles；不做 per-reference 修正。
- Verification: mixed 单测覆盖 CSet budget 截断、85% live 阈值、evacuate 后引用槽无需改写仍读新对象、目的 card re-dirty、mixed 不重复发布 dead handle；`gc_fragmentation_churn` 在 g1 下保持通过并记录 fragmentation 下降路径。
- Stop: T3.6 验证通过，checkpoint/evidence 更新，然后进入 T3.7。

## Evidence refs

详见 `90-evidence.md`。P0/P1/P2、T3.0、T3.1、T3.2、T3.3、T3.4、T3.5 已完成；T3.6 正在进行。

## Blocked-on items

无。

## ResumeStateHint

恢复时先执行：

1. 读取本文件、`90-evidence.md` 与父计划 T3.6 / spec §10.5。
2. 继续 mixed GC：先实现 CSet 选择与 evacuation helpers，再接入 safepoint/full collect。
3. 完成后运行 mixed 单测、`WJSM_TEST_GC=g1 cargo nextest run -E 'test(happy__gc_fragmentation_churn)'`、`cargo check -p wjsm-runtime` 与 G1 happy 子集。

# DriftCheckDraft

- Does current work still serve original task intent? 是，T3.5 已完成 concurrent mark/cleanup，当前进入 mixed evacuation。
- Does current work still serve goal and stop condition? 是，T3.6 只交付 old CSet compaction，不提前做 T3.7/P4。
- Compatibility boundary: 默认 mark-sweep 行为保持；mixed 只更新 obj_table，不修改引用槽 handle。
- New owner/fallback/adapter/branch: `runtime_gc::g1::mixed` 将成为 old region CSet 选择与 evacuation owner。
- Retirement track: “old/humongous 永不回收”的临时限制已退休；T3.6 开始退休 old region 碎片不压缩限制。
- Evidence sufficiency: T3.5 sufficient；T3.6 pending。
- Decision: continue。
