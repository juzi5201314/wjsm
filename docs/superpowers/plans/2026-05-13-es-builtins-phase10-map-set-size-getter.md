# ES Builtins Phase 10: Map/Set size Getter Fix — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix `Map.prototype.size` and `Set.prototype.size` to be accessor getter properties instead of plain data properties, matching ECMAScript specification behavior.

**Architecture:** Currently, `size` is stored as a data property on the Map/Set host object and updated on every mutating operation (`set`, `delete`, `clear`). The spec requires `size` to be an accessor property on `Map.prototype` / `Set.prototype` that computes the count dynamically from the internal slot. We will implement this by adding getter host functions that read from the internal map/set tables.

**Tech Stack:** Rust, wasmtime, wjsm-ir, wjsm-semantic, wjsm-backend-wasm, wjsm-runtime

**Files to modify:**
- `crates/wjsm-ir/src/lib.rs` — Builtin enum + Display
- `crates/wjsm-semantic/src/lib.rs` — builtin_from_map_proto_method, builtin_from_set_proto_method
- `crates/wjsm-backend-wasm/src/lib.rs` — type registration, imports, builtin_arity, builtin_func_indices
- `crates/wjsm-runtime/src/lib.rs` — size getter host functions, remove size updates from mutating ops

**Design decisions:**
- Add `MapProtoSizeGet` and `SetProtoSizeGet` builtin variants
- Remove `size` data property from Map/Set host objects on creation
- Remove size updates from `map_proto_set`, `map_proto_delete`, `map_proto_clear`, `set_proto_add`, `set_proto_delete`, `set_proto_clear`
- The getter reads the internal table and returns the count
- For prototype property access: when JS reads `map.size`, the semantic layer detects it and calls the getter builtin

---

### Task 1: Add size getter Builtin variants to IR

**Files:**
- Modify: `crates/wjsm-ir/src/lib.rs`

- [ ] **Step 1: Add getter variants to Builtin enum**

After `SetProtoEntries`:

```rust
    MapProtoSizeGet,
    SetProtoSizeGet,
```

- [ ] **Step 2: Add Display impl entries**

```rust
            Self::MapProtoSizeGet => "Map.prototype.size",
            Self::SetProtoSizeGet => "Set.prototype.size",
```

- [ ] **Step 3: Build check and commit**

Run: `cargo check -p wjsm-ir`
Expected: compiles

```bash
git add crates/wjsm-ir/src/lib.rs
git commit -m "feat(ir): add Map.prototype.size and Set.prototype.size getter variants"
```

---

### Task 2: Add semantic layer recognition for size getter

**Files:**
- Modify: `crates/wjsm-semantic/src/lib.rs`

- [ ] **Step 1: Add size to prototype method helpers**

In `builtin_from_map_proto_method`, add:
```rust
        "size" => Some(MapProtoSizeGet),
```

In `builtin_from_set_proto_method`, add:
```rust
        "size" => Some(SetProtoSizeGet),
```

- [ ] **Step 2: Build check and commit**

Run: `cargo check -p wjsm-semantic`
Expected: compiles

```bash
git add crates/wjsm-semantic/src/lib.rs
git commit -m "feat(semantic): add Map/Set size getter recognition"
```

---

### Task 3: Register WASM types and imports for size getters

**Files:**
- Modify: `crates/wjsm-backend-wasm/src/lib.rs`

- [ ] **Step 1: Add WASM types**

No new types needed — reuse `(i64) -> (i64)` (Type 3).

- [ ] **Step 2: Add import declarations**

After the last existing import:
```rust
        imports.import("env", "map_proto_size_get", EntityType::Function(3));
        imports.import("env", "set_proto_size_get", EntityType::Function(3));
```

- [ ] **Step 3: Add builtin_arity entries**

```rust
        Builtin::MapProtoSizeGet => ("map_proto_size_get", 0),
        Builtin::SetProtoSizeGet => ("set_proto_size_get", 0),
```

- [ ] **Step 4: Add builtin_func_indices entries**

```rust
        builtin_func_indices.insert(Builtin::MapProtoSizeGet, 384);
        builtin_func_indices.insert(Builtin::SetProtoSizeGet, 385);
```

- [ ] **Step 5: Build check and commit**

Run: `cargo check -p wjsm-backend-wasm`
Expected: compiles

```bash
git add crates/wjsm-backend-wasm/src/lib.rs
git commit -m "feat(wasm-backend): register Map/Set size getter WASM imports"
```

---

### Task 4: Implement size getter host functions and remove data property

**Files:**
- Modify: `crates/wjsm-runtime/src/lib.rs`

- [ ] **Step 1: Remove size data property from Map/Set allocation**

Find `alloc_map` and remove:
```rust
    // REMOVE these lines:
    // let size_val = value::encode_f64(0.0);
    // let _ = define_host_data_property_from_caller(caller, obj, "size", size_val);
```

