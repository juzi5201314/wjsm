# TodoCheckpointDraft

## Current todo

- Active: `T3.3 生成 G1 support 变体`（in progress）。
- Next: `T3.4 实现 G1 young GC`。

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

## Active slice card

- Goal: P3 T3.3，生成 G1 support 变体并在 G1 flavor 的 `obj_set`/`elem_set` 等引用槽写入点插入 24B barrier event 序列。
- Parent plan/spec: `docs/aegis/plans/2026-07-03-pluggable-gc-v2.md`；`docs/aegis/specs/2026-07-03-pluggable-gc-v2-design.md` §8.2、§10.2。
- Files: `support_module.rs` / `support_object_helpers.rs`、`wjsm-runtime-support/build.rs`、runtime support install 路径与 eval flavor 传递。
- Boundary: 本 slice 只生成/安装 G1 support module event 序列；young/concurrent/mixed GC 行为仍留给 T3.4+。
- Verification: dump-wat/结构测试证明 G1 变体写入统一 barrier event 序列且无 `__card_table_base`/`__region_meta_base`；`WJSM_TEST_GC=g1` 跑 happy 子集冒烟。
- Stop: T3.3 验证通过，checkpoint/evidence 更新，然后进入 T3.4。

## Evidence refs

详见 `90-evidence.md`。P0/P1/P2、T3.0、T3.1、T3.2 已完成；T3.3 正在进行。

## Blocked-on items

无。

## ResumeStateHint

恢复时先执行：

1. 读取本文件、`90-evidence.md` 与父计划 T3.3 / spec §8.2。
2. 继续 support emitter flavor 化与 G1 event 序列；不要实现 young GC。
3. 完成后运行 support 结构测试、`cargo check -p wjsm-backend-wasm -p wjsm-runtime-support -p wjsm-runtime` 与 `WJSM_TEST_GC=g1 cargo nextest run -E 'test(happy__)'` 子集。

# DriftCheckDraft

- Does current work still serve original task intent? 是，T3.2 已完成 host-side barrier owner，当前进入 WASM support event 生成。
- Does current work still serve goal and stop condition? 是，T3.3 只交付 support 变体与 barrier event 序列，不提前实现 young GC。
- Compatibility boundary: 默认 mark-sweep support 保持；G1 support 只在 `WJSM_TEST_GC=g1` / runtime flavor 选择时安装。
- New owner/fallback/adapter/branch: support emitter 按 GC flavor 成为 WASM 写屏障事件生成 owner；runtime `barrier_flush` 是唯一 event consumer。
- Retirement track: host 写无 barrier 记录已退休；T3.3 开始退休单一 support cwasm 假设。
- Evidence sufficiency: T3.2 sufficient；T3.3 pending。
- Decision: continue。
