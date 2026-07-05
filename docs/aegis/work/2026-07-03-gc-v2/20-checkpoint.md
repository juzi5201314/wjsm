# TodoCheckpointDraft

## Current todo

- Active: `T4.5 组装 ZGC registry`（in progress）。
- Next: `T4.6 验证 P4 阶段`。

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

  - `T4.4 实现 ZGC relocate`
## Active slice card

- Goal: P4 T4.5，审计并收口 `ZgcCollector` 的完整 v2 `GcAlgorithm` 钩子与 registry 接入：alloc/mark/relocate/full/barrier/load-barrier 路径协同一致。
- Parent plan/spec: `docs/aegis/plans/2026-07-03-pluggable-gc-v2.md` T4.5。
- Files: `zgc/mod.rs`、`registry.rs`，必要时只做收口修正。
- Boundary: 本 slice 不新增算法阶段；只确认 ZGC 对外组装、registry 与 residual fallback/未实现路径已干净。
- Verification: ZGC 单测全绿、`WJSM_TEST_GC=zgc cargo nextest run -E 'test(happy__)'` 与 workspace 绿。
- Stop: T4.5 验证通过，checkpoint/evidence 更新，然后进入 T4.6。

## Evidence refs

详见 `90-evidence.md`。P0/P1/P2/P3、T4.1、T4.2、T4.3、T4.4 已完成；T4.5 正在进行。

## Blocked-on items

无。

## ResumeStateHint

恢复时先执行：

1. 读取本文件、`90-evidence.md` 与父计划 T4.5。
2. 审计 ZgcCollector alloc/safepoint/full/barrier/load-barrier 组装、registry Zgc → Ok。
3. 完成后运行 T4.5 指定验证并进入 T4.6 阶段矩阵。

# DriftCheckDraft

- Does current work still serve original task intent? 是，T4.4 已完成 relocation/heal，当前进入 ZGC 组装收口。
- Does current work still serve goal and stop condition? 是，T4.5 只审计/收口 ZGC registry 与 hooks，不提前做 P5。
- Compatibility boundary: 默认 mark-sweep、G1 与 ZGC happy/workspace 路径保持。
- New owner/fallback/adapter/branch: 无新增 owner；ZGC 子模块 owner 已在 T4.1-T4.4 建立。
- Retirement track: ZGC 不搬迁限制已退休；T4.5 清理残余组装缺口。
- Evidence sufficiency: T4.4 sufficient；T4.5 pending。
- Decision: continue。
