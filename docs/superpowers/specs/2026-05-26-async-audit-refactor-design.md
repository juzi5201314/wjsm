# 异步实现深度审计与重构设计

日期: 2026-05-26
状态: 待审批
范围: wjsm 异步/运行时实现的正确性、性能、代码整洁度全面审计

## 概述

对 wjsm 的异步实现（IR → 语义降级 → WASM 后端 → 运行时）进行深度审计，
发现 3 个正确性 Bug、4 个性能问题、4 个耦合/整洁度问题。
采用 trait 抽象方案消除 `_from_caller` / `_from_store` 大规模代码重复，
同时修复所有发现的问题。

## 审计发现

### 一、正确性 Bug

#### Bug 1: `async_generator_next` 在 Completed 状态下行为错误

**文件**: `crates/wjsm-runtime/src/host_imports/promise_async.rs:1134-1158`

**问题**: 当 generator 已处于 `Completed` 状态时调用 `.next()`，代码无条件将状态设为 `SuspendedYield`。
按 ES 规范 §27.6.3.7，已完成的 generator 调用 `.next()` 应返回 `{value: undefined, done: true}` 的 fulfilled promise。

**影响**: 已完成的 async generator 调用 `.next()` 会挂起而非立即返回完成结果。

**修复**: 在操作前检查当前状态，Completed 时直接 fulfill resume_promise。

#### Bug 2: `async_function_suspend` 双重锁获取

**文件**: `crates/wjsm-runtime/src/host_imports/promise_async.rs:909-933`

**问题**: 先锁 `continuation_table` 更新 `captured_vars`，释放锁，再锁同一把锁读取 `fn_table_idx`。
存在 TOCTOU 理论风险和不必要的 mutex contention。

**影响**: 性能损失 + 理论上的数据竞争窗口。

**修复**: 在单次锁获取中完成更新和读取。
**安全注意**: 当前 `unwrap_or(0)` 在 continuation 不存在时静默回退到 fn_table_idx=0，这会导致调用错误的函数。修复时应改为 early return 或 panic（取决于上下文是否允许 continuation 缺失）。

#### Bug 3: `async_generator_return` / `throw` 不检查 Executing 状态

**文件**: `crates/wjsm-runtime/src/host_imports/promise_async.rs:1161-1240`

**问题**: 两个方法直接设置 `state = Completed`，不检查当前状态。按规范 §27.6.3.8/27.6.3.9，generator 正在执行（`Executing`）或尚未开始（`SuspendedStart`）时 `.return()` 和 `.throw()` 行为不同：
- `Executing`: 请求应排队到 `queue`
- `SuspendedStart`: `.throw()` 应立即 reject，`.return()` 应立即 fulfill `{value, done:true}`
- `SuspendedYield`: 当前代码已正确处理（立即终止）

**影响**: generator 执行期间或未开始时调用 `.return()`/`.throw()` 行为违反规范。

**修复**: 检查状态，Executing 时入队；SuspendedStart 时按规范分别处理；其他状态立即处理。

### 二、性能问题

#### Perf 1: `pump_async_generator` 使用 `Vec::remove(0)` — O(n)

**文件**: `crates/wjsm-runtime/src/runtime_promises.rs:204,236`

**问题**: `entry.queue.remove(0)` 对 Vec 是 O(n)。async generator 频繁 yield 时累积开销显著。

**修复**: `AsyncGeneratorEntry::queue` 改为 `VecDeque`，`remove(0)` → `pop_front()`。

#### Perf 2: Continuation / Promise / NativeCallable / CombinatorContext 表只增不减

**文件**: `crates/wjsm-runtime/src/lib.rs` (RuntimeState 定义)

**问题**: 所有侧表都是 `Vec` 只 push 不 remove。长时间运行程序持续泄漏内存。

**修复**:
- ContinuationEntry: async 函数完成后标记可回收，drain_microtasks 后压缩
- PromiseEntry: GC mark-sweep 阶段同步清理
- NativeCallable: 表过大时做 compaction
- CombinatorContext: remaining==0 时标记可回收
**GC 交互**: Continuation 和 Promise 的清理必须与现有 mark-sweep GC 协调。Continuation 在 drain 结束后清理（安全点）。Promise 在 GC 阶段清理（与现有 GC 周期同步）。两者不引入新的并发问题，因为 GC 和 drain 都在主线程顺序执行。

#### Perf 3: 未处理 rejection 扫描全表

**文件**: `crates/wjsm-runtime/src/runtime_promises.rs:911-929`

**问题**: 每次 drain 结束后遍历整个 promise_table。大量 promise 时 O(n)。

**修复**: 维护 `pending_unhandled_rejections: HashSet<usize>`，reject 时添加，handled 时移除。

