# Eval Arguments / TDZ / Super Gap Closure Plan

> **Goal:** Close all remaining spec gaps in direct eval's arguments binding, TDZ enforcement, and super propagation. Bring test262 direct eval pass rate from ~29/286 (~10%) to ~190/286 (~67%).

**Architecture:** Fix a critical detection-order bug in `eval_caller_has_arguments`, dispatch `CreateMappedArgumentsObject` in non-strict paths, add `callee`/`Symbol.iterator` to arguments objects, pass `new.target` through eval, complete the interpreted eval AST walker, and enforce strict-mode `ReferenceError` on undeclared eval writes.

**Tech Stack:** Rust 2024, swc_core, wasm-encoder, wasmtime

**Authority:** ECMAScript spec §15.1.2 (EvalDeclarationInstantiation), §10.4.4 (Arguments Exotic Objects), §8.1.1.1 (GetIdentifierReference)

**Compatibility Boundary:** Must not break existing fixtures (470+ E2E tests). No IR format changes needed — all required Builtin variants and runtime host functions already exist.

---

## Plan Pressure Test

- **Owner / contract / retirement:** Arguments IR (CreateMappedArgumentsObject) already exists but unused. ScopeRecord framework is complete. No retirement needed — this is activation of dormant code.
- **Verification scope:** 470+ existing fixtures + test262 direct eval suite (~286 tests) + manual argument fixture tests.
- **Task executability:** Each task is a localized fix in a single crate. No multi-crate orchestration needed.
- **Pressure result:** proceed

## Plan-Time Complexity Check

- **Target files:** `lowerer_functions.rs`, `lowerer_function_decls.rs`, `lowerer_declarations.rs`, `lowerer_calls_eval.rs`, `runtime_eval.rs`, `runtime_arguments.rs`, `lowerer_classes_ts.rs`
- **Existing size / shape signals:** `lowerer_classes_ts.rs` is large (~2000 lines) but tasks only touch specific call sites (~4 lines each). `runtime_eval.rs` needs significant additions to interpreted path (~100 lines).
- **Owner fit:** All changes extend existing code paths with existing patterns. No new files needed.
- **Recommendation:** edit-in-place for all tasks

---

## Task 1: Fix `eval_caller_has_arguments` detection-order bug

**Root cause:** In `lowerer_functions.rs:52-55`, `eval_caller_has_arguments` is computed *before* `emit_arguments_init`, which declares the implicit `arguments` binding. For `function f(a) { eval("...") }`, the implicit `arguments` exists but the flag is already `false`, causing ScopeRecord metadata to be wrong.

**Impact:** ~144 test262 `arguments` tests fail because the SyntaxError check never fires, and `has_arguments_binding` is always `0` for regular functions (even though `arguments` binding exists within the function).

**Spec reference:** ES §15.1.2 step 3: "Let callContextHasArgumentsBinding = envRec.HasLexicalBinding('arguments')" — this checks the *live* environment record, which includes the implicit arguments binding.

### Files

- Modify: `crates/wjsm-semantic/src/lowerer_functions.rs` (lines 52-56)
- Modify: `crates/wjsm-semantic/src/lowerer_function_decls.rs` (all 5 call sites: lines 44-46, 376-378, 522-524, 968-970, 1129-1131)
- Modify: `crates/wjsm-semantic/src/lowerer_classes_ts.rs` (call sites that set eval_caller_has_arguments)

### Task Steps

- [ ] **Step 1: Move eval_caller_has_arguments computation AFTER emit_arguments_init in lowerer_functions.rs**

In `lowerer_functions.rs`, at the function body lowering path (line 52-56), swap the order:

```rust
// BEFORE (buggy):
let has_explicit_arguments = self.scopes.lookup("arguments").is_ok();
self.eval_caller_has_arguments =
    Self::detect_param_arguments(&fn_expr.function.params) || has_explicit_arguments;
let body_entry = self.emit_arguments_init(body_entry)?;

// AFTER (correct):
let body_entry = self.emit_arguments_init(body_entry)?;
self.eval_caller_has_arguments =
    Self::detect_param_arguments(&fn_expr.function.params)
    || self.scopes.lookup("arguments").is_ok();
```

`emit_arguments_init` declares the implicit `arguments` binding. By checking `lookup("arguments")` *after*, the flag correctly detects the implicit binding.

- [ ] **Step 2: Apply same fix to all function_decls.rs call sites**

In `lowerer_function_decls.rs`, there are 5 call sites that set `eval_caller_has_arguments` before `emit_arguments_init`. Move the computation after `emit_arguments_init` at each site. The affected lines are:

- Line 44-46 (function declaration)
- Line 376-378 (async function declaration)
- Line 522-524 (async closure wrapper)
- Line 968-970 (method wrapper)
- Line 1129-1131 (method wrapper)

For each, apply the same pattern: call `emit_arguments_init` first, then compute `eval_caller_has_arguments` from `lookup("arguments").is_ok()`.

- [ ] **Step 3: Apply same fix to functions.rs async paths**

In `lowerer_functions.rs`, there are 2 more call sites for async function expressions and async closure wrappers (lines 380, 536). Apply the same fix.

- [ ] **Step 4: Verify class method call sites**

In `lowerer_classes_ts.rs`, check each class method lowering path that calls `emit_arguments_init`. Class methods with direct eval need `eval_caller_has_arguments` set. Method bodies call `emit_arguments_init` which declares implicit `arguments`. After that call, compute:

```rust
self.emit_arguments_init(block)?;
self.eval_caller_has_arguments = self.scopes.lookup("arguments").is_ok();
```

Check all 15+ `emit_arguments_init` call sites in `lowerer_classes_ts.rs` and add the `eval_caller_has_arguments` computation after each.

- [ ] **Step 5: Build check**

```bash
cargo check -p wjsm-semantic
```
Expected: no errors.

- [ ] **Step 6: Run existing tests**

```bash
cargo nextest run -E 'test(happy__eval_arguments)'
cargo nextest run -E 'test(errors__eval_arguments)'
```
Expected: `eval-arguments-ok` still passes, `eval-arguments-conflict` still catches conflict.

