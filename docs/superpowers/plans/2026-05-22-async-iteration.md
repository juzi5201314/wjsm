# Async Iteration 完善 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 补齐 ECMAScript 异步迭代协议：`%AsyncIteratorPrototype%` 原型链、`for-await-of` 的 `[Symbol.asyncIterator]()` 协议、`CreateAsyncFromSyncIterator` 包装。

**Architecture:** 新增 1 个 IR builtin (`AsyncIteratorFrom`)，复用现有 `IteratorState::ObjectIter` + `IteratorNext`/`IteratorClose` 管线。Runtime 创建原型对象并新增 `AsyncFromSyncIterator` 包装逻辑。Semantic 层改一行 builtin 调用。

**Tech Stack:** Rust 2024 + wjsm IR + wasm-encoder + wasmtime

**Spec:** `docs/superpowers/specs/2026-05-22-async-iteration-design.md`

---

## 文件结构

| 文件 | 改动类型 | 职责 |
|---|---|---|
| `crates/wjsm-ir/src/builtin.rs` | 修改 | 新增 `AsyncIteratorFrom` 枚举变体 |
| `crates/wjsm-backend-wasm/src/compiler_core.rs` | 修改 | 注册 import 378 + builtin index |
| `crates/wjsm-backend-wasm/src/compiler_builtins.rs` | 修改 | `AsyncIteratorFrom` → WASM call 编译 |
| `crates/wjsm-backend-wasm/src/lib.rs` | 修改 | `"async_iterator.from"` → import 名映射 |
| `crates/wjsm-runtime/src/lib.rs` | 修改 | 新类型 + 新 state 字段 + 原型创建 |
| `crates/wjsm-runtime/src/host_imports/promise_async.rs` | 修改 | Import 378 + 修改 `async_generator_start` + `CreateAsyncFromSyncIterator` |
| `crates/wjsm-runtime/src/runtime_builtins.rs` | 修改 | 新 `NativeCallable` 变体调用分发 |
| `crates/wjsm-semantic/src/lowerer_stmt.rs` | 修改 | `lower_for_await_of` 改用 `AsyncIteratorFrom` |
| `fixtures/happy/for_await_sync_array.js` | 新建 | 测试 sync array 的 for-await |
| `fixtures/happy/for_await_custom_async_iter.js` | 新建 | 测试自定义 async iterable |
| `fixtures/happy/async_iterator_proto.js` | 新建 | 测试 %AsyncIteratorPrototype% |
| `fixtures/happy/async_gen_proto_chain.js` | 新建 | 测试 async generator 原型链 |
| `fixtures/errors/for_await_non_iterable.js` | 新建 | 测试非 iterable 的 for-await |
| `fixtures/semantic/for_await_async_generator.ir` | 更新 | IR 快照变化 |

---

### Task 1: IR 层 — 新增 `AsyncIteratorFrom` builtin

**Files:**
- Modify: `crates/wjsm-ir/src/builtin.rs`

- [ ] **Step 1: 在 Builtin 枚举中添加 `AsyncIteratorFrom` 变体**

在 `IteratorClose` 之后插入：

```rust
// crates/wjsm-ir/src/builtin.rs — Builtin 枚举中 IteratorClose 之后
    IteratorClose,
    /// AsyncIteratorFrom — 获取异步迭代器(先尝试 @@asyncIterator, 回退到 @@iterator)
    AsyncIteratorFrom,
    IteratorValue,
```

- [ ] **Step 2: 添加 Display impl**

```rust
// builtin.rs — Display impl for Builtin (在 IteratorClose 分支后)
            Self::IteratorClose => "iterator.close",
            Self::AsyncIteratorFrom => "async_iterator.from",
            Self::IteratorValue => "iterator.value",
```

- [ ] **Step 3: 编译检查**

```bash
cargo check -p wjsm-ir
```

期望：编译通过。

---

### Task 2: Backend WASM — 注册 import 和编译分支

**Files:**
- Modify: `crates/wjsm-backend-wasm/src/compiler_core.rs`
- Modify: `crates/wjsm-backend-wasm/src/compiler_builtins.rs`
- Modify: `crates/wjsm-backend-wasm/src/lib.rs`

