# this 绑定与跨函数异常传播 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix `this` binding in script mode (global object) and cross-function exception propagation in WASM.

**Architecture:** Threads a `script` flag through the pipeline, creates a real global object in the runtime, changes `Terminator::Throw` to return a `TAG_EXCEPTION` handle via a host exception table, and inserts explicit exception-check blocks in IR after every `Instruction::Call`.

**Tech Stack:** Rust, wasm-encoder, wasmtime, NaN-boxed value encoding (i64)

**Design Doc:** `docs/superpowers/specs/2026-05-16-this-exception-propagation-design.md`

---

### Task 1: IR + Backend + Runtime — Foundation

**Files:**
- Create: `crates/wjsm-ir/src/exception.rs` (new module for exception table helpers if needed; but likely just Builtin variants suffice)
- Modify: `crates/wjsm-ir/src/builtin.rs` — add `CreateGlobalObject`, `CreateException`, `ExceptionValue`; remove `GetBuiltinGlobal`
- Modify: `crates/wjsm-ir/src/lib.rs` — add `script_mode: bool` to `Program`; `EncodeException` / `ExceptionToObject` → delete or rename to match new semantics
- Modify: `crates/wjsm-ir/src/value.rs` — no changes needed (TAG_EXCEPTION already exists)
- Modify: `crates/wjsm-backend-wasm/src/compiler_core.rs` — register new imports (create_global_object, create_exception, exception_value); unregister get_builtin_global imports
- Modify: `crates/wjsm-backend-wasm/src/compiler_module.rs` — change main WASM type from Type1 `() -> ()` to TypeN `() -> i64`; handle `CompileMode::Script`
- Modify: `crates/wjsm-backend-wasm/src/compiler_control.rs` — change `Terminator::Throw` from `call throw_fn + unreachable` to `call create_exception + return` for JS functions; for main, `call throw_fn + call create_exception + return`
- Modify: `crates/wjsm-backend-wasm/src/compiler_instructions.rs` — delete `EncodeException` / `ExceptionToObject` compilation; add `CreateException` / `ExceptionValue` compilation placeholder
- Modify: `crates/wjsm-backend-wasm/src/compiler_builtins.rs` — compile `CreateGlobalObject`, `CreateException`, `ExceptionValue`; remove `GetBuiltinGlobal` compilation
- Modify: `crates/wjsm-backend-wasm/src/lib.rs` — update `GenerateImportList`, remove `get_builtin_global`
- Modify: `crates/wjsm-runtime/src/lib.rs` — change `main` call from `TypedFunc<(), ()>` to `TypedFunc<(), i64>`; check return for `TAG_EXCEPTION`
- Modify: `crates/wjsm-runtime/src/host_imports/collections_buffers.rs` — add `create_global_object_fn`; add `create_exception_fn` and `exception_value_fn`; remove `get_builtin_global_fn`
- Modify: `crates/wjsm-runtime/src/host_imports/core.rs` — `throw_fn` adjusted: now only called for main uncaught exceptions
- Modify: `crates/wjsm-runtime/src/runtime_eval.rs` — `call_eval_function_from_caller` already returns `TAG_EXCEPTION` handle via `value::encode_handle(value::TAG_EXCEPTION, 0)`. Change to use new exception table (`create_exception` equivalent).

- [ ] **Step 1: IR changes — Add new Builtin variants and remove GetBuiltinGlobal**

In `crates/wjsm-ir/src/builtin.rs`:
- Remove `GetBuiltinGlobal` from the `Builtin` enum
- Add `CreateGlobalObject`, `CreateException`, `ExceptionValue`
- Update `fmt::Display` impl for all three
- Update `builtin_call_signature` in `crates/wjsm-semantic/src/builtins.rs`:

```rust
Builtin::CreateGlobalObject => ("create_global_object", 0),
Builtin::CreateException => ("create_exception", 1),
Builtin::ExceptionValue => ("exception_value", 1),
```

- Remove `Builtin::GetBuiltinGlobal` entry

- [ ] **Step 2: IR changes — EncodeException / ExceptionToObject migration**

In `crates/wjsm-ir/src/lib.rs`:
- Remove `Instruction::EncodeException` and `Instruction::ExceptionToObject` variants from the Instruction enum
- Update all `matches!()` and pattern-match sites (compile_error will show them all)
- Instruct removal AFTER disabling existing uses in semantic layer (deferred to Task 3)

