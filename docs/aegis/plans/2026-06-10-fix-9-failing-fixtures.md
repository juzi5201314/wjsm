# Implementation Plan: Fix 9 Failing Fixtures

**Date:** 2026-06-10  
**Status:** Revised after review
**Parent Specs:** 
- `docs/aegis/specs/2026-06-10-fixture-failures-analysis.md`
- `docs/aegis/specs/2026-06-10-fixture-failures-approaches.md`
- `docs/aegis/specs/2026-06-10-fixture-failures-plan.md`

---

## Goal

Fix 9 failing fixtures representing ECMAScript specification violations. All failures are spec-compliance bugs requiring fixes in semantic lowering, backend codegen, or runtime layers. Target: 835/835 tests passing (currently 826/835).

---

## Architecture

Linear fix sequence across three layers:
- **Semantic layer** (`wjsm-semantic`): AST lowering, scope analysis, control flow
- **Backend layer** (`wjsm-backend-wasm`): IR → WASM codegen
- **Runtime layer** (`wjsm-runtime`): WASM execution, host functions, builtins

Pipeline: `source → parser → semantic → backend → runtime`

---

## Tech Stack

- Rust 2024, Cargo workspace
- `swc_core` for parsing
- `wasm-encoder` for codegen
- `wasmtime` for execution
- Nextest for test execution

---

## Baseline/Authority Refs

- AGENTS.md: ECMAScript spec compliance requirement (lines 214-222)
- ES §14.13 Labelled Statements
- ES §14.7.5.6 ForIn/OfBodyEvaluation
- ES §7.4.6 IteratorClose
- ES §19.2.1.1 eval(x)
- ES §28.2.1.1 Proxy(target, handler)
- ES §10.5 Proxy Object Internal Methods

---

## Compatibility Boundary

- Internal fixes only — no public API changes
- Existing test coverage must not regress (826 passing tests stay passing)
- NaN-boxed value encoding unchanged
- IR instruction set unchanged
- WASM contract (imports/exports) unchanged

---

## Verification

Per fix:
```bash
# Verify specific fixture
cargo run -- run fixtures/happy/<name>.js

# Run category tests
cargo nextest run -E 'test(happy__<category>)'

# Full suite
cargo nextest run --workspace
```

---

## Plan Pressure Test

- **Owner / contract / retirement:** Internal bug fixes, no new owners, no retirement needed
- **Verification scope:** Per-fix fixture + category + full suite
- **Task executability:** Validation/runtime tasks are directly executable; control-flow and iterator tasks require IR/WAT/runtime trace before editing
- **Pressure result:** proceed after correcting per-task investigation steps below

---

## Plan-Time Complexity Check

- **Target files:** `compiler_control.rs`, `lowerer_stmt.rs` (1067 lines), `lowerer_branching.rs` (778 lines), `compiler_instructions.rs`, `lowerer_calls_eval.rs`, `runtime_eval.rs`, `proxy_reflect.rs`, `proxy_traps.rs`, `reentrant_async.rs`
- **Existing size / shape signals:** Large files but modular (submodules per feature); loop/iterator failures cross semantic lowering, backend structured codegen, and runtime iterator state
- **Owner fit:** Each fix targets existing owner after root cause confirmation; do not assume semantic changes when IR is already correct
- **Add-in-place risk:** Moderate for CFG/iterator/eval fixes, low for isolated validation fixes
- **Better file boundary:** None known yet; confirm with local trace before adding new abstraction
- **Recommendation:** edit-in-place

---

## Files

### Phase 1: Labeled Statements
- Modify: `crates/wjsm-backend-wasm/src/compiler_control.rs` (loop codegen)

### Phase 2: For-Of Iterator Cleanup
- Investigate: `crates/wjsm-semantic/src/lowerer_stmt.rs` (for-of lowering), `crates/wjsm-semantic/src/lowerer_branching.rs` (abrupt completion cleanup), `crates/wjsm-backend-wasm/src/compiler_control.rs` (structured loop/codegen), iterator runtime implementation
- Modify only the layer proven by IR/WAT/runtime trace; current `lower_for_of` already has a `next_block` continue target

