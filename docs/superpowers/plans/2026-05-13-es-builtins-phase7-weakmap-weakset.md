# ES Builtins Phase 7: WeakMap + WeakSet — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the ECMAScript `WeakMap` and `WeakSet` built-in objects in the wjsm JavaScript engine.

**Architecture:** WeakMap/WeakSet use object-handle-keyed hash maps in the runtime. Keys must be objects (host objects, arrays, functions). Since the engine has a GC, WeakMap/WeakSet entries are weak references — when a key object is collected, its entry is automatically removed. For MVP: implement as strong references (same as Map/Set) since GC integration for weak refs is complex; mark as "not actually weak" in documentation.

**Tech Stack:** Rust, wasmtime, wjsm-ir, wjsm-semantic, wjsm-backend-wasm, wjsm-runtime

**Files to modify:**
- `crates/wjsm-ir/src/lib.rs` — Builtin enum + Display
- `crates/wjsm-semantic/src/lib.rs` — builtin_from_global_ident, prototype method helpers
- `crates/wjsm-backend-wasm/src/lib.rs` — type registration, imports, builtin_arity, builtin_func_indices
- `crates/wjsm-runtime/src/lib.rs` — WeakMap/WeakSet data structures, host functions, imports

**Design decisions:**
- WeakMap: `HashMap<u32, i64>` keyed by object handle, storing values
- WeakSet: `HashSet<u32>` of object handles
- Keys are validated to be objects (host objects, arrays, functions, proxies)
- `WeakMap.prototype.set`, `get`, `has`, `delete`
- `WeakSet.prototype.add`, `has`, `delete`
- No `size` property (per spec), no iteration methods
- No `clear` method (per spec)

---

### Task 1: Add WeakMap + WeakSet data structures to runtime

**Files:**
- Modify: `crates/wjsm-runtime/src/lib.rs`

- [ ] **Step 1: Add WeakMapEntry and WeakSetEntry to RuntimeState**

After ProxyEntry:

```rust
#[derive(Clone, Debug)]
struct WeakMapEntry {
    map: HashMap<u32, i64>, // object handle -> value
}

#[derive(Clone, Debug)]
struct WeakSetEntry {
    set: HashSet<u32>, // object handles
}
```

Add fields to `RuntimeState`:
```rust
    weakmap_table: Arc<Mutex<Vec<WeakMapEntry>>>,
    weakset_table: Arc<Mutex<Vec<WeakSetEntry>>>,
```

Initialize in `RuntimeState::new()`:
```rust
            weakmap_table: Arc::new(Mutex::new(Vec::new())),
            weakset_table: Arc::new(Mutex::new(Vec::new())),
```

- [ ] **Step 2: Add helper functions**

```rust
fn alloc_weakmap(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    let state = caller.data();
    let mut table = state.weakmap_table.lock().expect("weakmap_table mutex");
    let handle = table.len() as u32;
    table.push(WeakMapEntry { map: HashMap::new() });
    let obj = alloc_host_object_from_caller(caller, 4);
    let handle_val = value::encode_f64(handle as f64);
    let _ = define_host_data_property_from_caller(caller, obj, "__weakmap_handle__", handle_val);
    obj
}

fn alloc_weakset(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    let state = caller.data();
    let mut table = state.weakset_table.lock().expect("weakset_table mutex");
    let handle = table.len() as u32;
    table.push(WeakSetEntry { set: HashSet::new() });
    let obj = alloc_host_object_from_caller(caller, 4);
    let handle_val = value::encode_f64(handle as f64);
    let _ = define_host_data_property_from_caller(caller, obj, "__weakset_handle__", handle_val);
    obj
}

fn is_valid_weak_key(val: i64) -> bool {
    value::is_object(val) || value::is_array(val) || value::is_function(val) || value::is_proxy(val)
}

fn get_weakmap_entry<'a>(state: &'a RuntimeState, obj_val: i64) -> Option<(std::sync::MutexGuard<'a, Vec<WeakMapEntry>>, usize)> {
    let obj_ptr = {
        let handles = state.object_handles.lock().expect("object_handles mutex");
        let idx = value::decode_object_handle(obj_val) as usize;
        handles.get(idx).copied()
    }?;
    let table = state.weakmap_table.lock().expect("weakmap_table mutex");
    let handle_val = read_object_property_by_name_static(state, obj_ptr, "__weakmap_handle__")?;
    let handle = value::decode_f64(handle_val) as usize;
    Some((table, handle))
}

fn get_weakset_entry<'a>(state: &'a RuntimeState, obj_val: i64) -> Option<(std::sync::MutexGuard<'a, Vec<WeakSetEntry>>, usize)> {
    let obj_ptr = {
        let handles = state.object_handles.lock().expect("object_handles mutex");
        let idx = value::decode_object_handle(obj_val) as usize;
        handles.get(idx).copied()
    }?;
    let table = state.weakset_table.lock().expect("weakset_table mutex");
    let handle_val = read_object_property_by_name_static(state, obj_ptr, "__weakset_handle__")?;
    let handle = value::decode_f64(handle_val) as usize;
    Some((table, handle))
}
```