- [ ] **Step 7: Commit**

```bash
git add crates/wjsm-semantic/src/lowerer_functions.rs \
        crates/wjsm-semantic/src/lowerer_function_decls.rs \
        crates/wjsm-semantic/src/lowerer_classes_ts.rs
git commit -m "fix(semantic): compute eval_caller_has_arguments after emit_arguments_init"
```

---

## Task 2: Dispatch Mapped vs Unmapped arguments objects

**Root cause:** `emit_arguments_init` (lowerer_declarations.rs:391-452) always emits `CreateUnmappedArgumentsObject` regardless of strictness or function type. Non-strict regular functions should use `CreateMappedArgumentsObject` for `callee` support and [[ParameterMap]] preparation.

**Spec reference:** ES §10.4.4.7 (CreateMappedArgumentsObject) for non-strict non-arrow non-method functions. ES §10.4.4.6 (CreateUnmappedArgumentsObject) for strict mode, arrow functions, and methods.

### Files

- Modify: `crates/wjsm-semantic/src/lowerer_declarations.rs` (emit_arguments_init)
- Modify: `crates/wjsm-semantic/src/lowerer_core.rs` (Lowerer fields)
- Modify: `crates/wjsm-semantic/src/lowerer_arrows.rs` (set is_arrow)
- Modify: `crates/wjsm-semantic/src/lowerer_classes_ts.rs` (set is_method)
- Modify: `crates/wjsm-semantic/src/lowerer_functions.rs` (set is_arrow for arrow expr)
- Modify: `crates/wjsm-semantic/src/lowerer_function_decls.rs` (set is_method for method wrappers)
- Modify: `crates/wjsm-semantic/src/lowerer_jsx_objects.rs` (set is_arrow for JSX components)

### Task Steps

- [ ] **Step 1: Add is_arrow and is_method fields to Lowerer struct**

In `crates/wjsm-semantic/src/lowerer_core.rs`, add to the `Lowerer` struct (after `strict_mode: bool`):

```rust
pub(crate) is_arrow: bool,
pub(crate) is_method: bool,
```

In `Lowerer::new()` (same file), initialize both:

```rust
is_arrow: false,
is_method: false,
```

Set `is_arrow = true` in `lowerer_arrows.rs` at arrow function lowering entry (before `emit_arguments_init`).
Set `is_method = true` in `lowerer_classes_ts.rs` at each class method lowering entry.
Set `is_arrow = false / is_method = true` as appropriate in `lowerer_functions.rs`, `lowerer_function_decls.rs`, and `lowerer_jsx_objects.rs`.

- [ ] **Step 2: Rewrite emit_arguments_init to dispatch based on context**

In `crates/wjsm-semantic/src/lowerer_declarations.rs`, replace `emit_arguments_init` body with:

```rust
pub(crate) fn emit_arguments_init(
    &mut self,
    block: BasicBlockId,
) -> Result<BasicBlockId, LoweringError> {
    let scope_id = match self.scopes.declare("arguments", VarKind::Let, true) {
        Ok(id) => {
            self.scopes.set_implicit_arguments("arguments")?;
            id
        }
        Err(_) => self.scopes.resolve_scope_id("arguments").unwrap_or(0),
    };
    let ir_name = format!("${scope_id}.arguments");

    let args_array = self.alloc_value();
    self.current_function.append_instruction(
        block,
        Instruction::CollectRestArgs { dest: args_array, skip: 0 },
    );

    let param_count = self.current_function.params().len() as f64;
    let param_count_val = self.alloc_value();
    self.current_function.append_instruction(
        block,
        Instruction::Const {
            dest: param_count_val,
            constant: self.module.add_constant(Constant::Number(param_count)),
        },
    );

    let arguments_obj = self.alloc_value();
    let needs_mapped = !self.strict_mode && !self.is_arrow && !self.is_method;

    if needs_mapped {
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(arguments_obj),
                builtin: Builtin::CreateMappedArgumentsObject,
                args: vec![args_array, param_count_val, value::encode_undefined()],
            },
        );
    } else {
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(arguments_obj),
                builtin: Builtin::CreateUnmappedArgumentsObject,
                args: vec![args_array, param_count_val],
            },
        );
    }

    let store_block = self.resolve_store_block(block);
    self.current_function.append_instruction(
        store_block,
        Instruction::StoreVar { name: ir_name, value: arguments_obj },
    );

    self.scopes.mark_initialised("arguments").ok();
    Ok(self.resolve_store_block(block))
}
```

**func_ref strategy:** Pass `value::encode_undefined()` as the 3rd arg to `CreateMappedArgumentsObject`. The runtime already has access to the caller via wasmtime's `Caller` context, so `create_mapped_arguments_object` can derive the callee from `caller` without needing the semantic layer to pass a FunctionRef constant. This avoids plumbing `FunctionId` through `emit_arguments_init` which currently has no access to it.

`Constant::FunctionRef(FunctionId)` exists (confirmed: used in 27 call sites across the codebase) but the function bodies calling `emit_arguments_init` would need to pass the id through — an unnecessary plumbing change.

- [ ] **Step 3: Update runtime create_mapped_arguments_object to derive callee from Caller**

In `crates/wjsm-runtime/src/runtime_arguments.rs`, modify `create_mapped_arguments_object`:

```rust
pub(crate) fn create_mapped_arguments_object(
    caller: &mut Caller<'_, RuntimeState>,
    args_array: i64,
    param_count: i64,
    func_ref: i64,
) -> i64 {
    let _param_count = value::decode_f64(param_count) as u32;

    let arr_ptr = if value::is_array(args_array) {
        resolve_handle(caller, args_array)
    } else {
        None
    };
    let len = arr_ptr
        .and_then(|ptr| read_array_length(caller, ptr))
        .unwrap_or(0);
    let capacity = (len + 2).max(4);
    let obj = {
        let _wjsm_env = WasmEnv::from_caller(caller).expect("WasmEnv");
        alloc_host_object(caller, &_wjsm_env, capacity)
    };

    // 覆写 heap type
    if let Some(ptr) = resolve_handle(caller, obj) {
        if let Some(Extern::Memory(mem)) = caller.get_export("memory") {
            let data = mem.data_mut(&mut *caller);
            if ptr + 4 < data.len() {
                data[ptr + 4] = wjsm_ir::HEAP_TYPE_ARGUMENTS;
            }
        }
    }

    if let Some(ptr) = arr_ptr {
        for i in 0..len as usize {
            let val = read_array_elem(caller, ptr, i as u32).unwrap_or(value::encode_undefined());
            let _ = define_host_data_property_from_caller(caller, obj, &i.to_string(), val);
        }
    }

    let _ = define_host_data_property_from_caller(
        caller, obj, "length", value::encode_f64(len as f64),
    );

    // callee: derive from Caller context when func_ref is undefined
    if !value::is_undefined(func_ref) {
        let _ = define_host_data_property_from_caller(caller, obj, "callee", func_ref);
    }
    // NOTE: full callee derivation from Caller state requires tracking the
    // current function reference in RuntimeState. Defer to a follow-up task.

    obj
}
```

- [ ] **Step 4: Build check**

```bash
cargo check -p wjsm-semantic
```
Expected: no errors.

- [ ] **Step 5: Verify with arguments fixtures**

```bash
cargo run -- run fixtures/happy/arguments-basic.js
cargo run -- run fixtures/happy/arguments-strict.js
```
Expected: both pass. Non-strict functions route through `CreateMappedArgumentsObject`.

- [ ] **Step 6: Commit**

```bash
git add crates/wjsm-semantic/src/lowerer_declarations.rs \
        crates/wjsm-semantic/src/lowerer_core.rs \
        crates/wjsm-semantic/src/lowerer_arrows.rs \
        crates/wjsm-semantic/src/lowerer_classes_ts.rs \
        crates/wjsm-semantic/src/lowerer_functions.rs \
        crates/wjsm-semantic/src/lowerer_function_decls.rs \
        crates/wjsm-semantic/src/lowerer_jsx_objects.rs \
        crates/wjsm-runtime/src/runtime_arguments.rs
git commit -m "feat(arguments): dispatch CreateMappedArgumentsObject for non-strict functions"
```

---

## Task 3: Add callee and Symbol.iterator to arguments objects

**Root cause:** `runtime_arguments.rs` creates basic arguments objects but doesn't set `callee` (already partially done for mapped) or `Symbol.iterator`.

**Spec reference:** ES §10.4.4.7 step 15 (callee), ES §CreateUnmappedArgumentsObject step 8 (Symbol.iterator for unmapped). ES §CreateMappedArgumentsObject step 18 (Symbol.iterator for mapped).

### Files

- Modify: `crates/wjsm-runtime/src/runtime_arguments.rs`

### Task Steps

- [ ] **Step 1: Add Symbol.iterator to create_unmapped_arguments_object**

In `crates/wjsm-runtime/src/runtime_arguments.rs`, after setting `length`:

```rust
// ES 10.4.4.6 step 6a: Perform CreateDataPropertyOrThrow(obj, @@iterator,
// %Array.prototype.values%)
// The well-known symbol Symbol.iterator is encoded as a tagged i64.
// WK_SYMBOL_ITERATOR = 0 per crates/wjsm-semantic/src/lib.rs:14 and
// wjsm-ir encodes well-known symbols as TAG_SYMBOL | handle.
let sym_iterator = value::encode_handle(value::TAG_SYMBOL, 0); // Symbol.iterator
// Get Array.prototype.values from the runtime's builtin table.
// 索引来自 crates/wjsm-semantic/src/builtins.rs Builtin::TypedArrayProtoValues
// 但更简单的路径: 遍历 heap 找 Array 原型或直接创建 symbol 属性名。
// 实际上 runtime 并不需要 Array.prototype.values 的引用——
// arguments 对象的 Symbol.iterator 属性只需设为 Array.prototype.values
// 函数引用。最简单的手动方式:
let arr_values = get_global_builtin(caller, "Array")?;
// 然后读 Array.prototype.values:
// 这需要读取 Array → prototype → values 链。
// 操作路径: 如果已有 WasmEnv 则直接注册。
// 实际实现可能通过 runtime 现有的 register_host_property 机制。
let _ = define_host_data_property_from_caller(caller, obj, &sym_iterator_name, arr_values_fn);
```

**重要:** Symbol.iterator 的实际键是一个 Symbol 值（`TAG_SYMBOL | 0`），不是字符串。`define_host_data_property_from_caller` 当前可能只支持字符串键。需要在 `runtime_values.rs` 添加一个变体支持 i64 键，或用 encode 后的 i64 值作为属性键。

**简化方案 (先合并):** 如果 symbol-typed key 支持尚未就绪，先 skip Symbol.iterator，单独做一个 follow-up 任务。test262 中 arguments 的 Symbol.iterator 相关测试（~3-5 个）可标记为 KNOWN-BROKEN。

- [ ] **Step 2: Add Symbol.iterator to create_mapped_arguments_object**

同上。两个 create 函数末尾添加相同逻辑。

- [ ] **Step 3: Document strict callee gap**

严格模式下 `arguments.callee` 应抛 TypeError 但当前返回 `undefined`。原因：需要 accessor property 支持来定义 throwing getter。创建一个 marked fixture 跟踪此 gap:

`fixtures/happy/arguments-callee-strict.js`:
```js
// KNOWN-BROKEN: strict mode arguments.callee should throw TypeError
// Currently returns undefined because throwing accessor property not implemented
"use strict";
function f() {
    try {
        arguments.callee;
        print("no_throw");
    } catch (e) {
        print("throw");
    }
}
f();
```

`fixtures/happy/arguments-callee-strict.expected` (KNOWN-BROKEN: 当前输出 `no_throw`，规范要求 `throw`):
```
exit_code: 0
--- stdout ---
throw
--- stderr ---
```

- [ ] **Step 4: Build check**

```bash
cargo check -p wjsm-runtime
```
Expected: no errors.

- [ ] **Step 5: Commit**

