# Eval ScopeRecord Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace eval flat-object scope bridge with a ScopeRecord host object that enforces TDZ, const immutability, super/new.target propagation, and arguments conflict detection. Raise direct eval test262 pass rate from ~10% to ~67%.

**Architecture:** Seven new IR Builtin variants create/manipulate a `ScopeRecord` host object. Semantic layer rewrites `lower_direct_eval_call` to snapshot all bindings (including TDZ) into the record. In eval modules, identifier access routes through `EvalGetBinding`/`EvalSetBinding` instead of `GetProp`/`SetProp`. Runtime implements the ScopeRecord with RAII lifetime.

**Tech Stack:** Rust 2024, swc_core, wasm-encoder, wasmtime

---

## File Map

| File | Role |
|---|---|
| `crates/wjsm-ir/src/builtin.rs` | 7 new `Builtin` variants |
| `crates/wjsm-ir/src/lib.rs` | `Display` impls for new variants |
| `crates/wjsm-ir/src/value.rs` | `TAG_SCOPE_RECORD` + encode/decode helpers |
| `crates/wjsm-backend-wasm/src/lib.rs` | Extend `HOST_IMPORT_NAMES` to 355 |
| `crates/wjsm-backend-wasm/src/compiler_core.rs` | Register `builtin_func_indices` 348-354 |
| `crates/wjsm-backend-wasm/src/compiler_builtins.rs` | WASM instruction emission for new builtins |
| `crates/wjsm-runtime/src/runtime_eval.rs` | ScopeRecord struct + 7 host fns + arguments detection |
| `crates/wjsm-runtime/src/lib.rs` | Register 7 host imports; TAG_SCOPE_RECORD in type dispatch |
| `crates/wjsm-semantic/src/lowerer_predeclare.rs` | Fix class predeclare TDZ |
| `crates/wjsm-semantic/src/lib.rs` | `visible_bindings_all()` on ScopeTree; Lowerer fields |
| `crates/wjsm-semantic/src/lowerer_calls_eval.rs` | Rewrite `lower_direct_eval_call` |
| `crates/wjsm-semantic/src/lowerer_assignments.rs` | Eval mode routes to EvalGet/EvalSet |
| `crates/wjsm-semantic/src/lowerer_async_eval.rs` | `lower_super_prop` eval path; `new.target` eval path |
| `crates/wjsm-semantic/src/lowerer_core.rs` | Set `eval_caller_has_arguments` |
| `crates/wjsm-semantic/src/eval_scan.rs` | Bump `eval_literal_binding_names` visibility to `pub` |

---

### Task 1: IR — 7 new Builtin variants + TAG_SCOPE_RECORD

**Files:**
- Modify: `crates/wjsm-ir/src/builtin.rs`
- Modify: `crates/wjsm-ir/src/lib.rs`
- Modify: `crates/wjsm-ir/src/value.rs`

- [ ] **Step 1: Add 7 Builtin variants to the enum**

In `crates/wjsm-ir/src/builtin.rs`, add the following variants before the closing `}` of `pub enum Builtin` (after `CreateMappedArgumentsObject`):

```rust
    // ── ScopeRecord eval bridge ───────────────────────────────────────
    /// dest: i64 — scope record handle
    ScopeRecordCreate,
    /// args[0]: record, args[1]: name (string), args[2]: value (i64), args[3]: is_tdz (bool), args[4]: is_const (bool)
    ScopeRecordAddBinding,
    /// dest: i64 — value (or TAG_EXCEPTION if TDZ)
    EvalGetBinding,
    /// dest: i64 — written value
    EvalSetBinding,
    /// dest: i64 — bool (0 or 1)
    EvalHasBinding,
    /// dest: i64 — prototype | undefined | TAG_EXCEPTION
    EvalSuperBase,
    /// args[0]: record, args[1]: key (i64 integer tag), args[2]: value (i64)
    ScopeRecordSetMeta,
```

- [ ] **Step 2: Add Display impl strings**

In the `Display` impl `match self` block (near the end of `builtin.rs`), add:

```rust
            Self::ScopeRecordCreate => "scope_record_create",
            Self::ScopeRecordAddBinding => "scope_record_add_binding",
            Self::EvalGetBinding => "eval_get_binding",
            Self::EvalSetBinding => "eval_set_binding",
            Self::EvalHasBinding => "eval_has_binding",
            Self::EvalSuperBase => "eval_super_base",
            Self::ScopeRecordSetMeta => "scope_record_set_meta",
```

- [ ] **Step 3: Add TAG_SCOPE_RECORD and encode/decode functions to value.rs**

In `crates/wjsm-ir/src/value.rs`, after the existing TAG_PROXY block (around line 24-36):

