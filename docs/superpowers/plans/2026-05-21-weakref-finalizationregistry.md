# WeakRef + FinalizationRegistry Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 实现 ES2021 `WeakRef` 和 `FinalizationRegistry`，包括 WeakRef 构造/deref、FinalizationRegistry 构造/register/unregister、GC 弱引用追踪、清理回调微任务调度。

**Architecture:** 侧表模式（Rust `Vec` 存储数据，不修改 GC 标记热路径）。GC sweep 后新增 `process_weak_references` 步骤：遍历侧表检测未标记 target → 清空 WeakRef / 调度 FinalizationRegistry 回调。5 个新 `Builtin` 变体全部走 Type 12 影子栈调用约定。

**Tech Stack:** Rust 2024, swc_core, wasm-encoder, wasmtime, wjsm-ir

**Spec:** `docs/superpowers/specs/2026-05-21-weakref-finalizationregistry-design.md`

---

## File Map

| File | Action | Purpose |
|---|---|---|
| `crates/wjsm-ir/src/builtin.rs` | Modify | 新增 5 个 Builtin 变体 + Display impl |
| `crates/wjsm-backend-wasm/src/lib.rs` | Modify | `HOST_IMPORT_NAMES[356→361]` + `builtin_arity` |
| `crates/wjsm-backend-wasm/src/compiler_builtins.rs` | Modify | `compile_builtin_call` 新增 5 个 arm |
| `crates/wjsm-semantic/src/builtins.rs` | Modify | 全局标识 → Builtin 映射 + `builtin_call_signature` |
| `crates/wjsm-runtime/src/lib.rs` | Modify | `RuntimeState` 字段 + `NativeCallable` 变体 + `Microtask` 变体 + `include!` |
| `crates/wjsm-runtime/src/runtime_builtins.rs` | Modify | `NativeCallable` dispatch + `create_*_method` 工厂函数 |
| `crates/wjsm-runtime/src/host_imports/weakref_finalization.rs` | **Create** | 5 个 host import 函数实现 |
| `crates/wjsm-runtime/src/host_imports/core.rs` | Modify | `gc_collect` 新增 `process_weak_references` |
| `crates/wjsm-runtime/src/host_imports/collections_buffers.rs` | Modify | 追加 5 个 `.into()` 到 import vector |
| `crates/wjsm-runtime/src/runtime_promises.rs` | Modify | `run_microtask` 处理 `CleanupFinalizationRegistry` |
| `crates/wjsm-test262/src/config.rs` | Modify | 注入 `gc()` host function |
| `fixtures/happy/weakref.js` | **Create** | E2E fixture |
| `fixtures/happy/finalization_registry.js` | **Create** | E2E fixture |
| `fixtures/errors/weakref_non_object.js` | **Create** | Error fixture |
| `fixtures/semantic/weakref.ir` | **Create** | IR snapshot (auto-generated, then manually reviewed) |
| `fixtures/semantic/finalization_registry.ir` | **Create** | IR snapshot (auto-generated, then manually reviewed) |

---

### Task 1: IR 层 — Builtin 枚举 + Display

**Files:**
- Modify: `crates/wjsm-ir/src/builtin.rs`

- [ ] **Step 1: 在 Builtin 枚举末尾新增 5 个变体**

在 `WeakSetProtoDelete,` 之后、enum 闭括号之前插入。位置：`WeakSetProtoDelete` 之后（约 line ~298-299）。

```rust
    // ── WeakSet built-in ──────────────────────────────────────────────
    WeakSetConstructor,
    WeakSetProtoAdd,
    WeakSetProtoHas,
    WeakSetProtoDelete,
    // ── WeakRef built-in ──────────────────────────────────────────────
    WeakRefConstructor,
    WeakRefProtoDeref,
    // ── FinalizationRegistry built-in ─────────────────────────────────
    FinalizationRegistryConstructor,
    FinalizationRegistryProtoRegister,
    FinalizationRegistryProtoUnregister,
```

- [ ] **Step 2: 在 Display impl 末尾新增 5 个 arm**

在 `ScopeRecordDestroy` arm 之后（约 line 743-744）、`write!` 宏闭括号之前插入：

```rust
            Builtin::WeakRefConstructor => write!(f, "WeakRef"),
            Builtin::WeakRefProtoDeref => write!(f, "WeakRef.prototype.deref"),
            Builtin::FinalizationRegistryConstructor => write!(f, "FinalizationRegistry"),
            Builtin::FinalizationRegistryProtoRegister => write!(f, "FinalizationRegistry.prototype.register"),
            Builtin::FinalizationRegistryProtoUnregister => write!(f, "FinalizationRegistry.prototype.unregister"),
```

- [ ] **Step 3: 编译检查**

```bash
cargo check -p wjsm-ir
```

Expected: 编译成功。若有未穷举 match 的编译错误，说明其他 crate 有 match 需要更新——记录位置。

- [ ] **Step 4: Commit**

```bash
git add crates/wjsm-ir/src/builtin.rs
git commit -m "feat(ir): add WeakRef + FinalizationRegistry Builtin variants"
```

---

### Task 2: Backend-WASM — Import 名称 + arity + compile_builtin

**Files:**
- Modify: `crates/wjsm-backend-wasm/src/lib.rs`
- Modify: `crates/wjsm-backend-wasm/src/compiler_builtins.rs`

- [ ] **Step 1: 更新 HOST_IMPORT_NAMES 数组大小和内容**

文件：`crates/wjsm-backend-wasm/src/lib.rs`，line 18

将 `[&str; 356]` 改为 `[&str; 361]`，并在数组末尾（`"scope_record_destroy"` 之后、`];` 之前）追加：