```bash
git add crates/wjsm-runtime/src/runtime_arguments.rs \
        fixtures/happy/arguments-callee-strict.js \
        fixtures/happy/arguments-callee-strict.expected
git commit -m "feat(arguments): add Symbol.iterator stub and document strict callee gap"
```
```

---

## Task 4: Pass new.target through eval ScopeRecord

**Root cause:** `lower_direct_eval_call` doesn't emit a `new.target` binding into the ScopeRecord. Eval code inside constructors can't access the original `new.target`.

**Spec reference:** ES §15.1.2 step 4: "Let callContextNewTarget = GetNewTarget()" — the eval code inherits the caller's new.target.

### Files

- Modify: `crates/wjsm-semantic/src/lowerer_calls_eval.rs`
- Modify: `crates/wjsm-semantic/src/lowerer_async_eval.rs`

### Task Steps

- [ ] **Step 1: Add new.target binding to ScopeRecord in lower_direct_eval_call**

In `lowerer_calls_eval.rs`, inside `lower_direct_eval_call`, after the super base section (around line 720, between existing "7. Set meta: super base" and "8. Call Eval"), add:

```rust
// 7b. Set meta: new.target (key=3) — only for non-arrow functions
if !self.is_arrow {
    let nt_key = self.const_val_i64(eval_block, 3);
    let new_target = self.alloc_value();
    self.current_function.append_instruction(
        eval_block,
        Instruction::NewTarget { dest: new_target },
    );
    self.current_function.append_instruction(
        eval_block,
        Instruction::CallBuiltin {
            dest: None,
            builtin: Builtin::ScopeRecordSetMeta,
            args: vec![scope_record, nt_key, new_target],
        },
    );

    // Also add as a ScopeRecord binding so eval code can read via EvalGetBinding
    let nt_name = self.module.add_constant(Constant::String("__wjsm_new_target".to_string()));
    let nt_name_val = self.alloc_value();
    self.current_function.append_instruction(
        eval_block,
        Instruction::Const { dest: nt_name_val, constant: nt_name },
    );
    let nt_false = self.const_val_i64(eval_block, 0);
    self.current_function.append_instruction(
        eval_block,
        Instruction::CallBuiltin {
            dest: None,
            builtin: Builtin::ScopeRecordAddBinding,
            args: vec![scope_record, nt_name_val, new_target, nt_false, nt_false],
        },
    );
}
```

- [ ] **Step 2: Implement new.target read in eval modules**

In `lowerer_async_eval.rs` (or wherever `new.target` meta property is lowered), add an eval mode branch:

```rust
// In the new.target lowering path:
if self.eval_scope_record {
    let env = self.load_eval_scope_env(block);
    let nt_name = self.module.add_constant(Constant::String("__wjsm_new_target".to_string()));
    let nt_name_val = self.alloc_value();
    self.current_function.append_instruction(
        block,
        Instruction::Const { dest: nt_name_val, constant: nt_name },
    );
    let nt = self.alloc_value();
    self.current_function.append_instruction(
        block,
        Instruction::CallBuiltin {
            dest: Some(nt),
            builtin: Builtin::EvalGetBinding,
            args: vec![env, nt_name_val],
        },
    );
    return Ok(nt);
}
```

- [ ] **Step 3: Build check**

```bash
cargo check -p wjsm-semantic
```
Expected: no errors.

- [ ] **Step 4: Add fixture for new.target in eval**

`fixtures/happy/eval-new-target.js`:
```js
var result;
function Ctor() {
    result = eval('new.target');
}
new Ctor();
print(typeof result);
print(result === Ctor);
```

`fixtures/happy/eval-new-target.expected`:
```
exit_code: 0
--- stdout ---
function
true
--- stderr ---
```

- [ ] **Step 5: Commit**

```bash
git add crates/wjsm-semantic/src/lowerer_calls_eval.rs \
        crates/wjsm-semantic/src/lowerer_async_eval.rs \
        fixtures/happy/eval-new-target.js \
        fixtures/happy/eval-new-target.expected
git commit -m "feat(eval): pass new.target through ScopeRecord for eval code"
```

---

## Task 5: Fix TDZ writeback — skip uninitialized bindings

**Root cause:** In `lowerer_calls_eval.rs:785`, the writeback loop iterates over all `all_bindings` and unconditionally writes back. For TDZ bindings (still uninitialized), writing back an undefined value could incorrectly mark a binding as initialized.

**Spec reference:** ES §15.1.2 step 8.b.iii: "If val.[[Initialized]] is true, then set the variable with name n in the caller's lexical environment to val.[[Value]]." — Only initialized bindings get written back.

### Files

- Modify: `crates/wjsm-semantic/src/lowerer_calls_eval.rs` (writeback loop, ~line 780)

### Task Steps

- [ ] **Step 1: Filter writeback to only initialized bindings**

In the writeback loop in `lower_direct_eval_call`, change:

```rust
// BEFORE (writes back all bindings):
for (scope_id, name, _, _) in &all_bindings {
    // ... EvalGetBinding + StoreVar ...
}

// AFTER (only writes back initialized bindings):
for (scope_id, name, _, is_initialised) in &all_bindings {
    if !is_initialised {
        continue; // skip TDZ bindings — no writeback needed
    }
    // ... EvalGetBinding + StoreVar ...
}
```

- [ ] **Step 2: Build check**

```bash
cargo check -p wjsm-semantic
```
Expected: no errors.

- [ ] **Step 3: Add TDZ fixture inside eval**

`fixtures/happy/eval-tdz-let.js`:
```js
// Eval code reads a let-declared variable that is still in TDZ in the calling scope.
// EvalGetBinding should throw ReferenceError because initialized=false in ScopeRecord.
let x;
try {
    eval('var r = x;');
    print("no_error");
} catch (e) {
    print("tdz_error");
}
```

`fixtures/happy/eval-tdz-let.expected`:
```
exit_code: 0
--- stdout ---
tdz_error
--- stderr ---
```

The eval code's `x` read triggers `EvalGetBinding(scope_record, "x")` which checks the `initialized` flag and throws since `x` is in TDZ at the time of the eval call.

- [ ] **Step 4: Commit**

```bash
git add crates/wjsm-semantic/src/lowerer_calls_eval.rs \
        fixtures/happy/eval-tdz-let.js \
        fixtures/happy/eval-tdz-let.expected