```rust
// ── ScopeRecord ───────────────────────────────────────────────────────
pub const TAG_SCOPE_RECORD: u64 = 0x11;

pub fn encode_scope_record_handle(handle: u32) -> i64 {
    encode_handle(TAG_SCOPE_RECORD, handle)
}

pub fn is_scope_record(val: i64) -> bool {
    let uval = val as u64;
    (uval & BOX_BASE) == BOX_BASE && ((uval >> 32) & TAG_MASK) == TAG_SCOPE_RECORD
}

pub fn decode_scope_record_handle(val: i64) -> u32 {
    (val as u64 & 0xFFFF_FFFF) as u32
}
```

- [ ] **Step 4: Add IR dump format to lib.rs**

In `crates/wjsm-ir/src/lib.rs`, find the `Display` impl for `Instruction` (the `CallBuiltin` match arm). Add format strings for the new variants alongside existing eval ones:

```rust
            Builtin::ScopeRecordCreate => {
                write!(f, "call builtin.scope_record_create(%{})", dest_name(dest))?;
            }
            Builtin::ScopeRecordAddBinding => {
                let args_fmt: Vec<String> = args.iter().map(|a| format!("%{}", a.0)).collect();
                write!(f, "call builtin.scope_record_add_binding({})", args_fmt.join(", "))?;
            }
            Builtin::EvalGetBinding => {
                write!(f, "%{} = call builtin.eval_get_binding(%{}, %{})", dest_name(dest), args[0].0, args[1].0)?;
            }
            Builtin::EvalSetBinding => {
                write!(f, "%{} = call builtin.eval_set_binding(%{}, %{}, %{})", dest_name(dest), args[0].0, args[1].0, args[2].0)?;
            }
            Builtin::EvalHasBinding => {
                write!(f, "%{} = call builtin.eval_has_binding(%{}, %{})", dest_name(dest), args[0].0, args[1].0)?;
            }
            Builtin::EvalSuperBase => {
                write!(f, "%{} = call builtin.eval_super_base(%{})", dest_name(dest), args[0].0)?;
            }
            Builtin::ScopeRecordSetMeta => {
                write!(f, "call builtin.scope_record_set_meta(%{}, %{}, %{})", args[0].0, args[1].0, args[2].0)?;
            }
```

- [ ] **Step 5: Build check**

```bash
cargo check -p wjsm-ir
```
Expected: no errors.

- [ ] **Step 6: Commit**

```bash
git add crates/wjsm-ir/src/builtin.rs crates/wjsm-ir/src/lib.rs crates/wjsm-ir/src/value.rs
git commit -m "feat(ir): add 7 ScopeRecord Builtin variants and TAG_SCOPE_RECORD"
```

---

### Task 2: Backend — WASM imports + codegen

**Files:**
- Modify: `crates/wjsm-backend-wasm/src/lib.rs`
- Modify: `crates/wjsm-backend-wasm/src/compiler_core.rs`
- Modify: `crates/wjsm-backend-wasm/src/compiler_builtins.rs`

- [ ] **Step 1: Extend HOST_IMPORT_NAMES array**

In `crates/wjsm-backend-wasm/src/lib.rs`, change the array size from `348` to `355` and append after `"typedarray_proto_values"` (line ~378):

```rust
const HOST_IMPORT_NAMES: [&str; 355] = [
    // ... existing entries ...
    "typedarray_proto_values",
    // ── ScopeRecord eval bridge ──
    "scope_record_create",
    "scope_record_add_binding",
    "eval_get_binding",
    "eval_set_binding",
    "eval_has_binding",
    "eval_super_base",
    "scope_record_set_meta",
];
```

- [ ] **Step 2: Register builtin_func_indices in compiler_core.rs**

In `crates/wjsm-backend-wasm/src/compiler_core.rs`, find the `builtin_func_indices.insert` block (around line 1267, after `CreateMappedArgumentsObject`). Add:

```rust
        // ── ScopeRecord eval bridge ──
        builtin_func_indices.insert(Builtin::ScopeRecordCreate, 348);
        builtin_func_indices.insert(Builtin::ScopeRecordAddBinding, 349);
        builtin_func_indices.insert(Builtin::EvalGetBinding, 350);
        builtin_func_indices.insert(Builtin::EvalSetBinding, 351);
        builtin_func_indices.insert(Builtin::EvalHasBinding, 352);
        builtin_func_indices.insert(Builtin::EvalSuperBase, 353);
        builtin_func_indices.insert(Builtin::ScopeRecordSetMeta, 354);
```

- [ ] **Step 3: Add WASM instruction emission in compiler_builtins.rs**

In `crates/wjsm-backend-wasm/src/compiler_builtins.rs`, after the `Builtin::EvalIndirect` arm (around line 178), add arms for all 7 new builtins following the same pattern as `Builtin::Eval`:

```rust
            Builtin::ScopeRecordCreate => {
                let capacity = args.first().context("scope_record_create expects capacity")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(capacity.0)));
                let func_idx = self.builtin_func_indices.get(builtin).copied()
                    .with_context(|| format!("no WASM func index for {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                } else {
                    self.emit(WasmInstruction::Drop);
                }
                Ok(())
            }
            Builtin::ScopeRecordAddBinding => {
                let rec = args.first().context("scope_record_add_binding expects record")?;
                let name = args.get(1).context("scope_record_add_binding expects name")?;
                let val = args.get(2).context("scope_record_add_binding expects value")?;
                let is_tdz = args.get(3).context("scope_record_add_binding expects is_tdz")?;
                let is_const = args.get(4).context("scope_record_add_binding expects is_const")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(rec.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(name.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(val.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(is_tdz.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(is_const.0)));
                let func_idx = self.builtin_func_indices.get(builtin).copied()
                    .with_context(|| format!("no WASM func index for {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                self.emit(WasmInstruction::Drop);
                Ok(())
            }
            Builtin::EvalGetBinding => {
                let rec = args.first().context("eval_get_binding expects record")?;
                let name = args.get(1).context("eval_get_binding expects name")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(rec.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(name.0)));
                let func_idx = self.builtin_func_indices.get(builtin).copied()
                    .with_context(|| format!("no WASM func index for {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                } else {
                    self.emit(WasmInstruction::Drop);
                }
                Ok(())
            }
            Builtin::EvalSetBinding => {
                let rec = args.first().context("eval_set_binding expects record")?;
                let name = args.get(1).context("eval_set_binding expects name")?;
                let val = args.get(2).context("eval_set_binding expects value")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(rec.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(name.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(val.0)));
                let func_idx = self.builtin_func_indices.get(builtin).copied()
                    .with_context(|| format!("no WASM func index for {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                } else {
                    self.emit(WasmInstruction::Drop);
                }
                Ok(())
            }
            Builtin::EvalHasBinding => {
                let rec = args.first().context("eval_has_binding expects record")?;
                let name = args.get(1).context("eval_has_binding expects name")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(rec.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(name.0)));
                let func_idx = self.builtin_func_indices.get(builtin).copied()
                    .with_context(|| format!("no WASM func index for {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                } else {
                    self.emit(WasmInstruction::Drop);
                }
                Ok(())
            }
            Builtin::EvalSuperBase => {
                let rec = args.first().context("eval_super_base expects record")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(rec.0)));
                let func_idx = self.builtin_func_indices.get(builtin).copied()
                    .with_context(|| format!("no WASM func index for {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                } else {
                    self.emit(WasmInstruction::Drop);
                }
                Ok(())
            }
            Builtin::ScopeRecordSetMeta => {
                let rec = args.first().context("scope_record_set_meta expects record")?;
                let key = args.get(1).context("scope_record_set_meta expects key")?;
                let val = args.get(2).context("scope_record_set_meta expects value")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(rec.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(key.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(val.0)));
                let func_idx = self.builtin_func_indices.get(builtin).copied()
                    .with_context(|| format!("no WASM func index for {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                self.emit(WasmInstruction::Drop);
                Ok(())
            }
```

- [ ] **Step 4: Build check**

```bash
cargo check -p wjsm-backend-wasm
```
Expected: no errors (unused import warnings OK for now).

- [ ] **Step 5: Commit**

```bash
git add crates/wjsm-backend-wasm/src/lib.rs crates/wjsm-backend-wasm/src/compiler_core.rs crates/wjsm-backend-wasm/src/compiler_builtins.rs
git commit -m "feat(backend): register 7 ScopeRecord WASM imports and codegen"
```

---

### Task 3: Runtime — ScopeRecord struct + host functions

**Files:**
- Modify: `crates/wjsm-runtime/src/runtime_eval.rs`
- Modify: `crates/wjsm-runtime/src/lib.rs`

- [ ] **Step 1: Add ScopeRecord struct and helper table to runtime_eval.rs**

At the top of `crates/wjsm-runtime/src/runtime_eval.rs`, add:

```rust
use std::collections::HashMap;

/// 0=is_strict, 1=has_arguments, 2=home_object, 3=new_target
const META_IS_STRICT: u8 = 0;
const META_HAS_ARGUMENTS: u8 = 1;
const META_HOME_OBJECT: u8 = 2;
const META_NEW_TARGET: u8 = 3;

/// Host-allocated scope record implementing spec-like scope behavior.
struct ScopeRecord {
    bindings: Vec<(String, i64, bool, bool)>, // (name, value, initialized, is_const)
    home_object: Option<i64>,
    new_target: Option<i64>,
    has_arguments_binding: bool,
    is_strict: bool,
}
```

- [ ] **Step 2: Add ScopeRecord table to RuntimeState**

In `crates/wjsm-runtime/src/lib.rs`, find the `RuntimeState` struct. Add:

```rust
    /// Temporary ScopeRecord handles for active eval calls.
    /// Keyed by handle index; entries removed when eval returns.
    pub(crate) scope_records: std::collections::HashMap<u32, ScopeRecord>,
```

Also add `use crate::runtime_eval::ScopeRecord;` if needed, or re-export.

Ensure `RuntimeState::default()` or `new()` initializes `scope_records: HashMap::new()`.

- [ ] **Step 3: Implement host function stubs in runtime_eval.rs**

After the `perform_eval_from_caller` function, add 7 host functions:

```rust
pub(crate) fn scope_record_create(
    mut caller: Caller<'_, RuntimeState>,
    capacity: i64,
) -> i64 {
    let handle = caller.data().scope_records.len() as u32;
    caller.data().scope_records.insert(handle, ScopeRecord {
        bindings: Vec::with_capacity(capacity as usize),
        home_object: None,
        new_target: None,
        has_arguments_binding: false,
        is_strict: false,
    });
    value::encode_scope_record_handle(handle)
}

pub(crate) fn scope_record_add_binding(
    mut caller: Caller<'_, RuntimeState>,
    record: i64,
    name: i64,
    val: i64,
    is_tdz: i64,
    is_const: i64,
) {
    let handle = value::decode_scope_record_handle(record);
    let name_str = read_value_string_bytes(&mut caller, name)
        .and_then(|b| String::from_utf8(b).ok())
        .unwrap_or_default();
    let initialized = !value::decode_bool(is_tdz);
    let constant = value::decode_bool(is_const);
    if let Some(rec) = caller.data().scope_records.get_mut(&handle) {
        rec.bindings.push((name_str, val, initialized, constant));
    }
}

pub(crate) fn eval_get_binding(
    mut caller: Caller<'_, RuntimeState>,
    record: i64,
    name: i64,
) -> i64 {
    let handle = value::decode_scope_record_handle(record);
    let name_str = read_value_string_bytes(&mut caller, name)
        .and_then(|b| String::from_utf8(b).ok())
        .unwrap_or_default();
    if let Some(rec) = caller.data().scope_records.get(&handle) {
        for (n, v, init, _) in &rec.bindings {
            if n == &name_str {
                if !init {
                    let msg = format!("Cannot access '{}' before initialization", name_str);
                    set_runtime_error(caller.data(), msg);
                    // Return TAG_EXCEPTION via error table
                    let mut errors = caller.data().error_table.lock().unwrap();
                    let idx = errors.len() as u32;
                    errors.push(crate::ErrorEntry {
                        name: "ReferenceError".to_string(),
                        message: msg,
                        value: value::encode_undefined(),
                    });
                    return value::encode_handle(value::TAG_EXCEPTION, idx + 1);
                }
                return *v;
            }
        }
    }
    value::encode_undefined()
}

pub(crate) fn eval_set_binding(
    mut caller: Caller<'_, RuntimeState>,
    record: i64,
    name: i64,
    val: i64,
) -> i64 {
    let handle = value::decode_scope_record_handle(record);
    let name_str = read_value_string_bytes(&mut caller, name)
        .and_then(|b| String::from_utf8(b).ok())
        .unwrap_or_default();
    if let Some(rec) = caller.data().scope_records.get_mut(&handle) {
        for (n, v, init, is_const) in rec.bindings.iter_mut() {
            if n == &name_str {
                if *is_const && *init {
                    let msg = format!("assignment to constant `{}`", name_str);
                    set_runtime_error(caller.data(), msg);
                    let mut errors = caller.data().error_table.lock().unwrap();
                    let idx = errors.len() as u32;
                    errors.push(crate::ErrorEntry {
                        name: "TypeError".to_string(),
                        message: msg,
                        value: value::encode_undefined(),
                    });
                    return value::encode_handle(value::TAG_EXCEPTION, idx + 1);
                }
                *v = val;
                *init = true;
                return val;
            }
        }
        // Non-strict: write to global object. The eval module has access to
        // $0.$global via its scope bridge; assignments to undeclared vars in
        // sloppy eval create properties on the global object per spec §15.1.2.
        // This is handled by the semantic layer's existing scope bridge fallback
        // which routes undeclared assignments to SetProp on $0.$global.
        // At runtime, the binding simply passes through — the semantic layer
        // handles the routing. Return val as-is.
        return val;
        // Strict: ReferenceError
        let msg = format!("assignment to undeclared variable '{}'", name_str);
        set_runtime_error(caller.data(), msg);
        let mut errors = caller.data().error_table.lock().unwrap();
        let idx = errors.len() as u32;
        errors.push(crate::ErrorEntry {
            name: "ReferenceError".to_string(),
            message: msg,
            value: value::encode_undefined(),
        });
        return value::encode_handle(value::TAG_EXCEPTION, idx + 1);
    }
    value::encode_undefined()
}

pub(crate) fn eval_has_binding(
    mut caller: Caller<'_, RuntimeState>,
    record: i64,
    name: i64,
) -> i64 {
    let handle = value::decode_scope_record_handle(record);
    let name_str = read_value_string_bytes(&mut caller, name)
        .and_then(|b| String::from_utf8(b).ok())
        .unwrap_or_default();
    if let Some(rec) = caller.data().scope_records.get(&handle) {
        let found = rec.bindings.iter().any(|(n, _, _, _)| n == &name_str);
        return value::encode_bool(found);
    }
    value::encode_bool(false)
}

pub(crate) fn eval_super_base(
    mut caller: Caller<'_, RuntimeState>,
    record: i64,
) -> i64 {
    let handle = value::decode_scope_record_handle(record);
    if let Some(rec) = caller.data().scope_records.get(&handle) {
        if let Some(home) = rec.home_object {
            // home_object 已由语义层通过 GetSuperBase 预计算为原型引用
            return home;
        }
    }
    value::encode_undefined()
}

pub(crate) fn scope_record_set_meta(
    mut caller: Caller<'_, RuntimeState>,
    record: i64,
    key: i64,
    val: i64,
) {
    let handle = value::decode_scope_record_handle(record);
    let tag = f64::from_bits(key as u64) as u8;
    if let Some(rec) = caller.data().scope_records.get_mut(&handle) {
        match tag {
            0 => rec.is_strict = value::decode_bool(val),
            1 => rec.has_arguments_binding = value::decode_bool(val),
            2 => rec.home_object = Some(val),
            3 => rec.new_target = Some(val),
            _ => debug_assert!(false, "unknown scope record meta key: {}", tag),
        }
    }
}
```

