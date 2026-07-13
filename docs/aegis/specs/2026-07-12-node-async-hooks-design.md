# node:async_hooks 全量实现设计

- 日期：2026-07-12（修订：同日纳入 `agent://async-hooks-inventory` 独立 Node v24.15 复核）
- 状态：设计已定（方案 B）+ **P0 硬约束已闭合**；实现权威以 `docs/aegis/plans/2026-07-12-node-async-hooks.md` 为准
- 关联 issue：#313（正文将「`async_hooks` 全量实现」列为**非目标**；用户明确要求完整落地 = **覆盖该非目标**；**不 close #313**）
- 兼容基准：**Node.js v24.15.0 公共 API 与可观察行为**
  - https://nodejs.org/docs/v24.15.0/api/async_hooks.html
  - https://nodejs.org/docs/v24.15.0/api/async_context.html
  - 内部参考：`opensrc path nodejs/node`（main）——**非**公共契约；**禁止** v27-only `withScope`
- 输入：issue #313、用户覆盖授权、host seams 调查、inventory 独立复核报告、ADR 0002/0003/0005/0006/0008、`AGENTS.md`

## 0. Inventory 修订摘要（必须读）

| ID | 决策（写入契约） |
|---|---|
| **P0-1** | hook throw = **fatal**：stderr 打栈、`exit≠0`、**不**触发 `process.on('uncaughtException')`，用户 body 不得在 init/before 失败后继续（after 失败则 body 已跑完再 fatal） |
| **P0-2** | `trackPromises:false` + `promiseResolve` 同设 → `ERR_INVALID_ARG_VALUE`；`trackPromises:false` **抑制 PROMISE** init/before/after/promiseResolve（Timeout 等仍 init） |
| **P0-3** | `Timeout`/`Immediate` 必须是 **JS 可见稳定对象**：`init.resource === setTimeout 返回值 === 回调内 executionAsyncResource()`；**禁止** number-only timer id 伪装 resource |
| **P0-4** | then 派生 **child PROMISE.triggerAsyncId === parent PROMISE.asyncId**；`promiseResolve` 在 settle 时、then body 前 |
| **P0-5** | **所有**异步 owner 在**构造/调度**时捕获 frame；fire 时 restore 该 frame，**禁止** fire-time `current` |
| **P0-6** | `setImmediate`/`clearImmediate` 是全量**硬依赖**（非可选） |
| **P0-7** | emit 期间 enable/disable 只改 **snapshot**；当前 emit 列表不变；depth=0 再应用 |
| **P1-1** | `asyncWrapProviders`：`Object.freeze`；**Node 编号**；**仅已接线子集**（如 `PROMISE===27` 金丝雀） |
| **P1-3** | Node v24 `AsyncResource` **无 `domain` 属性** → **不实现** domain getter/throw（删除旧 ERR_DOMAIN 要求） |
| **P1-8** | `fs.promises` 只验收 **Promise/ALS 传播**；**不得**宣称 `FSREQ*` provider，除非真异步 fs host |

报告全文：`agent://async-hooks-inventory`（本修订的权威 gap 列表）。

## 1. 背景与当前事实

### 1.1 Node 公共面（v24.15.0）

| 导出 | 稳定性 | 必须行为 |
|---|---|---|
| `createHook(options)` | Stab 1 | 选项对象；回调非函数 → `ERR_ASYNC_CALLBACK`；`null`/缺参 → `TypeError`；返回 `AsyncHook` |
| `AsyncHook.enable/disable` | | 幂等；emit 中变更走快照 |
| hooks | | `init(asyncId,type,triggerAsyncId,resource)`；`before/after/destroy/promiseResolve(asyncId)` |
| `trackPromises` | | 默认 true；`false` 抑制 PROMISE 类 hook；与 `promiseResolve` 互斥 |
| `executionAsyncId` / `triggerAsyncId` | | bootstrap 常见 1 / 0；**无需** enable hooks 即可读 |
| `executionAsyncResource` | Stab 1 | 顶层 Object sentinel；资源回调内 === 该资源对象 |
| `asyncWrapProviders` | | **frozen**；名→**Node 整数**；wjsm 仅导出已接线键 |
| `AsyncResource` | Stable | ctor / `runInAsyncScope` / `emitDestroy` / `asyncId` / `triggerAsyncId` / `bind` / `static bind` / `requireManualDestroy`；**无 domain** |
| `AsyncLocalStorage` | Stable 2 | `defaultValue`/`name`/`run`/`enterWith`/`exit`/`getStore`/`disable`/`static bind`/`snapshot` |

### 1.2 可观察语义（v24 探针固化）

