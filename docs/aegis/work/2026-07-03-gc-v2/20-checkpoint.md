# TodoCheckpointDraft

## Current todo

- Active: `T4.3 实现 ZGC mark`（in progress）。
- Next: `T4.4 实现 ZGC relocate`。

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

## Active slice card

- Goal: P4 T4.3，实现 ZGC 增量 mark：MarkStart STW good=本周期 mark 色 + root snapshot，增量 drain，load barrier 协助标记入 worklist，MarkEnd STW drain SATB/侧表 fixed-point，生成 dead_handle_set 并按 §14 cleanup。
- Parent plan/spec: `docs/aegis/plans/2026-07-03-pluggable-gc-v2.md` T4.3；`docs/aegis/specs/2026-07-03-pluggable-gc-v2-design.md` §11.3、§12、§14。
- Files: 新建 `runtime_gc/zgc/mark.rs`，更新 `zgc/mod.rs`、必要的 page/color/roots/object_walker helper。
- Boundary: 本 slice 只实现 mark/dead handle cleanup；relocate/heal 留给 T4.4。
- Verification: 单测覆盖坏色命中标记、SATB 覆盖写场景、dead_handle_set 完整性、WeakRef/FinalizationRegistry 先 cleanup 再 handle 复用；`WJSM_TEST_GC=zgc cargo nextest run -E 'test(happy__)'`。
- Stop: T4.3 验证通过，checkpoint/evidence 更新，然后进入 T4.4。

## Evidence refs

详见 `90-evidence.md`。P0/P1/P2/P3、T4.1、T4.2 已完成；T4.3 正在进行。

## Blocked-on items

无。

## ResumeStateHint

恢复时先执行：

1. 读取本文件、`90-evidence.md` 与父计划 T4.3 / spec §11.3、§14。
2. 继续 ZGC mark：root snapshot + SATB worklist + dead handle cleanup owner。
3. 完成后运行 ZGC mark 单测、`cargo check -p wjsm-runtime`、`WJSM_TEST_GC=zgc cargo nextest run -E 'test(happy__)'`。

# DriftCheckDraft

- Does current work still serve original task intent? 是，T4.2 已完成 ZGC support/load barrier 与 registry 冒烟，当前进入 mark。
- Does current work still serve goal and stop condition? 是，T4.3 只交付 mark/dead-handle cleanup，不提前实现 relocate。
- Compatibility boundary: 默认 mark-sweep、G1 与 ZGC happy 路径保持；ZGC relocation 仍不启用。
- New owner/fallback/adapter/branch: `runtime_gc::zgc::mark` 将成为 ZGC mark bitmap/worklist/dead_handle_set owner。
- Retirement track: ZGC registry 拒绝路径已退休；T4.3 开始退休 ZGC 仅委托 mark-sweep full collect 的临时路径。
- Evidence sufficiency: T4.2 sufficient；T4.3 pending。
- Decision: continue。