git commit -m "fix(eval): skip TDZ bindings in writeback loop"
```

---

## Task 6: Fix eval_set_binding strict mode error

**Root cause:** `runtime_eval.rs` `eval_set_binding` handles the "binding not found in ScopeRecord" case. In strict mode, writing an undeclared variable should throw `ReferenceError`. In non-strict mode, it should create a global property. The current code has both paths but the non-strict `return val` at line ~1720 precedes the strict error check, shadowing it.

### Files

- Modify: `crates/wjsm-runtime/src/runtime_eval.rs` (eval_set_binding)

### Task Steps

- [ ] **Step 1: Fix control flow in eval_set_binding**

In `runtime_eval.rs`, find `eval_set_binding` (around line 1715). The function should check `is_strict` on the scope record before deciding whether to silently return or throw:

```rust
pub(crate) fn eval_set_binding(
    mut caller: Caller<'_, RuntimeState>,
    record: i64, name: i64, val: i64,
) -> i64 {
    let handle = value::decode_scope_record_handle(record);
    let name_str = /* decode name */;
    if let Some(rec) = caller.data().scope_records.get_mut(&handle) {
        // Check is_strict to determine undeclared-variable behavior
        let is_strict = rec.is_strict;
        for (n, v, init, is_const) in rec.bindings.iter_mut() {
            if n == &name_str {
                if *is_const && *init {
                    // TypeError: assignment to constant
                    // ... error handling ...
                }
                *v = val;
                *init = true;
                return val;
            }
        }
        // Binding not found in scope record
        if is_strict {
            // Strict: ReferenceError
            let msg = format!("assignment to undeclared variable '{}'", name_str);
            set_runtime_error(caller.data(), msg);
            // return TAG_EXCEPTION ...
        }
        // Non-strict: the semantic layer handles global property creation
        // via the existing scope bridge fallback
    }
    val
}
```

- [ ] **Step 2: Build check**

```bash
cargo check -p wjsm-runtime
```
Expected: no errors.

- [ ] **Step 3: Add strict eval undeclared write fixture**

`fixtures/errors/eval-strict-undeclared.js`:
```js
"use strict";
eval('x = 5');
```

Expected: ReferenceError.

- [ ] **Step 4: Commit**

```bash
git add crates/wjsm-runtime/src/runtime_eval.rs \
        fixtures/errors/eval-strict-undeclared.js \
        fixtures/errors/eval-strict-undeclared.expected
git commit -m "fix(runtime): eval_set_binding throws ReferenceError in strict mode"
```

---

## Task 7: Complete interpreted eval AST walker

**Root cause:** `eval_stmt` / `eval_stmt_async` in `runtime_eval.rs` only handles `Empty`, `Expr`, `VarDecl`, `FnDecl`, `Block`, `If`, `Throw`. Missing: `For`, `ForIn`, `ForOf`, `While`, `DoWhile`, `Switch`, `Try`, `Labeled`, `Break`, `Continue`, `Return`.

**Impact:** When compiled eval path fails (e.g., due to unsupported syntax), the interpreted fallback hits `"SyntaxError: unsupported eval statement"`. This causes spurious failures in test262 for tests with loops/try-catch inside eval strings.

### Files

- Modify: `crates/wjsm-runtime/src/runtime_eval.rs`

### Task Steps

- [ ] **Step 1: Implement For loop as first priority**

Add to `eval_stmt` in `crates/wjsm-runtime/src/runtime_eval.rs`:

```rust
swc_ast::Stmt::For(for_stmt) => {
    if let Some(init) = &for_stmt.init {
        match init {
            swc_ast::ForHead::VarDecl(var_decl) => {
                for declarator in &var_decl.decls {
                    if let Some(name) = pat_ident_name(&declarator.name) {
                        let value = if let Some(init_expr) = &declarator.init {
                            eval_expr(caller, init_expr, scope_env, eval_locals)?
                        } else {
                            value::encode_undefined()
                        };
                        match var_decl.kind {
                            swc_ast::VarDeclKind::Var if var_writes_to_scope =>
                                eval_write_binding(caller, scope_env, eval_locals, name, value)?,
                            swc_ast::VarDeclKind::Var =>
                                eval_declare_local(eval_locals, name, EvalLocalKind::Var, value)?,
                            swc_ast::VarDeclKind::Let =>
                                eval_declare_local(eval_locals, name, EvalLocalKind::Let, value)?,
                            swc_ast::VarDeclKind::Const =>
                                eval_declare_local(eval_locals, name, EvalLocalKind::Const, value)?,
                        }
                    }
                }
            }
            swc_ast::ForHead::Expr(expr) => {
                eval_expr(caller, expr, scope_env, eval_locals)?;
            }
        }
    }
    loop {
        if let Some(test) = &for_stmt.test {
            let test_val = eval_expr(caller, test, scope_env, eval_locals)?;
            if value::is_falsy(test_val) { break; }
        }
        if let Some(value) =
            eval_stmt(caller, &for_stmt.body, scope_env, var_writes_to_scope, eval_locals)?
        {
            completion = Some(value);
        }
        if let Some(update) = &for_stmt.update {
            eval_expr(caller, update, scope_env, eval_locals)?;
        }
    }
    Ok(completion)
}
```

- [ ] **Step 2: Implement ForIn loop**

For-in requires property enumeration on the object. Use the runtime's existing enumeration utilities:

```rust
swc_ast::Stmt::ForIn(for_in) => {
    let iterable = eval_expr(caller, &for_in.right, scope_env, eval_locals)?;
    let Some(ptr) = resolve_handle(caller, iterable) else {
        return Ok(None);
    };
    let keys = collect_object_keys(caller, ptr);
    for key in keys {
        match &for_in.left {
            swc_ast::ForHead::Pat(pat) => {
                if let Some(name) = pat_ident_name(pat) {
                    let key_val = store_runtime_string(caller, key.clone());
                    match var_writes_to_scope {
                        true => eval_write_binding(caller, scope_env, eval_locals, name, key_val)?,
                        false => eval_declare_local(eval_locals, name, EvalLocalKind::Var, key_val)?,
                    }
                }
            }
            _ => { /* skip complex LHS in interpreted path */ }
        }
        if let Some(value) =
            eval_stmt(caller, &for_in.body, scope_env, var_writes_to_scope, eval_locals)?
        {
            completion = Some(value);
        }
    }
    Ok(completion)
}
```

Check if `collect_object_keys` exists in `runtime_values.rs` or `runtime_heap.rs`. If not, add a simple helper that walks the object's property table and returns a Vec<String>.

- [ ] **Step 3: Implement ForOf loop**

For-of requires Symbol.iterator. Use the runtime's iteration helpers:

```rust
swc_ast::Stmt::ForOf(for_of) => {
    let iterable = eval_expr(caller, &for_of.right, scope_env, eval_locals)?;
    let Some(ptr) = resolve_handle(caller, iterable) else {
        return Ok(None);
    };
    // Get @@iterator method from the object
    let sym_iterator_val = value::encode_handle(value::TAG_SYMBOL, 0);
    let iterator_fn = read_object_property_by_symbol(caller, ptr, sym_iterator_val)
        .flatten()
        .unwrap_or(value::encode_undefined());
    if value::is_undefined(iterator_fn) {
        return Err("TypeError: iterable is not iterable".to_string());
    }
    let iterator = call_function(caller, iterator_fn, iterable, &[])?;
    loop {
        let Some(iter_ptr) = resolve_handle(caller, iterator) else { break; };
        let result = read_object_property_by_name(caller, iter_ptr, "next")
            .flatten()
            .unwrap_or(value::encode_undefined());
        let next_result = call_function(caller, result, iterator, &[])?;
        let Some(next_ptr) = resolve_handle(caller, next_result) else { break; };
        let done = read_object_property_by_name(caller, next_ptr, "done")
            .flatten()
            .map(|v| !value::is_falsy(v))
            .unwrap_or(true);
        if done { break; }
        let elem = read_object_property_by_name(caller, next_ptr, "value")
            .flatten()
            .unwrap_or(value::encode_undefined());
        match &for_of.left {
            swc_ast::ForHead::Pat(pat) => {
                if let Some(name) = pat_ident_name(pat) {
                    match var_writes_to_scope {
                        true => eval_write_binding(caller, scope_env, eval_locals, name, elem)?,
                        false => eval_declare_local(eval_locals, name, EvalLocalKind::Var, elem)?,
                    }
                }
            }
            _ => {}
        }
        if let Some(value) =
            eval_stmt(caller, &for_of.body, scope_env, var_writes_to_scope, eval_locals)?
        {
            completion = Some(value);
        }
    }
    Ok(completion)
}
```

Note: ForOf depends on `call_function` helper existing in the runtime. If not available, defer ForOf implementation under a `// TODO: implement via native call path` comment and keep the `SyntaxError` fallback.