### Phase 3: Eval Exception Propagation
- Modify after confirmation: `crates/wjsm-backend-wasm/src/compiler_instructions.rs` or `crates/wjsm-semantic/src/lowerer_calls_eval.rs` (eval exception path)

### Phase 4: Proxy Invariants
- Modify: `crates/wjsm-runtime/src/host_imports/proxy_reflect.rs`, `crates/wjsm-runtime/src/host_imports/proxy_traps.rs`, and related async proxy path if used

### Phase 5: Timer Validation
- Modify: `crates/wjsm-runtime/src/host_imports/reentrant_async.rs` (setTimeout registration)

### Phase 6: Eval TDZ
- Modify: `crates/wjsm-runtime/src/runtime_eval.rs` (TDZ check)
- Modify: `crates/wjsm-semantic/src/eval_scan.rs` (TDZ metadata)

### Phase 7: Class Private Methods (Optional)
- Modify: `crates/wjsm-semantic/src/lowerer_classes_ts.rs` (scope check)

---

## Tasks

### Task 1: Fix Labeled Statement Loop Initialization

**Files:**
- Read: `crates/wjsm-backend-wasm/src/compiler_control.rs`
- Modify: `crates/wjsm-backend-wasm/src/compiler_control.rs`
- Test: `fixtures/happy/labeled.js`, `fixtures/happy/labeled_break_continue.js`

**Why:** Labeled break/continue output 0 instead of 2 even though the current IR shows `init → condition → body → update → condition`. This points to backend structured-loop codegen or loop detection, not semantic for-loop lowering.

**Impact/Compatibility:** Backend-only likely change. IR should remain unchanged. Affects structured compilation of for-loop branches, labeled continue, and labeled break.

**Verification:**
```bash
cargo run -- run fixtures/happy/labeled.js          # should output: 2
cargo run -- run fixtures/happy/labeled_break_continue.js  # should output: 2
cargo nextest run -E 'test(happy__for_) or test(happy__while) or test(happy__do_while)'
cargo nextest run --workspace
```

**Steps:**

- [ ] **Write test** — Run failing fixtures to confirm bug
  ```bash
  cd ~/project/wjsm/.worktrees/arguments
  cargo run -- run fixtures/happy/labeled.js
  # Actual output: 0
  # Expected output: 2
  ```

- [ ] **Verify RED** — Confirmed outputs 0 instead of 2

- [ ] **Investigate IR and WASM**
  ```bash
  cargo run -- dump-ir fixtures/happy/labeled.js
  cargo run -- dump-wat fixtures/happy/labeled.js
  # IR should show: bb0 init → bb1 condition → bb2 body, bb3 update → bb1.
  # If IR differs, fix semantic lowering first. If IR matches, trace backend structured codegen.
  ```

- [ ] **Minimal code** — Trace backend loop compilation in `compiler_control.rs`
  Read `compile_structured`, `compile_branch_body`, `detect_loops`, `loop_continue_depth`, and `loop_break_depth`. Do not assume the init terminator is wrong: current IR already jumps from init to the condition block. Fix the confirmed backend path that skips or misorders the body/update blocks.

- [ ] **Verify GREEN**
  ```bash
  cargo run -- run fixtures/happy/labeled.js          # output: 2
  cargo run -- run fixtures/happy/labeled_break_continue.js  # output: 2
  cargo nextest run -E 'test(happy__for_) or test(happy__while)'
  # All pass
  ```

- [ ] **Commit**
  ```bash
  git add crates/wjsm-backend-wasm/src/compiler_control.rs
  git commit -m "fix(backend): correct labeled for-loop control flow

Labeled break/continue were outputting 0 instead of 2 even though the IR
orders init, condition, body, and update correctly. Fixed the backend
structured loop codegen path that miscompiled labeled for-loop control flow.

Fixes: labeled.js, labeled_break_continue.js"
  ```

---

### Task 2: Fix For-Of Continue (Iterator Re-entry)

**Files:**
- Read: `crates/wjsm-semantic/src/lowerer_stmt.rs:600-718`
- Read: `crates/wjsm-semantic/src/lowerer_branching.rs:30-63`
- Modify after confirmation: likely `crates/wjsm-backend-wasm/src/compiler_control.rs` or iterator runtime; only modify `lowerer_stmt.rs` if IR disproves the current lowering
- Test: `fixtures/happy/for_of_nested_break_continue.js`

