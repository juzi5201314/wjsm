# TodoCheckpointDraft

## Current todo

- Active: `T3.0 接入测试矩阵`（next）。
- Next: `T3.1 实现 G1 region`。

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

## Active slice card

- Goal: P3 T3.0，接入 `WJSM_TEST_GC` 测试矩阵入口，为 mark-sweep / g1 / zgc 分层验证提供统一选择路径。
- Parent plan/spec: `docs/aegis/plans/2026-07-03-pluggable-gc-v2.md`；`docs/aegis/specs/2026-07-03-pluggable-gc-v2-design.md` §6、§21、§22。
- Files: 预期涉及测试 harness / CLI runtime 选择路径；先沿现有 GC flavor registry 与 fixture runner 查证后改动。
- Boundary: 本 slice 只建立测试矩阵选择与非法值诊断；不实现 G1 region/card/young 行为。
- Verification: `WJSM_TEST_GC=mark-sweep cargo nextest run -E 'test(happy__hello)'` 绿；`WJSM_TEST_GC=bogus` 报错列合法值。
- Stop: T3.0 验证通过，checkpoint/evidence 更新，然后进入 T3.1。

## Evidence refs

详见 `90-evidence.md`。P0/P1/P2 已完成；T3.0 尚未产生实现后证据。

## Blocked-on items

无。

## ResumeStateHint

恢复时先执行：

1. 读取本文件、`90-evidence.md` 与父计划 P3。
2. 从 T3.0 测试矩阵入口开始，先定位现有 `GcFlavor` / fixture runner / CLI 选择路径。
3. 每个 P3 slice 后更新证据与 drift check；G1 行为实现前只允许 mark-sweep 矩阵冒烟通过。

# DriftCheckDraft

- Does current work still serve original task intent? 是，P2 已完成并进入 P3 G1 前置矩阵。
- Does current work still serve goal and stop condition? 是，T3.0 只建立算法选择测试入口，不伪造 G1 行为。
- Compatibility boundary: 默认 mark-sweep 行为保持；非法 `WJSM_TEST_GC` 必须显式报错并列合法值。
- New owner/fallback/adapter/branch: P2 新 owner `runtime_gc::heap_access` 已接管 host 引用槽写；P3 将继续通过 registry 接入算法实现，不新增 fallback。
- Retirement track: P2 host 裸写点已勾销；P3 开始退休单算法测试假设。
- Evidence sufficiency: P2 sufficient；T3.0 pending。
- Decision: continue。
