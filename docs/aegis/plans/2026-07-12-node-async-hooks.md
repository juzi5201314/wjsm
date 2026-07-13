# node:async_hooks 全量实现计划（Host-core）

- 日期：2026-07-12（修订：纳入 `agent://async-hooks-inventory` P0/P1 独立复核）
- 执行状态：**已完成（实现、整体 review、全量验收与 ADR 均闭合）**
- 关联 issue：#313 非目标覆盖；**不 close #313**
- 设计：`docs/aegis/specs/2026-07-12-node-async-hooks-design.md`（**§0 修订表为准**）
- 工作记录：`docs/aegis/work/2026-07-12-node-async-hooks/`
- 兼容基准：**Node v24.15.0**；排除 `withScope`；**无 AsyncResource.domain**

## Goal

完整交付 v24.15 公共导出与可观察语义：

- `createHook` 五类 hook + `trackPromises` 真语义
- `executionAsyncId` / `triggerAsyncId` / `executionAsyncResource`（**无需** enable）
- `asyncWrapProviders`：**Object.freeze** + **Node 编号** + **仅已接线子集**
- `AsyncResource` 全方法（**无 domain**）+ GC/manual destroy
- `AsyncLocalStorage`：name/defaultValue/run/enterWith/exit/getStore/disable/static bind/snapshot
- **JS 可见 Timeout/Immediate 对象身份**（非 number id）
- **setImmediate/clearImmediate 硬依赖**
- hook throw **fatal 且绕过 uncaughtException**
- **调度时** frame 捕获；PROMISE **parent trigger 链**
- emit 中 enable/disable **快照**
- fs.promises：**仅** Promise/ALS，**不** FSREQ*

**Phase = 执行顺序 only。合并门禁 = 全局验收全绿。禁止 Phase0 半导出 stub merge。**

## Architecture

见 design §2。强制：

- `CapturedScope` 在 hooks/ALS 开启时写入 **全部** 异步 owner（含 Materialize/HostTask）
- `fatal.rs`：UE-bypass 进程失败
- Timeout/Immediate：**对象** `===` init.resource `===` 回调内 `executionAsyncResource()`

## Baseline / Compatibility

- issue #313、design §0、inventory 报告、ADR 0002/0003/0005/0008
- timer 返回值 number→对象（Node 对齐）；clear* 可兼容 number 过渡
- 无 hooks 程序：快路径；现有无关 fixture 不变
- Worker 独立 id；ALS **不**自动继承；vm 同 Store 共享

## Verification（全局验收）

```bash
cargo nextest run -E 'test(modules__node_builtin_async_hooks) | test(happy__async_hooks) | test(happy__async_local) | test(happy__async_resource) | test(happy__set_immediate) | test(errors__async_hooks)'
WJSM_TEST_GC=mark-sweep cargo nextest run -E 'test(happy__async_hooks_destroy) | test(happy__async_resource_gc)'
WJSM_TEST_GC=g1 cargo nextest run -E 'test(happy__async_hooks_destroy) | test(happy__async_resource_gc)'
WJSM_TEST_GC=zgc cargo nextest run -E 'test(happy__async_hooks_destroy) | test(happy__async_resource_gc)'
cargo nextest run -p wjsm-runtime --test async_scheduler --test async_reentry_audit --test timer_timing
cargo build --workspace
cargo nextest run --workspace
```

## BaselineUsageDraft

- Required：v24.15 文档、design §0、inventory P0/P1、scheduler/microtask/promises/timers、ADR 0002/0003/0005/0008
- Acknowledged：P0-1…P0-8（含 fs 诚实边界）、domain 删除、providers Node 编号子集
- Decision：continue

## Architecture Integrity / Pressure / Complexity

- Owner：`runtime_async_hooks/*` + `node_async_hooks.js` + Timeout/Immediate 返回值契约
- 禁止：seams 私自 ALS；number 伪装 resource；FSREQ 假 provider；domain throw；半导出 merge
- Timeout 对象改造（Task 1.3）优先于 hook 顺序任务
- setImmediate（Task 1.7）合并前必须完成
- Recommendation：add owner modules；emit/fatal 不得塞进 `runtime_microtask.rs` 正文

## ADR Signal

0009：Host-core；Timeout 对象；fatal UE-bypass；providers 子集+Node 编号；fs 非 FSREQ。

