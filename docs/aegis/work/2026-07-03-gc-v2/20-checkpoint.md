# TodoCheckpointDraft

## Current todo

- Active: `T1.4 换代 host imports`
- Next: 开始 T1.4，将 host import 入口从 `gc_maybe_collect` 阈值语义切到 `gc_safepoint_poll`，保留旧 import 到本阶段结束后清理。

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

## Active slice card

- Goal: P1 T1.4，换代 host imports：新增/接入 `gc_safepoint_poll`，并把现有 `gc_maybe_collect` 调用语义迁移到 scheduler/alloc-window 体系。
- Parent plan/spec: `docs/aegis/plans/2026-07-03-pluggable-gc-v2.md`；`docs/aegis/specs/2026-07-03-pluggable-gc-v2-design.md` §8/§22。
- Files: 预计涉及 `host_imports/core.rs`、`host_import_registry`、backend import callsites、runtime GC scheduler。
- Boundary: 不实现 G1/ZGC；不改变 CLI；不删除仍被 support/runtime ABI 需要的 import，直到确认无调用者。
- Verification: targeted grep/wasmparser 确认调用点，`cargo check` 与 affected nextest。
- Stop: T1.4 完成 host import 换代并提交，或发现 support/runtime ABI 需父计划调整则暂停。

## Evidence refs

详见 `90-evidence.md`。T1.3 已有 wasmparser proof、backend/runtime package 测试与 workspace build 证据；T1.4 待补。

## Blocked-on items

无外部阻塞。

## ResumeStateHint

恢复时先执行：

1. `git status --short` 确认当前切片文件。
2. 阅读本文件与 `90-evidence.md`。
3. 从父计划 T1.4 开始，先定位 `gc_maybe_collect` 的剩余 producer/consumer，再设计 `gc_safepoint_poll` 换代边界。
4. 每完成子切片更新本 checkpoint/evidence/drift 记录。

# DriftCheckDraft

- Does current work still serve original task intent? 是，当前已推进到 P1 host import 换代。
- Does current work still serve goal and stop condition? 是，T1.3 未扩出父计划；T1.4 仍在 P1 布局后续。
- Compatibility boundary: env global ABI 已从 20 扩到 27；backend/support/runtime 已同步，ABI hash 通过 support layout hash 变化失效旧 snapshot。
- New owner/fallback/adapter/branch: 未新增 fallback；alloc window globals 与 wasmparser test 已锁定 canonical 分配路径。
- Retirement track: `gc_maybe_collect` 已从 backend/support 分配 fast-path 移出；T1.4 继续完成 host import 层 retirement。
- Evidence sufficiency: T1.3 足够；T1.4 尚未开始，证据待补。
- Decision: continue。
