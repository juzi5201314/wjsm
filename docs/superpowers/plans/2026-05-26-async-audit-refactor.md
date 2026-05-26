# 异步实现审计重构 — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 深度修复 wjsm 异步实现的 3 个正确性 Bug、4 个性能问题、4 个耦合/整洁度问题，核心是通过 WasmEnv + 泛型消除 ~740 行 `_from_caller`/`_from_store` 镜像代码重复。

**Architecture:** 创建 `WasmEnv` 结构体打包 WASM 导出句柄（Memory/Table/Global 都是 wasmtime Copy 类型），用 `impl AsContextMut<Data = RuntimeState>` 泛型约束统一 Caller 和 Store 两种上下文，消除 5 对镜像函数。重构 PromiseReaction 用 enum 区分普通/async 反应。修复 async generator 状态机的 3 个规范违反。

**Tech Stack:** Rust 2024, wasmtime 43.0.0, wasm-encoder

---

### Phase 1: WasmEnv + 泛型去重（核心基础设施）

#### Task 1.1: 创建 WasmEnv 结构体

**Files:**
- Create: `crates/wjsm-runtime/src/wasm_env.rs`
- Modify: `crates/wjsm-runtime/src/lib.rs` — 添加 `mod wasm_env;`

- [ ] **Step 1: 创建 wasm_env.rs**

```rust
use super::RuntimeState;
use wasmtime::*;

/// 打包所有 WASM 导出句柄（Memory / Table / Global 都是 wasmtime Copy 类型）。
/// 用于消除 _from_caller / _from_store 的代码重复。
#[derive(Clone, Copy)]
pub(crate) struct WasmEnv {
    pub memory: Memory,
    pub func_table: Table,
    pub shadow_sp: Global,
    pub heap_ptr: Global,
    pub obj_table_ptr: Global,
    pub obj_table_count: Global,
    pub object_proto_handle: Global,
}

impl WasmEnv {
    /// 从 Caller 上下文中一次性提取所有导出句柄。
    pub fn from_caller(caller: &mut Caller<'_, RuntimeState>) -> Option<Self> {
        Some(Self {
            memory: caller.get_export("memory")?.into_memory()?,
            func_table: caller.get_export("__table")?.into_table()?,
            shadow_sp: caller.get_export("__shadow_sp")?.into_global()?,
            heap_ptr: caller.get_export("__heap_ptr")?.into_global()?,
            obj_table_ptr: caller.get_export("__obj_table_ptr")?.into_global()?,
            obj_table_count: caller.get_export("__obj_table_count")?.into_global()?,
            object_proto_handle: caller.get_export("__object_proto_handle")?.into_global()?,
        })
    }
}
```

- [ ] **Step 2: 在 lib.rs 注册模块**

在 `crates/wjsm-runtime/src/lib.rs` 的 mod 声明区域添加：

```rust
mod wasm_env;
```

- [ ] **Step 3: 验证编译**

```bash
cd /home/soeur/project/wjsm && cargo check -p wjsm-runtime 2>&1 | tail -5
```

Expected: `Checking wjsm-runtime ... Finished`

- [ ] **Step 4: Commit**

```bash
git add crates/wjsm-runtime/src/wasm_env.rs crates/wjsm-runtime/src/lib.rs
git commit -m "feat: add WasmEnv struct for caller/store context unification"
```

---

#### Task 1.2: 统一 runtime_heap.rs（alloc_host_object, alloc_host_null_proto_object）

**Files:**
- Modify: `crates/wjsm-runtime/src/runtime_heap.rs` — 合并 `_from_caller` 和 `_from_store` 变体
- Search: `crates/wjsm-runtime/src/runtime_heap.rs` 中 `alloc_host_object_from_store` 的签名

- [ ] **Step 1: 读取现有 _from_store 变体确认签名**

```bash
cd /home/soeur/project/wjsm && grep -n 'alloc_host_object_from_store\|alloc_host_null_proto_object_from_store' crates/wjsm-runtime/src/runtime_heap.rs
```

- [ ] **Step 2: 将 alloc_host_object_from_caller 改为泛型版本**

将现有的 `alloc_host_object_from_caller` 重写为泛型版本，同时删除 `alloc_host_object_from_store`：

