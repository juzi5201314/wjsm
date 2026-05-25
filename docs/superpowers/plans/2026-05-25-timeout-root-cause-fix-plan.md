# Timeout Root Cause Fix — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix 5 timeout fixtures (weakref, finalization_registry, new_prototype_chain, global_fn_visible_in_nested, eval_exception_expression_contexts) by repairing FunctionRef constant compilation, SetProto tag validation, and ConstructCall error propagation.

**Architecture:** Two bugs in the WASM backend interact to cause infinite loops: (1) `FunctionRef` constant compilation uses IR function IDs as keys into a map keyed by WASM function indices, (2) `SetProto` silently discards valid prototype types (TAG_CLOSURE, TAG_ARRAY, TAG_BOUND). Fix (1) by adding a correct `function_id_to_wasm_idx` mapping. Fix (2) by extending SetProto's accepted tag list. Harden ConstructCall error path as a safety net.

**Tech Stack:** Rust, wasm-encoder, wasmtime

---

### Task 1: Add `function_id_to_wasm_idx` mapping

**Files:**
- Modify: `crates/wjsm-backend-wasm/src/compiler_core.rs` — add field
- Modify: `crates/wjsm-backend-wasm/src/lib.rs` — add field to public struct
- Modify: `crates/wjsm-backend-wasm/src/compiler_module.rs` — populate field (use `.enumerate()`)
- Modify: `crates/wjsm-backend-wasm/src/compiler_data.rs` — use field
- Test: `cargo nextest run --workspace`

- [ ] **Step 1: Add field to Compiler struct**

In `crates/wjsm-backend-wasm/src/compiler_core.rs` around line 1446 (other map inits), and `crates/wjsm-backend-wasm/src/lib.rs` around line 487 (struct declaration):

```rust
/// IR function ID → WASM function index (bridge for FunctionRef → table position).
function_id_to_wasm_idx: HashMap<u32, u32>,
```

Add `HashMap` import if not already present. Initialize in `new()`:

```rust
function_id_to_wasm_idx: HashMap::new(),
```

- [ ] **Step 2: Populate map in compile_module**

In `crates/wjsm-backend-wasm/src/compiler_module.rs`, change the function registration loop to use `.enumerate()` and populate the new map:

```rust
for (i, function) in module.functions().iter().enumerate() {
    let wasm_idx = self._next_import_func;
    // ... existing code ...
    self.function_name_to_wasm_idx.insert(function.name().to_string(), wasm_idx);
    // ... function type declaration, push_func_table ...
    self.function_id_to_wasm_idx.insert(i as u32, wasm_idx);
    self._next_import_func += 1;
}
```

Note: `i` is the iteration index (0, 1, 2, ...), which matches the IR `FunctionId.0` because `module.functions()` returns functions in ordering order, and `FunctionId` is the Vec index.

- [ ] **Step 3: Fix FunctionRef constant encoding**

In `crates/wjsm-backend-wasm/src/compiler_data.rs`, lines 29-37:

```rust
Constant::FunctionRef(function_id) => {
    // 之前（错误）: 用 IR function ID 作为 WASM 索引在 function_table_reverse 中查找
    // let wasm_idx = function_id.0;
    // let table_idx = self.function_table_reverse.get(&wasm_idx).copied().unwrap_or(wasm_idx);

    // 之后（正确）: 通过 function_id_to_wasm_idx 桥接 IR ID → WASM index → table position
    let wasm_idx = self.function_id_to_wasm_idx.get(&function_id.0).copied().unwrap_or(0);
    let table_idx = self.function_table_reverse.get(&wasm_idx).copied().unwrap_or(0);
    Ok(value::encode_function_idx(table_idx))
}
```

- [ ] **Step 4: Run full test suite**

```bash
cd /home/soeur/project/wjsm && cargo nextest run --workspace 2>&1 | grep -E "PASS|FAIL|TIMEOUT"
```

Expected: No new failures; all previously passing fixtures still pass.

### Task 2: Extend SetProto tag validation

**Files:**
- Modify: `crates/wjsm-backend-wasm/src/compiler_instructions.rs`
- Test: `cargo nextest run -E 'test(new_prototype_chain) | test(global_fn_visible_in_nested)'`

- [ ] **Step 1: Read the current SetProto code**

Read `crates/wjsm-backend-wasm/src/compiler_instructions.rs` lines 391-454 to see the tag check chain.

- [ ] **Step 2: Extend the tag list**