---

## 阶段总览

| Phase | 内容 |
|------|------|
| 0 | AsyncHooksState + 模块加载 + ids |
| 1 | frames + ALS + **Timeout 对象** + 传播 + **setImmediate** |
| 2 | createHook + trackPromises + 顺序 + **fatal** + enable 快照 + PROMISE 父链 |
| 3 | AsyncResource（无 domain）+ destroy/GC |
| 4 | providers 接线（net/tls/dgram/fetch/child/worker；**fs 仅 Promise**） |
| 5 | EAR、providers freeze 金丝雀、snapshot、回归、ADR |

---

# Phase 0 — 状态核 + 模块

## Task 0.1：`AsyncHooksState`

**Files:** `runtime_async_hooks/{mod,state}.rs`；`lib.rs`  
**Test:** `tests/async_hooks_state.rs`  
**Verify:** `cargo nextest run -p wjsm-runtime -E 'test(async_hooks_state)'`  
**Steps:** RED bootstrap ids + push/pop → 实现 → GREEN → commit

## Task 0.2：builtin 注册 + ids

**Files:** `node_async_hooks.js`；`builtin_modules.rs`；`runtime_node_globals.rs`  
**Fixtures:** `modules/node_builtin_async_hooks_main.js`；`happy/async_hooks_ids_bootstrap.js`  
**Verify:**  
`cargo nextest run -E 'test(modules__node_builtin_async_hooks_main) | test(happy__async_hooks_ids_bootstrap)'`  
**导出：** createHook, executionAsyncId, triggerAsyncId, executionAsyncResource, asyncWrapProviders, AsyncResource, AsyncLocalStorage  
**禁止** domain；**禁止** 半 stub merge（未实现函数不得 silent no-op 导出到保护分支）。

---

# Phase 1 — ALS + Timeout 对象 + 传播 + Immediate

## Task 1.1：context frames

**Files:** `context_frame.rs`  
**Verify:** `cargo nextest run -p wjsm-runtime -E 'test(async_context_frame)'`

## Task 1.2：ALS API

**Fixtures:**  
`async_local_run_getstore.js`, `async_local_enter_with.js`, `async_local_exit.js`, `async_local_default_name.js`, `async_local_disable.js`（disable→**defaultValue**）, `async_local_bind_snapshot.js`（捕获调用时刻）  
**Verify:** `cargo nextest run -E 'test(happy__async_local)'`  
**禁止 withScope / instance bind。**

## Task 1.3：Timeout **对象身份**（P0-3）+ CapturedScope（P0-5）

**Files:** Timeout 表示（建议 `runtime_node_timers.rs`）；`reentrant_async/mod.rs`；`types.rs` `TimerEntry`；`scheduler.rs`  
**Fixtures:**  
- `happy/async_hooks_timeout_resource_identity.js` — `init.resource === t === executionAsyncResource()` in cb；`ctor.name` Timeout  
- `happy/async_local_timer.js`  
- `happy/async_local_capture_at_schedule.js` — run(A) schedule 后 enterWith(B)，fire 仍 A（timer/nextTick/qm/promise 分支）  
**Verify:**  
```bash
cargo nextest run -E 'test(happy__async_hooks_timeout_resource_identity) | test(happy__async_local_timer) | test(happy__async_local_capture_at_schedule)'
```  
**硬门禁：** number-only 返回值 = 未完成。

## Task 1.4：nextTick CapturedScope

**Files:** `runtime_process.rs`, `runtime_microtask.rs`  
**Fixtures:** `async_local_next_tick.js`  
**Verify:** `cargo nextest run -E 'test(happy__async_local_next_tick)'`

## Task 1.5：Promise / async resume 传播

**Files:** `runtime_promises.rs`, `runtime_microtask.rs`  
**Fixtures:** `async_local_promise_then.js`, `async_local_async_await.js`  
**Verify:** `cargo nextest run -E 'test(happy__async_local_promise) | test(happy__async_local_async_await)'`

## Task 1.6：queueMicrotask

**Fixture:** `async_local_queue_microtask.js`；hooks 开启时 type 含 `Microtask`（子串断言）  
**Verify:** `cargo nextest run -E 'test(happy__async_local_queue_microtask)'`