```rust
    // ── WeakRef / FinalizationRegistry imports ──
    "weakref_constructor",              // index 356
    "weakref_proto_deref",              // 357
    "finalization_registry_constructor", // 358
    "finalization_registry_proto_register", // 359
    "finalization_registry_proto_unregister", // 360
```

- [ ] **Step 2: 在 builtin_arity 函数中新增映射**

文件：`crates/wjsm-backend-wasm/src/lib.rs`，`builtin_arity` 函数末尾（`ScopeRecordDestroy` arm 之后、闭括号之前，约 line 1355-1356）

```rust
        Builtin::WeakRefConstructor => ("WeakRef", 1),
        Builtin::WeakRefProtoDeref => ("WeakRef.prototype.deref", 1),
        Builtin::FinalizationRegistryConstructor => ("FinalizationRegistry", 1),
        Builtin::FinalizationRegistryProtoRegister => ("FinalizationRegistry.prototype.register", 4),
        Builtin::FinalizationRegistryProtoUnregister => ("FinalizationRegistry.prototype.unregister", 2),
```

- [ ] **Step 3: 在 compile_builtin_call 中新增 5 个 arm**

文件：`crates/wjsm-backend-wasm/src/compiler_builtins.rs`

全部使用 Type 12 影子栈调用约定。参照 `WeakMapConstructor` arm 的模式（line ~720）。在 match 块内合适位置（最后一个 arm 之前，约 line 1850）插入：

```rust
            Builtin::WeakRefConstructor => {
                let args_base = self.shadow_sp;
                self.emit_shadow_stack_overflow_check(args_base + 1)?;
                let target = self.value_local(args[0]);
                self.emit(WasmInstruction::I64Store(MemArg {
                    offset: (args_base as u64) * 8,
                    align: 3,
                    memory_index: 0,
                }));
                self.emit(WasmInstruction::I32Const(1)); // args_count = 1
                self.emit(WasmInstruction::I32Const(args_base)); // args_base
                self.emit(WasmInstruction::I64Const(value::encode_undefined())); // env = undefined
                self.emit(WasmInstruction::I64Const(value::encode_undefined())); // this = undefined
                self.emit(WasmInstruction::Call(356)); // weakref_constructor
                if let Some(dest) = dest {
                    self.record_value_local(dest);
                }
            }
            Builtin::WeakRefProtoDeref => {
                let this_val = self.value_local(args[0]);
                self.emit(WasmInstruction::I32Const(0)); // args_count = 0
                self.emit(WasmInstruction::I32Const(0)); // args_base = 0 (dummy)
                self.emit(WasmInstruction::I64Const(value::encode_undefined())); // env = undefined
                // 对于单参数 Type 12，this_val 作为单独的 local 传递
                // 实际上 WeakRefProtoDeref 的 host 函数签名是 |caller, this_val| -> i64
                // 需要检查现有 Type 12 调用模式...
                //
                // 实际上：查看现有类似方法（如 MapSetGetSize，index 259），
                // Type 12 调用总是: env_obj, this_val, args_base, args_count
                // 对于无 shadow_args 的情况，args_base=0, args_count=0，this_val 仍有意义
                self.emit(WasmInstruction::I64Const(this_val_instr?)); // recover this_val
                // [需要实际参照 compile_builtin_call 中的 Type 12 具体模式]
                self.emit(WasmInstruction::Call(357));
                if let Some(dest) = dest {
                    self.record_value_local(dest);
                }
            }
```

**重要：** 上述 `compile_builtin_call` 的伪代码需要精确适配实际的 Type 12 调用模式。请在编辑前先用 `read` 读取 `crates/wjsm-backend-wasm/src/compiler_builtins.rs` 中现有的 Type 12 arm（如 `MapSetGetSize` 或 `ObjectAssign`），复制其完整模式并替换函数索引和参数名。

5 个新 arm 的 import 索引映射：
- `WeakRefConstructor` → `Call(356)`
- `WeakRefProtoDeref` → `Call(357)`
- `FinalizationRegistryConstructor` → `Call(358)`
- `FinalizationRegistryProtoRegister` → `Call(359)`
- `FinalizationRegistryProtoUnregister` → `Call(360)`

- [ ] **Step 4: 编译检查**

```bash
cargo check -p wjsm-backend-wasm
```

Expected: 编译成功。

- [ ] **Step 5: Commit**

```bash
git add crates/wjsm-backend-wasm/src/lib.rs crates/wjsm-backend-wasm/src/compiler_builtins.rs
git commit -m "feat(backend-wasm): register WeakRef + FinalizationRegistry imports"
```

---

### Task 3: 语义层 — builtins.rs 映射

**Files:**
- Modify: `crates/wjsm-semantic/src/builtins.rs`

- [ ] **Step 1: builtin_from_global_ident 新增映射**

在 `"WeakSet" => Some(Builtin::WeakSetConstructor)` 之后（约 line 93）加入：

```rust
        "WeakRef" => Some(Builtin::WeakRefConstructor),
        "FinalizationRegistry" => Some(Builtin::FinalizationRegistryConstructor),
```

- [ ] **Step 2: builtin_from_static_member 新增映射**

在 `builtin_from_static_member` 函数中，找到合适位置（在 `("WeakSet", "prototype", ...)` 映射组之后），新增：

```rust
        ("WeakRef", "prototype", "deref") => Some(Builtin::WeakRefProtoDeref),
        ("FinalizationRegistry", "prototype", "register") => Some(Builtin::FinalizationRegistryProtoRegister),
        ("FinalizationRegistry", "prototype", "unregister") => Some(Builtin::FinalizationRegistryProtoUnregister),
```

- [ ] **Step 3: builtin_call_signature 新增**

