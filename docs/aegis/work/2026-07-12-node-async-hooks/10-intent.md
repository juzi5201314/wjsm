# TaskIntentDraft — node:async_hooks full implementation (host-core)

- Goal: Node **v24.15.0** 公共面 `node:async_hooks` 全量（createHook/trackPromises、ids、EAR、Node-numbered frozen providers 子集、AsyncResource **无 domain**、ALS 全方法、**Timeout/Immediate 对象身份**、**setImmediate**、fatal **绕过 uncaughtException**、**调度时** frame 捕获、PROMISE parent trigger、emit 快照）。Host-core 方案 B。Phase 仅顺序。
- Scope: 本切片 = design/plan/work 文档修订；实现后续。覆盖 #313 非目标。
- Non-goals: withScope；domain；FSREQ* 无真异步 fs；全量 69 providers；关闭 #313；本切片改生产代码。
- Risk: timer 返回值变更；fatal 通道；Materialize 漏 CapturedScope；半导出 merge。
- Parent design/plan: `docs/aegis/specs|plans/2026-07-12-node-async-hooks-*.md`
- Inventory: `agent://async-hooks-inventory`

# BaselineUsageDraft

- Required: issue #313、v24.15 docs、design §0、inventory P0/P1、ADR 0002/0003/0005/0008、scheduler/microtask/promises/timers
- Acknowledged: 全部 P0 硬门禁 + domain 删除 + fs 诚实边界
- Decision: **continue**