- [ ] **Step 4: Register host functions in lib.rs**

In `crates/wjsm-runtime/src/lib.rs`, find the linker section where `eval_direct_fn` and `eval_indirect_fn` are registered (in `promise_async.rs` or the main linker setup). Add:

```rust
    linker.func_wrap("env", "scope_record_create", scope_record_create)?;
    linker.func_wrap("env", "scope_record_add_binding", scope_record_add_binding)?;
    linker.func_wrap("env", "eval_get_binding", eval_get_binding)?;
    linker.func_wrap("env", "eval_set_binding", eval_set_binding)?;
    linker.func_wrap("env", "eval_has_binding", eval_has_binding)?;
    linker.func_wrap("env", "eval_super_base", eval_super_base)?;
    linker.func_wrap("env", "scope_record_set_meta", scope_record_set_meta)?;
```

- [ ] **Step 5: Build check**

```bash
cargo check -p wjsm-runtime
```
Expected: no errors. Fix any type mismatches in ScopeRecord usage.

- [ ] **Step 6: Commit**

```bash
git add crates/wjsm-runtime/src/runtime_eval.rs crates/wjsm-runtime/src/lib.rs
git commit -m "feat(runtime): add ScopeRecord struct and 7 host functions"
```

---

### Task 4: Semantic — class predeclare TDZ fix

**Files:**
- Modify: `crates/wjsm-semantic/src/lowerer_predeclare.rs`

- [ ] **Step 1: Change class predeclare to Let + uninitialized**

In `crates/wjsm-semantic/src/lowerer_predeclare.rs`, find the `Decl::Class` arm (around line 155):

```rust
// Before:
                swc_ast::Decl::Class(class_decl) => {
                    let name = class_decl.ident.sym.to_string();
                    let _scope_id = self
                        .scopes
                        .declare(&name, VarKind::Var, true)
                        .map_err(|msg| self.error(class_decl.span(), msg))?;
                }

// After:
                swc_ast::Decl::Class(class_decl) => {
                    let name = class_decl.ident.sym.to_string();
                    let _scope_id = self
                        .scopes
                        .declare(&name, VarKind::Let, false)
                        .map_err(|msg| self.error(class_decl.span(), msg))?;
                }
```

- [ ] **Step 2: Run existing tests to verify no regression**

```bash
cargo test -p wjsm-semantic
```
Expected: all existing snapshot and unit tests pass.

- [ ] **Step 3: Add happy-path fixture for class TDZ in eval**

Create `fixtures/happy/eval-tdz-class.js`:
```js
var caught = null;
try {
  eval('typeof C; class C {}');
} catch (e) {
  caught = e.constructor.name;
}
print(caught);
```

Create `fixtures/happy/eval-tdz-class.expected`:
```
exit_code: 0
--- stdout ---
ReferenceError
--- stderr ---
```

- [ ] **Step 4: Run fixture to verify**

```bash
cargo run -- run fixtures/happy/eval-tdz-class.js
```
Expected: exit 0, stdout "ReferenceError".

- [ ] **Step 5: Commit**