**Why:** `continue` in for-of exits after two iterations (`b\n2`) instead of completing all iterator steps (`b\nc\n3`). Current lowering already sets `continue_target: Some(next_block)`, and `next_block` calls `IteratorNext`, so the old "wrong continue_target" diagnosis is not valid.

**Impact/Compatibility:** Root cause unconfirmed. Candidate layers are backend structured loop codegen and runtime iterator state; semantic lowering must be changed only if IR proves it is wrong.

**Verification:**
```bash
cargo run -- run fixtures/happy/for_of_nested_break_continue.js  # should output: b\nc\n3
cargo nextest run -E 'test(happy__for_of)'
cargo nextest run --workspace
```

**Steps:**

- [ ] **Write test** — Run failing fixture
  ```bash
  cargo run -- run fixtures/happy/for_of_nested_break_continue.js
  # Actual: b\n2
  # Expected: b\nc\n3
  ```

- [ ] **Verify RED** — Confirmed early exit

- [ ] **Read for-of lowering**
  ```bash
  # Inspect lower_for_of and lower_continue.
  # Current code should show continue_target: Some(next_block), and next_block should call IteratorNext then jump to header.
  ```

- [ ] **Investigate IR/WAT/runtime state**
  ```bash
  cargo run -- dump-ir fixtures/happy/for_of_nested_break_continue.js
  cargo run -- dump-wat fixtures/happy/for_of_nested_break_continue.js
  # Confirm whether continue jumps to next_block and whether next_block advances exactly once per iteration.
  # If IR is correct, trace IteratorNext/IteratorDone state for string iterators and backend handling of next_block → header.
  ```

- [ ] **Minimal code** — Fix the confirmed layer
  Do not create a duplicate `iter_next` block unless the IR proves the current `next_block` wiring is absent or wrong. Current source already has the intended shape: `continue_target` points to `next_block`, `next_block` calls `IteratorNext`, and then jumps to `header`. Fix the actual cause of the premature done state or miscompiled jump.
- [ ] **Verify GREEN**
  ```bash
  cargo run -- run fixtures/happy/for_of_nested_break_continue.js  # output: b\nc\n3
  cargo nextest run -E 'test(happy__for_of)'
  # All pass
  ```

- [ ] **Commit**
  ```bash
  git add crates/wjsm-backend-wasm/src/compiler_control.rs crates/wjsm-semantic/src/lowerer_stmt.rs crates/wjsm-semantic/src/lowerer_branching.rs crates/wjsm-runtime/src/lib.rs crates/wjsm-runtime/src/host_imports/core.rs
  git commit -m "fix(iterator): for-of continue completes iterator iteration

continue in for-of was exiting early after two iterations. Fixed the
confirmed iterator re-entry bug without duplicating the existing next_block
lowering, which already calls IteratorNext before returning to the header.

Per ES §14.7.5.6 ForIn/OfBodyEvaluation.

Fixes: for_of_nested_break_continue.js"
  ```

---

### Task 3: Fix For-Of Throw (Iterator Cleanup)

**Files:**
- Read: `crates/wjsm-semantic/src/lowerer_branching.rs:472-503`
- Read: `crates/wjsm-semantic/src/lowerer_stmt.rs:600-718`
- Modify after confirmation: backend exception/codegen path, iterator runtime, or `lowerer_branching.rs` only if IR lacks cleanup
- Test: `fixtures/happy/for_of_throw_close.js`

**Why:** Exception in for-of body crashes instead of calling `iterator.return()`.

**Impact/Compatibility:** Exception handling path. Must preserve existing try-catch behavior.

**Verification:**
```bash
cargo run -- run fixtures/happy/for_of_throw_close.js  # should output: 1\nboom\ntrue
cargo nextest run -E 'test(happy__for_of) or test(happy__try_catch)'
cargo nextest run --workspace
```

**Steps:**

- [ ] **Write test** — Run failing fixture
  ```bash
  cargo run -- run fixtures/happy/for_of_throw_close.js
  # Actual: crash with wasm unreachable
  # Expected: 1\nboom\ntrue
  ```