- [ ] **Step 1: compiler_core.rs — 添加 import 声明**

在 atomics imports 之后（最后一行 `imports.import("env", "atomics_wait_async", ...)` 之后）：

```rust
        // Import index 378: async_iterator_from: (i64) -> i64
        imports.import("env", "async_iterator_from", EntityType::Function(3));
```

- [ ] **Step 2: compiler_core.rs — 注册 builtin → import index 映射**

在 `builtin_func_indices.insert(Builtin::AbortShadowStackOverflow, 77);` 附近，找到 iterator 相关行，在 `IteratorClose` 之后插入：

```rust
        builtin_func_indices.insert(Builtin::IteratorClose, 6);
        builtin_func_indices.insert(Builtin::AsyncIteratorFrom, 378);
        builtin_func_indices.insert(Builtin::IteratorValue, 7);
```

- [ ] **Step 3: compiler_builtins.rs — 添加编译分支**

在 `Builtin::IteratorFrom | Builtin::EnumeratorFrom` 分支附近，插入新分支（与 IteratorFrom 模式相同）：

```rust
            Builtin::AsyncIteratorFrom => {
                let val = args
                    .first()
                    .context("AsyncIteratorFrom expects 1 arg")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(val.0)));
                let func_idx = self.builtin_func_indices.get(builtin).copied().unwrap_or(0);
                self.emit(WasmInstruction::Call(func_idx));
            }
```

- [ ] **Step 4: lib.rs — 添加 import name 映射**

在 `Builtin::IteratorClose => ("iterator.close", 1),` 之后：

```rust
        Builtin::IteratorClose => ("iterator.close", 1),
        Builtin::AsyncIteratorFrom => ("async_iterator.from", 1),
        Builtin::IteratorValue => ("iterator.value", 1),
```

- [ ] **Step 5: 编译检查**

```bash
cargo check -p wjsm-backend-wasm
```

期望：编译通过。

---

### Task 3: Runtime lib.rs — 新类型、State 字段、原型创建

**Files:**
- Modify: `crates/wjsm-runtime/src/lib.rs`

- [ ] **Step 1: 添加 `AsyncFromSyncIteratorEntry` 结构体**

在 `AsyncGeneratorCompletionType` 定义之后添加：

```rust
/// async-from-sync iterator 内部状态
#[derive(Clone, Debug)]
struct AsyncFromSyncIteratorEntry {
    /// 同步迭代器句柄 (TAG_ITERATOR handle)
    sync_iterator: i64,
    /// 同步迭代器是否已完成
    sync_done: bool,
}
```

- [ ] **Step 2: `NativeCallable` 枚举新增 4 个变体**

在现有 `AsyncGeneratorIdentity { generator }` 之后添加：

```rust
    /// %AsyncIteratorPrototype%[Symbol.asyncIterator]() → return this
    AsyncIteratorProtoSymbolAsyncIterator,
    /// AsyncFromSyncIterator.prototype.next()
    AsyncFromSyncNext {
        handle: u32,
    },
    /// AsyncFromSyncIterator.prototype.return()
    AsyncFromSyncReturn {
        handle: u32,
    },
    /// AsyncFromSyncIterator.prototype.throw()
    AsyncFromSyncThrow {
        handle: u32,
    },
```

- [ ] **Step 3: `RuntimeState` 新增字段**

在 `async_generator_table` 字段之后添加：

```rust
    /// async-from-sync iterator 侧表
    async_from_sync_iterators: Arc<Mutex<Vec<AsyncFromSyncIteratorEntry>>>,
    /// %AsyncIteratorPrototype% 对象
    async_iterator_prototype: i64,
    /// AsyncGenerator.prototype 对象
    async_gen_prototype: i64,
```

- [ ] **Step 4: 在 `execute_with_writer` 中初始化新表和原型对象**

在 `let async_generator_table: Arc<Mutex<Vec<AsyncGeneratorEntry>>> = ...` 之后添加：