```rust
use crate::wasm_env::WasmEnv;

pub(crate) fn alloc_host_object<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    capacity: u32,
) -> i64 {
    let heap_ptr = env.heap_ptr.get(ctx.as_context()).i32().unwrap_or(0) as u32;
    let obj_table_count = env.obj_table_count.get(ctx.as_context()).i32().unwrap_or(0) as u32;
    let obj_table_ptr = env.obj_table_ptr.get(ctx.as_context()).i32().unwrap_or(0) as u32;
    let size = 16 + capacity * 32;
    let new_heap_ptr = heap_ptr.saturating_add(size);
    // 通过 Global::get 读取 object_proto_handle（WasmEnv 需包含此 global）
    let proto = env.object_proto_handle.get(ctx.as_context()).i32().unwrap_or(-1) as u32;
    {
        let data = env.memory.data_mut(ctx.as_context_mut());
        let ptr = heap_ptr as usize;
        if new_heap_ptr as usize > data.len() {
            return value::encode_undefined();
        }
        data[ptr..ptr + 4].copy_from_slice(&proto.to_le_bytes());
        data[ptr + 4] = wjsm_ir::HEAP_TYPE_OBJECT;
        data[ptr + 5..ptr + 8].fill(0);
        data[ptr + 8..ptr + 12].copy_from_slice(&capacity.to_le_bytes());
        data[ptr + 12..ptr + 16].copy_from_slice(&0u32.to_le_bytes());
        let slot_addr = (obj_table_ptr + obj_table_count * 4) as usize;
        if slot_addr + 4 <= data.len() {
            data[slot_addr..slot_addr + 4].copy_from_slice(&heap_ptr.to_le_bytes());
        }
    }
    let _ = env.heap_ptr.set(ctx.as_context_mut(), Val::I32(new_heap_ptr as i32));
    let _ = env.obj_table_count.set(ctx.as_context_mut(), Val::I32((obj_table_count + 1) as i32));
    value::encode_object_handle(obj_table_count)
}
```


- [ ] **Step 3: 更新所有调用点**

在 `runtime_heap.rs` 中搜索所有 `alloc_host_object_from_caller` 调用，改为 `alloc_host_object(ctx, env, ...)`。

在 `host_imports/` 中搜索所有 `alloc_host_object_from_caller` 调用。

- [ ] **Step 4: 编译验证**

```bash
cd /home/soeur/project/wjsm && cargo check 2>&1 | grep -E "error|warning" | head -20
```

- [ ] **Step 5: Commit**

```bash
git add crates/wjsm-runtime/src/runtime_heap.rs
git commit -m "refactor: unify alloc_host_object with WasmEnv generics"
```

---

#### Task 1.3: 统一 runtime_values.rs 核心函数

**Files:**
- Modify: `crates/wjsm-runtime/src/runtime_values.rs`

- [ ] **Step 1: 统一 resolve_handle_idx**

需要处理的函数（从 caller 版本提取 `caller.get_export(...)` 调用 → 使用 `env` 参数）：
- `resolve_handle_idx`
- `resolve_array_ptr`
- `read_array_length` / `write_array_length`
- `read_array_elem` / `write_array_elem`
- `read_object_property_by_name`
- `find_property_slot_by_name_id`

模式：将 `caller.get_export("memory")` 替换为 `env.memory`，`caller.get_export("__obj_table_ptr")` 替换为 `env.obj_table_ptr`。

以 `resolve_handle_idx` 为例：

```rust
pub(crate) fn resolve_handle_idx<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    handle_idx: usize,
) -> Option<usize> {
    let obj_table_ptr = env.obj_table_ptr.get(ctx.as_context()).i32().unwrap_or(0) as usize;
    let slot_addr = obj_table_ptr + handle_idx * 4;
    let d = env.memory.data(ctx.as_context());
    if slot_addr + 4 > d.len() {
        return None;
    }
    let ptr = u32::from_le_bytes([
        d[slot_addr], d[slot_addr + 1], d[slot_addr + 2], d[slot_addr + 3],
    ]) as usize;
    if ptr == 0 { None } else { Some(ptr) }
}
```

- [ ] **Step 2: 更新所有 caller 调用点，添加 env 参数**

- [ ] **Step 3: 编译验证**

```bash
cd /home/soeur/project/wjsm && cargo check 2>&1 | grep -Ec "error"
```

- [ ] **Step 4: Commit**

```bash
git add crates/wjsm-runtime/src/runtime_values.rs
git commit -m "refactor: unify runtime_values with WasmEnv generics"
```

---

#### Task 1.4: 统一 runtime_host_helpers.rs

**Files:**
- Modify: `crates/wjsm-runtime/src/runtime_host_helpers.rs`

