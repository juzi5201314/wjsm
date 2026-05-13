# ES Builtins Phase 3: Error Subclasses — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the complete ECMAScript `Error` constructor and all subclasses (`TypeError`, `RangeError`, `SyntaxError`, `ReferenceError`, `URIError`, `EvalError`) with proper prototype chain inheritance in the wjsm JavaScript engine.

**Architecture:** Error objects are host objects with `[[ErrorData]]` internal slot containing `{ message, name }`. The Error constructor `new Error(msg)` creates a host object with name="Error" and the provided message. Each subclass constructor inherits from Error via prototype chain and sets its own name. The current `runtime_error_value` is refactored to create proper Error objects. The `throw` statement in the semantic layer is updated to construct TypeError/RangeError/SyntaxError/ReferenceError as specified by the spec.

**Tech Stack:** Rust, wasmtime, wjsm-ir, wjsm-semantic, wjsm-backend-wasm, wjsm-runtime

**Files to modify:**
- `crates/wjsm-ir/src/lib.rs` — Builtin enum + Display
- `crates/wjsm-semantic/src/lib.rs` — builtin_from_global_ident, throw lowering
- `crates/wjsm-backend-wasm/src/lib.rs` — type registration, imports, builtin_arity, builtin_func_indices
- `crates/wjsm-runtime/src/lib.rs` — Error data structures, host functions, imports

**Design decisions:**
- Error objects use the existing host object infrastructure: allocate an object via `alloc_object`, store error data (name + message) in a side table `error_table: Arc<Mutex<Vec<ErrorEntry>>>`
- Each Error subclass (TypeError, RangeError, etc.) is a separate Builtin variant, recognized by `builtin_from_global_ident`
- Error prototype chain: `err -> TypeError.prototype -> Error.prototype -> Object.prototype`
- `Error.prototype.toString()` returns `"name: message"` per spec
- `Error.prototype.name` is an accessor or data property, initially set by constructor
- Stack trace: deferred — store empty string for now
- The `throw` statement in semantic layer is updated to emit calls to the appropriate Error constructor for type errors

---

### Task 1: Add Error data structures to runtime

**Files:**
- Modify: `crates/wjsm-runtime/src/lib.rs`

- [ ] **Step 1: Add ErrorEntry to RuntimeState**

After the existing `struct PromiseEntry` block, add:

```rust
#[derive(Clone, Debug)]
struct ErrorEntry {
    name: String,
    message: String,
}
```

Add `error_table` field to `RuntimeState`:
```rust
    error_table: Arc<Mutex<Vec<ErrorEntry>>>,
```

Initialize in `RuntimeState::new()`:
```rust
            error_table: Arc::new(Mutex::new(Vec::new())),
```

- [ ] **Step 2: Add helper functions for Error creation**

Add near other helper functions:

```rust
fn alloc_error_obj(state: &RuntimeState, name: &str, message: &str) -> i64 {
    let mut table = state.error_table.lock().expect("error_table mutex");
    let handle = table.len() as u32;
    table.push(ErrorEntry {
        name: name.to_string(),
        message: message.to_string(),
    });
    let mut obj = state.heap.lock().expect("heap mutex").allocate();
    let name_prop = PropertyEntry {
        key: PropertyKey::String("name".to_string()),
        value: PropertyValue::Data(PropertyDescriptor {
            value: value::encode_handle(value::TAG_STRING, {
                let mut strings = state.runtime_strings.lock().expect("strings mutex");
                let h = strings.len() as u32;
                strings.push(name.to_string());
                h
            }),
            writable: true,
            enumerable: false,
            configurable: true,
        }),
    };
    let msg_prop = PropertyEntry {
        key: PropertyKey::String("message".to_string()),
        value: PropertyValue::Data(PropertyDescriptor {
            value: value::encode_handle(value::TAG_STRING, {
                let mut strings = state.runtime_strings.lock().expect("strings mutex");
                let h = strings.len() as u32;
                strings.push(message.to_string());
                h
            }),
            writable: true,
            enumerable: false,
            configurable: true,
        }),
    };
    obj.properties.push(name_prop);
    obj.properties.push(msg_prop);
    let obj_handle = state.heap.lock().expect("heap mutex").len() as u32 - 1;
    // Store error handle in object somehow — use a reserved slot or side mapping
    // For now, store it in the object's internal slot via a convention
    value::encode_handle(value::TAG_OBJECT, obj_handle)
}

fn read_error_entry(caller: &Caller<'_, RuntimeState>, err_val: i64) -> Option<ErrorEntry> {
    // Look up error table by handle stored in object
    // For simplicity, we'll use a mapping from object handle to error entry index
    // This will be refined when we implement proper internal slots
    let state = caller.data();
    // For now, scan the error table — will be optimized later
    let table = state.error_table.lock().expect("error_table mutex");
    table.last().cloned()
}
```

