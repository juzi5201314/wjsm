# TodoCheckpointDraft

## Current todo

- Active: `T3.1 实现 G1 region`（in progress）。
- Next: `T3.2 实现 G1 rset barrier`。

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

## Active slice card

- Goal: P3 T3.1，新增 G1 region 域组织与 `attach_heap`，让 registry 可创建 G1 并在 `WJSM_TEST_GC=g1` 下跑通 hello。
- Parent plan/spec: `docs/aegis/plans/2026-07-03-pluggable-gc-v2.md`；`docs/aegis/specs/2026-07-03-pluggable-gc-v2-design.md` §10.1、§10.2、§15.1。
- Files: 新建 `crates/wjsm-runtime/src/runtime_gc/g1/{mod.rs,region.rs}`，更新 `runtime_gc/mod.rs` 与 `runtime_gc/registry.rs`。
- Boundary: 本 slice 只实现 region metadata、索引计算、attach/grow/region 分配归还和 G1 最小算法骨架；RSet/barrier/young/concurrent/mixed 留给 T3.2+。
- Verification: region 单测覆盖域划分边界、humongous、immortal、card/region 索引、grow 扩展、无 Meta region、hello 不预留 8MiB card table；`WJSM_TEST_GC=g1 cargo nextest run -E 'test(happy__hello)'` 绿。
- Stop: T3.1 验证通过，checkpoint/evidence 更新，然后进入 T3.2。

## Evidence refs

详见 `90-evidence.md`。P0/P1/P2 与 T3.0 已完成；T3.1 正在进行。

## Blocked-on items

无。

## ResumeStateHint

恢复时先执行：

1. 读取本文件、`90-evidence.md` 与父计划 T3.1 / spec §10.1。
2. 继续 G1 region metadata 与 registry 接入；T3.1 不实现 RSet/barrier/young GC。
3. 完成后运行 region 单测与 `WJSM_TEST_GC=g1 cargo nextest run -E 'test(happy__hello)'`。

# DriftCheckDraft

- Does current work still serve original task intent? 是，T3.0 已建立矩阵入口，当前进入 G1 region owner。
- Does current work still serve goal and stop condition? 是，T3.1 只交付 region/attach/registry 冒烟，不提前实现 RSet 或 young GC。
- Compatibility boundary: 默认 mark-sweep 行为保持；`g1` 在 T3.1 起 registry 可创建，但仅具备 region 域组织与 mark-sweep 兼容收集骨架。
- New owner/fallback/adapter/branch: 新 owner 将是 `runtime_gc::g1::region` host-side metadata；metadata 不进入 wasm dynamic heap。
- Retirement track: 单算法测试假设已退休；T3.1 开始退休 G1 registry 拒绝路径。
- Evidence sufficiency: T3.0 sufficient；T3.1 pending。
- Decision: continue。
