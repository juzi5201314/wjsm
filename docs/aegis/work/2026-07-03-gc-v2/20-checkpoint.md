# TodoCheckpointDraft

## Current todo

- Active: `T6.2 同步文档描述`（in progress）。
- Next: `T6.3 新增 ADR 0005`。

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
- P6:
  - `T6.1 执行删除清单`

## Active slice card

- Goal: P6 T6.2，同步项目文档描述：WASM contract globals/host funcs/support helpers/GC 三算法选择，以及 N-API 设计文档中的 non-moving 描述改为 handle 恒定（INV-C1）。
- Parent plan/spec: `docs/aegis/plans/2026-07-03-pluggable-gc-v2.md` T6.2；`docs/aegis/specs/2026-07-03-pluggable-gc-v2-design.md` §15.2。
- Files: `AGENTS.md`、`docs/aegis/specs/2026-07-03-napi-native-addon-design.md`，必要时只改相关段落。
- Boundary: 本 slice 只同步已有实现事实，不新增架构决策（ADR 留给 T6.3）。
- Verification: 文档中的 globals/host funcs/support helper 数量与实现核对；grep 确认 “non-moving” 残留只在历史/无关上下文。
- Stop: T6.2 验证通过，checkpoint/evidence 更新，然后进入 T6.3。

## Evidence refs

详见 `90-evidence.md`。P0/P1/P2/P3/P4/P5、T6.1 已完成；T6.2 正在进行。

## Blocked-on items

无。

## ResumeStateHint

恢复时先执行：

1. 读取本文件、`90-evidence.md` 与父计划 T6.2。
2. 核对 `module_setup.rs` / host import tables / support ABI 的实数，再同步 `AGENTS.md` 与 N-API spec。
3. 完成后运行文档 grep/相关 build check。

# DriftCheckDraft

- Does current work still serve original task intent? 是，T6.1 已完成代码残留清理，当前进入文档同步。
- Does current work still serve goal and stop condition? 是，T6.2 只同步描述，不提前写 ADR。
- Compatibility boundary: 文档必须反映当前实现，不改变代码行为。
- New owner/fallback/adapter/branch: 无新增 owner。
- Retirement track: 旧文档中单算法/non-moving/旧 globals 描述开始退休。
- Evidence sufficiency: T6.1 sufficient；T6.2 pending。
- Decision: continue。