```rust
    let async_from_sync_iterators: Arc<Mutex<Vec<AsyncFromSyncIteratorEntry>>> =
        Arc::new(Mutex::new(Vec::new()));
```

在 `let mut store = Store::new(&engine, RuntimeState { ... })` 之前，创建两个原型对象。使用 `alloc_host_object_from_store` 和已有的 store 环境。由于原型创建需要 store 已存在，改为在 store 创建之后、imports 组装之前进行。在 `let mut store = Store::new(...)` 之后、`let mut imports: Vec<Extern> = ...` 之前：

```rust
    // ── 创建 %AsyncIteratorPrototype% ──
    let async_iterator_proto = alloc_object(&mut store, 2);
    let symbol_async_iterator =
        value::encode_symbol(3); // WK_SYMBOL_ASYNC_ITERATOR = 3
    let identity_callable = {
        let mut table = store.data().native_callables.lock().expect("native callables");
        let handle = table.len() as u32;
        table.push(NativeCallable::AsyncIteratorProtoSymbolAsyncIterator);
        value::encode_native_callable_idx(handle)
    };
    define_host_data_property(
        &mut store,
        async_iterator_proto,
        "Symbol.asyncIterator",
        identity_callable,
    );
    let async_iterator_tag = store_runtime_string(&mut store, "AsyncIterator".to_string());
    define_host_data_property(
        &mut store,
        async_iterator_proto,
        "Symbol.toStringTag",
        async_iterator_tag,
    );

    // ── 创建 AsyncGenerator.prototype ──
    let async_gen_proto = alloc_object(&mut store, 4);
    // 设置 [[Prototype]] = %AsyncIteratorPrototype%
    {
        // 通过 resolve_handle + reflect_set_prototype_of_fn_impl 设置原型
        // 这里直接操作堆内存中的 proto 指针来设置原型链
        let gen_proto_ptr =
            resolve_handle(&mut store, async_gen_proto)
                .expect("async_gen_proto handle resolve");
        let aip_proto_ptr =
            resolve_handle(&mut store, async_iterator_proto)
                .expect("async_iterator_proto handle resolve");
        // 写 proto 指针到对象 header (offset 4, u32 handle_idx)
        let memory = store
            .get_export("memory")
            .and_then(|e| e.into_memory())
            .expect("memory export");
        let data = memory.data_mut(&mut store);
        let proto_handle = async_iterator_proto;
        data[gen_proto_ptr + 4..gen_proto_ptr + 8]
            .copy_from_slice(&value::decode_object_handle(async_iterator_proto).to_le_bytes());
    }
    // [Symbol.toStringTag] = "AsyncGenerator"
    let ag_tag = store_runtime_string(&mut store, "AsyncGenerator".to_string());
    define_host_data_property(&mut store, async_gen_proto, "Symbol.toStringTag", ag_tag);

    store.data_mut().async_iterator_prototype = async_iterator_proto;
    store.data_mut().async_gen_prototype = async_gen_proto;
```

注意：上述代码中的 `define_host_data_property` 需要 `&mut Caller` 或等效方法。代码中 `&mut store` 和已导出的 `define_host_data_property_from_store` 配合使用。由于原型创建需要访问 store internal data，使用 `store.data_mut()` 方式。

实际实现时参考 `reflect_set_prototype_of_fn_impl` 的实现方式直接操作堆内存设置 proto 指针。

- [ ] **Step 5: 在 `RuntimeState` 初始化中传入新字段**

在 `Store::new` 的 `RuntimeState { ... }` 结构体中添加新字段的 clone：

```rust
            async_from_sync_iterators: Arc::clone(&async_from_sync_iterators),
            async_iterator_prototype: async_iterator_proto,
            async_gen_prototype: async_gen_proto,
```

同时确保 store 创建后，在 store 上修改 `async_iterator_prototype` 和 `async_gen_prototype` 字段（因为 store 已经 move 了 RuntimeState）：

在 `let mut store = Store::new(...)` 之后、任何 imports 之前：

```rust
    store.data_mut().async_iterator_prototype = async_iterator_proto;
    store.data_mut().async_gen_prototype = async_gen_proto;
```

