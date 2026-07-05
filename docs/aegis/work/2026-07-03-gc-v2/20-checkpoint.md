# TodoCheckpointDraft

## Current todo

- Active: `T3.5 实现 G1 concurrent mark`（in progress）。
- Next: `T3.6 实现 G1 mixed GC`。

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

## Active slice card

- Goal: P3 T3.5，实现 G1 增量 concurrent mark 状态机：IHOP 触发、initial mark、safepoint budget drain、final remark 与 cleanup。
- Parent plan/spec: `docs/aegis/plans/2026-07-03-pluggable-gc-v2.md`；`docs/aegis/specs/2026-07-03-pluggable-gc-v2-design.md` §10.4、§12、§14。
- Files: 新建 `g1/concurrent_mark.rs`，更新 `g1/mod.rs`、`g1/region.rs`、必要的 stats/roots/object_walker helper。
- Boundary: 本 slice 只实现 old/humongous 增量标记与 cleanup；mixed evacuation 留给 T3.6，ZGC 留给 P4。
- Verification: concurrent mark 单测覆盖 SATB 覆盖写旧引用存活、mark 中删除 host side-table 旧 root 仍存活到本周期、implicit-black region 不被 cleanup 回收、WeakRef/FinalizationRegistry 指向 old dead 对象时先清理再复用 handle；新增/运行 `WJSM_TEST_GC=g1` 长循环 fixture。
- Stop: T3.5 验证通过，checkpoint/evidence 更新，然后进入 T3.6。

## Evidence refs

详见 `90-evidence.md`。P0/P1/P2、T3.0、T3.1、T3.2、T3.3、T3.4 已完成；T3.5 正在进行。

## Blocked-on items

无。

## ResumeStateHint

恢复时先执行：

1. 读取本文件、`90-evidence.md` 与父计划 T3.5 / spec §10.4、§12、§14。
2. 继续 concurrent mark：先建立 mark cycle state 与 SATB worklist，再实现 final remark/cleanup。
3. 完成后运行 concurrent mark 单测、相关 fixture、`cargo check -p wjsm-runtime` 与 `WJSM_TEST_GC=g1 cargo nextest run -E 'test(happy__)'`。

# DriftCheckDraft

- Does current work still serve original task intent? 是，T3.4 已完成 STW young GC，当前进入 G1 增量标记。
- Does current work still serve goal and stop condition? 是，T3.5 只交付 concurrent mark，不提前实现 mixed evacuation。
- Compatibility boundary: 默认 mark-sweep 行为保持；G1 mark bitmap/implicit-black metadata 仅由 `g1::concurrent_mark` 维护。
- New owner/fallback/adapter/branch: `runtime_gc::g1::concurrent_mark` 将成为 old/humongous 标记与 cleanup owner；`g1::young` 继续只负责 young evacuation。
- Retirement track: G1 仅委托 mark-sweep 全收集的临时路径已退休；T3.5 开始退休“old/humongous 永不回收”的临时限制。
- Evidence sufficiency: T3.4 sufficient；T3.5 pending。
- Decision: continue。
