# WeakRef + FinalizationRegistry 设计文档

ES2021 `WeakRef` / `FinalizationRegistry` 特性实现。采用**侧表 + GC sweep 阶段感知**方案，不修改 GC 标记热路径。

## 1. 数据模型

### 1.1 NaN-boxing

不新增 tag。WeakRef 和 FinalizationRegistry 实例使用 `TAG_OBJECT`，内部 handle 通过隐藏属性存储（`__weakref_handle__` / `__finalization_registry_handle__`），模式与 WeakMap 的 `__weakmap_handle__` 完全一致。

### 1.2 侧表结构（`RuntimeState` 新增字段）

```rust
weakref_table: Arc<Mutex<Vec<WeakRefEntry>>>,
finalization_registry_table: Arc<Mutex<Vec<FinalizationRegistryEntry>>>,
/// GC 后待调度的清理回调：Vec<(callback_fn, [held_values])>。
/// 在 process_weak_references 中填充，由微任务调度消费。
pending_cleanup_callbacks: Arc<Mutex<Vec<(i64, Vec<i64>)>>>,

```rust
struct WeakRefEntry {
    /// target 对象的 handle index。0 表示已回收。
    target_handle: u32,
}

struct FinalizationRegistryEntry {
    /// FinalizationRegistry 对象自身的 handle index（用于 GC 存活检查）
    object_handle: u32,
    /// cleanup callback（i64 函数值），由 registry 对象强引用保持存活
    callback: i64,
    /// 所有活跃注册
    registrations: Vec<FinalizationRegistration>,

struct FinalizationRegistration {
    target_handle: u32,
    held_value: i64,
    /// None 表示未提供 unregister token
    unregister_token: Option<i64>,
}
```

### 1.3 生命周期

- **WeakRef 对象存活 + target 存活** → `deref()` 返回 target
- **WeakRef 对象存活 + target 已回收** → `deref()` 返回 undefined
- **WeakRef 对象被回收** → 侧表 entry 不再被引用（但保留在 Vec 中，成为"空洞"）。可选：GC 时一并清理已回收的 WeakRef entry。
- **FinalizationRegistry 对象存活** → 其 callback 保持可达；`register` 正常工作
- **FinalizationRegistry 对象被回收** → 侧表 entry 成为空洞，其所有注册静默丢弃（不触发回调，因为 callback 已不可达）
- **target 被回收** → 该 target 的所有注册产生回调（每个 registry 独立调度），注册条目从 registrations 中移除
- **同一 target 被多次 register（同一或不同 registry）** → 每次注册独立处理
- **target 存活但 unregister 被调用** → 匹配的注册条目移除，不触发回调

## 2. IR 层（`wjsm-ir`）

### 2.1 `Builtin` 枚举新增变体

```rust
// ── WeakRef built-in ──────────────────────────────────────────────
WeakRefConstructor,
WeakRefProtoDeref,
// ── FinalizationRegistry built-in ─────────────────────────────────
FinalizationRegistryConstructor,
FinalizationRegistryProtoRegister,
FinalizationRegistryProtoUnregister,
```

### 2.2 `Display` impl 新增

`WeakRefConstructor` → `"WeakRef"`，`WeakRefProtoDeref` → `"WeakRef.prototype.deref"`，`FinalizationRegistryConstructor` → `"FinalizationRegistry"`，`FinalizationRegistryProtoRegister` → `"FinalizationRegistry.prototype.register"`，`FinalizationRegistryProtoUnregister` → `"FinalizationRegistry.prototype.unregister"`。

## 3. 语义层（`wjsm-semantic`）

### 3.1 `builtins.rs`

- `BUILTIN_GLOBALS`: 已有 `"WeakRef"` 和 `"FinalizationRegistry"`，无需修改。
- `builtin_from_global_ident`:
  - `"WeakRef"` → `Some(Builtin::WeakRefConstructor)`
  - `"FinalizationRegistry"` → `Some(Builtin::FinalizationRegistryConstructor)`
- `builtin_from_static_member`:
  - `("WeakRef", "prototype", "deref")` → `Some(Builtin::WeakRefProtoDeref)`
  - `("FinalizationRegistry", "prototype", "register")` → `Some(Builtin::FinalizationRegistryProtoRegister)`
  - `("FinalizationRegistry", "prototype", "unregister")` → `Some(Builtin::FinalizationRegistryProtoUnregister)`
- `builtin_call_signature`: 为新变体补充调用约定信息，全部使用 Type 12（影子栈）。

### 3.2 scope 规则

无新增作用域规则。`new.target` 检查：`WeakRef` 和 `FinalizationRegistry` 可作为函数调用（无 `new`），行为等同于 `new` 调用——这是 ES2021 规范要求。

## 4. 后端（`wjsm-backend-wasm`）

### 4.1 `HOST_IMPORT_NAMES` 追加（从 356 → 361）

```rust
const HOST_IMPORT_NAMES: [&str; 361] = [
    // ... 现有 356 项保持不变 ...
    // ── WeakRef / FinalizationRegistry imports ──
    "weakref_constructor",              // index 356
    "weakref_proto_deref",              // 357
    "finalization_registry_constructor", // 358
    "finalization_registry_proto_register", // 359
    "finalization_registry_proto_unregister", // 360
];
```

### 4.2 `builtin_arity` 映射

```rust
Builtin::WeakRefConstructor => ("WeakRef", 1),
Builtin::WeakRefProtoDeref => ("WeakRef.prototype.deref", 1),
Builtin::FinalizationRegistryConstructor => ("FinalizationRegistry", 1),
Builtin::FinalizationRegistryProtoRegister => ("FinalizationRegistry.prototype.register", 4),
Builtin::FinalizationRegistryProtoUnregister => ("FinalizationRegistry.prototype.unregister", 2),
```

### 4.3 `compiler_builtins.rs` — `compile_builtin`

为 5 个新变体发射 Type 12 影子栈调用序列：
- `env_obj` = undefined
- `this_val` = args[0]
- `shadow_args` = args[1..]

特例：`WeakRefProtoDeref` 仅 1 个 arg (this_val)，无 shadow_args。

## 5. 运行时（`wjsm-runtime`）

### 5.1 新文件：`host_imports/weakref_finalization.rs`

包含 5 个导入函数的实现。通过 `include!` 机制嵌入到 `execute_with_writer` 中的 import vector。

### 5.2 WeakRef constructor

```
fn weakref_constructor(caller, target: i64) -> i64:
  1. 校验 target: is_js_object(target) || is_symbol(target)，否则返回 TypeError
  2. resolve_handle → target_handle: u32
  3. push WeakRefEntry { target_handle } → weakref_table，得 handle_idx
  4. obj = alloc_host_object(caller, 2)
  5. define_host_data_property(obj, "__weakref_handle__", encode_f64(handle_idx))
  6. deref_fn = create_weakref_deref_method(state)
  7. define_host_data_property(obj, "deref", deref_fn)
  8. return obj
```

### 5.3 WeakRef.prototype.deref

```
fn weakref_proto_deref(caller, this_val: i64) -> i64:
  1. 从 this_val 读取 __weakref_handle__ → handle_idx
  2. weakref_table[handle_idx].target_handle:
     a. 若 == 0 → return undefined (已回收)
     b. resolve_handle_idx(handle) → obj_ptr
        - 若 None → return undefined (handle table 已失效)
        - 若 Some → return 原始 i64 值 (重新 encode object handle)
```

注意：`deref()` 不检查 mark_bits——GC sweep 阶段已将回收 target 的 entry 清零。deref 只需检查 handle 是否仍有效。

### 5.4 FinalizationRegistry constructor

```
fn finalization_registry_constructor(caller, callback: i64) -> i64:
  1. 校验: is_callable(callback)，否则 typeof 检查 → TypeError
  2. obj = alloc_host_object(caller, 3)
  3. object_handle = decode_object_handle(obj)
  4. push FinalizationRegistryEntry { object_handle, callback, registrations: [] } → table
  5. define_host_data_property(obj, "__finalization_registry_handle__", encode_f64(handle_idx))
  6. register_fn = create_fr_register_method(state)
  7. unregister_fn = create_fr_unregister_method(state)
  8. define_host_data_property(obj, "register", register_fn)
  9. define_host_data_property(obj, "unregister", unregister_fn)
  10. return obj

### 5.5 FinalizationRegistry.prototype.register

```
fn fr_proto_register(caller, this_val, target, held_value, token?) -> i64:
  1. 校验: is_js_object(target) && !is_symbol(target)，否则 TypeError
  2. resolve_handle(target) → target_handle: u32
  3. 从 this_val 读取 __finalization_registry_handle__ → fr_handle_idx
  4. 获取 entry = &mut finalization_registry_table[fr_handle_idx]
  5. entry.registrations.push(FinalizationRegistration {
       target_handle,
       held_value,
       unregister_token: token_if_not_undefined,
     })
  6. return undefined
```

### 5.6 FinalizationRegistry.prototype.unregister

```
fn fr_proto_unregister(caller, this_val, token) -> i64:
  1. 从 this_val 读取 __finalization_registry_handle__ → fr_handle_idx
  2. 获取 entry = &mut finalization_registry_table[fr_handle_idx]
  3. removed = 0
  4. entry.registrations.retain(|reg| {
       if reg.unregister_token == Some(token) { removed += 1; false }
       else { true }
     })
  5. return encode_bool(removed > 0)
```

### 5.7 GC 集成

在 `gc_collect` 的 sweep/compact 完成后，新增 `process_weak_references` 步骤：

```
gc_collect 流程:
  1. mark 阶段（现有，不修改）
  2. sweep/compact 阶段（现有，不修改）
  3. [新增] process_weak_references:
     a. 锁定 gc_mark_bits (此时仍保留标记结果)
     b. 遍历 weakref_table:
        for entry in &mut weakref_table:
            if !is_marked(entry.target_handle):
                entry.target_handle = 0  // 清零表示已回收

     c. 遍历 finalization_registry_table:
        对每个 entry，检查 entry.object_handle 的 mark_bit:
           - 若 == 0 → 该 FinalizationRegistry 对象已回收，跳过（不触发回调）
        for entry in &mut finalization_registry_table (仅 entry.object_handle 存活):
            let mut held_values = vec![]
            entry.registrations.retain(|reg| {
                if !is_marked(reg.target_handle):
                    held_values.push(reg.held_value)
                    false  // 移除此注册
                else:
                    true
            })
            if !held_values.is_empty():
                pending_cleanup_callbacks.push((entry.callback, held_values))

     d. 遍历 pending_cleanup_callbacks:
        for (callback, held_values) in pending_cleanup_callbacks.drain(..):
            对每个 held_value:
                queue_microtask(Microtask::CleanupFinalizationRegistry {
                    callback,
                    held_value,
                })
```

`is_marked(handle_idx)` 实现：检查 `gc_mark_bits[handle_idx / 64] & (1 << (handle_idx % 64)) != 0`。

### 5.8 微任务调度

新增 `Microtask::CleanupFinalizationRegistry { callback: i64, held_value: i64 }` 变体。在 `run_microtask` 中处理：

```
Microtask::CleanupFinalizationRegistry { callback, held_value }:
  1. call_host_function(callback, undefined, [held_value])
  2. 忽略返回值和异常（规范要求：回调中抛出的异常不会传播，类似 Promise 回调）
```

注意：清理回调**不应**阻止其他微任务的执行。它们被调度为普通微任务，在现有 microtask 队列中排队。

## 6. test262 支持

### 6.1 显式 GC 触发

test262 中 WeakRef/FinalizationRegistry 测试依赖 `$262.gc()` 显式触发 GC。需要在 test262 harness 中暴露此能力。

方案：在 test262 运行器的全局作用域注入 `gc()` 函数，调用时直接触发 `gc_collect` 导入 → 完整的 mark + sweep + process_weak_references 流程。

### 6.2 需要的 fixture

- `fixtures/happy/weakref.js`: 基本构造、deref、目标存活/回收
- `fixtures/happy/finalization_registry.js`: 构造、register、unregister、回调触发
- `fixtures/semantic/weakref.ir`: IR snapshot
- `fixtures/semantic/finalization_registry.ir`: IR snapshot
- `fixtures/errors/weakref_non_object.js`: TypeError 检查

## 7. 实现顺序

1. **IR 层**: `Builtin` 枚举 + `Display` impl → 0.5h
2. **backend-wasm**: `HOST_IMPORT_NAMES` + `builtin_arity` + `compile_builtin` → 1h
3. **语义层**: `builtins.rs` 4 个函数的映射 + `builtin_call_signature` → 0.5h
4. **运行时侧表 + 导入函数**: `weakref_finalization.rs` + `RuntimeState` 字段 → 2h
5. **GC 集成**: `gc_collect` 中 `process_weak_references` + `Microtask` 变体 → 1.5h
6. **test262 harness**: 注入 `gc()` 函数 → 0.5h
7. **Fixture + 测试**: happy/error fixtures + E2E 验证 → 1h
8. **test262 验证**: 运行 built-ins/WeakRef + built-ins/FinalizationRegistry 套件 → 0.5h

## 8. 已知限制

1. **`CleanupSome` 未实现** — ES2021 规范的可选方法，允许同步执行部分待处理的清理回调。wjsm 的单线程协作式模型中不适用，test262 也不依赖它。
2. **WeakRef 侧表空洞** — 当 WeakRef 对象自身被回收时，其侧表 entry 保留在 Vec 中不会被复用。对于长时间运行的应用程序可能造成内存泄漏，但不会影响正确性。可在后续 GC 优化中处理（标记阶段识别存活 WeakRef，sweep 阶段清理未引用 entry）。
3. **FinalizationRegistry 回调异常** — 规范要求回调抛出异常时不传播（类似 Promise 回调）。wjsm 通过微任务调度实现此行为（微任务中的异常会被捕获并记录，不中断事件循环）。
4. **Symbol 作为 WeakRef target** — `new WeakRef(symbol)` 允许，但 `new FinalizationRegistry(...).register(symbol, ...)` 不允许（规范要求 target 必须是 Object，非 Symbol）。本设计遵循此规范。
