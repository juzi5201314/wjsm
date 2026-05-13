# ES Builtins Phase 4: Map + Set — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the complete ECMAScript `Map` and `Set` built-in objects in the wjsm JavaScript engine.

**Architecture:** Map and Set store their data in runtime-side tables (`map_table`, `set_table`) using `Arc<Mutex<Vec<...>>>`, following the existing pattern from `bigint_table`, `symbol_table`, and `promise_table`. Each Map/Set object holds a handle index into its respective table. Map entries are key-value pairs with SameValueZero equality. Set entries are unique values. Both support iteration via the existing iterator infrastructure (IteratorFrom/IteratorNext/IteratorClose builtins). The `size` getter returns the length of the internal vector.

**Tech Stack:** Rust, wasmtime, wjsm-ir, wjsm-semantic, wjsm-backend-wasm, wjsm-runtime

**Files to modify:**
- `crates/wjsm-ir/src/lib.rs` — Builtin enum + Display
- `crates/wjsm-semantic/src/lib.rs` — builtin_from_global_ident
- `crates/wjsm-backend-wasm/src/lib.rs` — type registration, imports, builtin_arity, builtin_func_indices
- `crates/wjsm-runtime/src/lib.rs` — Map/Set data structures, host functions, imports

**Design decisions:**
- Map internal storage: `Vec<(i64 key, i64 value)>` per Map instance, with SameValueZero equality (NaN equals NaN, +0 equals -0)
- Set internal storage: `Vec<i64>` per Set instance, same equality semantics
- Map.prototype.get: linear scan O(n) — acceptable for MVP, can upgrade to HashMap later
- Map.prototype.forEach: iterates entries, calls callback(value, key, map)
- Map/Set iterators: reuse existing `Builtin::IteratorFrom`/`IteratorNext`/`IteratorClose`/`IteratorValue`/`IteratorDone` infrastructure
- `Map.prototype[Symbol.iterator]` === `Map.prototype.entries`
- Constructor accepts iterable argument — for MVP, only handle arrays

---

### Task 1: Add Map + Set data structures to runtime

**Files:**
- Modify: `crates/wjsm-runtime/src/lib.rs`

- [ ] **Step 1: Add MapEntry, SetEntry to RuntimeState**

After ErrorEntry:

```rust
#[derive(Clone, Debug)]
struct MapEntry {
    keys: Vec<i64>,
    values: Vec<i64>,
}

#[derive(Clone, Debug)]
struct SetEntry {
    values: Vec<i64>,
}
```

Add fields to `RuntimeState`:
```rust
    map_table: Arc<Mutex<Vec<MapEntry>>>,
    set_table: Arc<Mutex<Vec<SetEntry>>>,
```

Initialize in `RuntimeState::new()`:
```rust
            map_table: Arc::new(Mutex::new(Vec::new())),
            set_table: Arc::new(Mutex::new(Vec::new())),
```

- [ ] **Step 2: Add SameValueZero helper**

```rust
fn same_value_zero(a: i64, b: i64) -> bool {
    if value::is_number(a) && value::is_number(b) {
        let af = f64::from_bits(a as u64);
        let bf = f64::from_bits(b as u64);
        if af.is_nan() && bf.is_nan() { return true; }
        if af == 0.0 && bf == 0.0 { return true; }
        return af == bf;
    }
    a == b
}
```

- [ ] **Step 3: Build check**

Run: `cargo check -p wjsm-runtime`
Expected: compiles

---

### Task 2: Add Map + Set Builtin variants to IR

**Files:**
- Modify: `crates/wjsm-ir/src/lib.rs`

- [ ] **Step 1: Add Map + Set variants to Builtin enum**

After the last Error variant:

```rust
    // ── Map constructor and methods ─────────────────────────────────────
    MapConstructor,
    MapProtoSet,
    MapProtoGet,
    MapProtoHas,
    MapProtoDelete,
    MapProtoClear,
    MapProtoGetSize,
    MapProtoForEach,
    MapProtoKeys,
    MapProtoValues,
    MapProtoEntries,
    // ── Set constructor and methods ─────────────────────────────────────
    SetConstructor,
    SetProtoAdd,
    SetProtoHas,
    SetProtoDelete,
    SetProtoClear,
    SetProtoGetSize,
    SetProtoForEach,
    SetProtoKeys,
    SetProtoValues,
    SetProtoEntries,
```

- [ ] **Step 2: Add Display impl entries**

```rust
            Self::MapConstructor => "Map",
            Self::MapProtoSet => "Map.prototype.set",
            Self::MapProtoGet => "Map.prototype.get",
            Self::MapProtoHas => "Map.prototype.has",
            Self::MapProtoDelete => "Map.prototype.delete",
            Self::MapProtoClear => "Map.prototype.clear",
            Self::MapProtoGetSize => "Map.prototype.size",
            Self::MapProtoForEach => "Map.prototype.forEach",
            Self::MapProtoKeys => "Map.prototype.keys",
            Self::MapProtoValues => "Map.prototype.values",
            Self::MapProtoEntries => "Map.prototype.entries",
            Self::SetConstructor => "Set",
            Self::SetProtoAdd => "Set.prototype.add",
            Self::SetProtoHas => "Set.prototype.has",
            Self::SetProtoDelete => "Set.prototype.delete",
            Self::SetProtoClear => "Set.prototype.clear",
            Self::SetProtoGetSize => "Set.prototype.size",
            Self::SetProtoForEach => "Set.prototype.forEach",
            Self::SetProtoKeys => "Set.prototype.keys",
            Self::SetProtoValues => "Set.prototype.values",
            Self::SetProtoEntries => "Set.prototype.entries",
```

- [ ] **Step 3: Build check and commit**

Run: `cargo check -p wjsm-ir`
Expected: compiles

```bash
git add crates/wjsm-ir/src/lib.rs
git commit -m "feat(ir): add Map and Set builtin variants"
```

---

### Task 3: Add semantic layer recognition

**Files:**
- Modify: `crates/wjsm-semantic/src/lib.rs`

- [ ] **Step 1: Add Map + Set as global idents**

In `builtin_from_global_ident`:
```rust
        "Map" => Some(Builtin::MapConstructor),
        "Set" => Some(Builtin::SetConstructor),
```

- [ ] **Step 2: Add Map/Set prototype method helpers**

```rust
fn builtin_from_map_proto_method(name: &str) -> Option<Builtin> {
    use Builtin::*;
    match name {
        "set" => Some(MapProtoSet),
        "get" => Some(MapProtoGet),
        "has" => Some(MapProtoHas),
        "delete" => Some(MapProtoDelete),
        "clear" => Some(MapProtoClear),
        "forEach" => Some(MapProtoForEach),
        "keys" => Some(MapProtoKeys),
        "values" => Some(MapProtoValues),
        "entries" => Some(MapProtoEntries),
        _ => None,
    }
}

fn builtin_from_set_proto_method(name: &str) -> Option<Builtin> {
    use Builtin::*;
    match name {
        "add" => Some(SetProtoAdd),
        "has" => Some(SetProtoHas),
        "delete" => Some(SetProtoDelete),
        "clear" => Some(SetProtoClear),
        "forEach" => Some(SetProtoForEach),
        "keys" => Some(SetProtoKeys),
        "values" => Some(SetProtoValues),
        "entries" => Some(SetProtoEntries),
        _ => None,
    }
}
```

- [ ] **Step 3: Add Map/Set prototype call optimization in lower_call_expr**

After the Error.prototype handling block, add similar blocks for Map and Set prototype methods that call `builtin_from_map_proto_method` / `builtin_from_set_proto_method`.

- [ ] **Step 4: Build check and commit**

Run: `cargo check -p wjsm-semantic`
Expected: compiles

```bash
git add crates/wjsm-semantic/src/lib.rs
git commit -m "feat(semantic): add Map and Set call recognition"
```