1. **init** 在资源构造/调度时同步触发。
2. **before → 用户回调 → after**；push/pop execution stack + resource。
3. **promiseResolve** 在 Promise settle 时；then body 外包 before/after。
4. **PROMISE 父子**：`Promise.resolve().then` 链上 child.trigger === parent.asyncId。
5. **destroy** 异步队列；默认 AR 可 GC auto-destroy；`requireManualDestroy:true` 需 `emitDestroy`。
6. **hook throw**：fatal，**绕过 uncaughtException**（exit=1，无 UE-HIT）。
7. **enable/disable during emit**：当前 call 使用快照列表。
8. **ALS**：调度瞬间 frame 在 fire 时恢复；中间 `enterWith` 不污染已调度工作。
9. **Timeout/Immediate 身份**：返回值对象 === init.resource === `executionAsyncResource()` in callback。
10. **Worker** 独立 hooks 状态；**vm 同 Store 共享** hooks/ALS。
11. **多 hook**：init 调用序 === enable 注册序。

### 1.3 wjsm 现状（能力鸿沟）

| 领域 | 现状 | 必须补齐 |
|---|---|---|
| async_hooks 模块 | 无 | 全量导出 |
| timer 返回值 | **数字 id** | **JS Timeout/Immediate 对象**（P0-3） |
| setImmediate | **无** | 全局 + 队列（P0-6） |
| Promise 侧表 | 无 async_id | parent 链 + promiseResolve（P0-4） |
| Materialize/HostTask | 无 CapturedScope 强制 | 调度捕获 frame（P0-5） |
| fatal | process_exit_signal 可恢复风险 | UE-bypass fatal 通道（P0-1） |
| fs | `fs.promises` ≈ sync+Promise | **仅 Promise 传播**；无 FSREQ*（P1-8） |
| GC / snapshot | weak + empty 表纪律 | AR destroy；hooks empty-check |

**方案 A（纯 JS）否决。方案 B（Host-core）选定。**

## 2. 架构（方案 B）

```
node:async_hooks
  → node_async_hooks.js          # API 外形、ERR_*、Timeout/Immediate 类外形（可与 host 协作）
  → __wjsm_node_async_hooks      # host bridge
  → runtime_async_hooks/
       state.rs | context_frame.rs | emit.rs | resource.rs | scope.rs | fatal.rs
  ← scheduler / microtask / promises / timers / immediate / I/O completions
```

- **唯一 owner**：emit/scope/frame 只在 `runtime_async_hooks`。
- **RuntimeState 扁平**（ADR 0002）：`async_hooks: AsyncHooksState` + 可选 `immediate_queue`。
- **快路径**：`!hooks_enabled && !als_in_use` → 不分配 async_id/frame；但 **Timeout 对象返回值** 仍须存在（与 Node 兼容的对象身份；hooks 关时可不填 async 元数据）。

## 3. 目标 / 非目标

### 3.1 目标

见 §0 表 + 完整 v24.15 导出；传播覆盖：Timeout/Immediate/TickObject/Microtask/PROMISE/async resume/net/tls/dgram/fetch/child/worker/port；vm 共享；Worker 隔离；三 GC destroy；snapshot empty；**setImmediate 硬依赖**。

### 3.2 非目标

- `withScope`；`domain` 模块与 AR.domain；perf_hooks 全量
- 伪造未接线 provider（含 **FSREQ*** 在无真异步 fs 时）
- 完整 69-key providers 表
- 关闭 #313 大 roadmap

## 4. 数据结构与资源身份

### 4.1 `AsyncHooksState`（概念）

```rust
// 计数、execution/trigger、id_stack、hooks + hook_counts、
// emit_depth + hooks_snapshot（P0-7）、
// destroy_queue、resources、frames、current_frame、flags
// fatal_in_progress: bool  // P0-1：阻止 UE 路径与继续调度
```

`HookRecord.track_promises: bool`（false → kNoPromiseHook，跳过 PROMISE 类 emit）。

### 4.2 Timeout / Immediate 对象（P0-3，硬）

- `setTimeout`/`setInterval`/`setImmediate` 返回 **对象**（非 number）：
  - 身份：`===` 稳定
  - `constructor.name` / 品牌：`Timeout` / `Immediate`（或 Node 可观察等价）
  - `clearTimeout`/`clearImmediate` 接受该对象（可兼容残留 number 过渡，但 **init.resource 必须是对象**）
  - 最小方法：`ref`/`unref`/`hasRef` 不炸（可 no-op 实现，但须存在若 Node 公有面依赖——以 fixture 为准）
- Host `TimerEntry` / immediate 条目持有 `resource: i64`（对象 handle）+ `async_id` + `context_frame`。

### 4.3 Promise（P0-4）

