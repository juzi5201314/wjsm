# TodoCheckpointDraft — node:async_hooks

## Status

**Decision: complete**（Phase 0–5、整体 review、全量验收与 ADR 已闭合）

## Completed

- Phase 0：`AsyncHooksState`、`node:async_hooks` host bridge、public exports/providers。
- Phase 1：ALS immutable frames；Timeout/nextTick/Promise/async-resume/queueMicrotask/Immediate 上下文传播。
- Phase 2：hook table snapshot mutation、五类生命周期、PROMISE parent trigger/promiseResolve、fatal channel。
- Phase 3：`AsyncResource` 完整 API、manual/auto destroy、`AsyncLocalStorage.bind/snapshot`。
- Phase 4：fetch/net/TLS/dgram/fs.promises/worker/child_process 调度时捕获与真实 provider fixtures。
- Phase 5：`executionAsyncResource`、snapshot empty gate、vm realm 共享、三 GC roots/handle reuse/100k 负载。
- Review 修复：Promise 特殊 reaction 尾声、fatal roots、ALS active/frame 生命周期、Promise side-table 回收、Module scope + top-level await continuation、constructor 可观察语义、cluster worker 显式回收。

## Evidence

- `cargo build --workspace`：通过，零 warning。
- `cargo nextest run --workspace`：1651 passed，2 skipped。
- async_hooks 计划集合：45 passed。
- mark-sweep/G1/ZGC hooks roots 集合：各 3 passed。
- startup snapshot off/on `async_hooks_phase_order`：各 1 passed。
- scheduler binaries：15 passed。
- cluster IPC：8 passed。

## DriftCheckDraft

- Scope：与计划一致；`withScope` 与 `AsyncResource.domain` 仍为明确非目标。
- Compatibility：Node v24.15 public surface；constructor `new`/`instanceof`、空 type 条件与 fatal bypass 已覆盖。
- Retirement：无旧 stub、双 ALS owner、number-only resource 或 runtime-only snapshot 状态残留。
- Decision：pass。

## ResumeStateHint

- 最终架构决策见 `docs/adr/0009-async-hooks-host-core.md`。
- Module Record 使用独立 `ScopeKind::Module` var 环境；async `$module_main` liveness 覆盖所有模块环境。
- handle 表覆盖两个 GC allocation windows；guest/host 共同消费 sweep free-list，Promise side-table 在 handle 发布前清理。