```bash
git add crates/wjsm-semantic/src/lowerer_predeclare.rs fixtures/happy/eval-tdz-class.js fixtures/happy/eval-tdz-class.expected
git commit -m "fix(semantic): class predeclare uses Let+uninitialized for correct TDZ"
```

---

### Task 5: Semantic — visible_bindings_all + lower_direct_eval_call rewrite

**Files:**
- Modify: `crates/wjsm-semantic/src/lib.rs`
- Modify: `crates/wjsm-semantic/src/lowerer_calls_eval.rs`

- [ ] **Step 1: Add visible_bindings_all() to ScopeTree in lib.rs**

Find `fn visible_bindings` in `ScopeTree` (around line 194 in `lib.rs`). Add a new method below it:

```rust
    /// Return all lexically visible bindings, including uninitialized (TDZ) ones.
    /// Returns (scope_id, name, kind, is_initialised).
    pub(crate) fn visible_bindings_all(&self) -> Vec<(usize, String, VarKind, bool)> {
        let mut result = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut cursor = Some(self.current);
        while let Some(idx) = cursor {
            let scope = &self.arenas[idx];
            for (name, info) in &scope.variables {
                if seen.insert(name.clone()) {
                    result.push((scope.id, name.clone(), info.kind, info.initialised));
                }
            }
            cursor = scope.parent;
        }
        result
    }
```

- [ ] **Step 2: Add Lowerer fields in lib.rs**

In the `Lowerer` struct, add after `eval_var_writes_to_scope`:

```rust
    pub(crate) eval_scope_record: bool,
    pub(crate) eval_caller_has_arguments: bool,
```

Initialize both to `false` in `Lowerer::new()`.

- [ ] **Step 3: Add helper const_val_i64 to lowerer_calls_eval.rs**

In `crates/wjsm-semantic/src/lowerer_calls_eval.rs`, add a helper method to `impl Lowerer`:

```rust
    fn const_val_i64(&mut self, block: BasicBlockId, value: i64) -> ValueId {
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest,
                constant: self.module.add_constant(Constant::Number(value as f64)),
            },
        );
        dest
    }

    fn const_val(&mut self, block: BasicBlockId, constant: ConstantId) -> ValueId {
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const { dest, constant },
        );
        dest
    }
```

- [ ] **Step 4: Rewrite lower_direct_eval_call in lowerer_calls_eval.rs**

Replace the existing `lower_direct_eval_call` function body (keeping the signature). The replacement builds a ScopeRecord instead of a flat object. The full code is ~120 lines — deploy using the spec §2.4 pseudocode as a guide, concretely adapted to the actual IR Instruction API:

1. Call `mark_has_eval()`
2. Lower the code argument via existing `lower_expr_then_continue`
3. Call `visible_bindings_all()` to get all bindings with TDZ status
4. Emit `ScopeRecordCreate` with capacity = bindings.len()
5. For each `(scope_id, name, kind, is_initialised)`:
   - Load binding value (reuse existing load logic from old code)
   - Emit `ScopeRecordAddBinding` with `is_tdz = !is_initialised`, `is_const = (kind == VarKind::Const)`
6. Emit `ScopeRecordSetMeta` for is_strict (key=0) and has_arguments (key=1)
7. If `current_function.home_object.is_some()`: emit `GetSuperBase`, then `ScopeRecordSetMeta` key=2
8. If not arrow function: emit `NewTarget`, then `ScopeRecordSetMeta` key=3
9. Emit `Builtin::Eval(code, env)` — same as before but with the scope record as env
10. Exception branch — same `IsException` + `ExceptionValue` + throw as before
11. Writeback: for each non-TDZ binding, `EvalGetBinding(env, name)` → `StoreVar`
12. Return `(dest, merge_block)`

- [ ] **Step 5: Update lower_eval_module_with_scope in lib.rs**

In `lower_eval_module_with_scope` (around line 596), set `eval_scope_record = true`:

```rust
pub fn lower_eval_module_with_scope(
    module: swc_ast::Module,
    has_scope_bridge: bool,
    var_writes_to_scope: bool,
) -> Result<Program, LoweringError> {
    let mut lowerer = Lowerer::new();
    lowerer.eval_mode = true;
    lowerer.eval_has_scope_bridge = has_scope_bridge;
    lowerer.eval_var_writes_to_scope = var_writes_to_scope;
    lowerer.eval_scope_record = true;  // NEW
    lowerer.lower_module(&module)
}
```

- [ ] **Step 6: Build check**

```bash
cargo check -p wjsm-semantic
```
Expected: no errors. Fix any missing imports.

- [ ] **Step 7: Run existing eval tests to check for regressions**

```bash
cargo test -p wjsm-semantic
cargo run -- run fixtures/happy/eval-*.js
```
Expected: tests pass, existing eval fixtures still work.

- [ ] **Step 8: Commit**