- [ ] **Verify RED** — Confirmed crash

- [ ] **Read iterator cleanup logic and fixture IR**
  ```bash
  cargo run -- dump-ir fixtures/happy/for_of_throw_close.js
  # Current IR should contain call builtin.iterator.close(...) in the exception path before jumping to catch.
  # If the close call is present, do not add duplicate cleanup in lower_for_of; trace why runtime/codegen still traps.
  ```

- [ ] **Minimal code** — Fix confirmed exception/cleanup layer
  If IR lacks `IteratorClose`, fix `emit_throw_value` / cleanup depth. If IR already emits `IteratorClose`, inspect backend handling of the exception branch and runtime `IteratorClose`/object-iterator behavior. Preserve existing try-catch behavior and rethrow/catch ordering.

- [ ] **Verify GREEN**
  ```bash
  cargo run -- run fixtures/happy/for_of_throw_close.js  # output: 1\nboom\ntrue
  cargo nextest run -E 'test(happy__for_of)'
  # All pass
  ```

- [ ] **Commit**
  ```bash
  git add crates/wjsm-semantic/src/lowerer_branching.rs crates/wjsm-semantic/src/lowerer_stmt.rs crates/wjsm-backend-wasm/src/compiler_control.rs crates/wjsm-runtime/src/lib.rs crates/wjsm-runtime/src/host_imports/core.rs
  git commit -m "fix(iterator): close for-of iterators on thrown completion

for-of throw handling was trapping instead of reaching the catch block after
iterator cleanup. Fixed the confirmed cleanup/codegen/runtime path so
IteratorClose runs before propagating the thrown value.

Per ES §7.4.6 IteratorClose.

Fixes: for_of_throw_close.js"
  ```

---

### Task 4: Fix Eval Exception Propagation

**Files:**
- Read: `crates/wjsm-semantic/src/lowerer_calls_eval.rs`
- Read: `crates/wjsm-backend-wasm/src/compiler_instructions.rs`
- Modify after confirmation: semantic eval lowering if IR lacks an exception branch, or backend eval codegen if IR is correct but WAT/runtime traps
- Test: `fixtures/happy/eval_exception_expression_contexts.js`

**Why:** `eval()` in expression position crashes instead of propagating exceptions. Direct eval has custom lowering, so first verify whether the IR emits an `IsException` branch before assuming the backend is missing the check.

**Impact/Compatibility:** Eval exception path change. Must preserve existing exception handling and direct-eval scope behavior.

**Verification:**
```bash
cargo run -- run fixtures/happy/eval_exception_expression_contexts.js  # should output: if\nseq\narg\nbinary\nnew\nnested
cargo nextest run -E 'test(happy__eval) or test(errors__eval)'
cargo nextest run --workspace
```

**Steps:**

- [ ] **Write test** — Run failing fixture
  ```bash
  cargo run -- run fixtures/happy/eval_exception_expression_contexts.js
  # Actual: crash with wasm unreachable
  # Expected: if\nseq\narg\nbinary\nnew\nnested
  ```

- [ ] **Verify RED** — Confirmed crash

- [ ] **Find eval exception path**
  ```bash
  cargo run -- dump-ir fixtures/happy/eval_exception_expression_contexts.js
  # Inspect lowerer_calls_eval.rs and compiler_instructions.rs.
  # If eval expression IR lacks an IsException branch, fix semantic lowering.
  # If IR is correct but WAT traps, fix backend eval codegen.
  ```

- [ ] **Minimal code** — Add the missing exception branch in the confirmed layer
  If the semantic IR lacks a branch after direct eval, lower eval like other throwable calls: check `IsException`, unwrap with `ExceptionValue`, and route through `emit_throw_value`. If IR already contains that branch, make backend codegen preserve the check and branch to the active exception handler.

- [ ] **Verify GREEN**
  ```bash
  cargo run -- run fixtures/happy/eval_exception_expression_contexts.js
  # output: if\nseq\narg\nbinary\nnew\nnested
  cargo nextest run -E 'test(happy__eval)'
  # All pass
  ```