- [ ] **Step 4: Implement While/DoWhile**

```rust
swc_ast::Stmt::While(while_stmt) => {
    loop {
        let test = eval_expr(caller, &while_stmt.test, scope_env, eval_locals)?;
        if value::is_falsy(test) { break; }
        if let Some(value) = eval_stmt(caller, &while_stmt.body, scope_env, var_writes_to_scope, eval_locals)? {
            completion = Some(value);
        }
    }
    Ok(completion)
}
swc_ast::Stmt::DoWhile(dw) => {
    loop {
        if let Some(value) = eval_stmt(caller, &dw.body, scope_env, var_writes_to_scope, eval_locals)? {
            completion = Some(value);
        }
        let test = eval_expr(caller, &dw.test, scope_env, eval_locals)?;
        if value::is_falsy(test) { break; }
    }
    Ok(completion)
}
```

- [ ] **Step 5: Implement Switch**

Add to `eval_stmt`:

```rust
swc_ast::Stmt::Switch(switch_stmt) => {
    let discriminant = eval_expr(caller, &switch_stmt.discriminant, scope_env, eval_locals)?;
    let mut matched = false;
    let mut default_case: Option<&[swc_ast::Stmt]> = None;
    for case in &switch_stmt.cases {
        if case.test.is_none() {
            default_case = Some(&case.cons);
        } else if matched || {
            let test = eval_expr(caller, case.test.as_ref().unwrap(), scope_env, eval_locals)?;
            value::strict_eq(test, discriminant)
        } {
            matched = true;
            for stmt in &case.cons {
                if let Some(value) = eval_stmt(caller, stmt, scope_env, var_writes_to_scope, eval_locals)? {
                    completion = Some(value);
                }
            }
        }
    }
    if !matched {
        if let Some(stmts) = default_case {
            for stmt in stmts {
                if let Some(value) = eval_stmt(caller, stmt, scope_env, var_writes_to_scope, eval_locals)? {
                    completion = Some(value);
                }
            }
        }
    }
    Ok(completion)
}
```

- [ ] **Step 6: Implement Try/Catch/Finally**

```rust
swc_ast::Stmt::Try(try_stmt) => {
    let result = eval_block(caller, &try_stmt.block.stmts, scope_env, var_writes_to_scope, eval_locals);
    match result {
        Ok(value) => {
            if let Some(handler) = &try_stmt.handler {
                // Execute finally if present, skip catch
                if let Some(finally) = &try_stmt.finalizer {
                    eval_block(caller, &finally.stmts, scope_env, var_writes_to_scope, eval_locals)?;
                }
                Ok(value)
            } else if let Some(finally) = &try_stmt.finalizer {
                eval_block(caller, &finally.stmts, scope_env, var_writes_to_scope, eval_locals)?;
                Ok(value)
            } else {
                Ok(value)
            }
        }
        Err(err_msg) => {
            if let Some(handler) = &try_stmt.handler {
                let param_name = pat_ident_name(&handler.param).unwrap_or("err");
                // Create error object and bind to param
                let error_obj = create_runtime_error(caller, &err_msg);
                eval_declare_local(eval_locals, param_name, EvalLocalKind::Let, error_obj)?;
                eval_stmt(caller, &handler.body, scope_env, var_writes_to_scope, eval_locals)?;
                if let Some(finally) = &try_stmt.finalizer {
                    eval_block(caller, &finally.stmts, scope_env, var_writes_to_scope, eval_locals)?;
                }
                Ok(None)
            } else if let Some(finally) = &try_stmt.finalizer {
                eval_block(caller, &finally.stmts, scope_env, var_writes_to_scope, eval_locals)?;
                Err(err_msg)
            } else {
                Err(err_msg)
            }
        }
    }
}
```