- [ ] **Step 1: 将 caller 版本函数改为泛型版本**

影响函数（部分）：
- `read_shadow_arg` — 使用 `shadow_sp` global
- `alloc_array` — 使用 memory + heap_ptr + obj_table_ptr + obj_table_count
- `alloc_object` — 同上
- `alloc_promise` — 同上
- `read_value_string_bytes` — 使用 memory
- `find_memory_c_string` — 使用 memory
- `alloc_heap_c_string` — 使用 memory + heap_ptr
- `define_host_data_property` — 使用 memory
- `store_runtime_string` — 使用 runtime_strings (通过 data())
- `set_array_elem` — 使用 memory

模式相同：添加 `env: &WasmEnv` 参数，用 `env.memory` 替换 `caller.get_export("memory")`。

- [ ] **Step 2: 更新所有 caller 调用点**

- [ ] **Step 3: 编译验证**

```bash
cd /home/soeur/project/wjsm && cargo check 2>&1 | grep -Ec "error"
```

Expected: `0`

- [ ] **Step 4: Commit**

```bash
git add crates/wjsm-runtime/src/runtime_host_helpers.rs
git commit -m "refactor: unify runtime_host_helpers with WasmEnv generics"
```

---

#### Task 1.5: 统一 runtime_promises.rs 核心 5 个函数

**Files:**
- Modify: `crates/wjsm-runtime/src/runtime_promises.rs`

- [ ] **Step 1: 合并 drain_microtasks**

将 `drain_microtasks_from_caller` 和 `drain_microtasks_from_store` 合并为泛型版本：

```rust
pub(crate) fn drain_microtasks<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
) {
    loop {
        let task = {
            let mut queue = ctx.data().microtask_queue.lock().expect("microtask queue mutex");
            queue.pop_front()
        };
        match task { /* ... 统一处理逻辑 ... */ }
    }
    // unhandled rejection tracking
}
```

**关键：** 在 `CleanupFinalizationRegistry` 处理中，caller 和 store 的差异（caller 直接调用 callback，store 通过 pending_cleanup_callbacks）。**统一版本采用 store 的延迟处理策略**，caller 上下文也先入队 `pending_cleanup_callbacks`。

- [ ] **Step 2: 合并 resolve_promise**

```rust
pub(crate) fn resolve_promise<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    promise: i64,
    resolution: i64,
) { /* 统一逻辑 */ }
```

- [ ] **Step 3: 合并 call_host_function**

```rust
pub(crate) fn call_host_function<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    handler: i64,
    argument: i64,
) -> Option<i64> { /* 统一逻辑 */ }
```

- [ ] **Step 4: 合并 resume_async_function**

```rust
pub(crate) fn resume_async_function<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    fn_table_idx: u32,
    continuation: i64,
    state: u32,
    resume_val: i64,
    is_rejected: bool,
) { /* 统一逻辑 */ }
```

- [ ] **Step 5: 合并 handle_combinator_reaction**

```rust
pub(crate) fn handle_combinator_reaction<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    handler: i64,
    argument: i64,
) -> bool { /* 统一逻辑 */ }
```

- [ ] **Step 6: 更新 host_imports/ 中的调用点**

在 `host_imports/promise_async.rs` 中，找到所有 `drain_microtasks_from_caller(...)` 调用，替换为：
```rust
let env = WasmEnv::from_caller(&mut caller).expect("WasmEnv extraction");
drain_microtasks(&mut caller, &env);
```

- [ ] **Step 7: 更新 execute_with_writer 中的调用点**

在 `lib.rs:execute_with_writer` 中，构造 `WasmEnv` 一次性，所有 `_from_store` 调用替换为泛型版本：

```rust
let env = WasmEnv {
    memory,
    func_table,
    shadow_sp: shadow_sp_global,
    heap_ptr: heap_ptr_global,
    obj_table_ptr: obj_table_ptr_global,
    obj_table_count: obj_table_count_global,
};
drain_microtasks(&mut store, &env);
```

- [ ] **Step 8: 删除所有 _from_caller 和 _from_store 旧函数**

- [ ] **Step 9: 运行全量测试**

```bash
cd /home/soeur/project/wjsm && cargo nextest run --workspace 2>&1 | tail -10
```

Expected: 0 failures, all async tests pass.

- [ ] **Step 10: Commit**

```bash
git add crates/wjsm-runtime/src/runtime_promises.rs crates/wjsm-runtime/src/lib.rs crates/wjsm-runtime/src/host_imports/promise_async.rs
git commit -m "refactor: unify runtime_promises with WasmEnv generics, eliminate ~740 line duplication"
```