`PromiseEntry { async_id, trigger_async_id, ... }`  
- 新 promise init 时分配 id；then 派生 promise 的 `trigger_async_id = parent.async_id`。  
- settle → `emit_promise_resolve`（若 hook 跟踪 promise）。

### 4.4 CapturedScope（P0-5）

```rust
struct CapturedScope { async_id, trigger_async_id, resource, frame_id }
```

**hooks 或 ALS 开启时强制** 写入：

- `TimerEntry` / Immediate 条目 / `ProcessNextTickTask` / Microtask 包装
- `AsyncHostCompletion::{Materialize,HostTask}`（**非可选**）
- PromiseReaction / AsyncResume 关联 scope

`process_one_completion` / drain 路径：`with_async_scope(captured)` 再跑用户逻辑。

## 5. Hook 发射

### 5.1 生命周期

同前：construct → capture frame → init → schedule；fire → push + restore frame → before → cb → after → pop；settle → promiseResolve；destroy 异步 drain。

### 5.2 trackPromises（P0-2）

| 配置 | PROMISE init/before/after | promiseResolve |
|---|---|---|
| 默认 / `trackPromises:true` | 开（若对应 hook 函数存在） | 若提供则开 |
| `trackPromises:false` 且无 promiseResolve | **全关** | 关 |
| `trackPromises:false` 且有 promiseResolve | **构造抛** `ERR_INVALID_ARG_VALUE` | — |

### 5.3 fatal（P0-1）

`emit_*` 单 try/catch：

1. 打印 stack 到 stderr/diagnostics（类 Node `_rawDebug`）
2. 置 `fatal_in_progress` + **进程失败出口**（exit code 对齐 Node 1）
3. **不**进入普通 uncaughtException 投递/可恢复路径
4. 中止后续 hook 与（init/before 失败时）用户 body

实现注意：不能只靠 `TAG_EXCEPTION` 冒泡到用户可 catch 的路径。

### 5.4 enable/disable 快照（P0-7）

- `emit_depth>0`：enable/disable 写 pending 结构
- 当前 for-loop 只遍历 **进入 emit 时** 的 hook 数组副本
- `emit_depth==0`：应用 pending，重算 `hook_counts`

验收：h1.init 内 `h1.disable()` → 该 Timeout 无 h1 before/after；h2 仍有。

### 5.5 Seam 表

| Seam | init type | frame 捕获点 |
|---|---|---|
| setTimeout/setInterval | `Timeout` | schedule（返回对象时） |
| setImmediate | `Immediate` | schedule |
| process.nextTick | `TickObject` | enqueue |
| queueMicrotask | `Microtask`（hooks/ALS on） | enqueue |
| Promise | `PROMISE` | alloc / then 派生 |
| async resume | 沿关联 promise/continuation | 创建 continuation 时 |
| fetch/net/tls/dgram Materialize | 已接线 provider 名 | **发起 op 时** CapturedScope |
| child/worker/port HostTask | `PROCESSWRAP`/`WORKER`/`MESSAGEPORT`… | 创建/post 时 |
| AsyncResource | 用户 type | ctor |

**fs.promises**：不 init FSREQ*；只走 PROMISE/ALS。

## 6. asyncWrapProviders（P1-1）

- `Object.freeze`；属性 non-writable / non-configurable（对齐 Node 可观察）
- **数值 = Node `async_wrap` provider 枚举**（金丝雀：`PROMISE === 27` 等以 v24 源/探针锁定）
- **键集合 = wjsm 实际 emit init 的类型**；未接线键 **不出现**（`undefined`）
- 禁止填 HTTP2 等假键

## 7. ALS

- `disable` 后 `getStore()` → **defaultValue**（非永久 undefined）
- `static bind`/`snapshot` 捕获**调用时刻** frame
- 无 instance `bind`；无 `withScope`

## 8. AsyncResource

- 全 API 除 domain：**不实现 domain**
- 校验：`type` 非 string → `ERR_INVALID_ARG_TYPE`；非法 trigger → `ERR_INVALID_ASYNC_ID`；`triggerAsyncId === -1` 允许（Node）
- GC auto-destroy vs manual：P1-4 fixtures + 三 GC

## 9. GC / snapshot / ABI

- hooks 回调、live frames、stack resources 进 `roots.rs`
- destroy 复用 `weak_refs` / FinalizationRegistry；禁止第二套 GC
- startup snapshot：`async_hooks` 侧表 empty；禁止动态 hook 入快照
- 新增 `NativeCallable` → snapshot bridge + **abi_hash bump**

## 10. 性能

双关断快路径；per-kind counts；COW frames；promise 跟踪按需。

## 11. 验收矩阵（fixture 名 + 命令）

全局：