- [ ] **Step 3: Build check**

Run: `cargo check -p wjsm-runtime`
Expected: compiles

---

### Task 2: Add WeakMap + WeakSet Builtin variants to IR

**Files:**
- Modify: `crates/wjsm-ir/src/lib.rs`

- [ ] **Step 1: Add WeakMap + WeakSet variants to Builtin enum**

After the last existing variant:

```rust
    // ── WeakMap constructor and methods ─────────────────────────────────
    WeakMapConstructor,
    WeakMapProtoSet,
    WeakMapProtoGet,
    WeakMapProtoHas,
    WeakMapProtoDelete,
    // ── WeakSet constructor and methods ─────────────────────────────────
    WeakSetConstructor,
    WeakSetProtoAdd,
    WeakSetProtoHas,
    WeakSetProtoDelete,
```

- [ ] **Step 2: Add Display impl entries**

```rust
            Self::WeakMapConstructor => "WeakMap",
            Self::WeakMapProtoSet => "WeakMap.prototype.set",
            Self::WeakMapProtoGet => "WeakMap.prototype.get",
            Self::WeakMapProtoHas => "WeakMap.prototype.has",
            Self::WeakMapProtoDelete => "WeakMap.prototype.delete",
            Self::WeakSetConstructor => "WeakSet",
            Self::WeakSetProtoAdd => "WeakSet.prototype.add",
            Self::WeakSetProtoHas => "WeakSet.prototype.has",
            Self::WeakSetProtoDelete => "WeakSet.prototype.delete",
```

- [ ] **Step 3: Build check and commit**

Run: `cargo check -p wjsm-ir`
Expected: compiles

```bash
git add crates/wjsm-ir/src/lib.rs
git commit -m "feat(ir): add WeakMap and WeakSet builtin variants"
```

---

### Task 3: Add semantic layer recognition

**Files:**
- Modify: `crates/wjsm-semantic/src/lib.rs`

- [ ] **Step 1: Add WeakMap + WeakSet as global idents**

In `builtin_from_global_ident`:
```rust
        "WeakMap" => Some(Builtin::WeakMapConstructor),
        "WeakSet" => Some(Builtin::WeakSetConstructor),
```

- [ ] **Step 2: Add WeakMap + WeakSet prototype method helpers**

```rust
fn builtin_from_weakmap_proto_method(name: &str) -> Option<Builtin> {
    use Builtin::*;
    match name {
        "set" => Some(WeakMapProtoSet),
        "get" => Some(WeakMapProtoGet),
        "has" => Some(WeakMapProtoHas),
        "delete" => Some(WeakMapProtoDelete),
        _ => None,
    }
}

fn builtin_from_weakset_proto_method(name: &str) -> Option<Builtin> {
    use Builtin::*;
    match name {
        "add" => Some(WeakSetProtoAdd),
        "has" => Some(WeakSetProtoHas),
        "delete" => Some(WeakSetProtoDelete),
        _ => None,
    }
}
```

- [ ] **Step 3: Add prototype call optimization in lower_call_expr**

After the existing prototype handling blocks, add:

```rust
                    // WeakMap.prototype methods
                    if let Some(builtin) = builtin_from_weakmap_proto_method(&method_name) {
                        return Expr::Call {
                            callee: Box::new(Expr::Builtin(builtin)),
                            args: vec![
                                Box::new(Expr::Ident(base_name.clone())),
                                args.into_iter().map(|a| *a).collect(),
                            ].concat(),
                        };
                    }
                    // WeakSet.prototype methods
                    if let Some(builtin) = builtin_from_weakset_proto_method(&method_name) {
                        return Expr::Call {
                            callee: Box::new(Expr::Builtin(builtin)),
                            args: vec![
                                Box::new(Expr::Ident(base_name.clone())),
                                args.into_iter().map(|a| *a).collect(),
                            ].concat(),
                        };
                    }
```

- [ ] **Step 4: Build check and commit**

Run: `cargo check -p wjsm-semantic`
Expected: compiles

```bash
git add crates/wjsm-semantic/src/lib.rs
git commit -m "feat(semantic): add WeakMap and WeakSet call recognition"
```

---

### Task 4: Register WASM types and imports for WeakMap + WeakSet

**Files:**
- Modify: `crates/wjsm-backend-wasm/src/lib.rs`

- [ ] **Step 1: Add WASM types**

No new types needed — reuse existing (i64, i64) -> i64 and (i64) -> i64 types.