---

### Phase 2: PromiseReaction enum 重构

#### Task 2.1: PromiseReactionKind enum 替换 handler 字段重载

**Files:**
- Modify: `crates/wjsm-runtime/src/lib.rs` — PromiseReaction, PromiseReactionKind, ReactionType
- Modify: `crates/wjsm-runtime/src/runtime_promises.rs` — queue_promise_reactions, drain_microtasks, settle_promise
- Modify: `crates/wjsm-runtime/src/host_imports/promise_async.rs` — async_function_suspend, promise_then 等

- [ ] **Step 1: 在 lib.rs 中定义 PromiseReactionKind 并修改 PromiseReaction**

```rust
#[derive(Clone)]
enum PromiseReactionKind {
    Normal { handler: i64 },
    AsyncResume { fn_table_idx: u32, state: u32 },
}

#[derive(Clone)]
struct PromiseReaction {
    kind: PromiseReactionKind,
    target_promise: i64,
    reaction_type: ReactionType,
}

impl PromiseReaction {
    fn new(handler: i64, target_promise: i64, reaction_type: ReactionType) -> Self {
        Self {
            kind: PromiseReactionKind::Normal { handler },
            target_promise,
            reaction_type,
        }
    }
    fn new_async(
        fn_table_idx: u32,
        target_promise: i64,
        reaction_type: ReactionType,
        state: u32,
    ) -> Self {
        Self {
            kind: PromiseReactionKind::AsyncResume { fn_table_idx, state },
            target_promise,
            reaction_type,
        }
    }
}
```

- [ ] **Step 2: 更新 queue_promise_reactions — 用 match kind 替代 if async_resume_state**

```rust
pub(crate) fn queue_promise_reactions(
    state: &RuntimeState,
    reactions: Vec<PromiseReaction>,
    value: i64,
    is_rejected: bool,
) {
    let mut queue = state.microtask_queue.lock().expect("microtask queue mutex");
    for reaction in reactions {
        match reaction.kind {
            PromiseReactionKind::AsyncResume { fn_table_idx, state: resume_state } => {
                queue.push_back(Microtask::AsyncResume {
                    fn_table_idx,
                    continuation: reaction.target_promise,
                    state: resume_state,
                    resume_val: value,
                    is_rejected,
                });
            }
            PromiseReactionKind::Normal { handler } => {
                queue.push_back(Microtask::PromiseReaction {
                    promise: reaction.target_promise,
                    reaction_type: reaction.reaction_type,
                    handler,
                    argument: value,
                });
            }
        }
    }
}
```

- [ ] **Step 3: 更新 async_function_suspend 中的反应创建**

将所有 `PromiseReaction::new_async(cont_fn_idx as i64, continuation, ...)` 改为使用正确的类型：

```rust
PromiseReaction::new_async(cont_fn_idx, continuation, ReactionType::Fulfill, state)
```

- [ ] **Step 4: 编译验证 + 测试**

```bash
cd /home/soeur/project/wjsm && cargo check 2>&1 | grep -Ec "error"
```

```bash
cd /home/soeur/project/wjsm && cargo nextest run --workspace 2>&1 | tail -10
```

- [ ] **Step 5: Commit**

```bash
git add crates/wjsm-runtime/src/lib.rs crates/wjsm-runtime/src/runtime_promises.rs
git commit -m "refactor: PromiseReactionKind enum replaces handler field overloading"
```

---

### Phase 3: Bug 修复

#### Task 3.1: Bug 1 — async_generator_next 在 Completed 状态下行为错误

**Files:**
- Modify: `crates/wjsm-runtime/src/host_imports/promise_async.rs:1134-1158`
- Create: `fixtures/happy/async_generator_next_completed.js`
- Create: `fixtures/happy/async_generator_next_completed.expected`

- [ ] **Step 1: 创建重现测试 fixture**

```javascript
// fixtures/happy/async_generator_next_completed.js
async function* gen() {
  yield 1;
}
async function main() {
  const g = gen();
  const r1 = await g.next();    // { value: 1, done: false }
  const r2 = await g.next();    // { value: undefined, done: true }
  const r3 = await g.next();    // 再次调用 Completed generator
  console.log(JSON.stringify(r1));
  console.log(JSON.stringify(r2));
  console.log(JSON.stringify(r3));
}
main();
```