```bash
cargo nextest run -E 'test(modules__node_builtin_async_hooks) | test(happy__async_hooks) | test(happy__async_local) | test(happy__async_resource) | test(happy__set_immediate) | test(errors__async_hooks)'
WJSM_TEST_GC=mark-sweep cargo nextest run -E 'test(happy__async_hooks_destroy) | test(happy__async_resource_gc)'
WJSM_TEST_GC=g1 cargo nextest run -E 'test(happy__async_hooks_destroy) | test(happy__async_resource_gc)'
WJSM_TEST_GC=zgc cargo nextest run -E 'test(happy__async_hooks_destroy) | test(happy__async_resource_gc)'
cargo nextest run --workspace
cargo build --workspace  # 零 warning
```

| ID | Fixture（`fixtures/…`） | 断言要点 |
|---|---|---|
| API-01 | `modules/node_builtin_async_hooks_main` | require/import 导出键 |
| API-02 | `happy/async_hooks_ids_bootstrap` | exec=1,trigger=0；无需 enable |
| API-03 | `happy/async_hooks_create_hook_validate` | ERR_ASYNC_CALLBACK；null TypeError；trackPromises 互斥 |
| P0-1a | `errors/async_hooks_throw_before` | exit≠0；无 UE；body 不跑 |
| P0-1b | `errors/async_hooks_throw_after` | body 先输出再 fatal；UE=0 |
| P0-1c | `errors/async_hooks_throw_init` | body 不跑 |
| P0-1d | `errors/async_hooks_throw_promise_resolve` | exit≠0 |
| P0-2a | `happy/async_hooks_track_promises_false` | 无 PROMISE init；有 Timeout |
| P0-2b | `errors/async_hooks_track_promises_mutex` | ERR_INVALID_ARG_VALUE |
| P0-3 | `happy/async_hooks_timeout_resource_identity` | resource===timer===executionAsyncResource |
| P0-4 | `happy/async_hooks_promise_parent_trigger` | child.trigger===parent.id；resolve 序 |
| P0-5 | `happy/async_local_capture_at_schedule` | 中间 enterWith 不污染已调度 then/timer/nextTick/qm |
| P0-5io | `happy/async_local_fetch_stable` / net… | 发起时 store 在完成回调保持 |
| P0-6 | `happy/set_immediate_api` + `async_hooks_immediate_type` | 全局存在；type Immediate；序 nextTick⊂Immediate⊂Timeout0 |
| P0-7 | `happy/async_hooks_enable_during_emit` | 双 hook 快照语义 |
| ALS-* | `happy/async_local_*` | run/exit/default/name/disable→default/bind/snapshot |
| AR-* | `happy/async_resource_*` | 全 API；**无 domain**；manual/GC destroy |
| PROV-01 | `happy/async_hooks_providers_frozen` | freeze；PROMISE===27；无 HTTP2SESSION |
| IO-fs | `happy/async_local_fs_promises` | **仅** ALS/Promise；**不断言** FSREQ |
| IO-net… | 各一条 | ALS + 可选 provider type 名 |
| WK-01 | `happy/async_hooks_worker_isolated` | 独立 id；不继承 parent ALS |
| VM-01 | `happy/async_local_vm_shared` | 同 Store 共享 |
| GC-01 | destroy fixtures × 三 GC | |
| SNAP-01 | snapshot empty + 无 hooks 程序 | |
| PERF-01 | 关断路径无 resource 表膨胀 | |
| ORD-multi | `happy/async_hooks_multi_hook_order` | 注册序=init 序 |

## 12. 文件地图

新建：`runtime_async_hooks/*`、`node_async_hooks.js`、Timeout/Immediate 实现落点（globals + host）、fixtures 上表、ADR 0009（实现后）。

修改：`builtin_modules.rs`、`lib.rs`、`types.rs`、`runtime_node_globals.rs`、`runtime_process.rs`、`runtime_microtask.rs`、`runtime_promises.rs`、`scheduler.rs`、`host_imports/reentrant_async/mod.rs`、I/O owners、`roots.rs`/`weak_refs.rs`、`startup_snapshot*.rs`、snapshot native bridge。

## 13. ADR 信号 / 风险

- ADR 0009：Host-core；Timeout 对象契约；fatal UE-bypass；providers 子集+Node 编号；fs 非 FSREQ。
- 风险：Timer 返回值变更可能影响依赖 number id 的用户代码 → 兼容层可接受 number 于 clear*，但 **公开 setTimeout 返回对象**（Node 兼容优先）。

## 14. Working artifacts

- Plan：`docs/aegis/plans/2026-07-12-node-async-hooks.md`
- Work：`docs/aegis/work/2026-07-12-node-async-hooks/`
- Inventory：`agent://async-hooks-inventory`