- [ ] **Commit**
  ```bash
  git add crates/wjsm-semantic/src/lowerer_calls_eval.rs crates/wjsm-backend-wasm/src/compiler_instructions.rs
  git commit -m "fix(eval): propagate exceptions from eval expression contexts

eval() calls in expression position (e.g. if (eval(...))) were not
reaching the active exception path, causing crashes. Now direct eval
routes thrown completions through the same exception propagation path.

Per ES §19.2.1.1 eval(x).

Fixes: eval_exception_expression_contexts.js"
  ```

---

### Task 5: Fix Proxy Invariants

**Files:**
- Read: `crates/wjsm-runtime/src/host_imports/proxy_reflect.rs`
- Read: `crates/wjsm-runtime/src/host_imports/proxy_traps.rs`
- Read if needed: `crates/wjsm-runtime/src/host_imports/proxy_reflect_async.rs`
- Modify: proxy constructor/trap implementation files confirmed by local search
- Test: `fixtures/happy/proxy_invariants.js`

**Why:** Proxy constructor and traps don't validate invariants.

**Impact/Compatibility:** Runtime validation only. No API changes.

**Verification:**
```bash
cargo run -- run fixtures/happy/proxy_invariants.js  # should show PASS lines, exit code 2
cargo nextest run -E 'test(happy__proxy)'
cargo nextest run --workspace
```

**Steps:**

- [ ] **Write test** — Run failing fixture
  ```bash
  cargo run -- run fixtures/happy/proxy_invariants.js
  # Actual: multiple INFO lines (checks skipped)
  # Expected: multiple PASS lines (checks enforced), exit code 2
  ```

- [ ] **Verify RED** — Confirmed missing validations

- [ ] **Find Proxy implementation**
  ```bash
  # Inspect proxy_reflect.rs, proxy_traps.rs, and proxy_reflect_async.rs.
  # Identify constructor, revocation, get/set/has, apply, and construct paths before editing.
  ```

- [ ] **Minimal code** — Add three validations
  
  **In Proxy constructor:**
  ```rust
  // Validate target is object
  let target_tag = (target >> 32) & 0x1F;
  if target_tag != TAG_OBJECT && target_tag != TAG_ARRAY && target_tag != TAG_FUNCTION {
      return encode_exception(create_type_error("Proxy target must be an object"));
  }
  
  // Validate handler is object
  let handler_tag = (handler >> 32) & 0x1F;
  if handler_tag != TAG_OBJECT {
      return encode_exception(create_type_error("Proxy handler must be an object"));
  }
  ```
  
  **In trap invocations (get/set/has/etc):**
  ```rust
  // Check if proxy is revoked (handler is null)
  if proxy_entry.handler == encode_null() {
      return encode_exception(create_type_error("Proxy has been revoked"));
  }
  ```
  
  **In construct trap:**
  ```rust
  // Validate result is object
  let result_tag = (result >> 32) & 0x1F;
  if result_tag != TAG_OBJECT && result_tag != TAG_ARRAY {
      return encode_exception(create_type_error("Proxy construct trap must return an object"));
  }
  ```

- [ ] **Verify GREEN**
  ```bash
  cargo run -- run fixtures/happy/proxy_invariants.js
  # output: multiple PASS lines, exit code 2 (expected final throw)
  cargo nextest run -E 'test(happy__proxy)'
  # All pass
  ```

- [ ] **Commit**
  ```bash
  git add crates/wjsm-runtime/src/host_imports/proxy_reflect.rs crates/wjsm-runtime/src/host_imports/proxy_traps.rs crates/wjsm-runtime/src/host_imports/proxy_reflect_async.rs
  git commit -m "fix(runtime): enforce Proxy invariants per ES spec

- Proxy constructor now validates target and handler are objects
- Revoked proxies now throw on all trap invocations
- Construct trap result is validated (must be object)

Per ES §28.2.1.1 Proxy(target, handler) and §10.5.14 [[Construct]].

Fixes: proxy_invariants.js"
  ```

---

### Task 6: Fix Timer Validation

**Files:**
- Read: `crates/wjsm-runtime/src/host_imports/reentrant_async.rs:235-260`
- Modify: `crates/wjsm-runtime/src/host_imports/reentrant_async.rs`
- Test: `fixtures/errors/timer_non_function.js`