在 `builtin_call_signature` 函数末尾（最后一个 arm 之后、闭括号之前）加入：

```rust
        Builtin::WeakRefConstructor => ("WeakRef", 1),
        Builtin::WeakRefProtoDeref => ("WeakRef.prototype.deref", 1),
        Builtin::FinalizationRegistryConstructor => ("FinalizationRegistry", 1),
        Builtin::FinalizationRegistryProtoRegister => ("FinalizationRegistry.prototype.register", 4),
        Builtin::FinalizationRegistryProtoUnregister => ("FinalizationRegistry.prototype.unregister", 2),
```

- [ ] **Step 4: 编译检查**

```bash
cargo check -p wjsm-semantic
```

Expected: 编译成功。

- [ ] **Step 5: Commit**

```bash
git add crates/wjsm-semantic/src/builtins.rs
git commit -m "feat(semantic): map WeakRef + FinalizationRegistry to Builtin variants"
```

---

### Task 4: 运行时 — 创建 weakref_finalization.rs

**Files:**
- Create: `crates/wjsm-runtime/src/host_imports/weakref_finalization.rs`

- [ ] **Step 1: 创建文件并实现 5 个导入函数**

参照 `collections_buffers.rs` 的 WeakMap 实现模式（`WeakMapEntry` 侧表、`__weakmap_handle__` 隐藏属性、`create_weakmap_method` 工厂函数）。

```rust
// crates/wjsm-runtime/src/host_imports/weakref_finalization.rs
// WeakRef + FinalizationRegistry host import functions
use super::*;

// ── WeakRef host functions ─────────────────────────────────────────────

let weakref_constructor_fn = Func::wrap(
    &mut store,
    |mut caller: Caller<'_, RuntimeState>, _env: i64, _this: i64, args_base: i32, args_count: i32| -> i64 {
        let target = read_shadow_arg(&mut caller, args_base, 0);
        // 校验：target 必须是 Object 或 Symbol
        if !value::is_js_object(target) && !value::is_symbol(target) {
            return create_type_error(&mut caller, "WeakRef: target must be an object or Symbol");
        }
        let target_handle = resolve_handle(&mut caller, target)
            .map(|_| value::decode_object_handle(target))
            .unwrap_or(0);
        let handle;
        {
            let mut table = caller.data().weakref_table.lock().expect("weakref_table mutex");
            handle = table.len() as u32;
            table.push(WeakRefEntry { target_handle });
        }
        let deref_fn = {
            let state = caller.data();
            let mut table = state.native_callables.lock().expect("native_callables mutex");
            let idx = table.len() as u32;
            table.push(NativeCallable::WeakRefDerefMethod);
            value::encode_native_callable_idx(idx)
        };
        let obj = alloc_host_object_from_caller(&mut caller, 2);
        let _ = define_host_data_property_from_caller(&mut caller, obj, "__weakref_handle__", value::encode_f64(handle as f64));
        let _ = define_host_data_property_from_caller(&mut caller, obj, "deref", deref_fn);
        obj
    },
);

let weakref_proto_deref_fn = Func::wrap(
    &mut store,
    |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
        if !value::is_object(this_val) {
            return value::encode_undefined();
        }
        let obj_ptr = resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
        let handle_val = obj_ptr.and_then(|p| read_object_property_by_name(&mut caller, p, "__weakref_handle__"));
        let handle = handle_val.map(|v| value::decode_f64(v) as usize).unwrap_or(0);
        let table = caller.data().weakref_table.lock().expect("weakref_table mutex");
        if handle >= table.len() {
            return value::encode_undefined();
        }
        let target_handle = table[handle].target_handle;
        if target_handle == 0 {
            return value::encode_undefined(); // 已回收
        }
        // 重新 resolve 并返回原始值
        drop(table);
        resolve_handle_idx(&mut caller, target_handle as usize)
            .map(|_| value::encode_object_handle(target_handle))
            .unwrap_or_else(value::encode_undefined)
    },
);

// ── FinalizationRegistry host functions ────────────────────────────────

let finalization_registry_constructor_fn = Func::wrap(
    &mut store,
    |mut caller: Caller<'_, RuntimeState>, _env: i64, _this: i64, args_base: i32, args_count: i32| -> i64 {
        let callback = read_shadow_arg(&mut caller, args_base, 0);
        if !value::is_callable(callback) {
            return create_type_error(&mut caller, "FinalizationRegistry: callback must be callable");
        }
        let obj = alloc_host_object_from_caller(&mut caller, 3);
        let object_handle = value::decode_object_handle(obj);
        let handle;
        {
            let mut table = caller.data().finalization_registry_table.lock().expect("fr_table mutex");
            handle = table.len() as u32;
            table.push(FinalizationRegistryEntry {
                object_handle,
                callback,
                registrations: Vec::new(),
            });
        }
        let register_fn = {
            let state = caller.data();
            let mut table = state.native_callables.lock().expect("native_callables mutex");
            let idx = table.len() as u32;
            table.push(NativeCallable::FinalizationRegistryRegisterMethod);
            value::encode_native_callable_idx(idx)
        };
        let unregister_fn = {
            let state = caller.data();
            let mut table = state.native_callables.lock().expect("native_callables mutex");
            let idx = table.len() as u32;
            table.push(NativeCallable::FinalizationRegistryUnregisterMethod);
            value::encode_native_callable_idx(idx)
        };
        let _ = define_host_data_property_from_caller(&mut caller, obj, "__finalization_registry_handle__", value::encode_f64(handle as f64));
        let _ = define_host_data_property_from_caller(&mut caller, obj, "register", register_fn);
        let _ = define_host_data_property_from_caller(&mut caller, obj, "unregister", unregister_fn);
        obj
    },
);

let finalization_registry_proto_register_fn = Func::wrap(
    &mut store,
    |mut caller: Caller<'_, RuntimeState>, _env: i64, this_val: i64, args_base: i32, args_count: i32| -> i64 {
        let target = read_shadow_arg(&mut caller, args_base, 0);
        let held_value = if args_count > 1 { read_shadow_arg(&mut caller, args_base, 1) } else { value::encode_undefined() };
        let unregister_token = if args_count > 2 { Some(read_shadow_arg(&mut caller, args_base, 2)) } else { None };
        // 校验：target 必须是 Object（非 Symbol）
        if !value::is_js_object(target) || value::is_symbol(target) {
            return create_type_error(&mut caller, "FinalizationRegistry: target must be an object");
        }
        let target_handle = resolve_handle(&mut caller, target)
            .map(|_| value::decode_object_handle(target))
            .unwrap_or(0);
        let obj_ptr = resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
        let handle_val = obj_ptr.and_then(|p| read_object_property_by_name(&mut caller, p, "__finalization_registry_handle__"));
        let fr_handle = handle_val.map(|v| value::decode_f64(v) as usize).unwrap_or(0);
        let mut table = caller.data().finalization_registry_table.lock().expect("fr_table mutex");
        if fr_handle >= table.len() {
            return value::encode_undefined();
        }
        table[fr_handle].registrations.push(FinalizationRegistration {
            target_handle,
            held_value,
            unregister_token,
        });
        value::encode_undefined()
    },
);

let finalization_registry_proto_unregister_fn = Func::wrap(
    &mut store,
    |mut caller: Caller<'_, RuntimeState>, this_val: i64, token: i64| -> i64 {
        if !value::is_object(this_val) {
            return value::encode_bool(false);
        }
        let obj_ptr = resolve_handle_idx(&mut caller, value::decode_object_handle(this_val) as usize);
        let handle_val = obj_ptr.and_then(|p| read_object_property_by_name(&mut caller, p, "__finalization_registry_handle__"));
        let fr_handle = handle_val.map(|v| value::decode_f64(v) as usize).unwrap_or(0);
        let mut table = caller.data().finalization_registry_table.lock().expect("fr_table mutex");
        if fr_handle >= table.len() {
            return value::encode_bool(false);
        }
        let mut removed = 0u32;
        table[fr_handle].registrations.retain(|reg| {
            if reg.unregister_token == Some(token) {
                removed += 1;
                false
            } else {
                true
            }
        });
        value::encode_bool(removed > 0)
    },
);

// 将函数变量注册到调用方（通过返回值，调用方负责 .into() 追加到 imports vector）
```