## Task 1.7：setImmediate / clearImmediate（P0-6）

**Files:** globals；`immediate_queue`；drain：**nextTick 后、timers 前**  
**Fixtures:**  
- `happy/set_immediate_api.js`  
- `happy/set_immediate_order.js` — 子序列 nextTick → Immediate → Timeout(0)  
- `happy/async_hooks_immediate_type.js` — type Immediate + 资源身份（接 Phase 2）  
**Verify:** `cargo nextest run -E 'test(happy__set_immediate)'`

---

# Phase 2 — createHook + fatal + Promise 链

## Task 2.1：Hook 表 + validate + 快照（P0-2, P0-7, P1-2）

**Files:** `emit.rs`；JS createHook  
**Fixtures:**  
- `async_hooks_enable_disable.js`  
- `async_hooks_enable_during_emit.js` — h1.init 内 disable → 无 h1 before/after；h2 有  
- `async_hooks_create_hook_validate.js` — null→TypeError；非函数→ERR_ASYNC_CALLBACK  
- `async_hooks_track_promises_false.js` — 无 PROMISE init，有 Timeout  
- `errors/async_hooks_track_promises_mutex.js` — ERR_INVALID_ARG_VALUE  
- `async_hooks_multi_hook_order.js` — enable 序 = init 序  
**Verify:**  
```bash
cargo nextest run -E 'test(happy__async_hooks_enable) | test(happy__async_hooks_create_hook) | test(happy__async_hooks_track_promises) | test(errors__async_hooks_track_promises_mutex) | test(happy__async_hooks_multi_hook)'
```

## Task 2.2：Timeout/TickObject/Immediate 顺序 + 身份

**Fixtures:**  
`async_hooks_timer_order.js`, `async_hooks_next_tick_order.js`, `async_hooks_timeout_resource_identity.js`, `async_hooks_immediate_type.js`  
**Verify:** `cargo nextest run -E 'test(happy__async_hooks_timer) | test(happy__async_hooks_next_tick) | test(happy__async_hooks_timeout_resource) | test(happy__async_hooks_immediate)'`

## Task 2.3：PROMISE parent trigger + promiseResolve（P0-4）

**Fixtures:**  
`async_hooks_promise_parent_trigger.js` — **child.trigger === parent.asyncId**（链断言，非仅“有 init”）  
`async_hooks_promise_init_resolve.js` — settle 上 promiseResolve，**早于** then body  
`async_hooks_promise_then_order.js` — before/after 包 then  
**Verify:** `cargo nextest run -E 'test(happy__async_hooks_promise)'`

## Task 2.4：fatal UE-bypass（P0-1）

**Files:** `fatal.rs` + emit；**禁止** 可恢复 UE 路径  
**Fixtures（均安装 `process.on('uncaughtException',()=>console.log('UE-HIT'))`）：**  
- `errors/async_hooks_throw_init.js` — body 不跑；exit≠0；无 UE-HIT  
- `errors/async_hooks_throw_before.js` — body 不跑；exit≠0；无 UE-HIT  
- `errors/async_hooks_throw_after.js` — body 先输出再 fatal；无 UE-HIT  
- `errors/async_hooks_throw_promise_resolve.js` — exit≠0；无 UE-HIT  
**Verify:** `cargo nextest run -E 'test(errors__async_hooks_throw)'`

---

# Phase 3 — AsyncResource + destroy

## Task 3.1：AsyncResource（无 domain）

**Fixtures:**  
`async_resource_run_in_scope.js`, `async_resource_bind.js`, `async_resource_static_bind.js`,  
`async_resource_no_domain.js` — `'domain' in ar === false` 且访问不 throw,  
`async_resource_validate.js` — type/trigger 错误码  
**Verify:** `cargo nextest run -E 'test(happy__async_resource)'`  
**删除** `ERR_ASYNC_RESOURCE_DOMAIN_REMOVED` 一切要求。

## Task 3.2：destroy 队列

**Fixtures:** `async_hooks_emit_destroy.js`（destroy **非** emitDestroy 同步栈；双 emitDestroy 不抛；断言 **目标 asyncId**）, `async_resource_manual_destroy.js`  
**Verify:** `cargo nextest run -E 'test(happy__async_hooks_emit_destroy) | test(happy__async_resource_manual)'`