```bash
git add crates/wjsm-semantic/src/lib.rs crates/wjsm-semantic/src/lowerer_calls_eval.rs
git commit -m "feat(semantic): rewrite lower_direct_eval_call to use ScopeRecord"
```

---

### Task 6: Semantic — eval module identifier routing + super/new.target eval paths

**Files:**
- Modify: `crates/wjsm-semantic/src/lowerer_assignments.rs`
- Modify: `crates/wjsm-semantic/src/lowerer_async_eval.rs`

- [ ] **Step 1: Route eval scope bridge to EvalGetBinding/EvalSetBinding**

In `crates/wjsm-semantic/src/lowerer_assignments.rs`, find `lower_eval_env_read` and `lower_assign_eval_env`. When `self.eval_scope_record` is true:

- `lower_eval_env_read`: emit `EvalGetBinding(env, name)` instead of `GetProp(env, name)`
- `lower_assign_eval_env`: emit `EvalSetBinding(env, name, value)` instead of `SetProp(env, name, value)`

Similarly, in `lower_ident` (the scope bridge fallback arm), check `self.eval_scope_record` and use `EvalGetBinding`.

- [ ] **Step 2: Add eval mode to lower_super_prop in lowerer_async_eval.rs**

In `lower_super_prop`, before the existing `GetSuperBase` instruction emission (around line 120), add a branch:

```rust
    pub(crate) fn lower_super_prop(
        &mut self,
        super_prop: &swc_ast::SuperPropExpr,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        let base_val = self.alloc_value();
        if self.eval_scope_record {
            // In eval module: get super base from scope record
            let env = self.load_eval_scope_env(block);
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: Some(base_val),
                    builtin: Builtin::EvalSuperBase,
                    args: vec![env],
                },
            );
        } else {
            self.current_function.append_instruction(
                block,
                Instruction::GetSuperBase { dest: base_val },
            );
        }
        // ... rest of property access unchanged ...
```

- [ ] **Step 3: Add eval mode for new.target in lowerer_jsx_objects.rs or lowerer_async_eval.rs**

Find where `new.target` is lowered (currently throws SyntaxError in eval top-level/arrow). When `self.eval_scope_record`:

```rust
    // new.target in eval: read from scope record binding named "__wjsm_new_target"
    // The value was stored by lower_direct_eval_call via ScopeRecordAddBinding
    // with is_tdz=false, is_const=true at creation time.
    let env = self.load_eval_scope_env(block);
    let key_cid = self.module.add_constant(Constant::String("__wjsm_new_target".to_string()));
    let key_val = self.const_val(block, key_cid);
    let nt = self.alloc_value();
    self.current_function.append_instruction(
        block,
        Instruction::CallBuiltin {
            dest: Some(nt),
            builtin: Builtin::EvalGetBinding,
            args: vec![env, key_val],
        },
    );
    return Ok(nt);
```

Additionally, in `lower_direct_eval_call` (Task 5 Step 4), the new.target value must be added as a ScopeRecord **binding** (via `ScopeRecordAddBinding` with name `"__wjsm_new_target"`, `is_tdz=false`, `is_const=true`) in addition to the existing `ScopeRecordSetMeta(key=3)` call. This ensures the eval module can read it through `EvalGetBinding`, which is the only read path available inside eval modules.
- [ ] **Step 4: Build check**

```bash
cargo check -p wjsm-semantic
```
Expected: no errors.

- [ ] **Step 5: Commit**

```bash
git add crates/wjsm-semantic/src/lowerer_assignments.rs crates/wjsm-semantic/src/lowerer_async_eval.rs crates/wjsm-semantic/src/lowerer_jsx_objects.rs
git commit -m "feat(semantic): route eval module idents through EvalGet/Set, add super/new.target eval paths"
```

---

### Task 7: Runtime — arguments conflict detection + cache version

**Files:**
- Modify: `crates/wjsm-runtime/src/runtime_eval.rs`
- Modify: `crates/wjsm-semantic/src/eval_scan.rs`
- Modify: `crates/wjsm-semantic/src/lowerer_core.rs`

- [ ] **Step 1: Bump eval_literal_binding_names visibility to pub**

In `crates/wjsm-semantic/src/eval_scan.rs`, change `pub(crate) fn eval_literal_binding_names` to `pub fn eval_literal_binding_names`.

In `crates/wjsm-semantic/src/lib.rs`, ensure `eval_literal_binding_names` is re-exported or accessible from `wjsm_semantic::`.

- [ ] **Step 2: Replace string-matching arguments check in perform_eval_from_caller**

In `crates/wjsm-runtime/src/runtime_eval.rs`, find the existing check (around line 249):

