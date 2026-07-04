# TodoCheckpointDraft

## Current todo

- Active: `T1.5 参数化 support emitter`
- Next: 开始 T1.5，将 support emitter 按 GC flavor 参数化，当前 mark-sweep 变体保持默认行为并预留 g1/zgc 变体入口。

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

## Active slice card

- Goal: P1 T1.5，参数化 support emitter：将当前单一 `emit_support_module()` 扩展为按 GC flavor 选择/生成 support 变体，mark-sweep 作为默认。
- Parent plan/spec: `docs/aegis/plans/2026-07-03-pluggable-gc-v2.md`；`docs/aegis/specs/2026-07-03-pluggable-gc-v2-design.md` §7/§8/§22。
- Files: 预计涉及 `support_module.rs`、`wjsm-runtime-support` build/API、runtime embedded support install 入口与 tests。
- Boundary: 不实现 G1/ZGC 算法体；只建立可插拔变体选择结构。
- Verification: affected cargo check、runtime-support/backend tests、workspace build。
- Stop: T1.5 完成 mark-sweep 变体默认链路与 ABI/test 更新并提交。

## Evidence refs

详见 `90-evidence.md`。T1.4 已有 host import retirement grep、targeted wasmparser/backend/runtime support/runtime 测试与 workspace build 证据；T1.5 待补。

## Blocked-on items

无外部阻塞。

## ResumeStateHint

恢复时先执行：

1. `git status --short` 确认当前切片文件。
2. 阅读本文件与 `90-evidence.md`。
3. 从父计划 T1.5 开始，先定位 support module 生成/嵌入/install 的 canonical owner，再参数化 GC flavor。
4. 每完成子切片更新本 checkpoint/evidence/drift 记录。

# DriftCheckDraft

- Does current work still serve original task intent? 是，当前已推进到 P1 support emitter 参数化。
- Does current work still serve goal and stop condition? 是，T1.4 完成 host import 换代且未扩出父计划。
- Compatibility boundary: env global ABI 已从 20 扩到 27；host import 旧入口已在 crates 下零命中；support host import 数量锁定为 26。
- New owner/fallback/adapter/branch: 未新增 fallback；`gc_safepoint_poll`/`gc_barrier_flush` 是父 spec 的 canonical host import。
- Retirement track: 旧 proactive import 与 RuntimeState alloc counter/threshold 已删除；T1.5 将继续消除单一 support emitter owner。
- Evidence sufficiency: T1.4 足够；T1.5 尚未开始，证据待补。
- Decision: continue。