**Why:** `setTimeout` with non-function callback crashes.

**Impact/Compatibility:** Runtime validation only. No API changes.

**Verification:**
```bash
cargo run -- run fixtures/errors/timer_non_function.js  # should output: start\nsync-throw: undefined\nno-sync-throw\nlog-caught-as-exception\nend
cargo nextest run -E 'test(happy__timer)'
cargo nextest run --workspace
```

**Steps:**

- [ ] **Write test** — Run failing fixture
  ```bash
  cargo run -- run fixtures/errors/timer_non_function.js
  # Actual: crash with wasm unreachable
  # Expected: start\nsync-throw: undefined\nno-sync-throw\nlog-caught-as-exception\nend
  ```

- [ ] **Verify RED** — Confirmed crash

- [ ] **Find setTimeout**
  ```bash
  # Inspect define_timers_arrays_async in reentrant_async.rs.
  # Current code schedules callback without checking value::is_callable(callback).
  ```

- [ ] **Minimal code** — Add callable check before scheduling
  ```rust
  if !value::is_callable(callback) {
      *caller.data().runtime_error.lock().expect("runtime error mutex") =
          Some("TypeError: setTimeout callback must be callable".to_string());
      return value::encode_undefined();
  }
  ```
  Follow existing runtime convention for non-callable callbacks: set a runtime error and return `undefined`; do not enqueue a timer entry. Verify whether the fixture expectation must stay as the current synchronous catch output or be blessed to the newly chosen spec/host behavior.

- [ ] **Verify GREEN**
  ```bash
  cargo run -- run fixtures/errors/timer_non_function.js
  # output: start\nsync-throw: undefined\nno-sync-throw\nlog-caught-as-exception\nend
  cargo nextest run -E 'test(happy__timer)'
  # All pass
  ```

- [ ] **Commit**
  ```bash
  git add crates/wjsm-runtime/src/host_imports/reentrant_async.rs
  git commit -m "fix(runtime): validate setTimeout callback is callable

setTimeout with non-function callback was causing crash when scheduler tried
to invoke it. Now validates the callback is callable before scheduling.

Fixes: errors/timer_non_function.js"
  ```

---

### Task 7: Fix Eval TDZ

**Files:**
- Read: `crates/wjsm-runtime/src/runtime_eval.rs`
- Read: `crates/wjsm-semantic/src/eval_scan.rs`
- Modify: `crates/wjsm-runtime/src/runtime_eval.rs`
- Modify: `crates/wjsm-semantic/src/eval_scan.rs`
- Test: `fixtures/happy/eval-tdz-let.js`

**Why:** Eval doesn't check TDZ state of outer-scope let/const.

**Impact/Compatibility:** Eval boundary change. Must preserve existing eval behavior.

**Verification:**
```bash
cargo run -- run fixtures/happy/eval-tdz-let.js  # should output: tdz_error
cargo nextest run -E 'test(happy__eval) or test(happy__tdz)'
cargo nextest run --workspace
```

**Steps:**

- [ ] **Write test** — Run failing fixture
  ```bash
  cargo run -- run fixtures/happy/eval-tdz-let.js
  # Actual: (no output)
  # Expected: tdz_error
  ```

- [ ] **Verify RED** — Confirmed no TDZ check

- [ ] **Read eval scope metadata**
  ```bash
  cat crates/wjsm-semantic/src/eval_scan.rs
  # Find where outer scope variables are recorded
  # Check: is TDZ state (initialized flag) passed?
  ```

- [ ] **Minimal code** — Pass TDZ metadata
  
  **In eval_scan.rs:**
  Add `initialized: bool` field to variable metadata structure.
  
  **In runtime_eval.rs:**
  When accessing outer variable:
  ```rust
  let var_meta = scope_metadata.get(var_name);
  if !var_meta.initialized {
      return encode_exception(create_reference_error(
          format!("Cannot access '{}' before initialization", var_name)
      ));
  }
  ```

- [ ] **Verify GREEN**
  ```bash
  cargo run -- run fixtures/happy/eval-tdz-let.js
  # output: tdz_error
  cargo nextest run -E 'test(happy__eval) or test(happy__tdz)'
  # All pass
  ```

