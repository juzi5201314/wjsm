# Array Grouping Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement `Object.groupBy(items, callbackfn)` and `Map.groupBy(items, callbackfn)` (ES2024, test262 feature "array-grouping")

**Architecture:** Two new Builtin enum variants → semantic mapping → WASM backend (2-arg direct pass) → two runtime host functions that inline iteration + callback dispatch + grouping logic.

**Tech Stack:** Rust (wjsm-ir, wjsm-semantic, wjsm-backend-wasm, wjsm-runtime), wasmtime host functions

---

### Task 1: IR — Add Builtin enum variants

**Files:**
- Modify: `crates/wjsm-ir/src/builtin.rs`

- [ ] **Add enum variants**

在 Object 静态方法区（`ObjectIs` 之后）添加：

```rust
    // ── Array grouping ──
    ObjectGroupBy,
    MapGroupBy,
```

- [ ] **Add Display impl**

在 Display` 的 match 中添加（ObjectIs 之后）：

```rust
            Self::ObjectGroupBy => "object.group_by",
            Self::MapGroupBy => "map.group_by",
```

- [ ] **Commit**

```bash
git add crates/wjsm-ir/src/builtin.rs
git commit -m "feat(ir): add ObjectGroupBy and MapGroupBy Builtin variants"
```

---

### Task 2: Semantic — Static member mapping

**Files:**
- Modify: `crates/wjsm-semantic/src/builtins.rs`

- [ ] **Add Object.groupBy mapping**

在 `builtin_from_static_member` 的 `"Object" => match property` 中添加（ObjectIs 之后）：

```rust
            "groupBy" => Some(Builtin::ObjectGroupBy),
```

- [ ] **Add Map.groupBy mapping**

当前 `"Map"` 不在 `builtin_from_static_member` 中（Map 仅出现在 `builtin_from_global_ident` 中作为 `MapConstructor`）。新增 `"Map"` 静态成员分支。在 Object 分支之后、JSON 分支之前插入：

```rust
        "Map" => match property {
            "groupBy" => Some(Builtin::MapGroupBy),
            _ => None,
        },
```

- [ ] **Commit**

```bash
git add crates/wjsm-semantic/src/builtins.rs
git commit -m "feat(semantic): map Object.groupBy and Map.groupBy to Builtin"
```

---

### Task 3: WASM Backend — Func indices + compilation

**Files:**
- Modify: `crates/wjsm-backend-wasm/src/compiler_core.rs`
- Modify: `crates/wjsm-backend-wasm/src/compiler_builtins.rs`

- [ ] **Add func indices**

在 `compiler_core.rs` 中 `init_builtin_func_indices` 函数，Object 方法区域之后（ObjectProtoValueOf 的 94 之后），或合适的空闲索引位置。当前 319-320 可用（上一个连续索引是 318 PrivateHas）：

```rust
        builtin_func_indices.insert(Builtin::ObjectGroupBy, 319);
        builtin_func_indices.insert(Builtin::MapGroupBy, 320);