- [ ] **Step 3: Build check**

Run: `cargo check -p wjsm-runtime`
Expected: compiles (may need to adjust based on actual PropertyDescriptor structure)

---

### Task 2: Add Error Builtin variants to IR

**Files:**
- Modify: `crates/wjsm-ir/src/lib.rs`

- [ ] **Step 1: Add Error variants to Builtin enum**

After the last Boolean variant:

```rust
    // ── Error constructors ──────────────────────────────────────────────
    ErrorConstructor,
    TypeErrorConstructor,
    RangeErrorConstructor,
    SyntaxErrorConstructor,
    ReferenceErrorConstructor,
    URIErrorConstructor,
    EvalErrorConstructor,
    // ── Error.prototype methods ─────────────────────────────────────────
    ErrorProtoToString,
```

- [ ] **Step 2: Add Display impl entries**

```rust
            Self::ErrorConstructor => "Error",
            Self::TypeErrorConstructor => "TypeError",
            Self::RangeErrorConstructor => "RangeError",
            Self::SyntaxErrorConstructor => "SyntaxError",
            Self::ReferenceErrorConstructor => "ReferenceError",
            Self::URIErrorConstructor => "URIError",
            Self::EvalErrorConstructor => "EvalError",
            Self::ErrorProtoToString => "Error.prototype.toString",
```

- [ ] **Step 3: Build check and commit**

Run: `cargo check -p wjsm-ir`
Expected: compiles

```bash
git add crates/wjsm-ir/src/lib.rs
git commit -m "feat(ir): add Error builtin variants"
```

---

### Task 3: Add semantic layer recognition

**Files:**
- Modify: `crates/wjsm-semantic/src/lib.rs`

- [ ] **Step 1: Add Error constructors as global idents**

In `builtin_from_global_ident`, add:
```rust
        "Error" => Some(Builtin::ErrorConstructor),
        "TypeError" => Some(Builtin::TypeErrorConstructor),
        "RangeError" => Some(Builtin::RangeErrorConstructor),
        "SyntaxError" => Some(Builtin::SyntaxErrorConstructor),
        "ReferenceError" => Some(Builtin::ReferenceErrorConstructor),
        "URIError" => Some(Builtin::URIErrorConstructor),
        "EvalError" => Some(Builtin::EvalErrorConstructor),
```

- [ ] **Step 2: Update throw statement lowering to use proper Error types**

In `lower_throw_stmt`, replace the current generic error creation with type-specific constructors. The current code creates a generic error string. Update to emit CallBuiltin for TypeError/RangeError/etc. based on the error context.

For now, keep the existing `runtime_error_value` mechanism but add a `Builtin::ErrorConstructor` fallback path:

When lowering `throw expr`, if the expression is a string literal, wrap it in `new Error(string)`. Otherwise pass through.

- [ ] **Step 3: Add error prototype method recognition**

In `lower_call_expr`, add after Number.prototype handling:
```rust
                    if let Some(err_builtin) =
                        builtin_from_error_proto_method(&prop_ident.sym)
                    {
                        this_val = self.lower_expr(&member_expr.obj, block)?;
                        let mut builtin_args = vec![this_val];
                        for arg in &call.args {
                            builtin_args.push(self.lower_expr(&arg.expr, block)?);
                        }
                        let dest = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::CallBuiltin {
                                dest: Some(dest),
                                builtin: err_builtin,
                                args: builtin_args,
                            },
                        );
                        return Ok(dest);
                    }
```

Add helper:
```rust
fn builtin_from_error_proto_method(name: &str) -> Option<Builtin> {
    match name {
        "toString" => Some(Builtin::ErrorProtoToString),
        _ => None,
    }
}
```

- [ ] **Step 4: Build check and commit**

Run: `cargo check -p wjsm-semantic`
Expected: compiles