并在 `RuntimeState` 的 `Store::new` 初始化中使用占位值：

```rust
            async_iterator_prototype: value::encode_undefined(),
            async_gen_prototype: value::encode_undefined(),
```

- [ ] **Step 6: 编译检查**

```bash
cargo check -p wjsm-runtime
```

期望：编译通过（可能有一些 dead_code 警告，属于正常）。

---

### Task 4: Runtime promise_async.rs — Import 378 + async_generator_start 修改 + CreateAsyncFromSyncIterator

**Files:**
- Modify: `crates/wjsm-runtime/src/host_imports/promise_async.rs`

- [ ] **Step 1: 修改 `async_generator_start` — 设置原型链**

在 `let generator = alloc_object(&mut caller, 4);` 之后、`if !value::is_object(generator)` 检查之前，插入原型设置：

```rust
            let generator = alloc_object(&mut caller, 4);

            // 设置 [[Prototype]] = AsyncGenerator.prototype
            {
                let async_gen_proto = caller.data().async_gen_prototype;
                if !value::is_undefined(async_gen_proto) {
                    let gen_ptr = resolve_handle(&mut caller, generator);
                    if let Some(ptr) = gen_ptr {
                        let memory = caller
                            .get_export("memory")
                            .and_then(|e| e.into_memory())
                            .expect("memory");
                        let data = memory.data_mut(&mut caller);
                        let proto_handle = value::decode_object_handle(async_gen_proto) as u32;
                        data[ptr + 4..ptr + 8].copy_from_slice(&proto_handle.to_le_bytes());
                    }
                }
            }

            if !value::is_object(generator) {
```

- [ ] **Step 2: 在 `promise_async.rs` 末尾添加 `create_async_from_sync_iterator` 函数**

在文件末尾（`include!` 被调用者 `lib.rs` 中）添加：

```rust
/// CreateAsyncFromSyncIterator(syncIterator)
/// 
/// 创建一个异步迭代器包装对象，该对象：
/// - next() → 调用同步迭代器的 next()，将结果包装为 Promise.resolve({value, done})
/// - return() → 调用同步迭代器的 return()（如存在），包装为 Promise
/// - throw() → Promise.reject(value)
pub(crate) fn create_async_from_sync_iterator(
    caller: &mut Caller<'_, RuntimeState>,
    sync_iter_handle: i64,
) -> i64 {
    // 分配 async-from-sync iterator entry
    let entry = AsyncFromSyncIteratorEntry {
        sync_iterator: sync_iter_handle,
        sync_done: false,
    };
    let mut table = caller
        .data()
        .async_from_sync_iterators
        .lock()
        .expect("async from sync iterators mutex");
    let handle = table.len() as u32;
    table.push(entry);
    drop(table);

    // 创建包装对象，原型为 %AsyncIteratorPrototype%
    let obj = alloc_object(caller, 3);
    let async_iterator_proto = caller.data().async_iterator_prototype;
    if !value::is_undefined(async_iterator_proto) {
        if let Some(ptr) = resolve_handle(caller, obj) {
            let memory = caller
                .get_export("memory")
                .and_then(|e| e.into_memory())
                .expect("memory");
            let data = memory.data_mut(caller);
            let proto_handle = value::decode_object_handle(async_iterator_proto) as u32;
            data[ptr + 4..ptr + 8].copy_from_slice(&proto_handle.to_le_bytes());
        }
    }

    // next() 方法
    let next_callable = {
        let mut nc_table = caller
            .data()
            .native_callables
            .lock()
            .expect("native callables");
        let nc_handle = nc_table.len() as u32;
        nc_table.push(NativeCallable::AsyncFromSyncNext { handle });
        value::encode_native_callable_idx(nc_handle)
    };
    define_host_data_property_from_caller(caller, obj, "next", next_callable);

    // return() 方法
    let return_callable = {
        let mut nc_table = caller
            .data()
            .native_callables
            .lock()
            .expect("native callables");
        let nc_handle = nc_table.len() as u32;
        nc_table.push(NativeCallable::AsyncFromSyncReturn { handle });
        value::encode_native_callable_idx(nc_handle)
    };
    define_host_data_property_from_caller(caller, obj, "return", return_callable);

    // throw() 方法
    let throw_callable = {
        let mut nc_table = caller
            .data()
            .native_callables
            .lock()
            .expect("native callables");
        let nc_handle = nc_table.len() as u32;
        nc_table.push(NativeCallable::AsyncFromSyncThrow { handle });
        value::encode_native_callable_idx(nc_handle)
    };
    define_host_data_property_from_caller(caller, obj, "throw", throw_callable);

    obj
}
```