```text
// fixtures/happy/async_generator_next_completed.expected
exit_code: 0
--- stdout ---
{"value":1,"done":false}
{"done":true}
{"done":true}
--- stderr ---
```

- [ ] **Step 2: 运行测试确认 bug**

```bash
cd /home/soeur/project/wjsm && cargo build && cargo nextest run -E 'test(async_generator_next_completed)' 2>&1 | grep -E "PASS|FAIL"
```

Expected: FAIL (bug 未修复时)

- [ ] **Step 3: 修复代码**

在 `async_generator_next_fn` 的开头添加 Completed 状态检查：

```rust
// 在 async_generator_next_fn 的 Func::wrap 闭包开头添加
let handle = value::decode_object_handle(generator) as usize;
let is_completed = {
    let table = caller.data().async_generator_table.lock().expect("async gen table mutex");
    table.get(handle).map(|e| matches!(e.state, AsyncGeneratorState::Completed)).unwrap_or(false)
};
if is_completed {
    let result = alloc_iterator_result_from_caller(&mut caller, value::encode_undefined(), true);
    return resolve_promise_from_caller(&mut caller, resume_promise, result); // resolve is called after alloc
}
```

实际上应该改为：

```rust
// async_generator_next_fn 中，在 alloc_promise 之后立即检查
let handle = value::decode_object_handle(generator) as usize;
{
    let table = caller.data().async_generator_table.lock()...;
    if let Some(entry) = table.get(handle) {
        if matches!(entry.state, AsyncGeneratorState::Completed) {
            drop(table);
            let result = alloc_iterator_result_from_caller(&mut caller, value::encode_undefined(), true);
            resolve_promise_from_caller(&mut caller, resume_promise, result);
            return resume_promise;
        }
    }
}
// 之后才是正常的 state = SuspendedYield 逻辑
```

- [ ] **Step 4: 运行测试确认修复**

```bash
cd /home/soeur/project/wjsm && cargo build && cargo nextest run -E 'test(async_generator_next_completed)' 2>&1 | tail -5
```

Expected: PASS

- [ ] **Step 5: 运行全部 async 测试确认无回归**

```bash
cd /home/soeur/project/wjsm && cargo nextest run -E 'test(happy__async_) | test(happy__promise_) | test(happy__for_await_) | test(happy__tla_)' 2>&1 | tail -5
```

- [ ] **Step 6: Commit**

```bash
git add fixtures/happy/async_generator_next_completed.js fixtures/happy/async_generator_next_completed.expected
git add crates/wjsm-runtime/src/host_imports/promise_async.rs
git commit -m "fix: async_generator_next on Completed generator returns {done:true}"
```

---

#### Task 3.2: Bug 2 — async_function_suspend 双重锁获取

**Files:**
- Modify: `crates/wjsm-runtime/src/host_imports/promise_async.rs:909-933`

- [ ] **Step 1: 修复为单次锁获取**

将函数中的两次 `continuation_table.lock()` 合并为一次：

```rust
let async_function_suspend_fn = Func::wrap(
    &mut store,
    |caller: Caller<'_, RuntimeState>, continuation: i64, awaited_promise: i64, state: i64| {
        let cont_handle = value::decode_object_handle(continuation) as usize;
        
        // 单次锁获取：同时更新状态和读取 fn_table_idx
        let cont_fn_idx = {
            let mut c_table = caller.data().continuation_table.lock()
                .expect("continuation table mutex");
            let Some(entry) = c_table.get_mut(cont_handle) else {
                return; // continuation 不存在，静默返回
            };
            while entry.captured_vars.len() < 4 {
                entry.captured_vars.push(value::encode_undefined());
            }
            entry.captured_vars[0] = value::encode_f64(state as f64);
            entry.captured_vars[1] = value::encode_bool(false);
            entry.fn_table_idx
        };
        
        // 之后用 cont_fn_idx 操作 promise_table...
    },
);
```

- [ ] **Step 2: 运行测试**

```bash
cd /home/soeur/project/wjsm && cargo nextest run -E 'test(happy__async_)' 2>&1 | tail -5
```

- [ ] **Step 3: Commit**

```bash
git add crates/wjsm-runtime/src/host_imports/promise_async.rs
git commit -m "fix: merge duplicate continuation_table lock in async_function_suspend"
```

---

#### Task 3.3: Bug 3 — async_generator_return/throw 不检查状态

**Files:**
- Modify: `crates/wjsm-runtime/src/host_imports/promise_async.rs:1161-1240`
- Create: `fixtures/happy/async_generator_return_executing.js`
- Create: `fixtures/happy/async_generator_return_executing.expected`