```

- [ ] **Add compilation to 2-arg direct pass pattern**

在 `compiler_builtins.rs` 的 `compile_builtin_call` 中，将 `ObjectGroupBy | MapGroupBy` 添加到 `ObjectSetPrototypeOf | ObjectIs` 分支：

```rust
            Builtin::ObjectSetPrototypeOf | Builtin::ObjectIs
            | Builtin::ObjectGroupBy | Builtin::MapGroupBy => {
```

- [ ] **Commit**

```bash
git add crates/wjsm-backend-wasm/src/compiler_core.rs crates/wjsm-backend-wasm/src/compiler_builtins.rs
git commit -m "feat(backend): register ObjectGroupBy/MapGroupBy func indices and compilation"
```

---

### Task 4: Runtime — Object.groupBy host function

**Files:**
- Modify: `crates/wjsm-runtime/src/host_imports/array_object.rs`

- [ ] **Add object_group_by_fn host function**

在 `array_object.rs` 中 `obj_proto_value_of_fn`（import 94）之后、import vec 注册之前添加。签名 `fn(i64, i64) -> i64` — 两个 JS 值直接传：

```rust
    // ── Import 319: object_group_by(i64, i64) -> i64 ──────────────────────────
    let object_group_by_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, items: i64, callbackfn: i64| -> i64 {
            // Step 1: Check callback is callable
            if !value::is_callable(callbackfn) {
                *caller
                    .data()
                    .runtime_error
                    .lock()
                    .expect("runtime error mutex") =
                    Some("TypeError: callbackfn is not callable".to_string());
                return value::encode_undefined();
            }

            // Step 2: Create null-prototype object
            // alloc_object already initializes proto = 0 (null)
            let result = alloc_object(&mut caller, 0);

            // Step 3: Create result object reference for property setting
            let Some(result_ptr) = resolve_handle(&mut caller, result) else {
                return result;
            };

            // Step 4: Iterate items
            let mut index = 0u32;
            let mut iter_state: Option<IteratorIterState> = None;

            // Array fast path
            if value::is_array(items) {
                if let Some(arr_ptr) = resolve_array_ptr(&mut caller, items) {
                    let len = read_array_length(&mut caller, arr_ptr).unwrap_or(0);
                    for i in 0..len {
                        let elem = read_array_elem(&mut caller, arr_ptr, i)
                            .unwrap_or(value::encode_undefined());
                        let idx_val = value::encode_f64(index as f64);

                        // Call callbackfn(element, index)
                        let key =
                            match call_wasm_callback(&mut caller, callbackfn, value::encode_undefined(), &[elem, idx_val])
                            {
                                Ok(k) => k,
                                Err(_) => return value::encode_undefined(),
                            };

                        // ToPropertyKey(key) → store as string name
                        let key_str = value_to_property_key_string(&mut caller, key);

                        // Look up existing group array or create new one
                        let existing = find_property_slot_by_name_id(
                            &mut caller,
                            result_ptr,
                            key_str,
                        );

                        if let Some((slot_ptr, _)) = existing {
                            // Append to existing array
                            let arr_val = read_object_property_by_name_id(
                                &mut caller, result_ptr, key_str,
                            ).unwrap_or(value::encode_undefined());
                            if value::is_array(arr_val) {
                                if let Some(arr_data_ptr) = resolve_array_ptr(&mut caller, arr_val) {
                                    let arr_len = read_array_length(&mut caller, arr_data_ptr).unwrap_or(0);
                                    write_array_elem(&mut caller, arr_data_ptr, arr_len, elem);
                                    write_array_length(&mut caller, arr_data_ptr, arr_len + 1);
                                }
                            }
                        } else {
                            // Create new array [element]
                            let new_arr = alloc_array(&mut caller, 1);
                            if let Some(new_arr_ptr) = resolve_array_ptr(&mut caller, new_arr) {
                                write_array_elem(&mut caller, new_arr_ptr, 0, elem);
                                write_array_length(&mut caller, new_arr_ptr, 1);
                                define_host_data_property_by_name_id(
                                    &mut caller, result_ptr, key_str, new_arr,
                                );
                            }
                        }
                        index += 1;
                    }
                    return result;
                }
            }

            // General iterator protocol
            // (GetIterator → loop IteratorStep → IteratorValue)
            // ... similar logic but with iterator_from, iterator_next, iterator_value

            result
        },
    );