- [ ] **Step 2: Add import declarations**

After the last existing import:
```rust
        // ── WeakMap imports ──
        imports.import("env", "weakmap_constructor", EntityType::Function(0));
        imports.import("env", "weakmap_proto_set", EntityType::Function(2));
        imports.import("env", "weakmap_proto_get", EntityType::Function(2));
        imports.import("env", "weakmap_proto_has", EntityType::Function(2));
        imports.import("env", "weakmap_proto_delete", EntityType::Function(2));
        // ── WeakSet imports ──
        imports.import("env", "weakset_constructor", EntityType::Function(0));
        imports.import("env", "weakset_proto_add", EntityType::Function(2));
        imports.import("env", "weakset_proto_has", EntityType::Function(2));
        imports.import("env", "weakset_proto_delete", EntityType::Function(2));
```

- [ ] **Step 3: Add builtin_arity entries**

```rust
        Builtin::WeakMapConstructor => ("weakmap_constructor", 0),
        Builtin::WeakMapProtoSet => ("weakmap_proto_set", 2),
        Builtin::WeakMapProtoGet => ("weakmap_proto_get", 1),
        Builtin::WeakMapProtoHas => ("weakmap_proto_has", 1),
        Builtin::WeakMapProtoDelete => ("weakmap_proto_delete", 1),
        Builtin::WeakSetConstructor => ("weakset_constructor", 0),
        Builtin::WeakSetProtoAdd => ("weakset_proto_add", 1),
        Builtin::WeakSetProtoHas => ("weakset_proto_has", 1),
        Builtin::WeakSetProtoDelete => ("weakset_proto_delete", 1),
```

- [ ] **Step 4: Add builtin_func_indices entries**

```rust
        builtin_func_indices.insert(Builtin::WeakMapConstructor, 331);
        builtin_func_indices.insert(Builtin::WeakMapProtoSet, 332);
        builtin_func_indices.insert(Builtin::WeakMapProtoGet, 333);
        builtin_func_indices.insert(Builtin::WeakMapProtoHas, 334);
        builtin_func_indices.insert(Builtin::WeakMapProtoDelete, 335);
        builtin_func_indices.insert(Builtin::WeakSetConstructor, 336);
        builtin_func_indices.insert(Builtin::WeakSetProtoAdd, 337);
        builtin_func_indices.insert(Builtin::WeakSetProtoHas, 338);
        builtin_func_indices.insert(Builtin::WeakSetProtoDelete, 339);
```

- [ ] **Step 5: Build check and commit**

Run: `cargo check -p wjsm-backend-wasm`
Expected: compiles

```bash
git add crates/wjsm-backend-wasm/src/lib.rs
git commit -m "feat(wasm-backend): register WeakMap and WeakSet WASM imports"
```

---

### Task 5: Implement WeakMap + WeakSet host functions in runtime

**Files:**
- Modify: `crates/wjsm-runtime/src/lib.rs`

- [ ] **Step 1: Implement WeakMap host functions**

```rust
    // ── WeakMap host functions ───────────────────────────────────────────
    let weakmap_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>| -> i64 {
            alloc_weakmap(&mut caller)
        },
    );

    let weakmap_proto_set_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, key: i64, val: i64| -> i64 {
            if !is_valid_weak_key(key) {
                *caller.data().runtime_error.lock().expect("error mutex") =
                    Some("TypeError: WeakMap key must be an object".to_string());
                return receiver;
            }
            let key_handle = value::decode_object_handle(key);
            let state = caller.data();
            let (mut table, handle) = match get_weakmap_entry(state, receiver) {
                Some(t) => t,
                None => return receiver,
            };
            if let Some(entry) = table.get_mut(handle) {
                entry.map.insert(key_handle, val);
            }
            receiver
        },
    );

    let weakmap_proto_get_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, key: i64| -> i64 {
            if !is_valid_weak_key(key) {
                return value::encode_undefined();
            }
            let key_handle = value::decode_object_handle(key);
            let state = caller.data();
            let (table, handle) = match get_weakmap_entry(state, receiver) {
                Some(t) => t,
                None => return value::encode_undefined(),
            };
            if let Some(entry) = table.get(handle) {
                if let Some(&val) = entry.map.get(&key_handle) {
                    return val;
                }
            }
            value::encode_undefined()
        },
    );

    let weakmap_proto_has_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, key: i64| -> i64 {
            if !is_valid_weak_key(key) {
                return value::encode_bool(false);
            }
            let key_handle = value::decode_object_handle(key);
            let state = caller.data();
            let (table, handle) = match get_weakmap_entry(state, receiver) {
                Some(t) => t,
                None => return value::encode_bool(false),
            };
            if let Some(entry) = table.get(handle) {
                return value::encode_bool(entry.map.contains_key(&key_handle));
            }
            value::encode_bool(false)
        },
    );

    let weakmap_proto_delete_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, key: i64| -> i64 {
            if !is_valid_weak_key(key) {
                return value::encode_bool(false);
            }
            let key_handle = value::decode_object_handle(key);
            let state = caller.data();
            let (mut table, handle) = match get_weakmap_entry(state, receiver) {
                Some(t) => t,
                None => return value::encode_bool(false),
            };
            if let Some(entry) = table.get_mut(handle) {
                return value::encode_bool(entry.map.remove(&key_handle).is_some());
            }
            value::encode_bool(false)
        },
    );
```

