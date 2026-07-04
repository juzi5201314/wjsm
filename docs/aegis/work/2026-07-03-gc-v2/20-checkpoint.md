# TodoCheckpointDraft

## Current todo

- Active: `T1.6 验证布局阶段`
- Next: 执行 P1 阶段验证：debug 构建相关全量 fixture、`dump-wat` 抽查 fast-path，以及 smoke 测 `WJSM_STARTUP_SNAPSHOT=0/1`。

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

## Active slice card

- Goal: P1 T1.6，验证布局阶段：确认 T1.1–T1.5 的 layout/global/support/host-import 切换在 cold/hot startup、backend support、runtime package 下可运行。
- Parent plan/spec: `docs/aegis/plans/2026-07-03-pluggable-gc-v2.md`；`docs/aegis/specs/2026-07-03-pluggable-gc-v2-design.md` §7/§8/§22。
- Files: 不预期继续编辑实现文件；若验证暴露缺陷，回到对应 owner 修复。
- Boundary: 只验证 P1；不进入 P2 host 读写层。
- Verification: 父计划 T1.6 命令、`cargo nextest` affected package、startup snapshot 双路 smoke。
- Stop: P1 验证通过并提交 T1.6 evidence/checkpoint，或定位到具体前置任务缺陷并修复。

## Evidence refs

详见 `90-evidence.md`。T1.5 已有 support variant/API cutover、runtime-support/backend/runtime/CLI 与 workspace build 证据；T1.6 待补。

## Blocked-on items

无外部阻塞。

## ResumeStateHint

恢复时先执行：

1. `git status --short` 确认当前切片文件。
2. 阅读本文件与 `90-evidence.md`。
3. 从父计划 T1.6 开始，先执行验证命令，不预先扩展实现范围。
4. 每完成子切片更新本 checkpoint/evidence/drift 记录。

# DriftCheckDraft

- Does current work still serve original task intent? 是，当前已推进到 P1 布局阶段验证。
- Does current work still serve goal and stop condition? 是，T1.5 保持 MarkSweep-only artifact，未产生 G1/ZGC 伪变体。
- Compatibility boundary: support ABI canonical hash 已切到 `support_abi_union_hash`；当前 union 仅含 MarkSweep。
- New owner/fallback/adapter/branch: 未新增 fallback；`LazyLock` 仅用于固定默认 artifact，`OnceLock` 仅用于运行时显式注入。
- Retirement track: 单一无参数 support emitter 与旧 `support_module_layout_hash` API 已退役；T1.6 只做验证。
- Evidence sufficiency: T1.5 足够；T1.6 尚未开始，证据待补。
- Decision: continue。
