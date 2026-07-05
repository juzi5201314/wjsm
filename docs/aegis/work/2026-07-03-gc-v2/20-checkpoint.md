# TodoCheckpointDraft

## Current todo

- Active: `T4.4 实现 ZGC relocate`（in progress）。
- Next: `T4.5 组装 ZGC registry`。

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
  - `T4.3 实现 ZGC mark`

## Active slice card

- Goal: P4 T4.4，实现 ZGC 增量 relocation + 强制 heal：选择 relocation set，主动搬迁与 load barrier 协助搬迁，更新 `obj_table[h]=new|11`，源 page 搬完后归还空间。
- Parent plan/spec: `docs/aegis/plans/2026-07-03-pluggable-gc-v2.md` T4.4；`docs/aegis/specs/2026-07-03-pluggable-gc-v2-design.md` §11.3、§12、§14。
- Files: 新建 `runtime_gc/zgc/relocate.rs`，更新 `zgc/mod.rs`、`zgc/page.rs`、必要的 heap access/on_host_resolve 测试。
- Boundary: 本 slice 只实现 relocation/heal；dead handles 仍只在 MarkEnd cleanup 后发布，RelocateStep/page reclaim 不发布 handles。
- Verification: 单测覆盖 RS 选择（live=0 已回收、fragmentation>25%、预算截断）、host 读/写 RS 内对象时同步 heal、`obj_table` remapped entry、源 page 回收不重复发布 handle；`WJSM_TEST_GC=zgc` fixture 冒烟。
- Stop: T4.4 验证通过，checkpoint/evidence 更新，然后进入 T4.5。

## Evidence refs

详见 `90-evidence.md`。P0/P1/P2/P3、T4.1、T4.2、T4.3 已完成；T4.4 正在进行。

## Blocked-on items

无。

## ResumeStateHint

恢复时先执行：

1. 读取本文件、`90-evidence.md` 与父计划 T4.4 / spec §11.3、§14。
2. 继续 ZGC relocate：先实现 relocation set/page allocator/heal owner，再接入 load_barrier_slow 与 safepoint/full collect。
3. 完成后运行 ZGC relocate 单测、`cargo check -p wjsm-runtime`、`WJSM_TEST_GC=zgc cargo nextest run -E 'test(happy__)'`。

# DriftCheckDraft

- Does current work still serve original task intent? 是，T4.3 已完成 ZGC mark/dead-handle cleanup，当前进入 relocation。
- Does current work still serve goal and stop condition? 是，T4.4 只交付 relocate/heal，不提前做 T4.5/P5。
- Compatibility boundary: 默认 mark-sweep、G1 与 ZGC mark 路径保持；RelocateStep 不发布 dead handles。
- New owner/fallback/adapter/branch: `runtime_gc::zgc::relocate` 将成为 relocation set、copy/heal 与 source page reclaim owner。
- Retirement track: ZGC 仅委托 mark-sweep full collect 的临时路径已退休；T4.4 开始退休 ZGC 不搬迁限制。
- Evidence sufficiency: T4.3 sufficient；T4.4 pending。
- Decision: continue。
