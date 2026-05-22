# Async Iteration 完善

## 问题

`for-await-of` 和 async generator 基础实现已就绪，但缺失关键语义：

1. **`%AsyncIteratorPrototype%`** 不存在 — 没有 `[Symbol.asyncIterator]()` 返回 `this` 的原型对象
2. **`for-await-of` 使用同步迭代协议** — 当前 `lower_for_await_of` 调用 `IteratorFrom`（同步迭代器获取），而非按 spec 走 `[Symbol.asyncIterator]()` 协议
3. **`AsyncGenerator.prototype` 无正确原型链** — async generator 实例是裸对象，不继承自 `%AsyncIteratorPrototype%`
4. **缺少 `CreateAsyncFromSyncIterator`** — 当 sync iterable 用于 `for-await-of` 时，spec 要求自动包装为异步迭代器

当前对 async generator 的 `for-await-of` **碰巧** 能工作：async generator 恰好有 `.next()` 方法（返回 Promise），被 `IteratorFrom` 当作普通迭代器处理，结果被 `Promise.resolve()` 包裹后 await。但对自定义 `[Symbol.asyncIterator]` 对象或同步可迭代对象则会失败。

## 根因

### 1. Semantic 层 — `lower_for_await_of`

```
当前：复用同步迭代协议
  iter_handle = IteratorFrom(iterable)        // ← 没有 [Symbol.asyncIterator] 查找
  next_result = IteratorNext(iter_handle)      // ← 同步 .next() 调用（返回 Promise）
  wrapped = PromiseResolveStatic(undef, next_result)  // ← 多余包装（no-op）
  Suspend { promise: wrapped, state }
```