Actually — these are used in the existing try/catch implementation via `emit_throw_value`. We cannot remove them until the semantic layer is updated. So instead:
- Keep them but mark as deprecated in a comment
- New code uses `CreateException` / `ExceptionValue`
- Will be removed in Task 3 when semantic layer is updated

Add `script_mode: bool` to `Program`:

```rust
// in lib.rs
pub struct Program {
    pub functions: Vec<Function>,
    pub constants: Vec<Constant>,
    pub builtins_used: HashSet<Builtin>,
    pub script_mode: bool, // NEW: true if source was parsed --script
}
```

Init in new() to `false`.

- [ ] **Step 3: Backend — Register new imports**

In `crates/wjsm-backend-wasm/src/compiler_core.rs`:

After the `typedarray_proto_subarray` import (index 311), add:

```rust
// Import index 312: create_global_object: () -> i64
imports.import("env", "create_global_object", EntityType::Function(0));
// Import index 313: create_exception: (i64) -> i64
imports.import("env", "create_exception", EntityType::Function(1));
// Import index 314: exception_value: (i64) -> i64
imports.import("env", "exception_value", EntityType::Function(1));
```

Remove the `get_builtin_global` import (index 312 → renumber to its new position if needed, but cleaner: keep existing indices, append new ones at 312/313/314).

Update the import-count capacity in `execute_with_writer()` (runtime/src/lib.rs line 168):

```rust
let mut imports: Vec<Extern> = Vec::with_capacity(322); // 319 + 3 new
```

In the `builtin_func_indices` insert section (around line 1175), add:

```rust
builtin_func_indices.insert(Builtin::CreateGlobalObject, 312);
builtin_func_indices.insert(Builtin::CreateException, 313);
builtin_func_indices.insert(Builtin::ExceptionValue, 314);
```

Remove the `GetBuiltinGlobal` entry:
```rust
// REMOVED: builtin_func_indices.insert(Builtin::GetBuiltinGlobal, 312);
```

Update `crates/wjsm-backend-wasm/src/lib.rs`:

In function name listings, remove `"get_builtin_global"`, add `"create_global_object"`, `"create_exception"`, `"exception_value"`.

- [ ] **Step 4: Backend — Change main WASM signature**

In `crates/wjsm-backend-wasm/src/compiler_module.rs`:

Find the `compile_module` function around line 72-84. Change Type 1 to a new type `() -> i64`:

```rust
if function.name() == "main" {
    if self.mode == CompileMode::Eval {
        // eval entry: Type 3 = (scope_env: i64) -> i64
        self.functions.function(3);
    } else {
        // main: Type N = () -> i64
        // Use type index 16 (next available after existing types)
        self.functions.function(16);
    }
}
```

Add the new type in the `WasmModule` initialization (where other types are defined like Type 1, Type 3, Type 12):

```rust
// Type 16: () -> i64 (main return value for exception checking)
// Look for the types section in compiler_module.rs or compiler_core.rs
types.ty(WasmFunctionType::new([], [ValType::I64]));
```

Also update `compile_function` for main — the function now returns a value:

```rust
// In compile_function, for "main" with non-eval mode:
self.current_func_returns_value = false; // CURRENT
// CHANGE TO:
self.current_func_returns_value = true;  // main now returns i64
```

And in the function finalization (around `Terminator::Return` for main), add a default return value if none was set:

When lowering `Terminator::Return { value: None }` for main, emit `encode_undefined()` before `return` instruction:

```wasm
i64.const <encode_undefined()>
return
```

Check `compiler_control.rs` around line 370-445 where terminator `Return` is compiled. For `self.current_func_returns_value && value.is_none()`, add a default `undefined` value.

- [ ] **Step 5: Backend — Terminator::Throw compile as create_exception + return**

In `crates/wjsm-backend-wasm/src/compiler_control.rs`, change:

```rust
Terminator::Throw { value } => {
    self.emit_eval_var_frame_exit();
    self.emit(WasmInstruction::LocalGet(self.local_idx(value.0)));
    let func_idx = self
        .builtin_func_indices
        .get(&Builtin::Throw)
        .copied()
        .unwrap_or(3);
    self.emit(WasmInstruction::Call(func_idx));
    self.emit(WasmInstruction::Unreachable);
    idx += 1;
}
```

To:

```rust
Terminator::Throw { value } => {
    self.emit_eval_var_frame_exit();
    // Save thrown value
    self.emit(WasmInstruction::LocalGet(self.local_idx(value.0)));
    if function.name() == "main" {
        // main: report error via throw_fn, then also create exception handle for return
        let throw_idx = self.builtin_func_indices.get(&Builtin::Throw).copied().unwrap_or(3);
        self.emit(WasmInstruction::Call(throw_idx));
        self.emit(WasmInstruction::LocalGet(self.local_idx(value.0)));
    }
    // All functions (including main): create exception handle and return it
    let create_exc_idx = self.builtin_func_indices.get(&Builtin::CreateException).copied().unwrap();
    self.emit(WasmInstruction::Call(create_exc_idx));
    // Return the TAG_EXCEPTION handle
    self.emit(WasmInstruction::Return);
    idx += 1;
}
```

Note: need access to `function.name()` here. Check if `self` already has `current_func_name` or similar.

- [ ] **Step 6: Backend — Compile CreateGlobalObject / CreateException / ExceptionValue**

In `crates/wjsm-backend-wasm/src/compiler_builtins.rs`:

For `Builtin::CreateGlobalObject`:
```rust
Builtin::CreateGlobalObject => {
    let func_idx = self.builtin_func_indices.get(&Builtin::CreateGlobalObject).copied().unwrap();
    self.emit(WasmInstruction::Call(func_idx));
    // result on stack — caller will local.set it
}
```

For `Builtin::CreateException`:
```rust
Builtin::CreateException => {
    // value already on stack from previous LocalGet
    let func_idx = self.builtin_func_indices.get(&Builtin::CreateException).copied().unwrap();
    self.emit(WasmInstruction::Call(func_idx));
}
```

For `Builtin::ExceptionValue`:
```rust
Builtin::ExceptionValue => {
    let func_idx = self.builtin_func_indices.get(&Builtin::ExceptionValue).copied().unwrap();
    self.emit(WasmInstruction::Call(func_idx));
}
```

Remove `Builtin::GetBuiltinGlobal` compilation.

- [ ] **Step 7: Runtime — add create_global_object_fn**

In `crates/wjsm-runtime/src/host_imports/collections_buffers.rs`, add a new function after the existing imports section (before the final `vec![]`):

```rust
let create_global_object_fn = Func::wrap(
    &mut store,
    |mut caller: Caller<'_, RuntimeState>| -> i64 {
        // 1. Allocate empty host object with enough capacity
        let obj = alloc_host_object_from_caller(&mut caller, 60);
        let mut native_callables = caller.data().native_callables.lock().unwrap();
        
        // 2. Populate with builtins (same NativeCallables as get_builtin_global created)
        let builtins: &[(&str, NativeCallable)] = &[
            ("Array", NativeCallable::ArrayConstructor),
            ("Object", NativeCallable::ObjectConstructor),
            ("Function", NativeCallable::FunctionConstructor),
            ("String", NativeCallable::StringConstructor),
            ("Boolean", NativeCallable::BooleanConstructor),
            ("Number", NativeCallable::NumberConstructor),
            ("Symbol", NativeCallable::SymbolConstructor),
            ("BigInt", NativeCallable::BigIntConstructor),
            ("RegExp", NativeCallable::RegExpConstructor),
            ("Error", NativeCallable::ErrorConstructor),
            ("TypeError", NativeCallable::TypeErrorConstructor),
            ("RangeError", NativeCallable::RangeErrorConstructor),
            ("SyntaxError", NativeCallable::SyntaxErrorConstructor),
            ("ReferenceError", NativeCallable::ReferenceErrorConstructor),
            ("URIError", NativeCallable::URIErrorConstructor),
            ("EvalError", NativeCallable::EvalErrorConstructor),
            ("AggregateError", NativeCallable::AggregateErrorConstructor),
            ("Map", NativeCallable::MapConstructor),
            ("Set", NativeCallable::SetConstructor),
            ("WeakMap", NativeCallable::WeakMapConstructor),
            ("WeakSet", NativeCallable::WeakSetConstructor),
            ("Date", NativeCallable::DateConstructorGlobal),
            ("Promise", NativeCallable::PromiseConstructor),
            ("ArrayBuffer", NativeCallable::ArrayBufferConstructorGlobal),
            ("DataView", NativeCallable::DataViewConstructorGlobal),
            ("Proxy", NativeCallable::ProxyConstructor),
            // Stub objects: create empty objects, don't populate properties here
            // These are resolved lazily via GetProp if user accesses Math.PI etc.
            // at runtime (non-optimized path).
        ];
        
        for (name, callable) in builtins {
            let idx = native_callables.len() as u32;
            native_callables.push(*callable);
            let val = value::encode_native_callable_idx(idx);
            let _ = define_host_data_property_from_caller(&mut caller, obj, name, val);
        }
        
        // 3. Set globalThis = self
        let _ = define_host_data_property_from_caller(&mut caller, obj, "globalThis", obj);
        
        // 4. Return the global object handle
        obj
    },
);
```