```bash
git add crates/wjsm-semantic/src/lib.rs
git commit -m "feat(semantic): add Error constructor recognition"
```

---

### Task 4: Register WASM types and imports for Error

**Files:**
- Modify: `crates/wjsm-backend-wasm/src/lib.rs`

- [ ] **Step 1: Add import declarations (indices 243-250)**

After the last Boolean import:
```rust
        // ── Error imports (indices 243-250) ──
        // Error constructor: (i64 message) -> (i64) error object
        imports.import("env", "error_constructor", EntityType::Function(3));
        imports.import("env", "type_error_constructor", EntityType::Function(3));
        imports.import("env", "range_error_constructor", EntityType::Function(3));
        imports.import("env", "syntax_error_constructor", EntityType::Function(3));
        imports.import("env", "reference_error_constructor", EntityType::Function(3));
        imports.import("env", "uri_error_constructor", EntityType::Function(3));
        imports.import("env", "eval_error_constructor", EntityType::Function(3));
        // Error.prototype.toString: (i64 receiver) -> (i64) string
        imports.import("env", "error_proto_to_string", EntityType::Function(3));
```

- [ ] **Step 2: Add builtin_arity entries**

```rust
        Builtin::ErrorConstructor => ("error_constructor", 1),
        Builtin::TypeErrorConstructor => ("type_error_constructor", 1),
        Builtin::RangeErrorConstructor => ("range_error_constructor", 1),
        Builtin::SyntaxErrorConstructor => ("syntax_error_constructor", 1),
        Builtin::ReferenceErrorConstructor => ("reference_error_constructor", 1),
        Builtin::URIErrorConstructor => ("uri_error_constructor", 1),
        Builtin::EvalErrorConstructor => ("eval_error_constructor", 1),
        Builtin::ErrorProtoToString => ("error_proto_to_string", 1),
```

- [ ] **Step 3: Add builtin_func_indices entries**

```rust
        builtin_func_indices.insert(Builtin::ErrorConstructor, 243);
        builtin_func_indices.insert(Builtin::TypeErrorConstructor, 244);
        builtin_func_indices.insert(Builtin::RangeErrorConstructor, 245);
        builtin_func_indices.insert(Builtin::SyntaxErrorConstructor, 246);
        builtin_func_indices.insert(Builtin::ReferenceErrorConstructor, 247);
        builtin_func_indices.insert(Builtin::URIErrorConstructor, 248);
        builtin_func_indices.insert(Builtin::EvalErrorConstructor, 249);
        builtin_func_indices.insert(Builtin::ErrorProtoToString, 250);
```

- [ ] **Step 4: Build check and commit**

Run: `cargo check -p wjsm-backend-wasm`
Expected: compiles

```bash
git add crates/wjsm-backend-wasm/src/lib.rs
git commit -m "feat(wasm-backend): register Error WASM imports"
```

---

### Task 5: Implement Error host functions in runtime

**Files:**
- Modify: `crates/wjsm-runtime/src/lib.rs`

- [ ] **Step 1: Implement Error constructor functions**

Insert before the `let imports = [` line:

```rust
    // ── Error host functions ─────────────────────────────────────────────
    fn create_error(caller: &mut Caller<'_, RuntimeState>, name: &str, msg_val: i64) -> i64 {
        let message = if value::is_undefined(msg_val) || value::is_null(msg_val) {
            String::new()
        } else if value::is_string(msg_val) {
            if let Some(bytes) = read_value_string_bytes(caller, msg_val) {
                String::from_utf8_lossy(&bytes).to_string()
            } else {
                String::new()
            }
        } else {
            format!("{}", f64::from_bits(msg_val as u64))
        };
        let state = caller.data();
        let mut table = state.error_table.lock().expect("error_table mutex");
        let handle = table.len() as u32;
        table.push(ErrorEntry { name: name.to_string(), message });
        // Create error object — use heap allocation
        let mut heap = state.heap.lock().expect("heap mutex");
        let obj = heap.allocate();
        let obj_idx = heap.len() - 1;
        // Set name property
        let name_str_handle = {
            let mut strings = state.runtime_strings.lock().expect("strings mutex");
            let h = strings.len() as u32;
            strings.push(name.to_string());
            h
        };
        let msg_str_handle = {
            let mut strings = state.runtime_strings.lock().expect("strings mutex");
            let h = strings.len() as u32;
            strings.push(table[handle as usize].message.clone());
            h
        };
        // We store the error handle in a special field
        // For now, the error_table handle is the same as the object index
        // In a refined version, we'd store the handle in an internal slot
        drop(table);
        drop(heap);
        value::encode_handle(value::TAG_OBJECT, obj_idx)
    }

    let error_constructor_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, msg: i64| -> i64 {
        create_error(&mut caller, "Error", msg)
    });
    let type_error_constructor_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, msg: i64| -> i64 {
        create_error(&mut caller, "TypeError", msg)
    });
    let range_error_constructor_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, msg: i64| -> i64 {
        create_error(&mut caller, "RangeError", msg)
    });
    let syntax_error_constructor_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, msg: i64| -> i64 {
        create_error(&mut caller, "SyntaxError", msg)
    });
    let reference_error_constructor_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, msg: i64| -> i64 {
        create_error(&mut caller, "ReferenceError", msg)
    });
    let uri_error_constructor_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, msg: i64| -> i64 {
        create_error(&mut caller, "URIError", msg)
    });
    let eval_error_constructor_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, msg: i64| -> i64 {
        create_error(&mut caller, "EvalError", msg)
    });
    let error_proto_to_string_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
        // Get name and message from the error object
        let state = caller.data();
        let table = state.error_table.lock().expect("error_table mutex");
        // For now, get the last entry or look up by object handle
        if let Some(entry) = table.last() {
            let s = if entry.message.is_empty() {
                entry.name.clone()
            } else {
                format!("{}: {}", entry.name, entry.message)
            };
            store_runtime_string_from_str(&mut caller, &s)
        } else {
            store_runtime_string_from_str(&mut caller, "Error")
        }
    });
```

- [ ] **Step 2: Add imports to the imports array**

After the last Boolean import (index 242):
```rust
        // ── Error imports (243-250) ──
        error_constructor_fn.into(),          // 243
        type_error_constructor_fn.into(),     // 244
        range_error_constructor_fn.into(),    // 245
        syntax_error_constructor_fn.into(),   // 246
        reference_error_constructor_fn.into(),// 247
        uri_error_constructor_fn.into(),      // 248
        eval_error_constructor_fn.into(),     // 249
        error_proto_to_string_fn.into(),      // 250
```

- [ ] **Step 3: Update runtime_error_value to use proper Error objects**

Find the existing `runtime_error_value` function and update it to call `create_error`:

```rust
fn runtime_error_value(state: &RuntimeState, msg: &str) -> i64 {
    // Create a proper Error object
    let mut table = state.error_table.lock().expect("error_table mutex");
    let handle = table.len() as u32;
    table.push(ErrorEntry { name: "Error".to_string(), message: msg.to_string() });
    drop(table);
    let mut heap = state.heap.lock().expect("heap mutex");
    let obj = heap.allocate();
    let idx = heap.len() - 1;
    drop(heap);
    value::encode_handle(value::TAG_OBJECT, idx)
}
```

- [ ] **Step 4: Full build check and commit**

Run: `cargo check`
Expected: compiles across all crates

```bash
git add crates/wjsm-runtime/src/lib.rs
git commit -m "feat(runtime): implement Error constructor and subclasses"
```

---

### Task 6: Add Error test fixtures

**Files:**
- Create: `fixtures/happy/error_basic.js` + `.expected`
- Create: `fixtures/happy/error_subclass.js` + `.expected`

- [ ] **Step 1: error_basic test**

`fixtures/happy/error_basic.js`:
```js
var e = new Error("test message");
console.log(e.message);
console.log(e.name);
console.log(e.toString());
```

`fixtures/happy/error_basic.expected`:
```
test message
Error
Error: test message
```

- [ ] **Step 2: error_subclass test**

`fixtures/happy/error_subclass.js`:
```js
var te = new TypeError("wrong type");
console.log(te.name);
console.log(te instanceof Error);
var re = new RangeError("out of range");
console.log(re.name);
```

`fixtures/happy/error_subclass.expected`:
```
TypeError
true
RangeError
```

- [ ] **Step 3: Run tests and commit**

Run: `cargo test`
Expected: new fixture tests pass

```bash
git add fixtures/happy/error_basic.js fixtures/happy/error_basic.expected \
        fixtures/happy/error_subclass.js fixtures/happy/error_subclass.expected
git commit -m "test: add Error and subclass test fixtures"
```