规范要求（[ECMAScript § 14.7.5.13](https://tc39.es/ecma262/#sec-runtime-semantics-forinofbodyevaluation)）：
1. `GetMethod(iterable, @@asyncIterator)` → 如果存在则调用
2. 如果不存在 → `GetMethod(iterable, @@iterator)` → 调用 → `CreateAsyncFromSyncIterator(syncIterator)`
3. 循环中调用 `asyncIterator.next()`，await 结果

### 2. Runtime 层 — `IteratorFrom` (Import 4)

当前只处理：字符串、数组、有 `.next()` 方法的对象。缺少 `Symbol.asyncIterator` 查找、回退、async-from-sync 包装。

### 3. Runtime 层 — `async_generator_start` (Import 137)

```rust
let generator = alloc_object(&mut caller, 4);  // 裸对象，默认继承 Object.prototype
// 缺少：[[Prototype]] = AsyncGenerator.prototype
```

## 方案

### 核心设计决策

**复用现有 `IteratorState::ObjectIter` 管线。** `AsyncIteratorFrom` 获取异步迭代器后，提取 `.next()`/`.return()` 方法，注册为 `IteratorState::ObjectIter`。`IteratorNext`/`IteratorClose` builtin 保持不变。

`Promise.resolve()` 包装在当前管线中为 no-op（`.next()` 已返回 Promise），可移除以简化 IR，但不是必须。

### 1. IR 层

**新增 builtin：**

```rust
// wjsm-ir/src/builtin.rs
AsyncIteratorFrom,  // (iterable: i64) -> iterator_handle: i64 (TAG_ITERATOR)
```

### 2. Runtime 层

#### 2a. `%AsyncIteratorPrototype%`

在 `execute_with_writer` 中（module instantiation 前）创建：

```
async_iterator_proto = alloc_object(…)
// [Symbol.asyncIterator]() → return this
define_data_property(async_iterator_proto, WK_SYMBOL_ASYNC_ITERATOR, {
  value: native_callable(AsyncIteratorProtoSymbolAsyncIterator),
  writable: true, configurable: true, enumerable: false,
})
// [Symbol.toStringTag] = "AsyncIterator"
define_data_property(async_iterator_proto, WK_SYMBOL_TO_STRING_TAG, {
  value: string("AsyncIterator"),
  writable: false, configurable: true, enumerable: false,
})
```

`NativeCallable::AsyncIteratorProtoSymbolAsyncIterator` — 调用时直接返回 `this_val`。

#### 2b. `AsyncGenerator.prototype`

```
async_gen_proto = alloc_object(…)
reflect_set_prototype(async_gen_proto, async_iterator_proto)
// next / return / throw → 复用现有 async_generator_next/return/throw host function import
define_data_property(async_gen_proto, "next", …)
define_data_property(async_gen_proto, "return", …)
define_data_property(async_gen_proto, "throw", …)
// [Symbol.toStringTag] = "AsyncGenerator"
```

存储在 `RuntimeState.async_gen_prototype: i64`。

#### 2c. 修改 `async_generator_start`

```rust
let generator = alloc_object(&mut caller, 4);
reflect_set_prototype(generator, caller.data().async_gen_prototype);  // 新增
// 挂载 next/return/throw + Symbol.asyncIterator（逻辑不变）
```

`next`/`return`/`throw` 方法现改为从 `AsyncGenerator.prototype` 继承读取，而非每次创建新的 native callable。即 `async_generator_start` 不再单独创建 `create_async_generator_method` — 改为直接拷贝 `AsyncGenerator.prototype` 上的方法引用。

实际实现：`AsyncGenerator.prototype` 上的 `next`/`return`/`throw` 仍为 native callable，创建实例时通过 `define_data_property` 复制引用（或直接用 getOwnProperty + define 从 prototype 拷贝）。async generator 实例的 `[Symbol.asyncIterator]` 仍为实例自身方法（返回 `this`），不可从 prototype 继承（那样会返回 prototype 而非实例）。

#### 2d. `AsyncIteratorFrom` host function (新 Import 378)

```rust
fn async_iterator_from(caller, iterable: i64) -> i64:
  // Step 1: GetMethod(iterable, @@asyncIterator)
  async_method = get_method_by_symbol(caller, iterable, ASYNC_ITERATOR_SYMBOL_ID)
  if !is_undefined(async_method):
    async_iter = call(caller, async_method, iterable, &[])
    return register_as_object_iter(caller, async_iter)
  
  // Step 2: fallback — GetMethod(iterable, @@iterator)
  sync_method = get_method_by_symbol(caller, iterable, ITERATOR_SYMBOL_ID)
  if is_undefined(sync_method):
    return throw_type_error(caller, "object is not iterable")
  
  sync_iter = call(caller, sync_method, iterable, &[])
  
  // Step 3: CreateAsyncFromSyncIterator(sync_iter)
  wrapped = create_async_from_sync_iterator(caller, sync_iter)
  return register_as_object_iter(caller, wrapped)

// 辅助：提取 .next/.return 并注册 IteratorState::ObjectIter
fn register_as_object_iter(caller, obj: i64) -> i64:
  next = get_prop(obj, "next")
  return_method = get_prop(obj, "return")  // 可能不存在
  iters.push(IteratorState::ObjectIter { next, return_method, ... })
  return encode_handle(TAG_ITERATOR, handle)
```

注册到 `iterators` 表而非新建表，复用 `IteratorNext`/`IteratorClose`。

#### 2e. `CreateAsyncFromSyncIterator`

新增数据结构：

```rust
// lib.rs
struct AsyncFromSyncIteratorEntry {
    sync_iterator: i64,   // 同步迭代器句柄 (TAG_ITERATOR)
    done: bool,
}
// RuntimeState 新增字段
async_from_sync_iterators: Arc<Mutex<Vec<AsyncFromSyncIteratorEntry>>>
```

创建包装对象：
- `[[Prototype]]` = `%AsyncIteratorPrototype%`
- `next()`: NativeCallable → 调同步 `IteratorNext` → `Promise.resolve({value, done})`；若 `done` 已为 true，直接 `Promise.resolve({value: undefined, done: true})`
- `return()`: NativeCallable → 若同步迭代器有 `.return()` 则调用，否则直接 `Promise.resolve({value, done: true})`
- `throw()`: NativeCallable → `Promise.reject(value)`，不调同步迭代器

`NativeCallable` 新增三个变体：`AsyncFromSyncNext`, `AsyncFromSyncReturn`, `AsyncFromSyncThrow`，各持有 `async_from_sync_handle`。

### 3. Semantic 层

修改 `lower_for_await_of`（`crates/wjsm-semantic/src/lowerer_stmt.rs`）：

**迭代器获取：**
```
// 旧：Builtin::IteratorFrom(iterable)
// 新：Builtin::AsyncIteratorFrom(iterable)
```

**移除 Promise.resolve() 包装：**
```
// 旧：next_result = PromiseResolveStatic(undef, next_call_result)
//     Suspend { promise: next_result, state }
// 新：Suspend { promise: next_call_result, state }
```
因为 `async_iterator.next()` 已返回 Promise，`Promise.resolve()` 是 no-op。移除后 IR 更清晰。

**关闭路径：** 保持现有逻辑（`label_context.iterator_to_close` → `IteratorClose` on break）。对于 async generator，`.return()` 返回 Promise，但 `IteratorClose` 不 await。这是已有的不完整行为，不在本次 scope 内修复（见"不做什么"）。

### 4. 原型链结构图

```
%AsyncIteratorPrototype%
  ├── [Symbol.asyncIterator]() → return this
  └── [Symbol.toStringTag] = "AsyncIterator"
        ↑ [[Prototype]]
  AsyncGenerator.prototype
    ├── next(value)     → Promise<{value, done}>
    ├── return(value)   → Promise<{value, done}>
    ├── throw(value)    → Promise<{value, done}>
    └── [Symbol.toStringTag] = "AsyncGenerator"
          ↑ [[Prototype]]
    async generator 实例
      ├── next          → 同 AsyncGenerator.prototype.next
      ├── return        → 同 AsyncGenerator.prototype.return
      ├── throw         → 同 AsyncGenerator.prototype.throw
      └── [Symbol.asyncIterator]() → return this  (实例自身方法)
```

### 5. AsyncFromSyncIterator 结构

```
%AsyncIteratorPrototype%
  ↑ [[Prototype]]
async-from-sync iterator 实例
  ├── next()    → Promise<IteratorResult>
  ├── return()  → Promise<IteratorResult>
  └── throw()   → Promise<rejected>

内部状态 (AsyncFromSyncIteratorEntry):
  sync_iterator: i64  // 同步迭代器句柄 (TAG_ITERATOR)
  done: bool
```

## 影响范围

| 文件 | 修改内容 |
|---|---|
| `crates/wjsm-ir/src/builtin.rs` | 新增 `AsyncIteratorFrom` 变体 + Display impl |
| `crates/wjsm-backend-wasm/src/compiler_core.rs` | 注册 `AsyncIteratorFrom` 为 import 378 |
| `crates/wjsm-backend-wasm/src/compiler_builtins.rs` | `AsyncIteratorFrom` → call 378 的编译分支 |
| `crates/wjsm-backend-wasm/src/lib.rs` | `"async_iterator_from"` → import name 映射 |
| `crates/wjsm-runtime/src/lib.rs` | 新增 `AsyncFromSyncIteratorEntry`、`async_from_sync_iterators` 表、`async_gen_prototype`、`async_iterator_prototype` 字段；`NativeCallable` 新增 4 个变体；创建 `%AsyncIteratorPrototype%` 和 `AsyncGenerator.prototype` |
| `crates/wjsm-runtime/src/host_imports/promise_async.rs` | 修改 `async_generator_start` 设置原型链；新增 `async_iterator_from` import 378；新增 `create_async_from_sync_iterator` 辅助函数 |
| `crates/wjsm-runtime/src/runtime_builtins.rs` | 处理 4 个新 `NativeCallable` 变体的调用分发 |
| `crates/wjsm-semantic/src/lowerer_stmt.rs` | `lower_for_await_of`: `IteratorFrom` → `AsyncIteratorFrom`，移除 `PromiseResolveStatic` 包装 |

## 测试

新增 fixtures：

| 文件 | 预期 |
|---|---|
| `fixtures/happy/for_await_sync_array.js` | `for await (x of [1,2,3])` 正确迭代 |
| `fixtures/happy/for_await_custom_async_iter.js` | 带 `[Symbol.asyncIterator]` 的自定义对象 |
| `fixtures/happy/async_iterator_proto.js` | `[Symbol.asyncIterator]()` 返回 this |
| `fixtures/happy/async_gen_proto_chain.js` | async generator 实例原型链检查 |
| `fixtures/errors/for_await_non_iterable.js` | `for await (x of 42)` 抛出 TypeError |

现有 fixture `for_await_async_generator.js` 应继续通过。需要更新 `fixtures/semantic/for_await_async_generator.ir`（如果该快照存在）。

## 不做什么

- **不实现 Iterator Helpers proposal** (`.map()`, `.filter()`, `.reduce()` 等) — Stage 3，非 core
- **不暴露 `AsyncIterator` 全局构造函数** — spec 中为 intrinsic
- **不修改同步 `for-of`** — 行为不变
- **不处理 `for-await-of` 中 break/return 时的异步 close**（`.return()` 的 await）— 这是已有的不完整行为，本次 scope 是补齐基础协议；close 路径的完整 await 需要 Suspend 支持，留待后续
- **不处理 `yield*` 与 async iteration 的交互** — 独立特性

## 风险

- **低风险**：`lower_for_await_of` 修改仅影响 async 函数内，现有测试只覆盖 async generator
- **兼容性**：现有 `for_await_async_generator` fixture 应继续通过
- **test262**：本地 submodule 未初始化，`async-iteration` feature 已在 `SUPPORTED_FEATURES` 中注册