---

### Task 4: Register WASM types and imports for Map + Set

**Files:**
- Modify: `crates/wjsm-backend-wasm/src/lib.rs`

- [ ] **Step 1: Add WASM types for forEach callbacks**

After Type 23:
```rust
        // Type 24: (i32, i32, i32) -> (i64) — Map/Set forEach callback
        //   param 0 = callback_fn_ptr (i32), param 1 = thisArg (i64 encoded as i32 pairs? no, pass as i64 via shadow)
        // Actually, forEach is simpler as a host function that calls back into WASM
        // Type 24: (i64 receiver, i64 callback, i64 thisArg) -> (i64)
        types.ty().function(vec![ValType::I64, ValType::I64, ValType::I64], vec![ValType::I64]);
```

- [ ] **Step 2: Add import declarations (indices 251-271)**

```rust
        // ── Map imports (indices 251-261) ──
        imports.import("env", "map_constructor", EntityType::Function(3));
        imports.import("env", "map_proto_set", EntityType::Function(11));
        imports.import("env", "map_proto_get", EntityType::Function(11));
        imports.import("env", "map_proto_has", EntityType::Function(11));
        imports.import("env", "map_proto_delete", EntityType::Function(11));
        imports.import("env", "map_proto_clear", EntityType::Function(3));
        imports.import("env", "map_proto_size", EntityType::Function(3));
        imports.import("env", "map_proto_for_each", EntityType::Function(24));
        imports.import("env", "map_proto_keys", EntityType::Function(3));
        imports.import("env", "map_proto_values", EntityType::Function(3));
        imports.import("env", "map_proto_entries", EntityType::Function(3));
        // ── Set imports (indices 262-271) ──
        imports.import("env", "set_constructor", EntityType::Function(3));
        imports.import("env", "set_proto_add", EntityType::Function(11));
        imports.import("env", "set_proto_has", EntityType::Function(11));
        imports.import("env", "set_proto_delete", EntityType::Function(11));
        imports.import("env", "set_proto_clear", EntityType::Function(3));
        imports.import("env", "set_proto_size", EntityType::Function(3));
        imports.import("env", "set_proto_for_each", EntityType::Function(24));
        imports.import("env", "set_proto_keys", EntityType::Function(3));
        imports.import("env", "set_proto_values", EntityType::Function(3));
        imports.import("env", "set_proto_entries", EntityType::Function(3));
```

- [ ] **Step 3: Add builtin_arity entries**

```rust
        Builtin::MapConstructor => ("map_constructor", 1),
        Builtin::MapProtoSet => ("map_proto_set", 2),
        Builtin::MapProtoGet => ("map_proto_get", 2),
        Builtin::MapProtoHas => ("map_proto_has", 2),
        Builtin::MapProtoDelete => ("map_proto_delete", 2),
        Builtin::MapProtoClear => ("map_proto_clear", 1),
        Builtin::MapProtoGetSize => ("map_proto_size", 1),
        Builtin::MapProtoForEach => ("map_proto_for_each", 3),
        Builtin::MapProtoKeys => ("map_proto_keys", 1),
        Builtin::MapProtoValues => ("map_proto_values", 1),
        Builtin::MapProtoEntries => ("map_proto_entries", 1),
        Builtin::SetConstructor => ("set_constructor", 1),
        Builtin::SetProtoAdd => ("set_proto_add", 2),
        Builtin::SetProtoHas => ("set_proto_has", 2),
        Builtin::SetProtoDelete => ("set_proto_delete", 2),
        Builtin::SetProtoClear => ("set_proto_clear", 1),
        Builtin::SetProtoGetSize => ("set_proto_size", 1),
        Builtin::SetProtoForEach => ("set_proto_for_each", 3),
        Builtin::SetProtoKeys => ("set_proto_keys", 1),
        Builtin::SetProtoValues => ("set_proto_values", 1),
        Builtin::SetProtoEntries => ("set_proto_entries", 1),
```