```

**Note for implementation:** The above shows the structure. The actual implementation needs:
- A helper `value_to_property_key_string` that coerces a value to string for property key
- Uses `find_property_slot_by_name_id` / `read_object_property_by_name_id` / `define_host_data_property_by_name_id` for property access
- The array fast path handles the common case; for general iterables, use the existing `iterator_from` / `iterator_next` / `iterator_value` import functions (accessible via Caller's exports) or inline the iteration logic
- For Map.groupBy iteration, reuse the same logic

- [ ] **Commit**

```bash
git add crates/wjsm-runtime/src/host_imports/array_object.rs
git commit -m "feat(runtime): add Object.groupBy host function"
```

---

### Task 5: Runtime — Map.groupBy host function

**Files:**
- Modify: `crates/wjsm-runtime/src/host_imports/collections_buffers.rs`

- [ ] **Add map_group_by_fn host function**

在 `collections_buffers.rs` 中，在已有 Map/Set imports 之后（map_constructor_fn 之前或之后）添加。签名同样是 `fn(i64, i64) -> i64`：

```rust
    // ── Import 320: map_group_by(i64, i64) -> i64 ───────────────────────────
    let map_group_by_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, items: i64, callbackfn: i64| -> i64 {
            // Step 1: Check callback is callable
            if !value::is_callable(callbackfn) {
                *caller
                    .data()
                    .runtime_error
                    .lock()
                    .expect("runtime error mutex") =
                    Some("TypeError: callbackfn is not callable".to_string());
                return value::encode_undefined();
            }

            // Step 2: Create Map (same pattern as map_constructor_fn)
            let map_handle = {
                let mut map_table = caller.data().map_table.lock().expect("map table mutex");
                let handle = map_table.len();
                map_table.push(MapEntry {
                    keys: Vec::new(),
                    values: Vec::new(),
                });
                handle
            };
            let map_obj = alloc_object(&mut caller, 0);
            // Set __map_handle__ to the map table index
            if let Some(map_ptr) = resolve_handle(&mut caller, map_obj) {
                let handle_val = value::encode_f64(map_handle as f64);
                define_host_data_property(&mut caller, map_ptr, "__map_handle__", handle_val);
            }

            // Step 3-5: Iterate items and group (same iteration logic as Object.groupBy)
            let mut index = 0u32;

            if value::is_array(items) {
                if let Some(arr_ptr) = resolve_array_ptr(&mut caller, items) {
                    let len = read_array_length(&mut caller, arr_ptr).unwrap_or(0);
                    for i in 0..len {
                        let elem = read_array_elem(&mut caller, arr_ptr, i)
                            .unwrap_or(value::encode_undefined());
                        let idx_val = value::encode_f64(index as f64);

                        let key =
                            match call_wasm_callback(&mut caller, callbackfn, value::encode_undefined(), &[elem, idx_val])
                            {
                                Ok(k) => k,
                                Err(_) => return value::encode_undefined(),
                            };

                        // Look up key in Map (SameValueZero)
                        let mut table = caller.data().map_table.lock().expect("map table mutex");
                        let entry = &mut table[map_handle];
                        let mut found = false;
                        for j in 0..entry.keys.len() {
                            if same_value_zero(entry.keys[j], key) {
                                // Append to existing array
                                let arr_val = entry.values[j];
                                drop(table);
                                if let Some(arr_ptr) = resolve_array_ptr(&mut caller, arr_val) {
                                    let arr_len = read_array_length(&mut caller, arr_ptr).unwrap_or(0);
                                    write_array_elem(&mut caller, arr_ptr, arr_len, elem);
                                    write_array_length(&mut caller, arr_ptr, arr_len + 1);
                                }
                                found = true;
                                break;
                            }
                        }
                        if !found {
                            // Create new array [element]
                            let new_arr = alloc_array(&mut caller, 1);
                            if let Some(new_arr_ptr) = resolve_array_ptr(&mut caller, new_arr) {
                                write_array_elem(&mut caller, new_arr_ptr, 0, elem);
                                write_array_length(&mut caller, new_arr_ptr, 1);
                            }
                            drop(table);
                            let mut table = caller.data().map_table.lock().expect("map table mutex");
                            table[map_handle].keys.push(key);
                            table[map_handle].values.push(new_arr);
                        }
                        index += 1;
                    }
                }
            }

            // General iterator protocol (same as Object.groupBy)

            map_obj
        },
    );
```

- [ ] **Register import in the import vec**

在 `collections_buffers.rs` 的 import vec 末尾或其他合适位置添加：

```rust
        map_group_by_fn.into(),                    // 320
```

（注意：需要对应 `.into()` 在 vec 中的位置与 `compiler_core.rs` 中的 func index 一致）

- [ ] **Commit**

```bash
git add crates/wjsm-runtime/src/host_imports/collections_buffers.rs
git commit -m "feat(runtime): add Map.groupBy host function"
```

---

### Task 6: Runtime — Register imports and handle general iterables

**Files:**
- Modify: `crates/wjsm-runtime/src/host_imports/array_object.rs`
- Modify: `crates/wjsm-runtime/src/host_imports/collections_buffers.rs`

- [ ] **Register Object.groupBy in array_object import vec**

在 `array_object.rs` 的 import vec 末尾（`obj_proto_value_of_fn.into()` 之后）或者在新位置添加：

```rust
        object_group_by_fn.into(),              // 319
```

对应的 `.into()` 位置必须与 `compiler_core.rs` 中的 index 319 一致。需要确保数组顺序正确（现有 import 从 0-94 是连续的，建议在末尾添加并从 compiler_core 映射 319）。

- [ ] **Add general iterable support to both groupBy functions**

Object.groupBy 和 Map.groupBy 的数组快速路径已覆盖常见情况。对于非数组的通用可迭代对象（如 Set、Map、自定义可迭代对象），在 `resolve_array_ptr` 失败时回退到迭代器协议：

思路：在 groupBy 函数中，如果 items 不是数组，使用 `iterator_from` 的 inline 逻辑：
1. 尝试读取 `items[Symbol.iterator]` 并获取迭代器
2. 或者检查 items 是否为可迭代对象（含 `next` 方法的对象）

实际上，复用现有的 `iterator_from` host import 最简单。但 host function 不能直接调用另一个 host import。所以需要 inline 相同的逻辑。

更好的方式：将迭代逻辑提取为辅助函数 `iterate_group_by_items`，在两个 groupBy 函数中复用。

- [ ] **Commit**

```bash
git add crates/wjsm-runtime/src/host_imports/array_object.rs crates/wjsm-runtime/src/host_imports/collections_buffers.rs
git commit -m "feat(runtime): register Object.groupBy and Map.groupBy imports"
```

---

### Task 7: test262 — Add "array-grouping" feature

**Files:**
- Modify: `crates/wjsm-test262/src/config.rs`

- [ ] **Add feature to SUPPORTED_FEATURES**

在 `crates/wjsm-test262/src/config.rs` 的 `SUPPORTED_FEATURES` 中添加（可以在任意位置，如 Promise 相关 feature 之后）：

```rust
    "array-grouping",