- [ ] **Step 7: Implement Labeled/Break/Continue/Return**

For these, implement a simple label stack approach:

```rust
// In eval_stmt, add:
swc_ast::Stmt::Labeled(label_stmt) => {
    // Push label onto break/continue stack
    // Use a simple struct: (label_name: Option<String>, break_target: frame_index, continue_target: frame_index)
    // Then lower the body via eval_stmt, then pop the label
    let result = eval_stmt(caller, &label_stmt.body, scope_env, var_writes_to_scope, eval_locals)?;
    Ok(result)
}
swc_ast::Stmt::Break(break_stmt) => {
    let target = if let Some(label) = &break_stmt.label {
        // walk label stack to find matching label
        find_break_target(label.sym.as_ref())
    } else {
        // break from innermost loop/switch
        innermost_loop_target()
    };
    // Use Rust's ? with a custom error type containing target info,
    // or restructure to use explicit stack pop
    Err(BreakSignal(target))
}
swc_ast::Stmt::Continue(cont_stmt) => {
    // Similar to break but finds continue target
    Err(ContinueSignal(target))
}
swc_ast::Stmt::Return(ret_stmt) => {
    let value = if let Some(arg) = &ret_stmt.arg {
        eval_expr(caller, arg, scope_env, eval_locals)?
    } else {
        value::encode_undefined()
    };
    completion = Some(value);
    Ok(completion)
}
```

Note: This requires restructuring the eval loop to catch Break/Continue signals as a custom error variant. Simplest approach: define private error variants that are caught in the loop/switch dispatchers. If too invasive, defer Break/Continue/Labeled support and keep `SyntaxError` fallback.

- [ ] **Step 8: Build check and verify**

```bash
cargo check -p wjsm-runtime
cargo run -- run fixtures/happy/eval-for-loop.js
```

Add a fixture testing eval with a for loop:

`fixtures/happy/eval-for-loop.js`:
```js
var result = eval('var sum = 0; for (var i = 0; i < 5; i++) { sum += i; } sum;');
print(result);
```

Expected: `10`.

- [ ] **Step 9: Commit**

```bash
git add crates/wjsm-runtime/src/runtime_eval.rs fixtures/happy/eval-for-loop.js fixtures/happy/eval-for-loop.expected
git commit -m "feat(eval): complete interpreted eval AST walker with for/while/try/switch"
```

---

## Task 8: Propagate eval_caller_has_arguments in all class methods and JSX components

**Root cause:** `lowerer_classes_ts.rs` has **zero** assignments to `eval_caller_has_arguments` across all 10 `emit_arguments_init` call sites. Same gap in `lowerer_jsx_objects.rs` (2 call sites). Any class method or JSX component containing direct `eval()` will have incorrect `has_arguments_binding` metadata in the ScopeRecord.

Confirmed via grep: 0 matches for `eval_caller_has_arguments` in `lowerer_classes_ts.rs`. The 10 `emit_arguments_init` call sites are at lines: 175, 308, 526, 667, 758, 958, 1087, 1296, 1432, 1516.

### Files

- Modify: `crates/wjsm-semantic/src/lowerer_classes_ts.rs` (all 10 call sites)
- Modify: `crates/wjsm-semantic/src/lowerer_jsx_objects.rs` (lines 1079, 1198)

### Task Steps

- [ ] **Step 1: Add eval_caller_has_arguments after every emit_arguments_init in lowerer_classes_ts.rs**

At each of the 10 call sites, add after `emit_arguments_init(block)?`:

```rust
self.eval_caller_has_arguments = self.scopes.lookup("arguments").is_ok();
```

The 10 locations (line numbers from grep output):
- Line 175: class constructor — `let m_entry = self.emit_arguments_init(m_entry)?;`
- Line 308: class method — `StmtFlow::Open(self.emit_arguments_init(match inner_flow {`
- Line 526: class getter — `let m_entry = self.emit_arguments_init(m_entry)?;`
- Line 667: class setter — `let m_entry = self.emit_arguments_init(m_entry)?;`
- Line 758: static method — `let m_entry = self.emit_arguments_init(m_entry)?;`
- Line 958: static getter — `let m_entry = self.emit_arguments_init(m_entry)?;`
- Line 1087: static setter — `StmtFlow::Open(self.emit_arguments_init(match inner_flow {`
- Line 1296: private method — `let m_entry = self.emit_arguments_init(m_entry)?;`
- Line 1432: static private method — `let m_entry = self.emit_arguments_init(m_entry)?;`
- Line 1516: static private getter/setter — `let m_entry = self.emit_arguments_init(m_entry)?;`

For the two "inline" call sites (308, 1087), the pattern is:
```rust
inner_flow = StmtFlow::Open(self.emit_arguments_init(match inner_flow {
    StmtFlow::Open(b) => b,
    StmtFlow::Terminated => return Ok(()),
})?);
// AFTER this block, add:
self.eval_caller_has_arguments = self.scopes.lookup("arguments").is_ok();
```

- [ ] **Step 2: Add to lowerer_jsx_objects.rs**

Same change at lines 1079 and 1198.

- [ ] **Step 3: Build check**

```bash
cargo check -p wjsm-semantic
```
Expected: no errors.

- [ ] **Step 4: Commit**