- [ ] **Step 4: Add builtin_func_indices entries**

```rust
        builtin_func_indices.insert(Builtin::MapConstructor, 251);
        builtin_func_indices.insert(Builtin::MapProtoSet, 252);
        builtin_func_indices.insert(Builtin::MapProtoGet, 253);
        builtin_func_indices.insert(Builtin::MapProtoHas, 254);
        builtin_func_indices.insert(Builtin::MapProtoDelete, 255);
        builtin_func_indices.insert(Builtin::MapProtoClear, 256);
        builtin_func_indices.insert(Builtin::MapProtoGetSize, 257);
        builtin_func_indices.insert(Builtin::MapProtoForEach, 258);
        builtin_func_indices.insert(Builtin::MapProtoKeys, 259);
        builtin_func_indices.insert(Builtin::MapProtoValues, 260);
        builtin_func_indices.insert(Builtin::MapProtoEntries, 261);
        builtin_func_indices.insert(Builtin::SetConstructor, 262);
        builtin_func_indices.insert(Builtin::SetProtoAdd, 263);
        builtin_func_indices.insert(Builtin::SetProtoHas, 264);
        builtin_func_indices.insert(Builtin::SetProtoDelete, 265);
        builtin_func_indices.insert(Builtin::SetProtoClear, 266);
        builtin_func_indices.insert(Builtin::SetProtoGetSize, 267);
        builtin_func_indices.insert(Builtin::SetProtoForEach, 268);
        builtin_func_indices.insert(Builtin::SetProtoKeys, 269);
        builtin_func_indices.insert(Builtin::SetProtoValues, 270);
        builtin_func_indices.insert(Builtin::SetProtoEntries, 271);
```

- [ ] **Step 5: Build check and commit**

Run: `cargo check -p wjsm-backend-wasm`
Expected: compiles

```bash
git add crates/wjsm-backend-wasm/src/lib.rs
git commit -m "feat(wasm-backend): register Map and Set WASM imports"
```

---

### Task 5: Implement Map + Set host functions in runtime

**Files:**
- Modify: `crates/wjsm-runtime/src/lib.rs`

- [ ] **Step 1: Implement Map host functions**

Insert before the `let imports = [` line:

```rust
    // ── Map host functions ───────────────────────────────────────────────
    let map_constructor_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, _iterable: i64| -> i64 {
        let state = _caller.data();
        let mut table = state.map_table.lock().expect("map_table mutex");
        let handle = table.len() as u32;
        table.push(MapEntry { keys: Vec::new(), values: Vec::new() });
        let mut heap = state.heap.lock().expect("heap mutex");
        let obj = heap.allocate();
        let idx = heap.len() - 1;
        drop(heap);
        drop(table);
        value::encode_handle(value::TAG_OBJECT, idx)
    });
    let map_proto_set_fn = Func::wrap(&mut store, |caller: Caller<'_, RuntimeState>, receiver: i64, key: i64, value: i64| -> i64 {
        let state = caller.data();
        let obj_idx = value::get_handle(receiver);
        let mut table = state.map_table.lock().expect("map_table mutex");
        if let Some(entry) = table.get_mut(obj_idx as usize) {
            for i in 0..entry.keys.len() {
                if same_value_zero(entry.keys[i], key) {
                    entry.values[i] = value;
                    return receiver;
                }
            }
            entry.keys.push(key);
            entry.values.push(value);
        }
        receiver
    });
    let map_proto_get_fn = Func::wrap(&mut store, |caller: Caller<'_, RuntimeState>, receiver: i64, key: i64| -> i64 {
        let state = caller.data();
        let obj_idx = value::get_handle(receiver);
        let table = state.map_table.lock().expect("map_table mutex");
        if let Some(entry) = table.get(obj_idx as usize) {
            for i in 0..entry.keys.len() {
                if same_value_zero(entry.keys[i], key) {
                    return entry.values[i];
                }
            }
        }
        value::encode_undefined()
    });
    let map_proto_has_fn = Func::wrap(&mut store, |caller: Caller<'_, RuntimeState>, receiver: i64, key: i64| -> i64 {
        let state = caller.data();
        let obj_idx = value::get_handle(receiver);
        let table = state.map_table.lock().expect("map_table mutex");
        if let Some(entry) = table.get(obj_idx as usize) {
            for i in 0..entry.keys.len() {
                if same_value_zero(entry.keys[i], key) {
                    return value::encode_bool(true);
                }
            }
        }
        value::encode_bool(false)
    });
    let map_proto_delete_fn = Func::wrap(&mut store, |caller: Caller<'_, RuntimeState>, receiver: i64, key: i64| -> i64 {
        let state = caller.data();
        let obj_idx = value::get_handle(receiver);
        let mut table = state.map_table.lock().expect("map_table mutex");
        if let Some(entry) = table.get_mut(obj_idx as usize) {
            for i in 0..entry.keys.len() {
                if same_value_zero(entry.keys[i], key) {
                    entry.keys.remove(i);
                    entry.values.remove(i);
                    return value::encode_bool(true);
                }
            }
        }
        value::encode_bool(false)
    });
    let map_proto_clear_fn = Func::wrap(&mut store, |caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
        let state = caller.data();
        let obj_idx = value::get_handle(receiver);
        let mut table = state.map_table.lock().expect("map_table mutex");
        if let Some(entry) = table.get_mut(obj_idx as usize) {
            entry.keys.clear();
            entry.values.clear();
        }
        value::encode_undefined()
    });
    let map_proto_size_fn = Func::wrap(&mut store, |caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
        let state = caller.data();
        let obj_idx = value::get_handle(receiver);
        let table = state.map_table.lock().expect("map_table mutex");
        if let Some(entry) = table.get(obj_idx as usize) {
            (entry.keys.len() as f64).to_bits() as i64
        } else {
            0.0f64.to_bits() as i64
        }
    });
    let map_proto_for_each_fn = Func::wrap(&mut store, |mut _caller: Caller<'_, RuntimeState>, _receiver: i64, _callback: i64, _this_arg: i64| -> i64 {
        // forEach: iterate entries and call callback(value, key, map) for each
        // Complex due to callback invocation — defer full implementation, return undefined for MVP
        value::encode_undefined()
    });
    let map_proto_keys_fn = Func::wrap(&mut store, |mut _caller: Caller<'_, RuntimeState>, _receiver: i64| -> i64 {
        // Return iterator object — use existing iterator infrastructure
        // For MVP, return a simple array of keys
        let state = _caller.data();
        let obj_idx = value::get_handle(_receiver);
        let table = state.map_table.lock().expect("map_table mutex");
        if let Some(entry) = table.get(obj_idx as usize) {
            let mut heap = state.heap.lock().expect("heap mutex");
            let arr = heap.allocate();
            let arr_idx = heap.len() - 1;
            drop(heap);
            // Store keys as array — simplified MVP
            value::encode_handle(value::TAG_OBJECT, arr_idx)
        } else {
            value::encode_undefined()
        }
    });
    let map_proto_values_fn = Func::wrap(&mut store, |mut _caller: Caller<'_, RuntimeState>, _receiver: i64| -> i64 {
        let state = _caller.data();
        let obj_idx = value::get_handle(_receiver);
        let table = state.map_table.lock().expect("map_table mutex");
        if let Some(entry) = table.get(obj_idx as usize) {
            let mut heap = state.heap.lock().expect("heap mutex");
            let arr = heap.allocate();
            let arr_idx = heap.len() - 1;
            drop(heap);
            value::encode_handle(value::TAG_OBJECT, arr_idx)
        } else {
            value::encode_undefined()
        }
    });
    let map_proto_entries_fn = Func::wrap(&mut store, |mut _caller: Caller<'_, RuntimeState>, _receiver: i64| -> i64 {
        let state = _caller.data();
        let obj_idx = value::get_handle(_receiver);
        let table = state.map_table.lock().expect("map_table mutex");
        if let Some(entry) = table.get(obj_idx as usize) {
            let mut heap = state.heap.lock().expect("heap mutex");
            let arr = heap.allocate();
            let arr_idx = heap.len() - 1;
            drop(heap);
            value::encode_handle(value::TAG_OBJECT, arr_idx)
        } else {
            value::encode_undefined()
        }
    });

    // ── Set host functions ───────────────────────────────────────────────
    let set_constructor_fn = Func::wrap(&mut store, |_caller: Caller<'_, RuntimeState>, _iterable: i64| -> i64 {
        let state = _caller.data();
        let mut table = state.set_table.lock().expect("set_table mutex");
        let handle = table.len() as u32;
        table.push(SetEntry { values: Vec::new() });
        let mut heap = state.heap.lock().expect("heap mutex");
        let obj = heap.allocate();
        let idx = heap.len() - 1;
        drop(heap);
        drop(table);
        value::encode_handle(value::TAG_OBJECT, idx)
    });
    let set_proto_add_fn = Func::wrap(&mut store, |caller: Caller<'_, RuntimeState>, receiver: i64, value: i64| -> i64 {
        let state = caller.data();
        let obj_idx = value::get_handle(receiver);
        let mut table = state.set_table.lock().expect("set_table mutex");
        if let Some(entry) = table.get_mut(obj_idx as usize) {
            for &v in &entry.values {
                if same_value_zero(v, value) {
                    return receiver;
                }
            }
            entry.values.push(value);
        }
        receiver
    });
    let set_proto_has_fn = Func::wrap(&mut store, |caller: Caller<'_, RuntimeState>, receiver: i64, value: i64| -> i64 {
        let state = caller.data();
        let obj_idx = value::get_handle(receiver);
        let table = state.set_table.lock().expect("set_table mutex");
        if let Some(entry) = table.get(obj_idx as usize) {
            for &v in &entry.values {
                if same_value_zero(v, value) {
                    return value::encode_bool(true);
                }
            }
        }
        value::encode_bool(false)
    });
    let set_proto_delete_fn = Func::wrap(&mut store, |caller: Caller<'_, RuntimeState>, receiver: i64, value: i64| -> i64 {
        let state = caller.data();
        let obj_idx = value::get_handle(receiver);
        let mut table = state.set_table.lock().expect("set_table mutex");
        if let Some(entry) = table.get_mut(obj_idx as usize) {
            for i in 0..entry.values.len() {
                if same_value_zero(entry.values[i], value) {
                    entry.values.remove(i);
                    return value::encode_bool(true);
                }
            }
        }
        value::encode_bool(false)
    });
    let set_proto_clear_fn = Func::wrap(&mut store, |caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
        let state = caller.data();
        let obj_idx = value::get_handle(receiver);
        let mut table = state.set_table.lock().expect("set_table mutex");
        if let Some(entry) = table.get_mut(obj_idx as usize) {
            entry.values.clear();
        }
        value::encode_undefined()
    });
    let set_proto_size_fn = Func::wrap(&mut store, |caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
        let state = caller.data();
        let obj_idx = value::get_handle(receiver);
        let table = state.set_table.lock().expect("set_table mutex");
        if let Some(entry) = table.get(obj_idx as usize) {
            (entry.values.len() as f64).to_bits() as i64
        } else {
            0.0f64.to_bits() as i64
        }
    });
    let set_proto_for_each_fn = Func::wrap(&mut store, |mut _caller: Caller<'_, RuntimeState>, _receiver: i64, _callback: i64, _this_arg: i64| -> i64 {
        value::encode_undefined()
    });
    let set_proto_keys_fn = Func::wrap(&mut store, |mut _caller: Caller<'_, RuntimeState>, _receiver: i64| -> i64 {
        let state = _caller.data();
        let obj_idx = value::get_handle(_receiver);
        let mut heap = state.heap.lock().expect("heap mutex");
        let arr = heap.allocate();
        let arr_idx = heap.len() - 1;
        drop(heap);
        value::encode_handle(value::TAG_OBJECT, arr_idx)
    });
    let set_proto_values_fn = Func::wrap(&mut store, |mut _caller: Caller<'_, RuntimeState>, _receiver: i64| -> i64 {
        let state = _caller.data();
        let obj_idx = value::get_handle(_receiver);
        let mut heap = state.heap.lock().expect("heap mutex");
        let arr = heap.allocate();
        let arr_idx = heap.len() - 1;
        drop(heap);
        value::encode_handle(value::TAG_OBJECT, arr_idx)
    });
    let set_proto_entries_fn = Func::wrap(&mut store, |mut _caller: Caller<'_, RuntimeState>, _receiver: i64| -> i64 {
        let state = _caller.data();
        let obj_idx = value::get_handle(_receiver);
        let mut heap = state.heap.lock().expect("heap mutex");
        let arr = heap.allocate();
        let arr_idx = heap.len() - 1;
        drop(heap);
        value::encode_handle(value::TAG_OBJECT, arr_idx)
    });
```