- [ ] **Step 1: 创建测试 fixture**

```javascript
// fixtures/happy/async_generator_return_executing.js
async function* gen() {
  yield 1;
  yield 2;
  yield 3;
}
async function main() {
  const g = gen();
  console.log(JSON.stringify(await g.next()));
  // 在 Executing 状态下不应 crash，return 应排队
  const retPromise = g.return("early");
  console.log(JSON.stringify(await retPromise));
  console.log(JSON.stringify(await g.next()));
}
main();
```

- [ ] **Step 2: 修复 async_generator_return_fn**

```rust
// async_generator_return_fn 中，检查状态而非直接设 Completed
let (needs_immediate, active, queued) = {
    let mut table = caller.data().async_generator_table.lock()...;
    let Some(entry) = table.get_mut(handle) else { return value::encode_undefined(); };
    
    match entry.state {
        AsyncGeneratorState::Executing => {
            // 正在执行 → 排队，不改变状态
            let promise = alloc_promise(&mut caller, PromiseEntry::pending());
            entry.queue.push(AsyncGeneratorRequest {
                completion_type: AsyncGeneratorCompletionType::Return,
                value,
                promise,
            });
            (false, None, vec![])
        }
        AsyncGeneratorState::SuspendedStart => {
            // 尚未开始 → 直接 fulfill，标记 Completed
            entry.state = AsyncGeneratorState::Completed;
            (true, None, vec![])
        }
        _ => {
            // SuspendedYield 等 → 立即终止
            entry.state = AsyncGeneratorState::Completed;
            let active = entry.active_request.take();
            let queued = std::mem::take(&mut entry.queue);
            (true, active, queued)
        }
    }
};
```

- [ ] **Step 3: 运行测试**

```bash
cd /home/soeur/project/wjsm && cargo nextest run -E 'test(async_generator_return)' 2>&1 | tail -5
```

- [ ] **Step 4: 运行全部 async 测试**

```bash
cd /home/soeur/project/wjsm && cargo nextest run --workspace 2>&1 | tail -5
```

- [ ] **Step 5: Commit**

```bash
git add fixtures/happy/async_generator_return_executing.js fixtures/happy/async_generator_return_executing.expected
git add crates/wjsm-runtime/src/host_imports/promise_async.rs
git commit -m "fix: async_generator return/throw check state before completing"
```

---

### Phase 4: 性能优化

#### Task 4.1: AsyncGeneratorEntry.queue Vec → VecDeque

**Files:**
- Modify: `crates/wjsm-runtime/src/lib.rs` — AsyncGeneratorEntry.queue 类型
- Modify: `crates/wjsm-runtime/src/runtime_promises.rs` — pump_async_generator 的 remove(0)
- Modify: `crates/wjsm-runtime/src/host_imports/promise_async.rs` — async_generator_return/throw 的 queue 操作

- [ ] **Step 1: 修改类型定义**

```rust
use std::collections::VecDeque;

struct AsyncGeneratorEntry {
    state: AsyncGeneratorState,
    continuation: i64,
    active_request: Option<AsyncGeneratorRequest>,
    waiting_resume_promise: Option<i64>,
    queue: VecDeque<AsyncGeneratorRequest>,  // Vec → VecDeque
}
```

初始化改为 `VecDeque::new()`。

- [ ] **Step 2: 替换所有 queue 操作**

- `entry.queue.remove(0)` → `entry.queue.pop_front().unwrap()`
- `entry.queue.push(...)` → `entry.queue.push_back(...)` (VecDeque 也支持 push_back)

- [ ] **Step 3: 运行测试**

```bash
cd /home/soeur/project/wjsm && cargo nextest run --workspace 2>&1 | tail -5
```

- [ ] **Step 4: Commit**

```bash
git add crates/wjsm-runtime/src/lib.rs crates/wjsm-runtime/src/runtime_promises.rs crates/wjsm-runtime/src/host_imports/promise_async.rs
git commit -m "perf: AsyncGeneratorEntry.queue Vec→VecDeque for O(1) pop_front"
```

---

#### Task 4.2: Continuation 表的生命周期回收

**Files:**
- Modify: `crates/wjsm-runtime/src/lib.rs` — ContinuationEntry 加 completed 字段
- Modify: `crates/wjsm-runtime/src/runtime_promises.rs` — drain_microtasks 后回收

- [ ] **Step 1: 添加 completed 字段**

