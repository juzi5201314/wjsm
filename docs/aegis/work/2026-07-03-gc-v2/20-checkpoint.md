# TodoCheckpointDraft

## Current todo

- Active: `T5.2 完善 GcStats v2`（in progress）。
- Next: `T5.3 添加 pause benchmark`。

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

## Active slice card

- Goal: P5 T5.2，补全 `GcStats` v2 字段与 pause 直方图：字段覆盖 spec §17，runtime 记录最近 256 次 pause，`WJSM_GC_LOG=1` 输出周期摘要。
- Parent plan/spec: `docs/aegis/plans/2026-07-03-pluggable-gc-v2.md` T5.2；`docs/aegis/specs/2026-07-03-pluggable-gc-v2-design.md` §17。
- Files: `runtime_gc/api.rs`、`runtime_gc/*` 各算法填充点、`lib.rs` runtime state pause hist、必要 tests。
- Boundary: 本 slice 只实现统计字段/直方图/log；pause benchmark 与定量达标调参留给 T5.3+。
- Verification: 单测覆盖直方图环形语义；`WJSM_GC_LOG=1` 三算法各跑 churn/hello，摘要字段存在且 mark/relocate load barrier hits 可区分。
- Stop: T5.2 验证通过，checkpoint/evidence 更新，然后进入 T5.3。

## Evidence refs

详见 `90-evidence.md`。P0/P1/P2/P3/P4 与 T5.1 已完成；T5.2 正在进行。

## Blocked-on items

无。

## ResumeStateHint

恢复时先执行：

1. 读取本文件、`90-evidence.md` 与父计划 T5.2 / spec §17。
2. 补全 `GcStats` 字段、pause hist ring buffer、`WJSM_GC_LOG=1` 摘要。
3. 完成后运行 runtime stats 单测、三算法 log 冒烟与必要 workspace/build。

# DriftCheckDraft

- Does current work still serve original task intent? 是，T5.1 已完成三入口选择，当前进入 stats/可观测性。
- Does current work still serve goal and stop condition? 是，T5.2 只交付 stats/log，不提前做 benchmark。
- Compatibility boundary: 三算法选择路径保持；新增 log 只在 `WJSM_GC_LOG=1` 时输出。
- New owner/fallback/adapter/branch: `GcStats` + runtime pause hist 成为 GC 可观测性 source of truth。
- Retirement track: 统计字段不完整限制开始退休；后续 T5.3 benchmark 消费这些字段。
- Evidence sufficiency: T5.1 sufficient；T5.2 pending。
- Decision: continue。