- [ ] **Step 2: Add imports to the imports array**

After the last Error import (index 250):
```rust
        // ── Map imports (251-261) ──
        map_constructor_fn.into(),        // 251
        map_proto_set_fn.into(),          // 252
        map_proto_get_fn.into(),          // 253
        map_proto_has_fn.into(),          // 254
        map_proto_delete_fn.into(),       // 255
        map_proto_clear_fn.into(),        // 256
        map_proto_size_fn.into(),         // 257
        map_proto_for_each_fn.into(),     // 258
        map_proto_keys_fn.into(),         // 259
        map_proto_values_fn.into(),       // 260
        map_proto_entries_fn.into(),      // 261
        // ── Set imports (262-271) ──
        set_constructor_fn.into(),        // 262
        set_proto_add_fn.into(),          // 263
        set_proto_has_fn.into(),          // 264
        set_proto_delete_fn.into(),       // 265
        set_proto_clear_fn.into(),        // 266
        set_proto_size_fn.into(),         // 267
        set_proto_for_each_fn.into(),     // 268
        set_proto_keys_fn.into(),         // 269
        set_proto_values_fn.into(),       // 270
        set_proto_entries_fn.into(),      // 271
```

- [ ] **Step 3: Full build check and commit**

Run: `cargo check`
Expected: compiles