- [ ] **Step 2: Implement WeakSet host functions**

```rust
    // ── WeakSet host functions ───────────────────────────────────────────
    let weakset_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>| -> i64 {
            alloc_weakset(&mut caller)
        },
    );

    let weakset_proto_add_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, key: i64| -> i64 {
            if !is_valid_weak_key(key) {
                *caller.data().runtime_error.lock().expect("error mutex") =
                    Some("TypeError: WeakSet value must be an object".to_string());
                return receiver;
            }
            let key_handle = value::decode_object_handle(key);
            let state = caller.data();
            let (mut table, handle) = match get_weakset_entry(state, receiver) {
                Some(t) => t,
                None => return receiver,
            };
            if let Some(entry) = table.get_mut(handle) {
                entry.set.insert(key_handle);
            }
            receiver
        },
    );

    let weakset_proto_has_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, key: i64| -> i64 {
            if !is_valid_weak_key(key) {
                return value::encode_bool(false);
            }
            let key_handle = value::decode_object_handle(key);
            let state = caller.data();
            let (table, handle) = match get_weakset_entry(state, receiver) {
                Some(t) => t,
                None => return value::encode_bool(false),
            };
            if let Some(entry) = table.get(handle) {
                return value::encode_bool(entry.set.contains(&key_handle));
            }
            value::encode_bool(false)
        },
    );

    let weakset_proto_delete_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, key: i64| -> i64 {
            if !is_valid_weak_key(key) {
                return value::encode_bool(false);
            }
            let key_handle = value::decode_object_handle(key);
            let state = caller.data();
            let (mut table, handle) = match get_weakset_entry(state, receiver) {
                Some(t) => t,
                None => return value::encode_bool(false),
            };
            if let Some(entry) = table.get_mut(handle) {
                return value::encode_bool(entry.set.remove(&key_handle));
            }
            value::encode_bool(false)
        },
    );
```

- [ ] **Step 3: Add imports to the imports array**

After the last existing import:
```rust
        // ── WeakMap imports ──
        weakmap_constructor_fn.into(),       // 331
        weakmap_proto_set_fn.into(),         // 332
        weakmap_proto_get_fn.into(),         // 333
        weakmap_proto_has_fn.into(),         // 334
        weakmap_proto_delete_fn.into(),      // 335
        // ── WeakSet imports ──
        weakset_constructor_fn.into(),       // 336
        weakset_proto_add_fn.into(),         // 337
        weakset_proto_has_fn.into(),         // 338
        weakset_proto_delete_fn.into(),      // 339
```

- [ ] **Step 4: Full build check and commit**

Run: `cargo check`
Expected: compiles across all crates

```bash
git add crates/wjsm-runtime/src/lib.rs
git commit -m "feat(runtime): implement WeakMap and WeakSet host functions"
```

---

### Task 6: Add WeakMap + WeakSet test fixtures

**Files:**
- Create: `fixtures/happy/weakmap_basic.js` + `.expected`
- Create: `fixtures/happy/weakset_basic.js` + `.expected`

- [ ] **Step 1: weakmap_basic test**

`fixtures/happy/weakmap_basic.js`:
```js
var wm = new WeakMap();
var key = { id: 1 };
wm.set(key, "value1");
console.log(wm.has(key));
console.log(wm.get(key));
wm.delete(key);
console.log(wm.has(key));
console.log(wm.get(key) === undefined);
```

`fixtures/happy/weakmap_basic.expected`:
```
true
value1
false
true
```

- [ ] **Step 2: weakset_basic test**

`fixtures/happy/weakset_basic.js`:
```js
var ws = new WeakSet();
var obj = { id: 1 };
ws.add(obj);
console.log(ws.has(obj));
ws.delete(obj);
console.log(ws.has(obj));
```

`fixtures/happy/weakset_basic.expected`:
```
true
false
```

- [ ] **Step 3: Run tests and commit**

Run: `cargo test`
Expected: new fixture tests pass

```bash
git add fixtures/happy/weakmap_basic.js fixtures/happy/weakmap_basic.expected \
        fixtures/happy/weakset_basic.js fixtures/happy/weakset_basic.expected
git commit -m "test: add WeakMap and WeakSet test fixtures"
```