Add it to the import vector:

```rust
create_global_object_fn.into(),  // 312
```

- [ ] **Step 8: Runtime — add create_exception_fn and exception_value_fn**

In the same file, add:

```rust
let create_exception_fn = Func::wrap(
    &mut store,
    |mut caller: Caller<'_, RuntimeState>, thrown_value: i64| -> i64 {
        // Store thrown value in error_table
        let mut errors = caller.data().error_table.lock().unwrap();
        let idx = errors.len() as u32;
        errors.push(ErrorEntry { value: thrown_value });
        value::encode_handle(value::TAG_EXCEPTION, idx)
    },
);

let exception_value_fn = Func::wrap(
    &mut store,
    |mut caller: Caller<'_, RuntimeState>, exception_handle: i64| -> i64 {
        let idx = value::decode_handle(exception_handle) as usize;
        let errors = caller.data().error_table.lock().unwrap();
        errors.get(idx).map(|e| e.value).unwrap_or(value::encode_undefined())
    },
);
```

Check what `ErrorEntry` looks like in the runtime. If it doesn't have a `value` field, add it:

```rust
// in crates/wjsm-runtime/src/lib.rs or wherever ErrorEntry is defined
struct ErrorEntry {
    value: i64,  // the original thrown JS value
    // ... existing fields like stack, message etc.
}
```

Add both to the import vector:

```rust
create_exception_fn.into(),    // 313
exception_value_fn.into(),     // 314
```

Update the `error_table` initialization if `ErrorEntry` gained a new field.

- [ ] **Step 9: Runtime — change main call to TypedFunc<(), i64>**

In `crates/wjsm-runtime/src/lib.rs`:

```rust
// CURRENT (line ~181):
let main = instance.get_typed_func::<(), ()>(&mut store, "main")?;
let main_result = main.call(&mut store, ());

// CHANGE TO:
let main = instance.get_typed_func::<(), i64>(&mut store, "main")?;
let main_result = main.call(&mut store, ());
match main_result {
    Ok(return_val) => {
        if value::is_exception(return_val) {
            // Check runtime_error for error message
            if let Some(runtime_state) = store.data_opt() {
                let err = runtime_state.runtime_error.lock().unwrap().take();
                if let Some(msg) = err {
                    // Write "Uncaught exception: {msg}" to output
                    // (the throw_fn already did this, but double-check
                    // to handle cases where throw_fn didn't fire)
                }
            }
            // Skip microtasks and timer loop — exception escaped
        } else {
            // Normal completion: proceed with microtasks + timers
            // Existing logic unchanged
            if let Some(Extern::Table(func_table)) = instance.get_export(&mut store, "__table") {
                // ... existing microtask drain + timer loop ...
            }
        }
    }
    Err(trap) => {
        // Real WASM trap — use existing error handling
        return Err(anyhow::anyhow!("WASM trap: {:?}", trap));
    }
}
```

- [ ] **Step 10: Runtime — adjust throw_fn**

In `crates/wjsm-runtime/src/host_imports/core.rs`, `throw_fn` currently renders the value and sets `runtime_error`. After this change, it's only called from `main` for uncaught exceptions. Keep the existing behavior (it handles the rendering output).

But also need to handle the case where `Terminator::Throw` no longer calls `throw_fn` for non-main functions. The `create_exception_fn` in Task 1 Step 8 stores the value. The actual error rendering for uncaught exceptions happens in the runtime's main return check (Step 9) or in `throw_fn` (called from main's `Terminator::Throw`).

- [ ] **Step 11: Runtime — eval helper changes**

In `crates/wjsm-runtime/src/runtime_eval.rs`, `call_eval_function_from_caller` currently returns `value::encode_handle(value::TAG_EXCEPTION, 0)`. It should use the new exception table instead. But since `lower_direct_eval_call` inlines eval code into the caller's function, exceptions from eval'd code are handled by the caller's exception blocks (post-call check). The `perform_eval_from_caller` (indirect eval) already returns `TAG_EXCEPTION` handles.

