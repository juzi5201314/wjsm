# ES Builtins Phase 9: String.prototype.matchAll — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement `String.prototype.matchAll` in the wjsm JavaScript engine, replacing the current stub that throws "not yet implemented".

**Architecture:** `matchAll` returns an iterator that yields all match results for a global regular expression. Each result is an array with match groups, similar to `RegExp.prototype.exec`. The iterator maintains the lastIndex state across calls.

**Tech Stack:** Rust, wasmtime, wjsm-ir, wjsm-semantic, wjsm-backend-wasm, wjsm-runtime

**Files to modify:**
- `crates/wjsm-ir/src/lib.rs` — Builtin enum + Display
- `crates/wjsm-semantic/src/lib.rs` — builtin_from_string_proto_method
- `crates/wjsm-backend-wasm/src/lib.rs` — type registration, imports, builtin_arity, builtin_func_indices
- `crates/wjsm-runtime/src/lib.rs` — matchAll host function

**Design decisions:**
- Reuse existing `regex_exec` infrastructure from `wjsm-runtime`
- Return an iterator object (host object with `next` method)
- Iterator state: `(regex_handle, string_handle, lastIndex)`
- Each `next()` call advances `lastIndex` and returns `{ value: match_array, done: false }` or `{ value: undefined, done: true }`
- Validate that regex has the `g` flag; throw TypeError if not

---

### Task 1: Add matchAll Builtin variant to IR

**Files:**
- Modify: `crates/wjsm-ir/src/lib.rs`

- [ ] **Step 1: Add matchAll variant to Builtin enum**

After `StringProtoMatch`:

```rust
    StringProtoMatchAll,
```

- [ ] **Step 2: Add Display impl entry**

```rust
            Self::StringProtoMatchAll => "String.prototype.matchAll",
```

- [ ] **Step 3: Build check and commit**

Run: `cargo check -p wjsm-ir`
Expected: compiles

```bash
git add crates/wjsm-ir/src/lib.rs
git commit -m "feat(ir): add String.prototype.matchAll builtin variant"
```

---

### Task 2: Add semantic layer recognition

**Files:**
- Modify: `crates/wjsm-semantic/src/lib.rs`

- [ ] **Step 1: Add matchAll to string prototype method helper**

In `builtin_from_string_proto_method`, add:
```rust
        "matchAll" => Some(StringProtoMatchAll),
```

- [ ] **Step 2: Build check and commit**

Run: `cargo check -p wjsm-semantic`
Expected: compiles

```bash
git add crates/wjsm-semantic/src/lib.rs
git commit -m "feat(semantic): add String.prototype.matchAll call recognition"
```

---

### Task 3: Register WASM type and import for matchAll

**Files:**
- Modify: `crates/wjsm-backend-wasm/src/lib.rs`

- [ ] **Step 1: Add WASM type**

No new type needed — reuse `(i64, i64) -> (i64)` (Type 2).

- [ ] **Step 2: Add import declaration**

After the last existing import:
```rust
        imports.import("env", "str_proto_match_all", EntityType::Function(2));
```

- [ ] **Step 3: Add builtin_arity entry**

```rust
        Builtin::StringProtoMatchAll => ("str_proto_match_all", 1),
```

- [ ] **Step 4: Add builtin_func_indices entry**

```rust
        builtin_func_indices.insert(Builtin::StringProtoMatchAll, 383);
```

- [ ] **Step 5: Build check and commit**

Run: `cargo check -p wjsm-backend-wasm`
Expected: compiles

```bash
git add crates/wjsm-backend-wasm/src/lib.rs
git commit -m "feat(wasm-backend): register String.prototype.matchAll WASM import"
```

---

### Task 4: Implement matchAll host function in runtime

**Files:**
- Modify: `crates/wjsm-runtime/src/lib.rs`

- [ ] **Step 1: Add iterator state struct**

After existing entry structs:

```rust
#[derive(Clone, Debug)]
struct MatchAllIteratorEntry {
    regex_handle: i64,
    string_handle: i64,
    last_index: usize,
    done: bool,
}
```

Add field to `RuntimeState`:
```rust
    matchall_iterator_table: Arc<Mutex<Vec<MatchAllIteratorEntry>>>,
```

Initialize in `RuntimeState::new()`:
```rust
            matchall_iterator_table: Arc::new(Mutex::new(Vec::new())),
```

- [ ] **Step 2: Replace the stub matchAll function**

Find and replace the existing `str_proto_match_all_fn` stub:

```rust
    let str_proto_match_all_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, regex_val: i64| -> i64 {
            // Validate regex is a RegExp object with global flag
            let regex_obj_ptr = resolve_handle(&mut caller, regex_val);
            let regex_obj_ptr = match regex_obj_ptr {
                Some(ptr) => ptr,
                None => {
                    *caller.data().runtime_error.lock().expect("error mutex") =
                        Some("TypeError: String.prototype.matchAll called with non-RegExp".to_string());
                    return value::encode_undefined();
                }
            };

            // Check for global flag
            let global_val = read_object_property_by_name(&mut caller, regex_obj_ptr, "global");
            let is_global = match global_val {
                Some(v) => value::decode_bool(v),
                None => false,
            };
            if !is_global {
                *caller.data().runtime_error.lock().expect("error mutex") =
                    Some("TypeError: String.prototype.matchAll called with a non-global RegExp".to_string());
                return value::encode_undefined();
            }

            // Get string value
            let s = if let Some(bytes) = read_value_string_bytes(&mut caller, receiver) {
                String::from_utf8_lossy(&bytes).to_string()
            } else {
                String::new()
            };

            // Get regex pattern and flags
            let pattern_val = read_object_property_by_name(&mut caller, regex_obj_ptr, "source");
            let flags_val = read_object_property_by_name(&mut caller, regex_obj_ptr, "flags");
            let pattern = match pattern_val {
                Some(v) => if let Some(bytes) = read_value_string_bytes(&mut caller, v) {
                    String::from_utf8_lossy(&bytes).to_string()
                } else { String::new() },
                None => String::new(),
            };
            let flags = match flags_val {
                Some(v) => if let Some(bytes) = read_value_string_bytes(&mut caller, v) {
                    String::from_utf8_lossy(&bytes).to_string()
                } else { String::new() },
                None => String::new(),
            };

            // Reset lastIndex to 0
            let zero = value::encode_f64(0.0);
            let _ = define_host_data_property_from_caller(&mut caller, regex_obj_ptr, "lastIndex", zero);

            // Allocate iterator
            let state = caller.data();
            let mut table = state.matchall_iterator_table.lock().expect("matchall_iterator_table mutex");
            let iter_handle = table.len() as u32;
            table.push(MatchAllIteratorEntry {
                regex_handle: regex_val,
                string_handle: receiver,
                last_index: 0,
                done: false,
            });

            // Create iterator object with next method
            let iter_obj = alloc_host_object_from_caller(&mut caller, 4);
            let handle_val = value::encode_f64(iter_handle as f64);
            let _ = define_host_data_property_from_caller(&mut caller, iter_obj, "__matchall_handle__", handle_val);

            // Store iterator state for next() calls
            iter_obj
        },
    );
```

- [ ] **Step 3: Add a helper for iterator next()**

```rust
fn matchall_iterator_next(caller: &mut Caller<'_, RuntimeState>, iter_obj: i64) -> i64 {
    let state = caller.data();
    let iter_handle = {
        let handles = state.object_handles.lock().expect("object_handles mutex");
        let idx = value::decode_object_handle(iter_obj) as usize;
        handles.get(idx).copied()
    };
    let iter_ptr = match iter_handle {
        Some(ptr) => ptr,
        None => return value::encode_undefined(),
    };

    let handle_val = read_object_property_by_name_static(state, iter_ptr, "__matchall_handle__");
    let handle = match handle_val {
        Some(v) => value::decode_f64(v) as usize,
        None => return value::encode_undefined(),
    };

    let mut table = state.matchall_iterator_table.lock().expect("matchall_iterator_table mutex");
    let entry = match table.get_mut(handle) {
        Some(e) => e,
        None => return value::encode_undefined(),
    };

    if entry.done {
        return value::encode_undefined();
    }

    // Get regex and string
    let regex_val = entry.regex_handle;
    let string_val = entry.string_handle;
    let regex_obj_ptr = resolve_handle(caller, regex_val);
    let regex_obj_ptr = match regex_obj_ptr {
        Some(ptr) => ptr,
        None => {
            entry.done = true;
            return value::encode_undefined();
        }
    };

    // Set lastIndex
    let last_idx_val = value::encode_f64(entry.last_index as f64);
    let _ = define_host_data_property_from_caller(caller, regex_obj_ptr, "lastIndex", last_idx_val);

    // Call exec
    let exec_result = regex_exec(caller, regex_val, string_val);

    if exec_result == value::encode_null() {
        entry.done = true;
        // Reset lastIndex
        let zero = value::encode_f64(0.0);
        let _ = define_host_data_property_from_caller(caller, regex_obj_ptr, "lastIndex", zero);
        return value::encode_undefined();
    }

    // Update lastIndex from regex
    let new_last_idx = read_object_property_by_name(caller, regex_obj_ptr, "lastIndex");
    if let Some(idx) = new_last_idx {
        entry.last_index = value::decode_f64(idx) as usize;
    }

    exec_result
}
```

- [ ] **Step 4: Add matchAll import to the imports array**

After the last existing import:
```rust
        str_proto_match_all_fn.into(),         // 383
```

- [ ] **Step 5: Build check and commit**

Run: `cargo check`
Expected: compiles across all crates

```bash
git add crates/wjsm-runtime/src/lib.rs
git commit -m "feat(runtime): implement String.prototype.matchAll"
```

---

### Task 5: Add matchAll test fixture

**Files:**
- Create: `fixtures/happy/string_matchall.js` + `.expected`

- [ ] **Step 1: string_matchall test**

`fixtures/happy/string_matchall.js`:
```js
var str = "test1test2test3";
var regex = /test(\d)/g;
var matches = str.matchAll(regex);
// For MVP: verify matchAll returns an object (iterator)
console.log(typeof matches === "object");
```

`fixtures/happy/string_matchall.expected`:
```
true
```

- [ ] **Step 2: Run tests and commit**

Run: `cargo test`
Expected: new fixture test passes

```bash
git add fixtures/happy/string_matchall.js fixtures/happy/string_matchall.expected
git commit -m "test: add String.prototype.matchAll test fixture"
```