- [ ] **Commit**
  ```bash
  git add crates/wjsm-semantic/src/eval_scan.rs crates/wjsm-runtime/src/runtime_eval.rs
  git commit -m "fix(semantic,runtime): eval now respects TDZ of outer let/const

eval() was not checking the TDZ state of outer-scope let/const bindings,
allowing access before initialization. Now passes TDZ metadata through
eval boundary and checks it on variable access.

Per ES §9.4.5 GetBindingValue (throw if uninitialized).

Fixes: eval-tdz-let.js"
  ```

---

### Task 8: Fix Class Private Methods (Optional — Can Defer)

**Files:**
- Read: `crates/wjsm-semantic/src/lowerer_classes_ts.rs`
- Modify: `crates/wjsm-semantic/src/lowerer_classes_ts.rs`
- Test: `fixtures/happy/class_private_method.js`

**Why:** External access to private methods doesn't throw.

**Impact/Compatibility:** Semantic validation. Low priority — can defer if time-constrained.

**Verification:**
```bash
cargo run -- run fixtures/happy/class_private_method.js  # should output: secret\nfunction() { [native code] }\ndirect-private-access-error
cargo nextest run -E 'test(happy__class)'
cargo nextest run --workspace
```

**Steps:**

- [ ] **Write test** — Run failing fixture
  ```bash
  cargo run -- run fixtures/happy/class_private_method.js
  # Actual: secret\nfunction() { [native code] }
  # Expected: secret\nfunction() { [native code] }\ndirect-private-access-error
  ```

- [ ] **Verify RED** — Confirmed missing error

- [ ] **Check feasibility**
  ```bash
  rg "PrivateName" crates/wjsm-semantic/src/
  # If PrivateName is handled, add scope check
  # If not handled (swc_core parses it), may need to defer
  ```

- [ ] **Minimal code** (if feasible) — Add scope check
  When encountering `MemberExpr` with `PrivateName`:
  ```rust
  if !self.is_inside_class_declaring(private_name) {
      return Err(self.error(
          private_name.span,
          format!("Private field '{}' must be declared in an enclosing class", private_name)
      ));
  }
  ```

- [ ] **Verify GREEN** (if implemented)
  ```bash
  cargo run -- run fixtures/happy/class_private_method.js
  # output: secret\nfunction() { [native code] }\ndirect-private-access-error
  cargo nextest run -E 'test(happy__class)'
  # All pass
  ```

- [ ] **Commit** (if implemented)
  ```bash
  git add crates/wjsm-semantic/src/lowerer_classes_ts.rs
  git commit -m "fix(semantic): reject private field access outside declaring class

External access to private class members (e.g. obj.#method) is now
rejected at semantic analysis with a clear error message.

Per ES §13.3.1.1 Static Semantics: Early Errors.

Fixes: class_private_method.js"
  ```
  
  **OR** defer explicitly if parser/semantic support is insufficient:
  ```bash
  # Do not bless the current non-compliant output in this plan.
  # Leave the fixture failing and record the parser/semantic blocker for a separate task.
  ```

---

## Risks

1. **Control flow changes** (Tasks 1-3) may affect other loop constructs
   - Mitigation: Run all loop/switch fixtures after each fix
   
2. **Exception handling changes** (Tasks 3-4) may break existing try-catch
   - Mitigation: Run all error-path and eval fixtures after each fix
   
3. **Iterator protocol changes** (Tasks 2-3) may affect destructuring/spread
   - Mitigation: Run all iterator-consuming fixtures after each fix

4. **Task 8 may be blocked by parser** (swc_core external)
   - Mitigation: Document limitation if not fixable at semantic layer

---

## Retirement

No retirement needed — these are bug fixes, not feature additions or deprecations.

---

## ADR Signal

No new ADR needed — fixes restore spec compliance for existing features. If future changes touch exception handling, control flow, or iterator protocol, reference this plan's verification strategy.

---

## Execution Strategy

**Recommended:** Subagent-driven execution
- Fresh subagent per task
- Review between tasks
- Fast iteration with clear boundaries

**Alternative:** Inline execution
- Batch execution with checkpoints
- Single session, manual verification between tasks

**Success criteria:** All 9 fixtures pass, no regressions (835/835 tests passing).
