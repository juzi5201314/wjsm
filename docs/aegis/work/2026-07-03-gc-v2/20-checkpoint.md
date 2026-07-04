# TodoCheckpointDraft

## Current todo

- Active: `T3.2 实现 G1 rset barrier`（in progress）。
- Next: `T3.3 生成 G1 support 变体`。

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

## Active slice card

- Goal: P3 T3.2，新增 G1 RSet/SATB host-side barrier 与 event buffer flush，确保 host 写与 WASM event 都进入统一脏卡/old-value 管线。
- Parent plan/spec: `docs/aegis/plans/2026-07-03-pluggable-gc-v2.md`；`docs/aegis/specs/2026-07-03-pluggable-gc-v2-design.md` §8.2、§10.2、§13。
- Files: 新建 `crates/wjsm-runtime/src/runtime_gc/g1/rset.rs`，更新 `g1/mod.rs`，必要时扩充 `GcContext` barrier buffer base/reset 辅助。
- Boundary: 本 slice 实现 host hook、24B event 编解码、dirty-card sparse set 与 precise-slot 升级；不生成 WASM 侧 event 写入序列（T3.3），不执行 young GC（T3.4）。
- Verification: RSet 单测覆盖 card 索引、sparse dirty 迭代、热点 precise-slot 升级、SATB value→handle、24B event 编码、flush 后 ptr 复位、满 24KB 边界 old→young 不漏、slot_addr 反查 owner region、host 写精确标 card。
- Stop: T3.2 验证通过，checkpoint/evidence 更新，然后进入 T3.3。

## Evidence refs

详见 `90-evidence.md`。P0/P1/P2、T3.0、T3.1 已完成；T3.2 正在进行。

## Blocked-on items

无。

## ResumeStateHint

恢复时先执行：

1. 读取本文件、`90-evidence.md` 与父计划 T3.2 / spec §8.2。
2. 继续 G1 RSet/SATB host barrier 与 barrier buffer flush；不要改 support emitter。
3. 完成后运行 RSet 单测、`cargo check -p wjsm-runtime` 与 `WJSM_TEST_GC=g1 cargo nextest run -E 'test(happy__hello)'`。

# DriftCheckDraft

- Does current work still serve original task intent? 是，T3.1 已完成 G1 region/registry，当前进入 RSet/barrier。
- Does current work still serve goal and stop condition? 是，T3.2 只交付 host-side barrier/flush，不提前生成 WASM G1 support。
- Compatibility boundary: 默认 mark-sweep 行为保持；G1 barrier 数据仅在 `G1Collector` 内维护。
- New owner/fallback/adapter/branch: `runtime_gc::g1::rset` 成为 dirty-card/SATB/event 解码 owner；`heap_access` 仍是 host 写入口 owner。
- Retirement track: G1 registry 拒绝路径已退休；T3.2 开始退休“host 写无 barrier 记录”的假设。
- Evidence sufficiency: T3.1 sufficient；T3.2 pending。
- Decision: continue。
