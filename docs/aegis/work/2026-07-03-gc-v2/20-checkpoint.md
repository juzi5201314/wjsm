# TodoCheckpointDraft

## Current todo

- Active: `T1.6 验证布局阶段`（completed）
- Next: 按用户要求停止；下次会话从 P2 `T2.1 新增 heap_access 断言` 开始。

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

## Active slice card

- Goal: P1 T1.6，验证布局阶段：确认 T1.1–T1.5 的 layout/global/support/host-import 切换在 cold/hot startup、backend support、runtime package 下可运行。
- Parent plan/spec: `docs/aegis/plans/2026-07-03-pluggable-gc-v2.md`；`docs/aegis/specs/2026-07-03-pluggable-gc-v2-design.md` §7/§8/§22。
- Files: 不预期继续编辑实现文件；若验证暴露缺陷，回到对应 owner 修复。
- Boundary: 只验证 P1；不进入 P2 host 读写层。
- Verification: 父计划 T1.6 命令、`cargo nextest` affected package、startup snapshot 双路 smoke。
- Stop: P1 验证通过并提交 T1.6 evidence/checkpoint，或定位到具体前置任务缺陷并修复。

## Evidence refs

详见 `90-evidence.md`。T1.6 已完成 normal workspace、cold workspace、targeted fixtures 与 build 验证。

## Blocked-on items

无。

## ResumeStateHint

恢复时先执行：

1. `git status --short` 确认当前切片文件。
2. 阅读本文件与 `90-evidence.md`。
3. 下次从 P2 `T2.1 新增 heap_access 断言` 开始；不要重复 P1，除非验证回归。
4. 开始 P2 前重新读取父计划 P2 与 v2 spec host read/write layer 小节。

# DriftCheckDraft

- Does current work still serve original task intent? 是，P1/T1.6 收尾验证已完成，未进入 P2。
- Does current work still serve goal and stop condition? 是，normal workspace 与 cold workspace 均通过。
- Compatibility boundary: support ABI canonical hash 已切到 `support_abi_union_hash`；当前 union 仅含 MarkSweep。
- New owner/fallback/adapter/branch: 未新增 fallback；T1.6 修复同步了 host 直接 bump 与 alloc window 的单一堆顶语义，并明确 startup no-GC 分配边界。
- Retirement track: 单一无参数 support emitter 与旧 `support_module_layout_hash` API 已退役；P2 未开始。
- Evidence sufficiency: T1.6 足够；按用户要求停止在 P1 末尾。
- Decision: continue-next-session-from-P2。