#### Perf 4: 绑定保存策略

**文件**: `crates/wjsm-semantic/src/lowerer_async_eval.rs:848`

**问题**: 每次 suspend 保存所有可见绑定，即使大部分未被修改。

**决策**: 保持保守策略（正确性优先），添加注释说明。后续可做 liveness 分析优化。

### 三、耦合 / 代码整洁度

#### Coupling 1: `_from_caller` / `_from_store` 大规模代码重复

**文件**: `crates/wjsm-runtime/src/runtime_promises.rs`, `runtime_host_helpers.rs`, `runtime_values.rs`, `runtime_heap.rs`

**问题**: ~740 行镜像代码（runtime_heap.rs 额外 ~200 行）。每次修改必须同步两处。

**修复**: WasmEnv 结构体 + `impl AsContextMut<Data = RuntimeState>` 泛型统一。

#### Coupling 2: `promise_async.rs` 2372 行 — 职责过多

**修复**: 拆分为 6 个文件（promise, promise_combinators, async_fn, async_generator, proxy_reflect, misc）。

#### Coupling 3: `PromiseReaction` 的 `handler` 字段语义重载

**修复**: 引入 `PromiseReactionKind` enum 区分 `Normal { handler }` 和 `AsyncResume { fn_table_idx, state }`。

#### Coupling 4: `runtime_promises.rs` 1369 行 — 职责混合

**修复**: 拆分为 4 个文件（promises, microtask, combinators, async_fn）。

## 设计方案

### 1. WasmEnv + 泛型函数（消除代码重复）

```rust
// crates/wjsm-runtime/src/wasm_env.rs
pub(crate) struct WasmEnv {
    pub memory: Memory,
    pub func_table: Table,
    pub shadow_sp: Global,
    pub heap_ptr: Global,
    pub obj_table_ptr: Global,
    pub obj_table_count: Global,
}

impl WasmEnv {
    pub fn extract_from_caller(caller: &mut Caller<'_, RuntimeState>) -> Option<Self> {
        Some(Self {
            memory: caller.get_export("memory")?.into_memory()?,
            func_table: caller.get_export("__table")?.into_table()?,
            shadow_sp: caller.get_export("__shadow_sp")?.into_global()?,
            heap_ptr: caller.get_export("__heap_ptr")?.into_global()?,
            obj_table_ptr: caller.get_export("__obj_table_ptr")?.into_global()?,
            obj_table_count: caller.get_export("__obj_table_count")?.into_global()?,
        })
    }
}
```

统一函数签名：
```rust
fn drain_microtasks<C: AsContextMut<Data = RuntimeState>>(ctx: &mut C, env: &WasmEnv) { ... }
fn resolve_promise<C: AsContextMut<Data = RuntimeState>>(ctx: &mut C, env: &WasmEnv, promise: i64, resolution: i64) { ... }
fn call_host_function<C: AsContextMut<Data = RuntimeState>>(ctx: &mut C, env: &WasmEnv, handler: i64, argument: i64) -> Option<i64> { ... }
fn resume_async_function<C: AsContextMut<Data = RuntimeState>>(ctx: &mut C, env: &WasmEnv, fn_table_idx: u32, continuation: i64, state: u32, resume_val: i64, is_rejected: bool) { ... }
fn handle_combinator_reaction<C: AsContextMut<Data = RuntimeState>>(ctx: &mut C, env: &WasmEnv, handler: i64, argument: i64) -> bool { ... }
```

辅助函数（resolve_handle, read_object_property 等）同样统一。

### 2. PromiseReaction 重构

```rust
enum PromiseReactionKind {
    Normal { handler: i64 },
    AsyncResume { fn_table_idx: u32, state: u32 },
}

struct PromiseReaction {
    kind: PromiseReactionKind,
    target_promise: i64,
    reaction_type: ReactionType,
}
```

构造函数：
```rust
impl PromiseReaction {
    fn new(handler: i64, target_promise: i64, reaction_type: ReactionType) -> Self {
        Self { kind: PromiseReactionKind::Normal { handler }, target_promise, reaction_type }
    }
    fn new_async(fn_table_idx: u32, target_promise: i64, reaction_type: ReactionType, state: u32) -> Self {
        Self { kind: PromiseReactionKind::AsyncResume { fn_table_idx, state }, target_promise, reaction_type }
    }
}
```

### 3. 文件拆分

**promise_async.rs (2372 行) → 6 个文件:**

