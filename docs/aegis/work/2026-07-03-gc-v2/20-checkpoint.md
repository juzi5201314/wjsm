# TodoCheckpointDraft

## Current todo

- Active: `T4.2 生成 ZGC support 变体`（in progress）。
- Next: `T4.3 实现 ZGC mark`。

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

## Active slice card

- Goal: P4 T4.2，生成 ZGC support 变体并在全部 helper 解引用点插入 load barrier；写入点复用统一 24B barrier buffer 记录 SATB event；分配序列写入当前 good color。
- Parent plan/spec: `docs/aegis/plans/2026-07-03-pluggable-gc-v2.md` T4.2；`docs/aegis/specs/2026-07-03-pluggable-gc-v2-design.md` §8.3、§11.2。
- Files: `support_module.rs` / `support_object_helpers.rs`、`wjsm-runtime-support/build.rs`、runtime support install/registry 选择路径。
- Boundary: 本 slice 只实现 ZGC support 变体/load barrier/SATB event 与 registry 冒烟；mark/relocate 行为留给 T4.3/T4.4。
- Verification: dump-wat/结构测试抽查 load barrier 序列、SATB event buffer 序列、无 `__satb_ptr`；`WJSM_TEST_GC=zgc cargo nextest run -E 'test(happy__hello)'` 绿。
- Stop: T4.2 验证通过，checkpoint/evidence 更新，然后进入 T4.3。

## Evidence refs

详见 `90-evidence.md`。P0/P1/P2/P3 与 T4.1 已完成；T4.2 正在进行。

## Blocked-on items

无。

## ResumeStateHint

恢复时先执行：

1. 读取本文件、`90-evidence.md` 与父计划 T4.2 / spec §8.3、§11.2。
2. 继续 ZGC support emitter：resolve/load barrier、SATB event、allocate-black，并打开 registry 冒烟。
3. 完成后运行 support 结构测试、`cargo check -p wjsm-backend-wasm -p wjsm-runtime-support -p wjsm-runtime` 与 `WJSM_TEST_GC=zgc cargo nextest run -E 'test(happy__hello)'`。

# DriftCheckDraft

- Does current work still serve original task intent? 是，T4.1 已完成 ZGC color/page owner，当前进入 ZGC support 变体。
- Does current work still serve goal and stop condition? 是，T4.2 只交付 support/load barrier 与 registry 冒烟，不提前实现 mark/relocate。
- Compatibility boundary: 默认 mark-sweep 与 G1 均保持；ZGC registry 只在 support 变体可用后打开。
- New owner/fallback/adapter/branch: backend support emitter 将成为 ZGC load barrier/SATB event/allocate-black 生成 owner。
- Retirement track: ZGC registry 拒绝路径将在 T4.2 support 变体可用后退休。
- Evidence sufficiency: T4.1 sufficient；T4.2 pending。
- Decision: continue。
