# TodoCheckpointDraft

## Current todo

- Active: `T4.1 实现 ZGC color page`（in progress）。
- Next: `T4.2 生成 ZGC support 变体`。

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

## Active slice card

- Goal: P4 T4.1，新增 ZGC color/page 基础：色协议、双 good 切换、host-side page metadata、attach live entry recolor 与全死 page 回收前置 owner。
- Parent plan/spec: `docs/aegis/plans/2026-07-03-pluggable-gc-v2.md` T4.1；`docs/aegis/specs/2026-07-03-pluggable-gc-v2-design.md` §11.2、§11.3。
- Files: 新建 `runtime_gc/zgc/{mod,color,page}.rs`，更新 `runtime_gc/mod.rs`、`registry.rs`。
- Boundary: 本 slice 只实现 color/page/attach 与 registry skeleton；load barrier support 变体留给 T4.2，mark/relocate 留给 T4.3/T4.4。
- Verification: color/page 单测覆盖双 good 转移、attach 后 live entry 非 00、host-side page meta grow、不占 wasm dynamic heap、Remapped good、坏色修复、全死 page immediate reclaim、weak cleanup before handle reuse owner helper。
- Stop: T4.1 验证通过，checkpoint/evidence 更新，然后进入 T4.2。

## Evidence refs

详见 `90-evidence.md`。P0/P1/P2/P3 已完成；T4.1 正在进行。

## Blocked-on items

无。

## ResumeStateHint

恢复时先执行：

1. 读取本文件、`90-evidence.md` 与父计划 T4.1 / spec §11.2-§11.3。
2. 实现 ZGC color/page 基础与 registry skeleton；不改 backend support emitter。
3. 完成后运行 `cargo check -p wjsm-runtime`、ZGC color/page 单测与 `WJSM_TEST_GC=zgc cargo nextest run -E 'test(happy__hello)'` 冒烟（若 T4.1 skeleton 允许）。

# DriftCheckDraft

- Does current work still serve original task intent? 是，P3 已完成并验证，当前进入 P4 ZGC。
- Does current work still serve goal and stop condition? 是，T4.1 只交付 color/page 基础，不提前实现 support load barrier/mark/relocate。
- Compatibility boundary: 默认 mark-sweep 与 G1 均保持；ZGC 在 T4.1 起 registry 可创建 skeleton，但不得提供伪 load barrier 行为。
- New owner/fallback/adapter/branch: `runtime_gc::zgc::{color,page}` 将成为 ZGC entry color 与 page metadata owner。
- Retirement track: P3 G1 临时路径已收口；T4.1 开始退休 ZGC registry 拒绝路径。
- Evidence sufficiency: T3.8 sufficient；T4.1 pending。
- Decision: continue。