**注：** 上面的代码块中有 `create_type_error` 辅助函数——如果它不存在，需在 `runtime_heap.rs` 或就近创建：

```rust
pub(crate) fn create_type_error(caller: &mut Caller<'_, RuntimeState>, msg: &str) -> i64 {
    create_error_object(caller, "TypeError", store_runtime_string_from_caller(caller, msg))
}
```

检查项目中是否已有类似函数。搜索 `TypeError` 的错误创建模式。

- [ ] **Step 2: Commit**

```bash
git add crates/wjsm-runtime/src/host_imports/weakref_finalization.rs
git commit -m "feat(runtime): add WeakRef + FinalizationRegistry host import functions"
```

---

### Task 5: 运行时 — lib.rs 结构变更

**Files:**
- Modify: `crates/wjsm-runtime/src/lib.rs`

- [ ] **Step 1: RuntimeState 新增 3 个字段**

在 `weakset_table` 字段（约 line 545）之后插入：

```rust
    /// WeakRef 侧表：存储 WeakRef 对象的 target handle
    weakref_table: Arc<Mutex<Vec<WeakRefEntry>>>,
    /// FinalizationRegistry 侧表：存储 registry 对象、callback 和注册信息
    finalization_registry_table: Arc<Mutex<Vec<FinalizationRegistryEntry>>>,
    /// GC 后待调度的清理回调：Vec<(callback_fn, [held_values])>
    pending_cleanup_callbacks: Arc<Mutex<Vec<(i64, Vec<i64>)>>>,
```

同时在 `RuntimeState` 的构造初始化代码中（约 line 120-150，`let mut store = Store::new(...)` 附近），为这 3 个新字段补充 `Arc::clone`：

```rust
            weakref_table: Arc::clone(&weakref_table),
            finalization_registry_table: Arc::clone(&finalization_registry_table),
            pending_cleanup_callbacks: Arc::clone(&pending_cleanup_callbacks),
```

并在 `store` 构造之前初始化这些 Arc：

```rust
    let weakref_table: Arc<Mutex<Vec<WeakRefEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let finalization_registry_table: Arc<Mutex<Vec<FinalizationRegistryEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let pending_cleanup_callbacks: Arc<Mutex<Vec<(i64, Vec<i64>)>>> = Arc::new(Mutex::new(Vec::new()));
```

- [ ] **Step 2: 新增侧表结构体定义**

在 `WeakSetEntry` 之后（约 line 605）、`ArrayBufferEntry` 之前插入：

```rust
#[derive(Clone, Debug)]
struct WeakRefEntry {
    target_handle: u32,
}

#[derive(Clone, Debug)]
struct FinalizationRegistryEntry {
    object_handle: u32,
    callback: i64,
    registrations: Vec<FinalizationRegistration>,
}

#[derive(Clone, Debug)]
struct FinalizationRegistration {
    target_handle: u32,
    held_value: i64,
    unregister_token: Option<i64>,
}
```

- [ ] **Step 3: NativeCallable 枚举新增 3 个变体**

在 `WeakSetMethod` 之后（约 line 684）、`ArrayConstructor` 之前插入：