```rust
    // OLD:
    if let Some(env) = scope_env
        && (code.contains("var arguments") || code.contains("function arguments"))
    { ... }

    // NEW:
    if let Some(env) = scope_env {
        let handle = value::decode_scope_record_handle(env);
        let has_arguments = caller.data().scope_records
            .get(&handle)
            .map(|r| r.has_arguments_binding)
            .unwrap_or(false);
        if has_arguments {
            let binding_names = wjsm_semantic::eval_literal_binding_names(&code);
            if binding_names.iter().any(|n| n == "arguments") {
                let msg = "SyntaxError: declaring 'arguments' in eval code is invalid";
                set_runtime_error(caller.data(), msg.to_string());
                return value::encode_undefined();
            }
        }
    }
```

- [ ] **Step 3: Set eval_caller_has_arguments in lowerer_core.rs**

In `crates/wjsm-semantic/src/lowerer_core.rs`, find where function bodies are lowered (around the `emit_arguments_init` call site). Before emitting arguments init, compute:

```rust
    // Detect if calling context has explicit arguments binding
    let has_param_arguments = params.iter().any(|p| {
        let mut names = Vec::new();
        Self::extract_pat_bindings(std::slice::from_ref(&p.pat), &mut names);
        names.iter().any(|n| n == "arguments")
    });
    let has_explicit_arguments = self.scopes.lookup("arguments").is_ok();
    self.eval_caller_has_arguments = has_param_arguments || has_explicit_arguments;
```

- [ ] **Step 4: Add cache version to cached_eval_wasm**

In `crates/wjsm-runtime/src/runtime_eval.rs`, in `cached_eval_wasm` (around line 76):

```rust
    const SCOPE_RECORD_CACHE_VERSION: u64 = 1;
    SCOPE_RECORD_CACHE_VERSION.hash(&mut hasher);
```

Add this line after `data_base.hash(&mut hasher);`.

- [ ] **Step 5: Build check**

```bash
cargo check -p wjsm-semantic -p wjsm-runtime
```
Expected: no errors.

- [ ] **Step 6: Commit**

```bash
git add crates/wjsm-runtime/src/runtime_eval.rs crates/wjsm-semantic/src/eval_scan.rs crates/wjsm-semantic/src/lowerer_core.rs crates/wjsm-semantic/src/lib.rs
git commit -m "feat: arguments conflict detection via byte scanner, cache version bump"
```

---

### Task 8: Integration tests + test262 verification

**Files:**
- Create: `fixtures/happy/eval-scope-record.js` + `.expected`
- Create: `fixtures/happy/eval-super-prop.js` + `.expected`
- Create: `fixtures/happy/eval-arguments-ok.js` + `.expected`
- Create: `fixtures/errors/eval-arguments-conflict.js` + `.expected`

- [ ] **Step 1: Create eval-scope-record fixture**

`fixtures/happy/eval-scope-record.js`:
```js
// Verify that eval can read/write bindings through the scope record
var x = 10;
var result = eval('x = x + 1; x');
print(result);
print(x);
```

`fixtures/happy/eval-scope-record.expected`:
```
exit_code: 0
--- stdout ---
11
11
--- stderr ---
```

- [ ] **Step 2: Create eval-super-prop fixture**

`fixtures/happy/eval-super-prop.js`:
```js
var superProp = null;
var proto = { test262: 262 };
var o = {
  __proto__: proto,
  method() {
    superProp = eval('super.test262;');
  }
};
o.method();
print(superProp);
```

`fixtures/happy/eval-super-prop.expected`:
```
exit_code: 0
--- stdout ---
262
--- stderr ---
```

- [ ] **Step 3: Create eval-arguments fixtures**

`fixtures/happy/eval-arguments-ok.js`:
```js
(function() {
  eval('var arguments = "from_eval";');
  print(arguments);
})();
```

`fixtures/happy/eval-arguments-ok.expected`:
```
exit_code: 0
--- stdout ---
from_eval
--- stderr ---
```

`fixtures/errors/eval-arguments-conflict.js`:
```js
(function(arguments) {
  eval('var arguments = "bad";');
})();
```

`fixtures/errors/eval-arguments-conflict.expected`:
```
exit_code: 1
--- stdout ---
--- stderr ---
SyntaxError: declaring 'arguments' in eval code is invalid
```

- [ ] **Step 4: Run all fixtures**

```bash
cargo test
```
Expected: all existing tests pass + new fixtures pass.

- [ ] **Step 5: Run test262 eval suite**

```bash
cargo run -p wjsm-test262 -- run --suite test/language/eval-code/direct --all --plain 2>&1 | tail -5
```
Expected: pass count significantly higher than 29/286.

- [ ] **Step 6: Update semantic snapshots if needed**

```bash
WJSM_UPDATE_FIXTURES=1 cargo test -p wjsm-semantic
```
Review any changed `.ir` snapshots. Commit the updates.

- [ ] **Step 7: Final commit**

```bash
git add fixtures/
git commit -m "test: add ScopeRecord integration fixtures and update snapshots"
```