- [ ] **Step 3: 添加 `async_iterator_from` Import 378**

在 `async_generator_throw_fn` (import 140) 之后添加：

```rust
    // ── Import 378: async_iterator_from(i64) -> i64 ──────────────────────
    let async_iterator_from_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, iterable: i64| -> i64 {
            // Step 1: 尝试 GetMethod(iterable, @@asyncIterator)
            if let Some(ptr) = resolve_handle(&mut caller, iterable) {
                let async_method = read_object_property_by_name(
                    &mut caller, ptr, "Symbol.asyncIterator"
                );
                if let Some(method) = async_method {
                    if value::is_callable(method) {
                        let async_iter =
                            call_wasm_callback(
                                &mut caller, method, iterable, &[]
                            ).unwrap_or_else(|_| value::encode_undefined());
                        if value::is_exception(async_iter) {
                            return async_iter;
                        }
                        // 注册为 ObjectIter — 提取 .next()/.return()
                        if let Some(iter_ptr) = resolve_handle(&mut caller, async_iter) {
                            let next = read_object_property_by_name(
                                &mut caller, iter_ptr, "next"
                            ).unwrap_or_else(value::encode_undefined);
                            let return_method = read_object_property_by_name(
                                &mut caller, iter_ptr, "return"
                            ).filter(|v| value::is_callable(*v));
                            let mut iters = caller
                                .data()
                                .iterators
                                .lock()
                                .expect("iterators mutex");
                            let handle = iters.len() as u32;
                            iters.push(IteratorState::ObjectIter {
                                next,
                                return_method,
                                current_value: value::encode_undefined(),
                                has_current: false,
                                done: false,
                            });
                            return value::encode_handle(value::TAG_ITERATOR, handle);
                        }
                    }
                }

                // Step 2: fallback — GetMethod(iterable, @@iterator)
                let sync_method = read_object_property_by_name(
                    &mut caller, ptr, "Symbol.iterator"
                );
                if let Some(method) = sync_method {
                    if value::is_callable(method) {
                        let sync_iter =
                            call_wasm_callback(
                                &mut caller, method, iterable, &[]
                            ).unwrap_or_else(|_| value::encode_undefined());
                        if value::is_exception(sync_iter) {
                            return sync_iter;
                        }
                        // Step 3: CreateAsyncFromSyncIterator(sync_iter)
                        // 先注册 sync_iter 到 iterators 表
                        if let Some(sync_ptr) = resolve_handle(&mut caller, sync_iter) {
                            let sync_next = read_object_property_by_name(
                                &mut caller, sync_ptr, "next"
                            ).unwrap_or_else(value::encode_undefined);
                            let sync_return = read_object_property_by_name(
                                &mut caller, sync_ptr, "return"
                            ).filter(|v| value::is_callable(*v));
                            let mut iters = caller
                                .data()
                                .iterators
                                .lock()
                                .expect("iterators mutex");
                            let sync_handle = iters.len() as u32;
                            iters.push(IteratorState::ObjectIter {
                                next: sync_next,
                                return_method: sync_return,
                                current_value: value::encode_undefined(),
                                has_current: false,
                                done: false,
                            });
                            let sync_handle_val =
                                value::encode_handle(value::TAG_ITERATOR, sync_handle);
                            drop(iters);
                            // 创建 async-from-sync 包装
                            let wrapped = create_async_from_sync_iterator(
                                &mut caller, sync_handle_val,
                            );
                            // 注册 wrapped 为 ObjectIter
                            if let Some(wrapped_ptr) = resolve_handle(&mut caller, wrapped) {
                                let w_next = read_object_property_by_name(
                                    &mut caller, wrapped_ptr, "next"
                                ).unwrap_or_else(value::encode_undefined);
                                let w_return = read_object_property_by_name(
                                    &mut caller, wrapped_ptr, "return"
                                ).filter(|v| value::is_callable(*v));
                                let mut iters2 = caller
                                    .data()
                                    .iterators
                                    .lock()
                                    .expect("iterators mutex");
                                let w_handle = iters2.len() as u32;
                                iters2.push(IteratorState::ObjectIter {
                                    next: w_next,
                                    return_method: w_return,
                                    current_value: value::encode_undefined(),
                                    has_current: false,
                                    done: false,
                                });
                                return value::encode_handle(value::TAG_ITERATOR, w_handle);
                            }
                        }
                    }
                }
            }

            // 都不是→ TypeError
            create_error_object(&mut caller, "TypeError", iterable)
        },
    );
```