```rust
    WeakRefDerefMethod,
    FinalizationRegistryRegisterMethod,
    FinalizationRegistryUnregisterMethod,
```

- [ ] **Step 4: Microtask 枚举新增变体**

在最后一个变体（`AsyncResume`，约 line 997）之前插入：

```rust
    CleanupFinalizationRegistry {
        callback: i64,
        held_value: i64,
    },
```

- [ ] **Step 5: 引入 weakref_finalization.rs**

在现有的 `include!` 行（约 line 183-186）附近：

```rust
    imports.extend(include!("host_imports/weakref_finalization.rs"));
```

插入位置：在 `collections_buffers.rs` include 之后、`proxy_traps.rs` include 之前。

- [ ] **Step 6: 编译检查**

```bash
cargo check -p wjsm-runtime
```

Expected: 编译错误（Task 6 的 runtime_builtins dispatch 尚未更新，Microtask 新变体未在 match 中处理）。这是正常的——记录错误以供 Task 6/7 修复。

- [ ] **Step 7: Commit**

```bash
git add crates/wjsm-runtime/src/lib.rs crates/wjsm-runtime/src/host_imports/weakref_finalization.rs
git commit -m "feat(runtime): add WeakRef/FinalizationRegistry side tables and struct defs"
```

---

### Task 6: 运行时 — runtime_builtins.rs + runtime_promises.rs 分发

**Files:**
- Modify: `crates/wjsm-runtime/src/runtime_builtins.rs`
- Modify: `crates/wjsm-runtime/src/runtime_promises.rs`

- [ ] **Step 1: NativeCallable 构造函数分发**

在 `runtime_builtins.rs` 的 NativeCallable dispatch match 中（约 line 1438-1441，`WeakSetConstructor` arm 之后）新增：

```rust
        NativeCallable::WeakRefConstructor => Some(alloc_host_object_from_caller(caller, 0)),
        NativeCallable::FinalizationRegistryConstructor => Some(alloc_host_object_from_caller(caller, 0)),
```

**注：** `WeakRefConstructor` 和 `FinalizationRegistryConstructor` 的构造函数实际逻辑在 `weakref_finalization.rs` 的 host import 函数中完成——这些 `NativeCallable` 变体是用于 `new WeakRef()` 在语义层作为 `CallBuiltin(WeakRefConstructor, ...)` 发出的路径。但检查现有模式：`WeakMapConstructor` 的 `NativeCallable` arm 返回 `alloc_host_object_from_caller(caller, 0)`（空对象），真正的构造函数逻辑在 `Func::wrap` 中。WeakRef 同理。

**重要：** 检查 `WeakMapConstructor` 和 `WeakSetConstructor` 的实际分发路径——它们是否通过 `NativeCallable` 还是通过 host import 的 `Func::wrap`。如果是后者，则 `NativeCallable::WeakRefConstructor` 和 `NativeCallable::FinalizationRegistryConstructor` 实际上**不需要**新增——因为 Type 12 调用直接走 import index。

重新检查：`get_builtin_global_entry.rs` 中，`"WeakMap"` → `NativeCallable::WeakMapConstructor`（通过 `encode_native_callable_idx`），然后 `NativeCallable::WeakMapConstructor` 的 arm 是 `Some(alloc_host_object_from_caller(caller, 0))`。但……这不对——真正的 WeakMap 构造函数在 `collections_buffers.rs` 的 `Func::wrap` 中做复杂初始化。

实际上 wjsm 有**两条路径**：
1. **全局变量引用**（`get_builtin_global` → `NativeCallable`）—用于 `WeakMap` 作为值在表达式中使用
2. **new 调用**（`CallBuiltin(WeakMapConstructor, ...)` → host import）—用于 `new WeakMap()` 

对于 WeakRef/FinalizationRegistry:
- 当用户写 `let wr = new WeakRef(obj)` → 走路径 2（`CallBuiltin` → import 356）
- 当用户写 `WeakRef` 作为表达式 → 走路径 1（`NativeCallable::WeakRefConstructor` 返回构造函数对象）

所以需要：
- `NativeCallable` 新增 `WeakRefConstructor` 和 `FinalizationRegistryConstructor` 变体
- 它们的 arm 应返回一个**可调用的构造函数对象**——这可以通过 `create_weakref_constructor` 类似的工厂函数，或者直接使用 host import 的 wrapper

最简单的做法：为 NativeCallable 的这两个变体直接 dispatch 到对应的 host import 函数。但 NativeCallable 的 dispatch 返回 `Option<i64>`（直接值），不是通过 WASM 调用。

**更简单的方案：** 参照 `PromiseConstructor` 的模式——它在 `NativeCallable` 中返回空对象，实际的 `new Promise(...)` 通过 `CallBuiltin(PromiseCreate, ...)` 走 host import。对于 WeakRef，`new WeakRef(target)` → 走 Type 12 host import → 完整构造函数。`WeakRef` 作为值 → `NativeCallable::WeakRefConstructor` → 返回一个占位对象。但这个占位对象不能作为构造函数调用……

让我重新看 `NativeCallable` 的调用路径。在 `runtime_builtins.rs` 中：

```rust
NativeCallable::WeakMapConstructor => Some(alloc_host_object_from_caller(caller, 0)),
```

这返回的是一个空 host object——它被当作 WeakMap 构造函数的「JS 值」存储。当用户通过 `get_builtin_global` 获取 `WeakMap` 引用时，得到这个空对象。然后当 `new` 作用于它时，语义层发出 `CallBuiltin(WeakMapConstructor, ...)` → 走 host import → 真正构造。

所以 `NativeCallable::WeakRefConstructor` 的 arm 应该是：

