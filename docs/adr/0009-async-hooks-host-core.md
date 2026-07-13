# ADR 0009：`node:async_hooks` 的单 Store 异步上下文所有权

- **状态**：已接受
- **日期**：2026-07-12
- **关联**：issue #313、ADR 0008、`docs/aegis/plans/2026-07-12-node-async-hooks.md`

## 背景

wjsm 的 Timer、nextTick、Promise、microtask、I/O completion、worker 与 `node:vm` realm 都在同一个运行时调度体系中执行。`node:async_hooks` 要求资源创建时捕获 trigger/context，在回调执行前后恢复，并且 hook、`AsyncLocalStorage` 与 `AsyncResource` 观察到同一条异步因果链。

若每个子系统或 realm 各自维护上下文，会产生 fire-time 捕获、Promise parent 丢失、worker 父回调泄漏、VM realm 状态分叉和 GC destroy 重复等不可组合行为。

## 决策

1. `RuntimeState` 唯一持有 `AsyncHooksState`；realm 不拥有第二份 async-hooks 状态。
2. 所有异步边界使用 `CapturedScope`，并在**资源创建/调度时**捕获，而不是在回调触发时读取当前上下文。
3. scheduler 统一执行 enter → callback → after/destroy → restore；Timer、TickObject、Immediate 与 Promise reaction 使用同一状态核。
4. Promise 资源在创建时分配 async id；`.then()` 结果 Promise 的 `triggerAsyncId` 指向 parent Promise async id；settle 发射 `promiseResolve`。
5. hook 发射使用稳定快照；发射期间的 enable/disable 延迟到最外层 emit 结束。
6. hook 回调抛错是 fatal：记录诊断并请求退出，不进入 `process.on('uncaughtException')`。
7. `AsyncResource.emitDestroy()` 只入队，destroy 在异步 checkpoint 发射；自动 destroy 由三种 GC 的统一 weak-ref sweep 后处理识别。
8. `AsyncHooksState` 中的 hook callback、frame store、默认值、pending Promise event 与资源对象按各自生命周期参与 GC roots；自动 destroy 资源元数据不反向强持有资源对象。
9. `node:vm` 新 realm 安装独立 host bridge 对象，但 bridge 访问同一个 `RuntimeState.async_hooks`。
10. `asyncWrapProviders` 只暴露已实际接线且与 Node v24 数值一致的子集，并冻结导出对象。
11. 长寿命异步负载复用 GC sweep 发布的 handle；handle 表覆盖两个完整 GC allocation window，并以 immutable barrier-buffer 基址作硬边界。Promise owner side-table 必须在 handle 复用前清空，禁止跨代保活旧 reaction/context。

## 备选方案

### 每个异步子系统自行保存 ALS store

拒绝。它无法统一 hook ids、trigger 链、destroy 和 VM/worker 边界，会形成多份事实源。

### 回调触发时捕获当前上下文

拒绝。资源创建与触发之间可能跨越多个 context，违反 Node 的 create-time capture 语义。

### 把 async-hooks 状态放进 realm

拒绝。ADR 0008 的单 Store multi-realm 模型要求共享运行时异步状态；realm 只隔离全局对象与 JS 可达图。

### 让 hook 异常走普通未捕获异常流程

拒绝。Node 将 hook callback 异常视为不可恢复 fatal，普通 `uncaughtException` 会允许状态已破坏的 hook 系统继续运行。

## 影响

- 新异步入口必须在 spawn/enqueue 前捕获 `CapturedScope`，并让 completion/task 携带它。
- 新资源 provider 只有在 init/before/after/destroy 或 Promise-only 语义完整接线后才能加入 providers 映射。
- 新 GC 算法必须调用统一 sweep 后处理，否则 AsyncResource 自动 destroy 契约不完整。
- startup snapshot 只能在 `AsyncHooksState::is_empty_for_snapshot()` 为真时捕获。
- 运行时 heap 扩容必须保证 GC 分配返回的源/目标对象区间已覆盖线性内存；长时间 hook 负载会持续触发对象扩容。
- 多模块 lowering 为每个 Module Record 分配独立 `Module` var scope；async `$module_main` continuation 的 liveness 同时覆盖全部模块环境，避免 await 后 import/顶层绑定退化为未初始化 local。
- handle 表布局常量属于 startup snapshot ABI 输入；容量或 GC window 改动必须使旧快照失配并冷启动。

## 验证

- createHook 校验、mutation snapshot 与 fatal fixtures
- TickObject / Timeout / Immediate 生命周期顺序 fixtures
- Promise parent / resolve / before-after fixture
- AsyncResource API、destroy 幂等与 mark-sweep/G1/ZGC 自动 destroy fixtures
- fetch/net/tls/dgram/fs.promises/worker 的 AsyncLocalStorage 传播 fixtures
- startup snapshot on/off 对照、VM realm bridge/state 保持、100k ALS/Promise/hook 负载 fixture