- [ ] **Step 4: 在 imports 数组末尾添加新 import**

在 `promise_async.rs` 的最终返回数组处，所有 `_fn.into()` 之后添加：

```rust
        async_iterator_from_fn.into(),             // 378
```

- [ ] **Step 5: 编译检查**

```bash
cargo check -p wjsm-runtime
```

期望：编译通过。可能需要处理 `call_wasm_callback` 的实际签名和导入路径问题。

---

### Task 5: Runtime runtime_builtins.rs — NativeCallable 新变体分发

**Files:**
- Modify: `crates/wjsm-runtime/src/runtime_builtins.rs`

- [ ] **Step 1: 在 `call_native_callable_with_args_from_caller` 中添加分发分支**

在 `NativeCallable::AsyncGeneratorIdentity { generator } => Some(generator),` 之后：

```rust
        NativeCallable::AsyncIteratorProtoSymbolAsyncIterator => {
            // [Symbol.asyncIterator]() → return this
            Some(this_val)
        }
        NativeCallable::AsyncFromSyncNext { handle } => {
            let handle = *handle as usize;
            let mut table = caller
                .data()
                .async_from_sync_iterators
                .lock()
                .expect("async from sync mutex");
            if let Some(entry) = table.get_mut(handle) {
                if entry.sync_done {
                    // 迭代器已完成，返回 {value: undefined, done: true}
                    let result = alloc_object(caller, 2);
                    define_host_data_property_from_caller(
                        caller, result, "value",
                        value::encode_undefined(),
                    );
                    define_host_data_property_from_caller(
                        caller, result, "done",
                        value::encode_bool(true),
                    );
                    // Promise.resolve(result)
                    let promise = alloc_promise(caller, PromiseEntry::pending());
                    // ... resolve promise with result
                    // 简化：直接分配已解决的 promise
                    let resolve_val = result;
                    let mut p_table = caller
                        .data()
                        .promise_table
                        .lock()
                        .expect("promise table");
                    let p_handle = value::decode_object_handle(promise) as usize;
                    if p_handle < p_table.len() {
                        p_table[p_handle] = PromiseEntry::Resolved(resolve_val);
                    }
                    return Some(promise);
                }
                // 调用同步迭代器的 next()
                let sync_handle = entry.sync_iterator;
                drop(table);
                let mut iters = caller
                    .data()
                    .iterators
                    .lock()
                    .expect("iterators mutex");
                let sync_idx = value::decode_handle(sync_handle) as usize;
                let result = if let Some(IteratorState::ObjectIter {
                    next,
                    current_value: _,
                    has_current: _,
                    done: _,
                    ..
                }) = iters.get(sync_idx)
                {
                    let next_fn = *next;
                    drop(iters);
                    // 调用 sync.next()
                    let (raw_result, current_value, is_done, has_current) = {
                        // 通过 func_table 调用
                        let func_table = caller
                            .get_export("__indirect_call_table")
                            .and_then(|e| e.into_table())
                            .expect("func table");
                        advance_object_iterator_from_caller(
                            caller,
                            &func_table,
                            next_fn,
                        )
                    };
                    // 更新 done 状态
                    let mut table2 = caller
                        .data()
                        .async_from_sync_iterators
                        .lock()
                        .expect("async from sync mutex");
                    if let Some(e) = table2.get_mut(handle) {
                        e.sync_done = is_done;
                    }
                    // 构造 {value, done}
                    let result_obj = alloc_object(caller, 2);
                    define_host_data_property_from_caller(
                        caller, result_obj, "value",
                        if has_current { current_value } else { value::encode_undefined() },
                    );
                    define_host_data_property_from_caller(
                        caller, result_obj, "done",
                        value::encode_bool(is_done),
                    );
                    // Promise.resolve(result_obj)
                    Promise::resolve(caller, result_obj)
                } else {
                    value::encode_undefined()
                };
                Some(result)
            } else {
                Some(value::encode_undefined())
            }
        }
        NativeCallable::AsyncFromSyncReturn { handle } => {
            // return() → 调同步迭代器 .return() 或直接 Promise.resolve({value, done:true})
            let sync_handle = {
                let table = caller
                    .data()
                    .async_from_sync_iterators
                    .lock()
                    .expect("async from sync mutex");
                table.get(*handle as usize).map(|e| e.sync_iterator)
            };
            let result_obj = alloc_object(caller, 2);
            let value_arg = args.first().copied().unwrap_or_else(value::encode_undefined);
            define_host_data_property_from_caller(
                caller, result_obj, "value", value_arg,
            );
            define_host_data_property_from_caller(
                caller, result_obj, "done", value::encode_bool(true),
            );
            let promise = alloc_promise(caller, PromiseEntry::pending());
            let p_handle = value::decode_object_handle(promise) as usize;
            let mut p_table = caller
                .data()
                .promise_table
                .lock()
                .expect("promise table");
            if p_handle < p_table.len() {
                p_table[p_handle] = PromiseEntry::Resolved(result_obj);
            }
            Some(promise)
        }
        NativeCallable::AsyncFromSyncThrow { handle: _ } => {
            // throw() → Promise.reject(reason)
            let reason = args.first().copied().unwrap_or_else(value::encode_undefined);
            let promise = alloc_promise(caller, PromiseEntry::pending());
            let p_handle = value::decode_object_handle(promise) as usize;
            let mut p_table = caller
                .data()
                .promise_table
                .lock()
                .expect("promise table");
            if p_handle < p_table.len() {
                p_table[p_handle] = PromiseEntry::Rejected(reason);
            }
            Some(promise)
        }
```