```rust
NativeCallable::WeakRefConstructor => Some(alloc_host_object_from_caller(caller, 0)),
NativeCallable::FinalizationRegistryConstructor => Some(alloc_host_object_from_caller(caller, 0)),
```

**恰好和 WeakMapConstructor 一样。**

- [ ] **Step 2: 在 get_builtin_global_entry 中注册全局变量**

`crates/wjsm-runtime/src/host_imports/get_builtin_global_entry.rs`（约 line 28-31），在 `WeakSet` 条目之后添加：

```rust
                "WeakRef" => native_callables.push(NativeCallable::WeakRefConstructor),
                "FinalizationRegistry" => native_callables.push(NativeCallable::FinalizationRegistryConstructor),
```

并在后面的 `NativeCallable` → 字符串映射对（约 line 1433-1435）中添加：

```rust
                ("WeakRef", NativeCallable::WeakRefConstructor),
                ("FinalizationRegistry", NativeCallable::FinalizationRegistryConstructor),
```

- [ ] **Step 3: runtime_builtins.rs — 新增 3 个方法分发**

在 NativeCallable dispatch match 中（约 line 1385-1390，`WeakSetMethod` arm 之后）新增：

```rust
        NativeCallable::WeakRefDerefMethod => {
            // deref() 在 weakref_finalization.rs 的 host import 中处理
            // 此 arm 不应被直接调用，但为完整性保留
            Some(value::encode_undefined())
        }
        NativeCallable::FinalizationRegistryRegisterMethod => {
            Some(value::encode_undefined())
        }
        NativeCallable::FinalizationRegistryUnregisterMethod => {
            Some(value::encode_undefined())
        }
```

- [ ] **Step 4: runtime_promises.rs — Microtask 新增 arm**

在 `run_microtask` 或微任务处理 match 中（约 line 977-998 的 `Microtask` 使用处），找到处理 `Microtask` 变体的 match 块，新增 arm：

```rust
        Microtask::CleanupFinalizationRegistry { callback, held_value } => {
            // 调用 callback(held_value)，忽略返回值和异常
            let _result = call_host_function_from_caller(
                caller,
                &func_table,
                callback,
                value::encode_undefined(),
            );
        }
```

- [ ] **Step 5: 编译检查**

```bash
cargo check -p wjsm-runtime
```

Expected: 编译成功（如果 Task 5 的编译错误已全部解决）。

- [ ] **Step 6: Commit**

```bash
git add crates/wjsm-runtime/src/runtime_builtins.rs crates/wjsm-runtime/src/runtime_promises.rs crates/wjsm-runtime/src/host_imports/get_builtin_global_entry.rs
git commit -m "feat(runtime): dispatch WeakRef/FinalizationRegistry native callables and microtasks"
```

---

### Task 7: 运行时 — GC 集成 process_weak_references

**Files:**
- Modify: `crates/wjsm-runtime/src/host_imports/core.rs`

- [ ] **Step 1: 在 gc_collect 的 sweep 后新增 process_weak_references**

读取 `crates/wjsm-runtime/src/host_imports/core.rs`，找到 `gc_collect` 函数（import 22，约 line 1322-1670）。

在 sweep/compact 阶段完成之后、`gc_collect` 返回之前（`Ok(new_ptr)` 或类似位置），插入以下代码块。需要先理解 sweep 阶段的具体代码结构（锁、变量状态）。

**插入位置判断规则：** sweep 完成后，`gc_mark_bits` 仍包含本次 GC 的标记结果（尚未清零）。在此处插入：

```rust
            // ── process_weak_references ─────────────────────────────────
            // 在 sweep 完成后、mark_bits 清零前处理弱引用
            {
                let mark_bits = caller.data().gc_mark_bits.lock().expect("gc_mark_bits mutex");
                let is_marked = |handle_idx: u32| -> bool {
                    let word = (handle_idx as usize) / 64;
                    let bit = (handle_idx as usize) % 64;
                    word < mark_bits.len() && (mark_bits[word] & (1u64 << bit)) != 0
                };

                // 处理 WeakRef
                {
                    let mut wr_table = caller.data().weakref_table.lock().expect("weakref_table mutex");
                    for entry in wr_table.iter_mut() {
                        if entry.target_handle != 0 && !is_marked(entry.target_handle) {
                            entry.target_handle = 0;
                        }
                    }
                }

                // 处理 FinalizationRegistry
                {
                    let mut fr_table = caller.data().finalization_registry_table.lock().expect("fr_table mutex");
                    for entry in fr_table.iter_mut() {
                        // 检查 FinalizationRegistry 对象自身是否存活
                        if !is_marked(entry.object_handle) {
                            continue; // registry 对象已回收，不触发回调
                        }
                        let mut held_values = Vec::new();
                        entry.registrations.retain(|reg| {
                            if !is_marked(reg.target_handle) {
                                held_values.push(reg.held_value);
                                false
                            } else {
                                true
                            }
                        });
                        if !held_values.is_empty() {
                            let mut pending = caller.data().pending_cleanup_callbacks.lock().expect("pending_cleanup_callbacks mutex");
                            pending.push((entry.callback, held_values));
                        }
                    }
                }
            } // mark_bits lock released here

            // 调度清理微任务
            {
                let mut pending = caller.data().pending_cleanup_callbacks.lock().expect("pending_cleanup_callbacks mutex");
                for (callback, held_values) in pending.drain(..) {
                    for held_value in held_values {
                        caller.data().microtask_queue.lock().expect("microtask_queue mutex")
                            .push_back(Microtask::CleanupFinalizationRegistry { callback, held_value });
                    }
                }
            }
```

- [ ] **Step 2: 编译检查**

```bash
cargo check -p wjsm-runtime
```

