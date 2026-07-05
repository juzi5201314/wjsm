# TodoCheckpointDraft

## Current todo

- Active: `T5.1 实现三入口选择`（in progress）。
- Next: `T5.2 完善 GcStats v2`。

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
## Active slice card

- Goal: P5 T5.1，实现三入口 GC 算法选择：`RuntimeOptions::with_gc_algorithm`、env `WJSM_GC` 默认链、CLI `--gc <mark-sweep|g1|zgc>`，优先级 CLI > env > 默认。
- Parent plan/spec: `docs/aegis/plans/2026-07-03-pluggable-gc-v2.md` T5.1。
- Files: `crates/wjsm-runtime/src/lib.rs`、`crates/wjsm-cli/src/lib.rs`、CLI/in-process tests。
- Boundary: 本 slice 只实现选择机制；GcStats v2、pause hist、benchmark 留给 T5.2+。
- Verification: CLI 集成测试覆盖三入口优先级与非法值列合法值；手验/测试 `wjsm run --gc g1 fixtures/happy/hello.js`。
- Stop: T5.1 验证通过，checkpoint/evidence 更新，然后进入 T5.2。

## Evidence refs

详见 `90-evidence.md`。P0/P1/P2/P3/P4 已完成；T5.1 正在进行。

## Blocked-on items

无。

## ResumeStateHint

恢复时先执行：

1. 读取本文件、`90-evidence.md` 与父计划 T5.1。
2. 实现 RuntimeOptions builder、`WJSM_GC` 解析、CLI `--gc` 与优先级测试。
3. 完成后运行 CLI/runtime 相关测试、三算法 hello 冒烟与必要 build/check。

# DriftCheckDraft

- Does current work still serve original task intent? 是，P4 已完成并验证，当前进入 P5 选择机制与可观测性。
- Does current work still serve goal and stop condition? 是，T5.1 只交付三入口选择，不提前实现 stats/bench。
- Compatibility boundary: 现有 `WJSM_TEST_GC` 测试矩阵入口保持；新增 `WJSM_GC` 与 CLI `--gc` 对用户路径生效。
- New owner/fallback/adapter/branch: `RuntimeOptions` 成为算法选择 source of truth；CLI/env 只负责填充 options。
- Retirement track: 测试专用 `WJSM_TEST_GC` 单入口假设开始退休，后续保留为测试覆盖入口或并入默认链。
- Evidence sufficiency: T4.6 sufficient；T5.1 pending。
- Decision: continue。