| 新文件 | 职责 |
|--------|------|
| `host_imports/promise.rs` | Promise.create, resolve/reject, then/catch/finally, withResolvers, is_promise, create_resolve/reject_function |
| `host_imports/promise_combinators.rs` | Promise.all/race/allSettled/any |
| `host_imports/async_fn.rs` | async_function_start/resume/suspend, continuation_create/save/load |
| `host_imports/async_generator.rs` | async_generator_start/next/return/throw |
| `host_imports/proxy_reflect.rs` | proxy_create, reflect_get/set/has/delete, proxy_revocable |
| `host_imports/misc.rs` | native_call, eval, jsx, module_namespace, dynamic_import, queue_microtask, drain_microtasks |

**runtime_promises.rs (1369 行) → 4 个文件:**

| 新文件 | 职责 |
|--------|------|
| `runtime_promises.rs` | Promise entry accessors, settle/resolve/adopt, reaction queuing, passive_reaction_settlement |
| `runtime_microtask.rs` | `drain_microtasks` (caller + store 统一为一个), unhandled rejection tracking |
| `runtime_combinators.rs` | combinator context create/set/mark, reaction handler, decrement |
| `runtime_async_fn.rs` | `resume_async_function`, `enqueue_async_resume`, `pump_async_generator` |

### 4. AsyncGeneratorEntry queue 改为 VecDeque

```rust
struct AsyncGeneratorEntry {
    state: AsyncGeneratorState,
    continuation: i64,
    active_request: Option<AsyncGeneratorRequest>,
    waiting_resume_promise: Option<i64>,
    queue: VecDeque<AsyncGeneratorRequest>,  // Vec → VecDeque
}
```

### 5. 侧表生命周期管理

**Continuation 回收**: `ContinuationEntry` 添加 `completed: bool` 字段（默认 false）。在 `drain_microtasks` 处理 `AsyncResume` 后，如果 async 函数已返回（outer_promise 已 settled），将对应 continuation 标记为 `completed = true`。drain 循环结束后，对 continuation_table 做 retain（保留 `completed=false` 的条目）。注意：不能在 drain 过程中删除，因为其他微任务可能仍引用同一 continuation。

**Promise 回收**: GC mark-sweep 阶段，对未标记的 promise handle 清理 promise_table 条目。

**CombinatorContext 回收**: `decrement_combinator_remaining` 返回 `Some(...)` 时（remaining==0），标记 context 为可回收。

**NativeCallable free-list**: 不做 compaction（会导致已存储的 handle 索引失效 → use-after-free）。改为 free-list 策略：维护 `native_callable_free_slots: Vec<u32>`，删除条目时将索引入队，新建时优先从 free_slots 分配。`native_callables` Vec 永不收缩，但空闲槽位可复用。

### 6. 未处理 rejection 追踪优化

```rust
// RuntimeState 新增字段
pending_unhandled_rejections: HashSet<usize>,  // 存储 promise handle，复用 RuntimeState 现有锁
```

在 `settle_promise` 中 reject 且 `handled=false` 时添加 handle。
在 `.then()` / `.catch()` 标记 `handled=true` 时移除。
drain_microtasks 结束后只遍历这个小集合。

## 实施顺序

1. **Phase 1: WasmEnv + 泛型去重** — 核心基础设施，后续所有修改基于此
2. **Phase 2: PromiseReaction enum 重构** — 类型安全改进
3. **Phase 3: Bug 修复** — 3 个正确性修复
4. **Phase 4: 性能优化** — VecDeque、侧表生命周期、rejection 追踪
5. **Phase 5: 文件拆分** — 整洁度改进，最后做因为会移动代码

## 验证策略

- 每个 Phase 完成后运行 `cargo nextest run --workspace` 确认无回归
- 49 个异步相关 fixture 测试必须全部通过
- 手动验证 Bug 修复：为每个 bug 编写专门的 fixture 测试
- Phase 1 完成后验证：`cargo check` 确认泛型约束正确
- `CleanupFinalizationRegistry` 微任务处理差异：caller 上下文直接调用 callback，store 上下文延迟到 `pending_cleanup_callbacks`（因为 store 上下文没有 shadow stack）。统一版本采用 store 的延迟处理策略（更安全），caller 上下文也改为先入队 `pending_cleanup_callbacks` 再统一处理。

## 风险评估

- **Phase 1 (WasmEnv)**: 中等风险 — 泛型约束和生命周期可能遇到 wasmtime API 限制
- **Phase 2 (PromiseReaction)**: 低风险 — 纯结构重构，逻辑不变
- **Phase 3 (Bug 修复)**: 低风险 — 局部修改，有明确的规范参照
- **Phase 4 (性能)**: 中等风险 — 侧表回收逻辑需谨慎，避免 use-after-free
- **Phase 5 (文件拆分)**: 低风险 — 纯移动代码，逻辑不变