## Task 3.3：GC auto-destroy × 三 GC

**Fixtures:** `async_hooks_destroy_gc.js`, `async_resource_gc.js`  
**Verify:** 全局 `WJSM_TEST_GC=*` 命令。

---

# Phase 4 — Providers / I/O

## Task 4.1：asyncWrapProviders（P1）

**Fixture:** `async_hooks_providers_frozen.js`  
- `Object.isFrozen(asyncWrapProviders)`  
- `asyncWrapProviders.PROMISE === 27`（以 v24 探针锁定；若不同写死探针值）  
- 未接线键 `undefined`（如 HTTP2SESSION）  
- **无** FSREQ* 除非真异步 fs  
**Verify:** `cargo nextest run -E 'test(happy__async_hooks_providers)'`

## Task 4.2：I/O CapturedScope

| Owner | Fixture | 断言 |
|---|---|---|
| fetch | `async_local_fetch_stable.js` | 发起 store；中间 enterWith 不变 |
| net/tls/dgram | `async_local_net.js` 等 | ALS；可选 provider type |
| child | `async_local_child_message.js` | |
| worker | `async_hooks_worker_isolated.js` | 独立 id；**不**继承 parent ALS |
| **fs.promises** | `async_local_fs_promises.js` | **仅 ALS/Promise**；**禁止** FSREQ 断言 |

**Materialize/HostTask：** hooks/ALS on → **强制** `CapturedScope`（P0-5）。  
**Verify:**  
```bash
cargo nextest run -E 'test(happy__async_local_fetch) | test(happy__async_local_net) | test(happy__async_local_fs_promises) | test(happy__async_hooks_worker)'
```

---

# Phase 5 — EAR、snapshot、vm、回归、ADR

## Task 5.1：executionAsyncResource

**Fixture:** `async_hooks_execution_async_resource.js` — 顶层 Object sentinel；Timeout/Promise/AR 身份；**无需 enable**  
**Verify:** `cargo nextest run -E 'test(happy__async_hooks_execution_async_resource)'`

## Task 5.2：snapshot empty + ABI

**Files:** `startup_snapshot.rs`；native bridge  
**Verify:** snapshot 回归 + empty 断言；NativeCallable 变更 → abi_hash

## Task 5.3：vm 共享

**Fixture:** `async_local_vm_shared.js`  
**Verify:** `cargo nextest run -E 'test(happy__async_local_vm)'`

## Task 5.4：roots + PERF 关断

关断路径 resource 表不增长；roots 扫 hooks/frames。

## Task 5.5：全局验收 + ADR 0009

跑全局命令块；写 `docs/adr/0009-async-hooks-host-core.md`。

---

## P0 闭合表（inventory → fixture/命令）

| Inventory | Fixture / filter | Command |
|---|---|---|
| Timeout/Immediate 身份 | `async_hooks_timeout_resource_identity`, `async_hooks_immediate_type` | `test(happy__async_hooks_timeout_resource) \| test(happy__async_hooks_immediate)` |
| fatal UE-bypass | `errors/async_hooks_throw_*` | `test(errors__async_hooks_throw)` |
| capture-at-schedule | `async_local_capture_at_schedule`, `async_local_fetch_stable` | `test(happy__async_local_capture) \| test(happy__async_local_fetch)` |
| promise parent trigger | `async_hooks_promise_parent_trigger` | `test(happy__async_hooks_promise)` |
| setImmediate | `set_immediate_*` | `test(happy__set_immediate)` |
| emit enable/disable snapshot | `async_hooks_enable_during_emit` | `test(happy__async_hooks_enable_during_emit)` |
| trackPromises | `async_hooks_track_promises_*` + mutex error | 见 Task 2.1 |
| fs 非 FSREQ | `async_local_fs_promises` | `test(happy__async_local_fs_promises)` |
| no domain | `async_resource_no_domain` | `test(happy__async_resource)` |
| providers | `async_hooks_providers_frozen` | `test(happy__async_hooks_providers)` |

## Retirement

- 无旧 stub；#313 非目标历史保留 + comment 覆盖
- 禁止 number-only resource 终态；禁止双 ALS 栈

## 文档切片

- [x] design §0 + plan P0 表
- [x] work 更新
- [x] 生产代码本切片不改
- [x] 实现从 Task 0.1 TDD