- [ ] **Step 2: 编译检查**

```bash
cargo check -p wjsm-runtime
```

期望：编译通过（可能需要调整类型/方法名以匹配实际代码库）。

---

### Task 6: Semantic 层 — `lower_for_await_of` 改用 `AsyncIteratorFrom`

**Files:**
- Modify: `crates/wjsm-semantic/src/lowerer_stmt.rs`

- [ ] **Step 1: 修改 builtin 调用**

在 `lower_for_await_of` 函数中，找到 `Builtin::IteratorFrom`：

```rust
// 旧：
                builtin: Builtin::IteratorFrom,
                args: vec![iterable],
// 新：
                builtin: Builtin::AsyncIteratorFrom,
                args: vec![iterable],
```

- [ ] **Step 2: 移除 Promise.resolve() 包装**

在 `lower_for_await_of` 中，找到 `PromiseResolveStatic` 的调用。移除该段，直接使用 `next_call_result`：

```rust
// 旧（约在 lower_for_await_of 的 header block）：
            let next_result = self.alloc_value();
            {
                let undef_const = self.module.add_constant(Constant::Undefined);
                let undef_val = self.alloc_value();
                self.current_function.append_instruction(
                    header,
                    Instruction::Const {
                        dest: undef_val,
                        constant: undef_const,
                    },
                );
                self.current_function.append_instruction(
                    header,
                    Instruction::CallBuiltin {
                        dest: Some(next_result),
                        builtin: Builtin::PromiseResolveStatic,
                        args: vec![undef_val, next_call_result],
                    },
                );
            }
// 新：直接使用 next_call_result
            // async iterator 的 .next() 直接返回 Promise，不需要再包装
            let next_result = next_call_result;
```