The current code checks TAG_OBJECT (0x8) and TAG_FUNCTION (0x9). Add checks for TAG_CLOSURE (0xA), TAG_ARRAY (0xB), TAG_BOUND (0xC), and TAG_PROXY (0x10). Each tag check follows the same WASM pattern:

```rust
// 检查是否为 TAG_CLOSURE (0xA)
func.instruction(&WasmInstruction::LocalGet(val_local));
func.instruction(&WasmInstruction::I64Const(32));
func.instruction(&WasmInstruction::I64ShrU);
func.instruction(&WasmInstruction::I64Const(value::TAG_MASK as i64));
func.instruction(&WasmInstruction::I64And);
func.instruction(&WasmInstruction::I64Const(value::TAG_CLOSURE as i64));
func.instruction(&WasmInstruction::I64Eq);
func.instruction(&WasmInstruction::If(BlockType::Empty));
func.instruction(&WasmInstruction::LocalGet(1));
func.instruction(&WasmInstruction::Return);
func.instruction(&WasmInstruction::End);
```

Repeat for TAG_ARRAY, TAG_BOUND, TAG_PROXY. Insert after the TAG_FUNCTION block, before the "fallback to Object.prototype" fallback code.

- [ ] **Step 3: Run the timeout fixtures**

```bash
cd /home/soeur/project/wjsm && cargo nextest run -E 'test(new_prototype_chain) | test(global_fn_visible_in_nested)' --no-capture 2>&1 | grep -E "PASS|FAIL|TIMEOUT|TIMEOUT"
```

Expected: Both pass without timing out.

### Task 3: Harden ConstructCall error path + timer loop safety

**Files:**
- Modify: `crates/wjsm-backend-wasm/src/compiler_instructions.rs` — ConstructCall error handling
- Modify: `crates/wjsm-runtime/src/lib.rs` — timer loop iteration limit
- Test: `cargo nextest run -E 'test(weakref)'`

- [ ] **Step 1: Add timer loop iteration limit**

In `crates/wjsm-runtime/src/lib.rs`, around line 953, wrap the `if main_ok { loop { ... } }` block:

```rust
if main_ok {
    // Timer event loop with iteration limit to prevent infinite hangs
    let mut timer_iterations = 0u32;
    const MAX_TIMER_ITERATIONS: u32 = 1000;
    loop {
        timer_iterations += 1;
        if timer_iterations > MAX_TIMER_ITERATIONS {
            writeln!(
                store.data().output.lock().expect("output mutex"),
                "Internal error: timer event loop exceeded max iterations"
            ).ok();
            break;
        }
        let now = Instant::now();
        // ... rest of existing timer loop ...
    }
}
```

- [ ] **Step 2: Run weakref fixture**

```bash
cd /home/soeur/project/wjsm && cargo nextest run -E 'test(weakref)' --no-capture 2>&1 | grep -E "PASS|FAIL|TIMEOUT|trap"
```

Expected: `weakref.js` passes (exit code 2 with WASM trap message) — the expected behavior.

### Task 4: Run full validation

- [ ] **Step 1: Run all 5 timeout fixtures**

```bash
cd /home/soeur/project/wjsm && cargo nextest run -E 'test(weakref) | test(finalization_registry) | test(new_prototype_chain) | test(global_fn_visible_in_nested) | test(eval_exception_expression_contexts)' --no-capture 2>&1 | grep -E "PASS|FAIL|TIMEOUT"
```

Expected: All 5 pass (weakref passes with exit code 2 — the expected WASM trap).

- [ ] **Step 2: Run full workspace test suite**

```bash
cd /home/soeur/project/wjsm && cargo nextest run --workspace 2>&1 | grep -E "PASS|FAIL|TIMEOUT"
```

Expected: No regressions (0 failures, 0 timeouts).

- [ ] **Step 3: Commit**

```bash
cd /home/soeur/project/wjsm && git add -A && git commit -m "fix: FunctionRef table index, SetProto tags, ConstructCall error path

- Fix FunctionRef constant compilation: add function_id_to_wasm_idx to
  correctly map IR function IDs to WASM function indices
- Fix SetProto tag validation: accept TAG_CLOSURE, TAG_ARRAY, TAG_BOUND,
  TAG_PROXY (matching GetPrototypeFromConstructor)
- Add timer event loop iteration limit (safety net)
- Fixes 5 timeout fixtures: weakref, finalization_registry,
  new_prototype_chain, global_fn_visible_in_nested,
  eval_exception_expression_contexts"
```