```rust
struct ContinuationEntry {
    fn_table_idx: u32,
    outer_promise: i64,
    captured_vars: Vec<i64>,
    completed: bool,  // 新增
}
```

- [ ] **Step 2: drain_microtasks 结束后回收**

```rust
// drain_microtasks 结束时
let mut c_table = ctx.data().continuation_table.lock().expect("continuation table mutex");
c_table.retain(|entry| !entry.completed);
```

- [ ] **Step 3: 标记 continuation 为 completed**

在 `resume_async_function` 处理完成后（async 函数已返回），检查 outer_promise 是否已 settled，如果是则标记 completed：

```rust
// 在 resume_async_function 中，resume 后检查
{
    let mut c_table = ctx.data().continuation_table.lock()...;
    if let Some(entry) = c_table.get_mut(cont_handle as usize) {
        if is_promise_settled(ctx.data(), entry.outer_promise) {
            entry.completed = true;
        }
    }
}
```

- [ ] **Step 4: 运行测试**

```bash
cd /home/soeur/project/wjsm && cargo nextest run --workspace 2>&1 | tail -5
```

- [ ] **Step 5: Commit**

```bash
git add crates/wjsm-runtime/src/
git commit -m "perf: recycle completed ContinuationEntries after drain_microtasks"
```

---

#### Task 4.3: 未处理 rejection 追踪优化

**Files:**
- Modify: `crates/wjsm-runtime/src/lib.rs` — RuntimeState 加字段
- Modify: `crates/wjsm-runtime/src/runtime_promises.rs` — settle_promise, drain_microtasks

- [ ] **Step 1: RuntimeState 加字段**

```rust
struct RuntimeState {
    // ...
    pending_unhandled_rejections: HashSet<usize>,
}
```

- [ ] **Step 2: settle_promise 中跟踪**

```rust
// settle_promise 中，reject 且 !handled 时
if matches!(settlement, PromiseSettlement::Reject(_)) && !entry.handled {
    state.pending_unhandled_rejections.insert(handle);
}
```

- [ ] **Step 3: .then()/.catch() 中移除**

```rust
// 标记 handled=true 时
entry.handled = true;
state.pending_unhandled_rejections.remove(&handle);
```

- [ ] **Step 4: drain_microtasks 结束后只检查集合**

替换全表扫描：

```rust
let unhandled: Vec<i64> = {
    let rejections = std::mem::take(&mut ctx.data().pending_unhandled_rejections);
    let table = ctx.data().promise_table.lock()...;
    rejections.iter()
        .filter_map(|&h| match &table[h].state {
            PromiseState::Rejected(reason) if !table[h].handled => Some(*reason),
            _ => None,
        })
        .collect()
};
```

- [ ] **Step 5: 运行测试**

```bash
cd /home/soeur/project/wjsm && cargo nextest run --workspace 2>&1 | tail -5
```

- [ ] **Step 6: Commit**

```bash
git add crates/wjsm-runtime/src/
git commit -m "perf: use pending_unhandled_rejections set to avoid full table scan"
```

---

#### Task 4.4: NativeCallable free-list 复用

**Files:**
- Modify: `crates/wjsm-runtime/src/lib.rs` — RuntimeState 加 free_slots

- [ ] **Step 1: RuntimeState 加字段**

```rust
native_callable_free_slots: Vec<u32>,
```

- [ ] **Step 2: create 时优先从 free_slots 分配**

```rust
pub(crate) fn create_native_callable(state: &RuntimeState, callable: NativeCallable) -> i64 {
    let mut table = state.native_callables.lock()...;
    let handle = if let Some(slot) = state.native_callable_free_slots.pop() {
        table[slot as usize] = callable;
        slot
    } else {
        let handle = table.len() as u32;
        table.push(callable);
        handle
    };
    value::encode_native_callable_idx(handle)
}
```

- [ ] **Step 3: drop 时将索引入队**

在 combinator 完成、promise settled 等场景下，将不再使用的 native_callable 条目索引 push 回 free_slots。

- [ ] **Step 4: 运行测试**

```bash
cd /home/soeur/project/wjsm && cargo nextest run --workspace 2>&1 | tail -5
```

- [ ] **Step 5: Commit**

```bash
git add crates/wjsm-runtime/src/
git commit -m "perf: native_callable free-list recycling"
```

---

### Phase 5: 文件拆分

#### Task 5.1: 拆分 host_imports/promise_async.rs