For now, keep the existing eval TAG_EXCEPTION encoding (uses handle index 0 hardcoded). This will be unified in Task 3 when we wire up the full exception checking.

- [ ] **Step 12: Build and run existing tests**

```bash
cargo test -p wjsm-ir --test ir_dump
cargo test -p wjsm-semantic --test lowering_snapshots
cargo test --test fixture_runner
```

Expected: IR dump tests pass (new Builtin variants appear in dumps), semantic snapshots pass (no IR shape changes yet), fixture runner passes (main now returns i64, runtime handles it).

If IR dump tests fail due to new/removed Builtin entries, update the expected dump output.

If fixture runner tests fail, debug and fix.

- [ ] **Step 13: Commit**

```bash
git add -A
git commit -m "feat: foundation for this-binding and exception propagation

- Add Builtin::CreateGlobalObject, CreateException, ExceptionValue
- Remove Builtin::GetBuiltinGlobal
- Change main WASM signature ()->() to ()->i64
- Change Terminator::Throw to create_exception + return
- Add runtime host functions: create_global_object, create_exception, exception_value
- Add script_mode to IR Program
"
```

---

### Task 2: Semantic — this-binding, global object, builtins migration

**Files:**
- Modify: `crates/wjsm-semantic/src/lib.rs` — `lower_module()` signature change
- Modify: `crates/wjsm-semantic/src/lowerer_core.rs` — add `script_mode`, emit CreateGlobalObject + StoreVar in entry block
- Modify: `crates/wjsm-semantic/src/lowerer_assignments.rs` — change builtin reference resolution
- Modify: `crates/wjsm-semantic/src/lowerer_async_eval.rs` — add `lower_load_global()`; handle TLA/eval $global
- Modify: `crates/wjsm-semantic/src/builtins.rs` — remove GetBuiltinGlobal mapping, keep is_builtin_global
- Modify: `crates/wjsm-cli/src/lib.rs` — pass `script` flag to `lower_module()`, `compile()`
- Modify: `crates/wjsm-module/src/lib.rs` — pass `script` flag through if needed

- [ ] **Step 1: Lowerer — add script_mode field**

In `crates/wjsm-semantic/src/lowerer_core.rs`, add to `Lowerer` struct:

```rust
pub(crate) script_mode: bool,
```

Initialize in constructor (where Lowerer is built — look for `new()` or constructor function). Default to `false`.

- [ ] **Step 2: lower_module — add script param + CreateGlobalObject + $this/$global**

In `crates/wjsm-semantic/src/lib.rs`, change `lower_module`:

```rust
pub fn lower_module(module: swc_ast::Module, script: bool) -> Result<Program, LoweringError> {
    // ...
    lowerer.script_mode = script;
    lowerer.lower_module_inner(module)
}
```

In `crates/wjsm-semantic/src/lowerer_core.rs`, in the `lower_module` method, after the existing entry block initializations (undefined, NaN, Infinity, Math/Number constants), add:

```rust
// Create global object (for both script and module modes)
let global_obj = self.alloc_value();
self.current_function.append_instruction(
    entry,
    Instruction::CallBuiltin {
        dest: Some(global_obj),
        builtin: Builtin::CreateGlobalObject,
        args: vec![],
    },
);
// Store as $0.$global
self.current_function.append_instruction(
    entry,
    Instruction::StoreVar {
        name: "$0.$global".to_string(),
        value: global_obj,
    },
);

// Set $this based on mode
let this_val = if self.script_mode {
    global_obj
} else {
    let undef_const = self.module.add_constant(Constant::Undefined);
    let v = self.alloc_value();
    self.current_function.append_instruction(
        entry,
        Instruction::Const { dest: v, constant: undef_const },
    );
    v
};
// Overwrite or add StoreVar for $this
self.current_function.append_instruction(
    entry,
    Instruction::StoreVar {
        name: "$0.$this".to_string(),
        value: this_val,
    },
);
```

Also ensure main returns a value. Find where main finalization happens (look for `Terminator::Return { value: None }` for main) and add a default `undefined` return value.

- [ ] **Step 3: lower_ident — change builtin resolution**

In `crates/wjsm-semantic/src/lowerer_assignments.rs`, change the builtin resolution branch:

```rust
// CURRENT (line ~50-73):
Err(msg) if msg.starts_with("undeclared identifier") && is_builtin_global(&name) => {
    let name_const = self.module.add_constant(Constant::String(name));
    let name_val = self.alloc_value();
    self.current_function.append_instruction(block,
        Instruction::Const { dest: name_val, constant: name_const },
    );
    let dest = self.alloc_value();
    self.current_function.append_instruction(block,
        Instruction::CallBuiltin {
            dest: Some(dest),
            builtin: Builtin::GetBuiltinGlobal,
            args: vec![name_val],
        },
    );
    return Ok(dest);
}

// CHANGE TO:
Err(msg) if msg.starts_with("undeclared identifier") && is_builtin_global(&name) => {
    let global_obj = self.lower_load_global(block)?;
    let key_const = self.module.add_constant(Constant::String(name));
    let key_val = self.alloc_value();
    self.current_function.append_instruction(
        block,
        Instruction::Const { dest: key_val, constant: key_const },
    );
    let dest = self.alloc_value();
    self.current_function.append_instruction(
        block,
        Instruction::GetProp {
            dest,
            object: global_obj,
            key: key_val,
        },
    );
    return Ok(dest);
}
```

- [ ] **Step 4: Add lower_load_global method**

In `crates/wjsm-semantic/src/lowerer_async_eval.rs`:

```rust
pub(crate) fn lower_load_global(&mut self, block: BasicBlockId) -> Result<ValueId, LoweringError> {
    let dest = self.alloc_value();
    self.current_function.append_instruction(
        block,
        Instruction::LoadVar {
            dest,
            name: "$0.$global".to_string(),
        },
    );
    Ok(dest)
}
```

- [ ] **Step 5: Handle TLA mode — $global/$this in async main context**

In `crates/wjsm-semantic/src/lowerer_async_eval.rs`, `init_async_main_context()` already declares `$this`:

```rust
let this_scope_id = self.scopes.declare("$this", VarKind::Let, true)?;
```

Add similar declaration for `$global`:

```rust
let global_scope_id = self.scopes.declare("$global", VarKind::Let, true)?;
```

In the async main param list, add `$global` as a parameter:

```rust
let param_ir_names = vec![
    format!("${env_scope_id}.$env"),
    format!("${this_scope_id}.$this"),
    format!("${global_scope_id}.$global"),  // NEW
];
```

And in the entry block, restore `$global` from the continuation:

```rust
// Restore $global from continuation
let global_val = self.alloc_value();
// ... (similar to how $env and $this are restored)
```

- [ ] **Step 6: CLI — thread script flag**

In `crates/wjsm-cli/src/lib.rs`:

`compile_source()` — add `script: bool` parameter:

```rust
fn compile_source(source: &str, ... , script: bool) -> Result<Vec<u8>> {
```

Pass `script` to `lower_module`:
```rust
let program = wjsm_semantic::lower_module(module, script)?;
```

Don't need to pass to `compile()` — read from `program.script_mode`.

Update callers of `compile_source` — both `cmd_run` and `cmd_build` paths.

If `run_pipeline` exists, thread the flag through it as well.

- [ ] **Step 7: Build and run tests**

```bash
cargo test -p wjsm-ir --test ir_dump
cargo test -p wjsm-semantic --test lowering_snapshots
cargo test --test fixture_runner
```

Expected: fixture runner passes. Some semantic snapshots may change (new `$global`/`$this` variables in IR dump). Update snapshots with `WJSM_UPDATE_FIXTURES=1` if needed for semantic tests.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat: thread script flag, create global object, fix this-binding

