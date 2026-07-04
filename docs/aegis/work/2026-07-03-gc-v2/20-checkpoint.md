# TodoCheckpointDraft

## Current todo

- Active: `T1.6 验证布局阶段`（needs-verification：cold full-suite 仍有 2 个 startup builtins parity 缺口）
- Next: 下次会话先处理 `WJSM_STARTUP_SNAPSHOT=0` 下 `happy__error_constructor_new_target` 与 `happy__symbol_prototype_methods`，通过后再进入 P2。

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

详见 `90-evidence.md`。T1.6 已完成 normal workspace、targeted cold smoke、build 验证；cold full-suite 仍有 2 个失败项，不能标记完成。

## Blocked-on items

- `WJSM_STARTUP_SNAPSHOT=0 cargo nextest run --workspace` 仍失败 2 项：`happy__error_constructor_new_target`、`happy__symbol_prototype_methods`。

## ResumeStateHint

恢复时先执行：

1. `git status --short` 确认当前切片文件。
2. 阅读本文件与 `90-evidence.md`。
3. 先修复 cold startup builtins parity：Error constructor prototype 链、Symbol.prototype `[Symbol.toStringTag]`。
4. 重新运行 `WJSM_STARTUP_SNAPSHOT=0 cargo nextest run --workspace`；通过后标记 T1.6 done，再开始 P2。

# DriftCheckDraft

- Does current work still serve original task intent? 是，仍停在 P1/T1.6 收尾验证。
- Does current work still serve goal and stop condition? 部分满足：normal workspace 与 targeted cold smoke 通过；cold full-suite 未通过。
- Compatibility boundary: support ABI canonical hash 已切到 `support_abi_union_hash`；当前 union 仅含 MarkSweep。
- New owner/fallback/adapter/branch: 未新增 fallback；T1.6 修复同步了 host 直接 bump 与 alloc window 的单一堆顶语义。
- Retirement track: 单一无参数 support emitter 与旧 `support_module_layout_hash` API 已退役；P2 未开始。
- Evidence sufficiency: normal/hot 路径足够；cold full-suite 证据不足，T1.6 不标记完成。
- Decision: blocked。