**Files:**
- Create: `crates/wjsm-runtime/src/host_imports/promise.rs`
- Create: `crates/wjsm-runtime/src/host_imports/promise_combinators.rs`
- Create: `crates/wjsm-runtime/src/host_imports/async_fn.rs`
- Create: `crates/wjsm-runtime/src/host_imports/async_generator.rs`
- Create: `crates/wjsm-runtime/src/host_imports/proxy_reflect.rs`
- Create: `crates/wjsm-runtime/src/host_imports/misc.rs`
- Modify: `crates/wjsm-runtime/src/host_imports/promise_async.rs` — 保留为 re-export hub
- Modify: `crates/wjsm-runtime/src/host_imports/mod.rs` — 注册新模块

- [ ] **Step 1: 创建 promise.rs**

从 `promise_async.rs` 中提取 Promise.create, resolve/reject, then/catch/finally, withResolvers, is_promise, create_resolve/reject_function (~400 行)。

- [ ] **Step 2: 创建 promise_combinators.rs**

提取 Promise.all/race/allSettled/any (~400 行)。

- [ ] **Step 3: 创建 async_fn.rs**

提取 async_function_start/resume/suspend, continuation_create/save/load (~300 行)。

- [ ] **Step 4: 创建 async_generator.rs**

提取 async_generator_start/next/return/throw (~300 行)。

- [ ] **Step 5: 创建 proxy_reflect.rs**

提取 proxy_create, reflect_get/set/has/delete, proxy_revocable (~500 行)。

- [ ] **Step 6: 创建 misc.rs**

提取 native_call, eval, jsx, module_namespace, dynamic_import, queue_microtask, drain_microtasks, is_callable (~400 行)。

- [ ] **Step 7: 更新 promise_async.rs 为 re-export hub**

```rust
mod promise;
mod promise_combinators;
mod async_fn;
mod async_generator;
mod proxy_reflect;
mod misc;

pub(crate) use promise::*;
pub(crate) use promise_combinators::*;
pub(crate) use async_fn::*;
pub(crate) use async_generator::*;
pub(crate) use proxy_reflect::*;
pub(crate) use misc::register_all_imports;
```

- [ ] **Step 8: 运行全量测试**

```bash
cd /home/soeur/project/wjsm && cargo nextest run --workspace 2>&1 | tail -10
```

Expected: 0 failures.

- [ ] **Step 9: Commit**

```bash
git add crates/wjsm-runtime/src/host_imports/
git commit -m "refactor: split promise_async.rs into 6 focused modules"
```

---

#### Task 5.2: 拆分 runtime_promises.rs

**Files:**
- Create: `crates/wjsm-runtime/src/runtime_microtask.rs`
- Create: `crates/wjsm-runtime/src/runtime_combinators.rs`
- Create: `crates/wjsm-runtime/src/runtime_async_fn.rs`
- Modify: `crates/wjsm-runtime/src/runtime_promises.rs` — 保留核心 promise 逻辑
- Modify: `crates/wjsm-runtime/src/lib.rs` — 注册新模块

- [ ] **Step 1: 创建 runtime_microtask.rs**

移动 `drain_microtasks`（统一版本）、unhandled rejection tracking。

- [ ] **Step 2: 创建 runtime_combinators.rs**

移动 combinator context create/set/mark/reaction handler/decrement。

- [ ] **Step 3: 创建 runtime_async_fn.rs**

移动 `resume_async_function`、`enqueue_async_resume`、`pump_async_generator`。

- [ ] **Step 4: 在 lib.rs 注册**

```rust
mod runtime_microtask;
mod runtime_combinators;
mod runtime_async_fn;
```

- [ ] **Step 5: 运行全量测试**

```bash
cd /home/soeur/project/wjsm && cargo nextest run --workspace 2>&1 | tail -10
```

- [ ] **Step 6: Commit**

```bash
git add crates/wjsm-runtime/src/
git commit -m "refactor: split runtime_promises.rs into 4 focused modules"
```

---

### 最终验证

- [ ] **Full workspace test suite**

```bash
cd /home/soeur/project/wjsm && cargo nextest run --workspace 2>&1 | tail -10
```

Expected: 0 failures. All 49+ async tests pass.

- [ ] **Run test262 async subset**

```bash
cd /home/soeur/project/wjsm && cargo run -p wjsm-test262 2>&1 | grep -E "async|promise|generator" | head -20
```

- [ ] **Final commit if needed**

```bash
git add -A && git commit -m "chore: final verification of async audit refactor"
git push
```