- Add script_mode to Lowerer and thread through CLI
- lower_module emits CreateGlobalObject + StoreVar $global/$this
- builtin resolution changed from GetBuiltinGlobal to GetProp($global, name)
- Add lower_load_global() helper
- Handle TLA mode $global/$this
"
```

---

### Task 3: Semantic — call exception checking blocks

**Files:**
- Modify: `crates/wjsm-semantic/src/lowerer_calls_eval.rs` — after each `Instruction::Call`, insert `IsException` + `Branch` + `exception_value` + `emit_throw_value`
- Modify: `crates/wjsm-semantic/src/lowerer_branching.rs` — minor adjustment if needed for `emit_throw_value`
- Modify: `crates/wjsm-backend-wasm/src/compiler_builtins.rs` — compile `IsException` (already exists), `ExceptionValue` (new)
- Modify: `crates/wjsm-backend-wasm/src/compiler_instructions.rs` — remove old `EncodeException` / `ExceptionToObject` compilation

- [ ] **Step 1: ExprResult and lower_call_expr change**

In `crates/wjsm-semantic/src/lowerer_calls_eval.rs`, define a helper:

```rust
struct ExprResult {
    value: ValueId,
    block: BasicBlockId,
}
```

Change the core `lower_call_expr` return type from `Result<ValueId>` to `Result<ExprResult>`:

```rust
pub(crate) fn lower_call_expr(
    &mut self,
    call: &swc_ast::CallExpr,
    block: BasicBlockId,
) -> Result<ExprResult, LoweringError> {
```

At the end, where `Instruction::Call` is emitted (around line 390), replace:

```rust
let dest = self.alloc_value();
self.current_function.append_instruction(
    block,
    Instruction::Call {
        dest: Some(dest),
        callee: callee_val,
        this_val,
        args,
    },
);
Ok(dest)
```

With:

```rust
let dest = self.alloc_value();
self.current_function.append_instruction(
    block,
    Instruction::Call {
        dest: Some(dest),
        callee: callee_val,
        this_val,
        args,
    },
);

// Insert exception check block
let continue_block = self.current_function.new_block();
let exc_block = self.current_function.new_block();

let is_exc = self.alloc_value();
self.current_function.append_instruction(
    block,
    Instruction::IsException { dest: is_exc, value: dest },
);
self.current_function.set_terminator(
    block,
    Terminator::Branch {
        condition: is_exc,
        true_block: exc_block,
        false_block: continue_block,
    },
);

// Exception path: unwrap the thrown value and emit_throw_value
let thrown_val = self.alloc_value();
self.current_function.append_instruction(
    exc_block,
    Instruction::CallBuiltin {
        dest: Some(thrown_val),
        builtin: Builtin::ExceptionValue,
        args: vec![dest],
    },
);
self.emit_throw_value(exc_block, thrown_val)?;

// Return with continue_block as the "active" block for call results
Ok(ExprResult { value: dest, block: continue_block })
```

- [ ] **Step 2: Update callers of lower_call_expr**

Search for all call sites of `lower_call_expr`. They currently use `Ok(dest)` pattern matching. Change to handle `ExprResult`:

```rust
// CURRENT:
let result = self.lower_call_expr(call, block)?;
// ... use result as ValueId ...

// CHANGE TO:
let ExprResult { value: result, block: continue_block } = self.lower_call_expr(call, block)?;
// ... use result as ValueId, use continue_block as new active block ...
```

Key callers that need updating:
- Inside `lower_call_expr` itself (recursive calls for callee lowering)
- `lower_new_expr` in the same file
- `lower_await` calls
- `lower_member_call_like` or similar helper exprs
- Possibly `lower_expr` dispatch in `lowerer_jsx_objects.rs`

For each caller: all instructions AFTER the call must use the returned `continue_block` instead of the original `block`.

For callers that currently use the call result in a context where they need to continue on the same block (e.g., binary operation with a call as one operand), the `block` from `ExprResult` replaces the old block parameter.

- [ ] **Step 3: Handle CallBuiltin that can return TAG_EXCEPTION**

Walk through all `Builtin` variants that are called via `CallBuiltin` and identify which ones can return `TAG_EXCEPTION`:

Key ones that already return `TAG_EXCEPTION`:
- `Builtin::Eval` — direct eval, which can compile/evaluate code that throws
- `Builtin::EvalIndirect` — indirect eval, same
- `Builtin::EvalResult` — result retrieval after eval

For these, wrap the `CallBuiltin` in the same `IsException` + `Branch` pattern.

For `Builtin::Throw` — this is called from `Terminator::Throw` path, not from user code directly. No change needed.

For other builtins (`Builtin::ObjectConstructor`, `Builtin::ArrayConstructor`, etc.) — these are host functions that handle errors internally (set runtime_error), they do NOT return `TAG_EXCEPTION`. No change needed.

Find where `Builtin::Eval` is called (in `lower_direct_eval_call`) and wrap it:

```rust
// After CallBuiltin(Eval), add exception check
let is_exc = self.alloc_value();
self.current_function.append_instruction(
    current_block,
    Instruction::IsException { dest: is_exc, value: eval_result },
);
let continue_block = self.current_function.new_block();
let exc_block = self.current_function.new_block();
self.current_function.set_terminator(
    current_block,
    Terminator::Branch {
        condition: is_exc,
        true_block: exc_block,
        false_block: continue_block,
    },
);
// exc_block: emit_throw_value with the TAG_EXCEPTION handle
let thrown_val = self.alloc_value();
self.current_function.append_instruction(
    exc_block,
    Instruction::CallBuiltin {
        dest: Some(thrown_val),
        builtin: Builtin::ExceptionValue,
        args: vec![eval_result],
    },
);
self.emit_throw_value(exc_block, thrown_val)?;
// continue_block: use eval_result normally
```

- [ ] **Step 4: Backend — remove old EncodeException / ExceptionToObject compilation**

In `crates/wjsm-backend-wasm/src/compiler_instructions.rs`, remove the `EncodeException` and `ExceptionToObject` cases from `compile_instruction`. Replace them with error messages or remove entirely.

The `IsException` compilation stays — it's used by the new exception checking blocks.

- [ ] **Step 5: Build and run tests**

```bash
cargo test --test fixture_runner
```

If semantic IR snapshots fail, update them.

Add a new test fixture that tests cross-function exception:
`fixtures/happy/exception_cross_function.js`:
```javascript
function throws() { throw "cross"; }
try {
    throws();
    console.log("SHOULD_NOT_REACH");
} catch (e) {
    console.log("caught:", e);
}
```

`fixtures/happy/exception_cross_function.expected`:
```
exit_code: 0
--- stdout ---
caught: cross
--- stderr ---
```

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: add cross-function exception propagation

- lower_call_expr emits IsException + Branch after every Call
- Exception path calls exception_value + emit_throw_value
- Wrap CallBuiltin(Eval) in same exception checking
- Remove old EncodeException/ExceptionToObject from backend
- Add exception_cross_function test fixture
"
```

---

### Task 4: Runtime — error table wiring and test262

**Files:**
- Modify: `crates/wjsm-runtime/src/runtime_eval.rs` — unify TAG_EXCEPTION handling with new exception table
- Modify: `crates/wjsm-runtime/src/lib.rs` — ErrorEntry struct if needed
- Modify: `crates/wjsm-runtime/src/host_imports/core.rs` — remove get_builtin_global reference if still present

- [ ] **Step 1: Unify eval TAG_EXCEPTION with exception table**

In `crates/wjsm-runtime/src/runtime_eval.rs`, change hardcoded `TAG_EXCEPTION` handle (index 0) to use the exception table:

```rust
// CURRENT: return value::encode_handle(value::TAG_EXCEPTION, 0);
// CHANGE TO: use create_exception equivalent
let mut errors = caller.data().error_table.lock().unwrap();
let idx = errors.len() as u32;
errors.push(ErrorEntry { value: /* thrown_value */ });
value::encode_handle(value::TAG_EXCEPTION, idx)
```

But `perform_eval_from_caller` returns the result directly to the WASM caller (import `eval.indirect`). The result is already a `TAG_EXCEPTION` handle that the post-call `IsException` check can detect. Need to ensure `ErrorEntry`'s `value` field is populated.

In `perform_eval_from_caller` (around line 246-270), where errors return `TAG_EXCEPTION`:

```rust
// Change from:
set_runtime_error(caller.data(), msg.to_string());
return value::encode_handle(value::TAG_EXCEPTION, 0);

// To:
set_runtime_error(caller.data(), msg.to_string());
let mut errors = caller.data().error_table.lock().unwrap();
let idx = errors.len() as u32;
errors.push(ErrorEntry { value: value::encode_undefined() }); // or the actual error value
return value::encode_handle(value::TAG_EXCEPTION, idx);
```

For `call_eval_function_from_caller` (line 766), similar change.

- [ ] **Step 2: ErrorEntry struct update**

In `crates/wjsm-runtime/src/lib.rs`, find `ErrorEntry` definition and add the `value` field if missing:

```rust
#[derive(Clone, Debug)]
struct ErrorEntry {
    value: i64,  // the original thrown JS value
    // ... any existing fields ...
}
```

- [ ] **Step 3: Cleanup — remove remaining get_builtin_global references**

Search for any remaining references to `GetBuiltinGlobal` or `get_builtin_global` across all crates and remove them.

Specifically check:
- `crates/wjsm-runtime/src/runtime_render.rs` — verify `is_exception` rendering still works with new exception handles

- [ ] **Step 4: Run test262 eval-direct tests**

```bash
cargo run -p wjsm-test262 2>&1 | grep "eval-direct"
```

Expected failure count decreases significantly (the this-binding fix alone should make many setup-phase failures pass).

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: unify exception handling, remove get_builtin_global

- Unify eval TAG_EXCEPTION with new exception table
- Add ErrorEntry.value field for preserving thrown values
- Remove remaining get_builtin_global references
"
```