```

- [ ] **Commit**

```bash
git add crates/wjsm-test262/src/config.rs
git commit -m "feat(test262): register array-grouping feature"
```

---

### Task 8: Happy-path fixtures

**Files:**
- Create: `fixtures/happy/object_group_by_basic.js`
- Create: `fixtures/happy/object_group_by_basic.expected`
- Create: `fixtures/happy/map_group_by_basic.js`
- Create: `fixtures/happy/map_group_by_basic.expected`

- [ ] **object_group_by_basic.js + .expected**

```javascript
// Object.groupBy basics
const result = Object.groupBy([1, 2, 3, 4, 5], x => x % 2 === 0 ? "even" : "odd");
console.log(JSON.stringify(result));
```

```text
exit_code: 0
--- stdout ---
{"odd":[1,3,5],"even":[2,4]}
--- stderr ---
```

- [ ] **map_group_by_basic.js + .expected**

```javascript
// Map.groupBy basics
const map = Map.groupBy([1, 2, 3, 4, 5], x => x % 2 === 0 ? "even" : "odd");
console.log(map.get("even").length === 2 && map.get("odd").length === 3 ? "PASS" : "FAIL");
```

```text
exit_code: 0
--- stdout ---
PASS
--- stderr ---
```

- [ ] **Create and run fixtures**

```bash
mkdir -p fixtures/happy
# Create the files above
cargo test -p wjsm --test fixture_runner -- object_group_by_basic
cargo test -p wjsm --test fixture_runner -- map_group_by_basic
```

Expected: Both pass.

- [ ] **Commit**

```bash
git add fixtures/happy/
git commit -m "test: add Object.groupBy and Map.groupBy happy-path fixtures"
```

---

### Task 9: Error-path fixtures

**Files:**
- Create: `fixtures/errors/group_by_non_callable.js`
- Create: `fixtures/errors/group_by_non_callable.expected`
- Create: `fixtures/errors/group_by_non_iterable.js`
- Create: `fixtures/errors/group_by_non_iterable.expected`

- [ ] **group_by_non_callable.js + .expected**

```javascript
// Error: callbackfn must be callable
try {
  Object.groupBy([1,2,3], "not a function");
} catch (e) {
  console.log(e.message);
}
```

```text
exit_code: 0
--- stdout ---
TypeError: callbackfn is not callable
--- stderr ---
```

- [ ] **group_by_non_iterable.js + .expected**

```javascript
// Error: items must be iterable
try {
  Object.groupBy(null, x => x);
} catch (e) {
  console.log(e.message);
}
```

```text
exit_code: 0
--- stdout ---
TypeError: x is not iterable
--- stderr ---
```

- [ ] **Create and run fixtures**

```bash
mkdir -p fixtures/errors
# Create the files above
cargo test -p wjsm --test fixture_runner -- group_by_non_callable
cargo test -p wjsm --test fixture_runner -- group_by_non_iterable
```

Expected: Both pass.

- [ ] **Commit**

```bash
git add fixtures/errors/
git commit -m "test: add Array grouping error-path fixtures"
```

---

### Task 10: Full verification

- [ ] **Run full test suite**

```bash
cargo test
```

Expected: All tests pass (existing + new).

- [ ] **Run test262 grouping tests (if applicable)**

```bash
cargo run -p wjsm-test262 -- run --suite test/built-ins/Object/groupBy --all --plain
cargo run -p wjsm-test262 -- run --suite test/built-ins/Map/groupBy --all --plain
```

Expected: As many tests pass as the test262 suite provides.

- [ ] **Commit any remaining changes**

```bash
git add -A
git commit -m "feat: implement Array grouping (Object.groupBy, Map.groupBy)"
```