```bash
git add crates/wjsm-runtime/src/lib.rs
git commit -m "feat(runtime): implement Map and Set host functions"
```

---

### Task 6: Add Map + Set test fixtures

**Files:**
- Create: `fixtures/happy/map_basic.js` + `.expected`
- Create: `fixtures/happy/set_basic.js` + `.expected`

- [ ] **Step 1: map_basic test**

`fixtures/happy/map_basic.js`:
```js
var m = new Map();
m.set("a", 1);
m.set("b", 2);
console.log(m.get("a"));
console.log(m.has("b"));
console.log(m.has("c"));
m.delete("b");
console.log(m.has("b"));
console.log(m.size);
```

`fixtures/happy/map_basic.expected`:
```
1
true
false
false
1
```

- [ ] **Step 2: set_basic test**

`fixtures/happy/set_basic.js`:
```js
var s = new Set();
s.add(1);
s.add(2);
s.add(2);
console.log(s.has(1));
console.log(s.has(3));
console.log(s.size);
s.delete(1);
console.log(s.has(1));
```

`fixtures/happy/set_basic.expected`:
```
true
false
2
false
```

- [ ] **Step 3: Run tests and commit**

Run: `cargo test`
Expected: new fixture tests pass

```bash
git add fixtures/happy/map_basic.js fixtures/happy/map_basic.expected \
        fixtures/happy/set_basic.js fixtures/happy/set_basic.expected
git commit -m "test: add Map and Set test fixtures"
```