- [ ] **Step 3: Commit**

```bash
git add crates/wjsm-runtime/src/host_imports/core.rs
git commit -m "feat(runtime): integrate process_weak_references into GC sweep"
```

---

### Task 8: 运行时 — Import 注册

**Files:**
- Modify: `crates/wjsm-runtime/src/host_imports/collections_buffers.rs`（或 `lib.rs`）

- [ ] **Step 1: 追加 import .into() 条目**

`weakref_finalization.rs` 使用了 `include!` 嵌入机制——文件内容在 `execute_with_writer` 中作为代码块内联。该文件定义了 5 个 `Func::wrap` 变量（`weakref_constructor_fn`, `weakref_proto_deref_fn`, `finalization_registry_constructor_fn`, `finalization_registry_proto_register_fn`, `finalization_registry_proto_unregister_fn`）。

但这与现有的 `include!` 模式不同——现有 `collections_buffers.rs` 是完整的 Rust 代码，最后以 `.into()` 条目结尾。`weakref_finalization.rs` 文件也应以 `.into()` 条目结尾：

在文件末尾追加：

```rust
    weakref_constructor_fn.into(),               // 356
    weakref_proto_deref_fn.into(),                // 357
    finalization_registry_constructor_fn.into(),   // 358
    finalization_registry_proto_register_fn.into(), // 359
    finalization_registry_proto_unregister_fn.into(), // 360
```

**注：** 如果 `weakref_finalization.rs` 通过 `include!` 直接内联到 `imports` 的构建位置，则这些 `.into()` 条目在文件内完成。检查现有模式——`collections_buffers.rs` 的最后一个非 `.into()` 行后紧跟 `.into()` 条目。`include!` 后结果直接成为 `imports` 的尾部。

在 `lib.rs` 中，现有模式是：
```rust
    imports.extend(include!("host_imports/collections_buffers.rs"));
    imports.extend(include!("host_imports/proxy_traps.rs"));
```

所以 `weakref_finalization.rs` 的内容也会通过 `imports.extend(...)` 被追加。

**最终确认：** `weakref_finalization.rs` 末尾的 `.into()` 条目是：

```rust
    weakref_constructor_fn.into(),               // 356
    weakref_proto_deref_fn.into(),                // 357
    finalization_registry_constructor_fn.into(),   // 358
    finalization_registry_proto_register_fn.into(), // 359
    finalization_registry_proto_unregister_fn.into(), // 360
```

- [ ] **Step 2: 编译检查**

```bash
cargo check -p wjsm-runtime
```

- [ ] **Step 3: Commit**

```bash
git add crates/wjsm-runtime/src/host_imports/weakref_finalization.rs crates/wjsm-runtime/src/lib.rs
git commit -m "feat(runtime): register WeakRef/FinalizationRegistry imports"
```

---

### Task 9: Fixtures + E2E 测试

**Files:**
- Create: `fixtures/happy/weakref.js`
- Create: `fixtures/happy/weakref.expected`
- Create: `fixtures/happy/finalization_registry.js`
- Create: `fixtures/happy/finalization_registry.expected`
- Create: `fixtures/errors/weakref_non_object.js`
- Create: `fixtures/errors/weakref_non_object.expected`
- Create: (optional) `fixtures/semantic/weakref.ir`
- Create: (optional) `fixtures/semantic/finalization_registry.ir`

- [ ] **Step 1: 创建 WeakRef happy fixture**

`fixtures/happy/weakref.js`:
```js
let obj = { x: 1 };
let wr = new WeakRef(obj);
console.log(wr.deref().x); // 1

// 清除强引用
obj = null;

// 触发 GC（如果 harness 支持 gc()）
if (typeof gc === 'function') gc();

// deref 在 GC 后可能返回 undefined（取决于 GC 是否回收）
let result = wr.deref();
console.log(result === undefined ? 'collected' : 'still-alive');
```

`fixtures/happy/weakref.expected`:
```
exit_code: 0
--- stdout ---
1
still-alive
--- stderr ---
```

**注：** 初始 expected 假设对象未被回收（因为没有分配压力触发 GC）。后续通过 test262 验证回收行为。

- [ ] **Step 2: 创建 FinalizationRegistry happy fixture**

`fixtures/happy/finalization_registry.js`:
```js
let cleaned = false;
let fr = new FinalizationRegistry((heldValue) => {
    console.log('cleaned:', heldValue);
    cleaned = true;
});

let obj = { data: 'test' };
fr.register(obj, 'my-value');
console.log('registered');

// 测试 unregister
let obj2 = { data: 'test2' };
let token = {};
fr.register(obj2, 'value2', token);
let unregistered = fr.unregister(token);
console.log('unregistered:', unregistered);
```

`fixtures/happy/finalization_registry.expected`:
```
exit_code: 0
--- stdout ---
registered
unregistered: true
--- stderr ---
```

- [ ] **Step 3: 创建 error fixture**

`fixtures/errors/weakref_non_object.js`:
```js
new WeakRef(42); // TypeError: target must be Object or Symbol
```

`fixtures/errors/weakref_non_object.expected`:
```
exit_code: 1
--- stdout ---
--- stderr ---
TypeError: WeakRef: target must be an object or Symbol
```

- [ ] **Step 4: 运行 E2E 测试**

```bash
cargo test -p wjsm --test integration
```

- [ ] **Step 5: 更新 expected 快照（如需要）**

```bash
WJSM_UPDATE_FIXTURES=1 cargo test -p wjsm --test integration
```

- [ ] **Step 6: Commit**

