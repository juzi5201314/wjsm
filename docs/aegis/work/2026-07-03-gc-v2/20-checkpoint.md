# TodoCheckpointDraft

## Current todo

- Active: `T3.4 实现 G1 young GC`（in progress）。
- Next: `T3.5 实现 G1 concurrent mark`。

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

## Active slice card

- Goal: P3 T3.4，实现 G1 STW young GC：young roots、复制/晋升、obj_table 更新、dirty card re-dirty、weak/side-table cleanup 与 handle 复用协议。
- Parent plan/spec: `docs/aegis/plans/2026-07-03-pluggable-gc-v2.md`；`docs/aegis/specs/2026-07-03-pluggable-gc-v2-design.md` §10.3、§14。
- Files: 新建 `g1/young.rs`；新增/扩展 `runtime_gc/roots.rs` 与 `runtime_gc/object_walker.rs`，更新 `g1/mod.rs` 与相关测试/fixture。
- Boundary: 本 slice 只实现 STW young GC；不实现 concurrent mark、mixed GC 或 ZGC。复制期间不走 `alloc_slow`，避免 INV-C2 额外 GC 点。
- Verification: young 单测覆盖 age 晋升、survivor 溢出、humongous 不动、dirty card re-dirty、晋升目的 card re-dirty、immortal 扫描跳过 padding/abandoned、oblet 拆分不漏槽、WeakRef cleanup 先于 handle 复用；新增 `gc_g1_young_churn.js` fixture 并跑 `WJSM_TEST_GC=g1`。
- Stop: T3.4 验证通过，checkpoint/evidence 更新，然后进入 T3.5。

## Evidence refs

详见 `90-evidence.md`。P0/P1/P2、T3.0、T3.1、T3.2、T3.3 已完成；T3.4 正在进行。

## Blocked-on items

无。

## ResumeStateHint

恢复时先执行：

1. 读取本文件、`90-evidence.md` 与父计划 T3.4 / spec §10.3、§14。
2. 继续 young GC：先建立统一 object walker 与 immortal/dirty-card root 扫描，再实现 young copy/promote。
3. 完成后运行 young 单测、新 fixture、`cargo check -p wjsm-runtime` 与 `WJSM_TEST_GC=g1 cargo nextest run -E 'test(happy__)'`。

# DriftCheckDraft

- Does current work still serve original task intent? 是，T3.3 已完成 G1 support event 生成，当前进入 G1 young GC。
- Does current work still serve goal and stop condition? 是，T3.4 只交付 STW young GC，不提前实现 concurrent mark/mixed。
- Compatibility boundary: 默认 mark-sweep 行为保持；G1 young GC 必须遵守 handle-only 引用与 obj_table 更新契约。
- New owner/fallback/adapter/branch: `runtime_gc::g1::young` 将成为 young evacuation owner；`object_walker` 成为对象引用槽遍历 owner。
- Retirement track: 单一 support cwasm 假设已退休；T3.4 开始退休 G1 仅委托 mark-sweep 全收集的临时路径。
- Evidence sufficiency: T3.3 sufficient；T3.4 pending。
- Decision: continue。