Find `alloc_set` and remove the same lines.

- [ ] **Step 2: Remove size updates from Map mutating operations**

Find `map_proto_set_fn` and remove the size update block:
```rust
    // REMOVE:
    // let new_size_val = value::encode_f64(new_size as f64);
    // let _ = define_host_data_property_from_caller(&mut caller, obj_ptr, "size", new_size_val);
```

Find `map_proto_delete_fn` and remove:
```rust
    // REMOVE:
    // let new_size_val = value::encode_f64((size - 1) as f64);
    // let _ = define_host_data_property_from_caller(&mut caller, obj_ptr, "size", new_size_val);
```

Find `map_proto_clear_fn` and remove:
```rust
    // REMOVE:
    // let zero = value::encode_f64(0.0);
    // let _ = define_host_data_property_from_caller(&mut caller, obj_ptr, "size", zero);
```

- [ ] **Step 3: Remove size updates from Set mutating operations**

Similarly, find `set_proto_add_fn`, `set_proto_delete_fn`, `set_proto_clear_fn` and remove all `define_host_data_property_from_caller` calls for `"size"`.

- [ ] **Step 4: Implement size getter host functions**

```rust
    let map_proto_size_get_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
            let state = caller.data();
            let obj_ptr = {
                let handles = state.object_handles.lock().expect("object_handles mutex");
                let idx = value::decode_object_handle(receiver) as usize;
                handles.get(idx).copied()
            };
            let (table, handle) = match obj_ptr {
                Some(ptr) => {
                    let map_table = state.map_table.lock().expect("map_table mutex");
                    let h = read_object_property_by_name_static(state, ptr, "__map_handle__");
                    match h {
                        Some(v) => {
                            let handle = value::decode_f64(v) as usize;
                            if let Some(entry) = map_table.get(handle) {
                                return value::encode_f64(entry.map.len() as f64);
                            }
                            return value::encode_f64(0.0);
                        }
                        None => return value::encode_f64(0.0),
                    }
                }
                None => return value::encode_f64(0.0),
            };
        },
    );

    let set_proto_size_get_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
            let state = caller.data();
            let obj_ptr = {
                let handles = state.object_handles.lock().expect("object_handles mutex");
                let idx = value::decode_object_handle(receiver) as usize;
                handles.get(idx).copied()
            };
            match obj_ptr {
                Some(ptr) => {
                    let set_table = state.set_table.lock().expect("set_table mutex");
                    let h = read_object_property_by_name_static(state, ptr, "__set_handle__");
                    match h {
                        Some(v) => {
                            let handle = value::decode_f64(v) as usize;
                            if let Some(entry) = set_table.get(handle) {
                                return value::encode_f64(entry.set.len() as f64);
                            }
                            return value::encode_f64(0.0);
                        }
                        None => return value::encode_f64(0.0),
                    }
                }
                None => return value::encode_f64(0.0),
            }
        },
    );
```

- [ ] **Step 5: Add size getter imports to the imports array**

After the last existing import:
```rust
        map_proto_size_get_fn.into(),          // 384
        set_proto_size_get_fn.into(),          // 385
```

- [ ] **Step 6: Full build check and commit**

Run: `cargo check`
Expected: compiles across all crates

```bash
git add crates/wjsm-runtime/src/lib.rs
git commit -m "fix(runtime): make Map/Set size a dynamic getter instead of data property"
```

---

### Task 5: Add Map/Set size getter test fixtures

**Files:**
- Create: `fixtures/happy/map_size_getter.js` + `.expected`
- Create: `fixtures/happy/set_size_getter.js` + `.expected`

- [ ] **Step 1: map_size_getter test**

`fixtures/happy/map_size_getter.js`:
```js
var m = new Map();
console.log(m.size);
m.set("a", 1);
console.log(m.size);
m.set("b", 2);
console.log(m.size);
m.delete("a");
console.log(m.size);
m.clear();
console.log(m.size);
```

`fixtures/happy/map_size_getter.expected`:
```
0
1
2
1
0
```

- [ ] **Step 2: set_size_getter test**

`fixtures/happy/set_size_getter.js`:
```js
var s = new Set();
console.log(s.size);
s.add(1);
console.log(s.size);
s.add(2);
console.log(s.size);
s.delete(1);
console.log(s.size);
s.clear();
console.log(s.size);
```

`fixtures/happy/set_size_getter.expected`:
```
0
1
2
1
0
```

- [ ] **Step 3: Run tests and commit**

Run: `cargo test`
Expected: new fixture tests pass, existing Map/Set tests still pass

```bash
git add fixtures/happy/map_size_getter.js fixtures/happy/map_size_getter.expected \
        fixtures/happy/set_size_getter.js fixtures/happy/set_size_getter.expected
git commit -m "test: add Map/Set size getter test fixtures"
```