```bash
git add fixtures/happy/weakref.js fixtures/happy/weakref.expected \
        fixtures/happy/finalization_registry.js fixtures/happy/finalization_registry.expected \
        fixtures/errors/weakref_non_object.js fixtures/errors/weakref_non_object.expected
git commit -m "test: add WeakRef + FinalizationRegistry E2E fixtures"
```

---

### Task 10: test262 — gc() 注入

**Files:**
- Modify: `crates/wjsm-test262/src/`（config.rs 或 exec.rs）

- [ ] **Step 1: 找到 test262 harness 的全局对象初始化位置**

在 `crates/wjsm-test262/src/` 中搜索 `$262` 或全局变量注入的位置。

```bash
rg -n '\$262|create_global|globalThis' crates/wjsm-test262/src/
```

- [ ] **Step 2: 注入 gc() 函数**

在 test262 harness 创建全局对象的代码处，添加 `gc` 属性。gc() 需要调用 wjsm 运行时的 `gc_collect`。由于 test262 运行器直接调用 `execute()`，需要一种方式让 JS 代码触发 GC。

**方案 A：** 在 test262 的 JS 全局作用域中注入 `gc()` —— 通过 host import。

**方案 B：** 在 test262 harness 的前置 JS 代码中添加 `globalThis.gc = ...`。

**推荐方案 A：** 在 wjsm-runtime 中新增一个 `gc` host import（或复用 import 22 的 `gc_collect`），并在 test262 harness 中将其暴露为 `$262.gc()`。

最简方式：在 `get_builtin_global_entry.rs` 中注册 `"gc"` → 创建一个新的 `NativeCallable` 变体 `GarbageCollect`，其 arm 触发 GC：

```rust
// get_builtin_global_entry.rs
"gc" => {
    native_callables.push(NativeCallable::GarbageCollect);
    value::encode_native_callable_idx(idx)
}
```

```rust
// runtime_builtins.rs
NativeCallable::GarbageCollect => {
    // 调用 gc_collect 的等价逻辑
    // ...
    Some(value::encode_undefined())
}
```

**但这过于复杂。** 更简单的方式：在 test262 的 `exec.rs` 中，在运行测试前向全局作用域注入一个自定义属性。查看 `exec.rs` 的具体结构。

- [ ] **Step 3: 寻找 test262 exec 结构**

```bash
read crates/wjsm-test262/src/exec.rs
```

找到测试执行入口，看如何在执行前注入 `$262` 对象。

- [ ] **Step 4: 实现 gc 暴露**

根据 test262 exec 的实际结构，选择合适的方式暴露 gc()。可能需要：

1. 新增一个简单的 host import `gc_collect_trigger`（无参数，直接调用现有 gc_collect 逻辑）
2. 在 test262 的前置脚本中引用它

- [ ] **Step 5: 验证 gc() 可用**

```bash
cargo run -p wjsm-test262 -- run --suite test/built-ins/WeakRef --all --plain 2>&1 | head -30
```

- [ ] **Step 6: Commit**

```bash
git add crates/wjsm-test262/src/
git commit -m "feat(test262): expose gc() for WeakRef/FinalizationRegistry tests"
```

---

### Task 11: 全量构建 + test262 验证

- [ ] **Step 1: 全量构建**

```bash
cargo build
```

Expected: 编译成功，无警告。

- [ ] **Step 2: 运行所有现有测试**

```bash
cargo test
```

Expected: 所有已有测试通过（无回归）。

- [ ] **Step 3: 运行 test262 WeakRef 套件**

```bash
cargo run -p wjsm-test262 -- run --suite test/built-ins/WeakRef --all --plain
```

Expected: 列出通过/失败统计。

- [ ] **Step 4: 运行 test262 FinalizationRegistry 套件**

```bash
cargo run -p wjsm-test262 -- run --suite test/built-ins/FinalizationRegistry --all --plain
```

Expected: 列出通过/失败统计。

- [ ] **Step 5: 记录结果并 commit**

```bash
git add -A
git commit -m "feat: complete WeakRef + FinalizationRegistry implementation"
```

---

## Self-Review Notes

**实施前必读：**

1. **Type 12 调用约定** — Task 2 Step 3 中的 `compile_builtin_call` arm 是伪代码。必须在编辑前阅读 `compiler_builtins.rs` 中的实际 Type 12 arm（如 `ObjectAssign` 或 `MapSetForEach`），复制完整模式。关键：`env_obj=undefined, this_val=args[0], args_base, args_count` 通过影子栈的 `I64Store` 写入。

2. **NativeCallable vs Host Import 双路径** — `WeakRef` 作为值（`let x = WeakRef`）走 `NativeCallable`；`new WeakRef(target)` 走 host import。Task 6 中 `NativeCallable::WeakRefConstructor` 返回空对象（与 `WeakMapConstructor` 一致），真正的构造逻辑在 host import 中。

3. **`create_type_error` 辅助函数** — Task 4 引用了此函数。检查 `runtime_heap.rs` 中 `create_error_object` 的用法，确保 TypeError 创建路径一致。

4. **gc_collect 中的锁顺序** — Task 7 在 gc_collect 内部分层锁定 `gc_mark_bits` → `weakref_table` → `finalization_registry_table` → `pending_cleanup_callbacks` → `microtask_queue`。确保无死锁（所有路径使用相同顺序）。

5. **FinalizationRegistry register 的 `unregister_token` 比较** — `Some(token)` == `Some(token)` 使用 `same_value_zero` 还是 `strict_eq`？规范未明确规定，建议使用 `same_value_zero`（与 Map/Set 的 key 比较一致）。

6. **编译顺序依赖** — Tasks 1-3 可以并行；Tasks 4-8 串行（运行时部分相互依赖）；Tasks 9-10 可以在 Tasks 4-8 完成后并行；Task 11 最后执行。