- [ ] **Step 3: 编译检查**

```bash
cargo check -p wjsm-semantic
```

期望：编译通过。

---

### Task 7: 测试 — 新建 fixtures

**Files:**
- Create: `fixtures/happy/for_await_sync_array.js`
- Create: `fixtures/happy/for_await_sync_array.expected`
- Create: `fixtures/happy/for_await_custom_async_iter.js`
- Create: `fixtures/happy/for_await_custom_async_iter.expected`
- Create: `fixtures/happy/async_iterator_proto.js`
- Create: `fixtures/happy/async_iterator_proto.expected`
- Create: `fixtures/happy/async_gen_proto_chain.js`
- Create: `fixtures/happy/async_gen_proto_chain.expected`
- Create: `fixtures/errors/for_await_non_iterable.js`
- Create: `fixtures/errors/for_await_non_iterable.expected`

- [ ] **Step 1: `for_await_sync_array.js`**

```javascript
async function run() {
  let result = [];
  for await (let x of [1, 2, 3]) {
    result.push(x);
  }
  console.log(result.join(","));
}
run();
```

- [ ] **Step 2: 运行生成 `.expected`**

```bash
cargo run -- run fixtures/happy/for_await_sync_array.js
```

如果通过，手动创建 `.expected` 文件，或使用 `WJSM_UPDATE_FIXTURES=1 cargo test`。

- [ ] **Step 3: `for_await_custom_async_iter.js`**

```javascript
async function run() {
  let obj = {
    [Symbol.asyncIterator]() {
      let i = 0;
      return {
        async next() {
          if (i < 3) {
            return { value: ++i, done: false };
          }
          return { value: undefined, done: true };
        }
      };
    }
  };
  let result = [];
  for await (let x of obj) {
    result.push(x);
  }
  console.log(result.join(","));
}
run();
```

- [ ] **Step 4: `async_iterator_proto.js`**

```javascript
async function run() {
  // 通过 async generator 的 [Symbol.asyncIterator]() 验证原型链
  async function* gen() { yield 1; }
  let g = gen();
  let asyncIterMethod = g[Symbol.asyncIterator];
  // [Symbol.asyncIterator]() 应该返回 this（即 g 自身）
  let result = asyncIterMethod.call(g);
  console.log(result === g);
}
run();
```

- [ ] **Step 5: `async_gen_proto_chain.js`**

```javascript
async function run() {
  async function* gen() { yield 1; }
  let g = gen();
  // 验证方法存在
  console.log(typeof g.next);
  console.log(typeof g.return);
  console.log(typeof g.throw);
  console.log(typeof g[Symbol.asyncIterator]);
}
run();
```

- [ ] **Step 6: `for_await_non_iterable.js`**

```javascript
async function run() {
  for await (let x of 42) {
    console.log(x);
  }
}
run();
```

- [ ] **Step 7: 运行全部 fixture 测试**

```bash
cargo test -p wjsm --test fixture_runner
```

期望：所有新 fixture 通过。

---

### Task 8: 更新 IR 快照

**Files:**
- Modify: `fixtures/semantic/for_await_async_generator.ir`

- [ ] **Step 1: 运行 snapshot 测试查看 diff**

```bash
cargo test -p wjsm-semantic --test lowering_snapshots -- for_await_async_generator
```

因为 `IteratorFrom` → `AsyncIteratorFrom` 且移除了 `PromiseResolveStatic`，IR 输出会变化。

- [ ] **Step 2: 更新快照**

手动对比 diff 确认变化符合预期后，更新 `.ir` 文件：

```bash
# 复制实际输出到快照文件（或用 cargo test 输出的实际 IR 手动更新）
```

或直接跑全部快照测试，根据失败信息逐一手动更新。

- [ ] **Step 3: 确认所有现有测试通过**

```bash
cargo test
```

期望：所有测试通过（包括 fixture runner 和 snapshot）。
