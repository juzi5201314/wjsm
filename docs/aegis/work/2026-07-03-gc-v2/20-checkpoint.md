# TodoCheckpointDraft

## Current todo

- Active: `T6.1 执行删除清单`（in progress）。
- Next: `T6.2 同步文档描述`。

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
  - `T4.5 组装 ZGC registry`
  - `T4.6 验证 P4 阶段`
- P5:
  - `T5.1 实现三入口选择`
  - `T5.2 完善 GcStats v2`
  - `T5.3 添加 pause benchmark`
  - `T5.4 添加 footprint 指标`
  - `T5.5 审计 GC 回归矩阵`
  - `T5.6 验证 P5 阶段`

## Active slice card

- Goal: P6 T6.1，执行删除清单：按 spec §18 与计划 grep 复核 v1 trait 名、旧 `gc_maybe_collect`、WIP 残留与不再需要的 `#[allow(dead_code)]`。
- Parent plan/spec: `docs/aegis/plans/2026-07-03-pluggable-gc-v2.md` T6.1；`docs/aegis/specs/2026-07-03-pluggable-gc-v2-design.md` §18。
- Files: runtime/backend/support 代码与测试；只删除确认由 v2/三算法取代的内部残留。
- Boundary: 本 slice 只做代码清理/残留复核；文档同步留给 T6.2，ADR 留给 T6.3。
- Verification: grep 记录、`cargo check` / targeted tests / build 零 warning。
- Stop: T6.1 验证通过，checkpoint/evidence 更新，然后进入 T6.2。

## Evidence refs

详见 `90-evidence.md`。P0/P1/P2/P3/P4/P5 已完成；T6.1 正在进行。

## Blocked-on items

无。

## ResumeStateHint

恢复时先执行：

1. 读取本文件、`90-evidence.md` 与父计划 T6.1 / spec §18。
2. grep 复核 v1 trait/旧 collect/WIP/dead_code allow 残留，删除或记录保留理由。
3. 完成后运行清理验证与必要 build/check。

# DriftCheckDraft

- Does current work still serve original task intent? 是，P5 已完成并验证，当前进入 P6 清理。
- Does current work still serve goal and stop condition? 是，T6.1 只执行删除清单，不提前同步文档/ADR。
- Compatibility boundary: 默认/G1/ZGC 行为不得改变；清理只删除已由 v2 取代的内部残留。
- New owner/fallback/adapter/branch: 无新增 owner。
- Retirement track: P6 开始执行旧接口/旧路径/临时 allow 的最终退休。
- Evidence sufficiency: T5.6 sufficient；T6.1 pending。
- Decision: continue。