```bash
git add crates/wjsm-semantic/src/lowerer_classes_ts.rs \
        crates/wjsm-semantic/src/lowerer_jsx_objects.rs
git commit -m "fix(semantic): propagate eval_caller_has_arguments in all class methods and JSX"
```

---

## Task 9: Integration verification

### Files

- Create: `fixtures/happy/eval-arguments-implicit.js` + `.expected`
- Create: `fixtures/happy/eval-loop.js` + `.expected` (if not created in Task 7)
- Create: `fixtures/happy/eval-switch.js` + `.expected`

### Task Steps

- [ ] **Step 1: Create eval-arguments-implicit fixture**

`fixtures/happy/eval-arguments-implicit.js`:
```js
(function(a) {
    // eval sees 'arguments' in the scope record
    var result = eval('arguments[0]');
    print(result);
})(42);
```

Expected: `42`.

- [ ] **Step 2: Create eval-switch fixture**

`fixtures/happy/eval-switch.js`:
```js
var result = eval('var x = 1; switch (x) { case 1: x = 10; break; default: x = 20; } x;');
print(result);
```

Expected: `10`.

- [ ] **Step 3: Run all existing and new fixtures**

```bash
cargo nextest run --workspace
```
Expected: all 470+ existing fixtures pass. New fixtures pass.

- [ ] **Step 4: Run test262 direct eval suite**

```bash
cargo run -p wjsm-test262 -- run --suite test/language/eval-code/direct --all --plain 2>&1 | tee /tmp/test262-eval-results.txt
```

Count pass/fail:
```bash
grep -c "PASS" /tmp/test262-eval-results.txt
grep -c "FAIL" /tmp/test262-eval-results.txt
```

Expected: pass count significantly higher than 29.

- [ ] **Step 5: Run test262 arguments tests**

```bash
cargo run -p wjsm-test262 -- run --suite test/language/arguments-object --all --plain 2>&1 | tee /tmp/test262-args-results.txt
```

- [ ] **Step 6: Update snapshots if needed**

```bash
WJSM_UPDATE_FIXTURES=1 cargo nextest run -E 'test(happy__eval_)'
```

- [ ] **Step 7: Commit**

```bash
git add fixtures/
git commit -m "test: add eval integration fixtures for arguments, loops, and switch"
```

---

## Task 10: Cleanup — remove old flat-object scope bridge

**Root cause:** The old eval scope bridge code (flat JS object with GetProp/SetProp) is dead code now that ScopeRecord is in use. Remove it to reduce maintenance surface.

### Files

- Modify: `crates/wjsm-runtime/src/runtime_eval.rs` (remove old `eval_read_binding`/`eval_write_binding` flat-object paths)
- Modify: `crates/wjsm-semantic/src/lowerer_assignments.rs` (remove dead paths)

### Task Steps

- [ ] **Step 1: Audit old flat-object paths**

In `runtime_eval.rs`, check if `eval_read_binding` (uses `read_object_property_by_name`) is still reachable. With ScopeRecord, this is the fallback for non-ScopeRecord eval. Since all eval now uses ScopeRecord, this may be dead code. Verify by checking if `eval_scope_record` is ever `false` in practice.

In `lowerer_assignments.rs`, check if the old flat-object scope bridge (GetProp/SetProp on a JS object) is still emitted. With `eval_scope_record = true` set in `lower_eval_module_with_scope`, the old path should be unreachable.

- [ ] **Step 2: Remove dead code or add comments marking it as legacy fallback**

If the old paths are confirmed dead, remove them. Otherwise, add `// LEGACY: flat object scope bridge, unreachable with ScopeRecord but kept as fallback` comments.

- [ ] **Step 3: Build and test**

```bash
cargo check -p wjsm-runtime -p wjsm-semantic
cargo nextest run --workspace
```
Expected: no regressions.

- [ ] **Step 4: Commit**

```bash
git add crates/wjsm-runtime/src/runtime_eval.rs crates/wjsm-semantic/src/lowerer_assignments.rs
git commit -m "refactor(eval): remove dead flat-object scope bridge code"
```

---

## Risks

1. **Class method call sites (Task 1/8):** `lowerer_classes_ts.rs` has 15+ `emit_arguments_init` call sites. Missing one causes `eval_caller_has_arguments` to be `false` in that method's eval calls. Mitigation: audit via grep for `emit_arguments_init` and ensure each call site is followed by the flag computation.

2. **Test262 cache staleness:** If `cached_eval_wasm` has stale entries from runs before this change, tests may produce inconsistent results. Mitigation: cache key includes `cache_version` which is already `1`.

3. **For-in/For-of in interpreted eval (Task 7):** These depend on runtime iteration helpers that may have complex host function interactions. Mitigation: start with simple for-in/for-of that delegates to the runtime's existing iteration logic; fall back to compiled path for complex cases.

4. **Symbol.iterator lookup (Task 3):** Getting `Array.prototype.values` from the runtime may be fragile if the prototype chain isn't fully set up. Mitigation: test with `[].values` fallback or use a hardcoded reference.

5. **callee derivation (Task 2):** Non-strict mapped arguments need `callee` property set to the enclosing function. Current plan passes `undefined` from semantic layer; runtime must derive callee from wasmtime Caller context. If Caller context doesn't expose the current function ref, callee will be `undefined` — a spec gap but not a blocker (callee is rarely accessed in practice).

## Expected test262 Impact

| Category | Tests (est.) | Before | After | Mechanism |
|---|---|---|---|---|
| arguments binding in eval | ~144 | ~10 | ~130 | Tasks 1, 2, 8 |
| TDZ (let/const/class) | ~7 | ~2 | ~7 | Tasks 5 |
| super keyword | ~10 | ~2 | ~8 | (already working, Task 4 helps) |
| new.target | ~3 | ~0 | ~3 | Task 4 |
| var/function hoisting | ~6 | ~3 | ~5 | Tasks 1, 7 |
| loops/switch in eval | ~12 | ~2 | ~10 | Task 7 |
| try/catch in eval | ~4 | ~1 | ~3 | Task 7 |
| **Total** | **~186** | **~18** | **~166** | — |
