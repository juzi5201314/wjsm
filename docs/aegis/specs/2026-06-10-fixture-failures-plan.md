# Implementation Plan: Fix 9 Failing Fixtures

**Date:** 2026-06-10  
**Parent:** 2026-06-10-fixture-failures-analysis.md, 2026-06-10-fixture-failures-approaches.md  
**Approach:** Sequential fixes in priority order (Option A)

---

## Plan Overview

Fix 9 failing fixtures across 7 categories in priority order. Each fix is independently tested and committed before proceeding to the next.

**Total estimated time:** 8-10 days  
**Deliverables:** 7 commits (one per category), all fixtures passing

---

## Phase 1: Labeled Statements

**Goal:** Fix `labeled.js` and `labeled_break_continue.js` — both output `0` instead of `2`

**Root cause:** Loop initialization executes increment before first iteration body

**Files to modify:**
- `crates/wjsm-backend-wasm/src/compiler_control.rs` (for loop codegen)

**Steps:**

1. **Investigate IR → WASM mapping**
   - Run `cargo run -- dump-ir fixtures/happy/labeled.js > /tmp/labeled.ir`
   - Run `cargo run -- dump-wat fixtures/happy/labeled.js > /tmp/labeled.wat`
   - Trace basic block execution order in WAT
   - Identify where bb3 (increment) is being called before bb2 (body)

2. **Read for loop codegen**
   - Open `crates/wjsm-backend-wasm/src/compiler_control.rs`
   - Find `compile_for_loop` or equivalent
   - Understand how init/condition/body/update blocks are wired

3. **Fix loop entry point**
   - Ensure first jump goes to condition check (bb1), not increment (bb3)
   - Verify break/continue targets are correct
   - For labeled loops, ensure label context is preserved

4. **Test fix**
   ```bash
   cargo run -- run fixtures/happy/labeled.js
   # Should output: 2
   cargo run -- run fixtures/happy/labeled_break_continue.js
   # Should output: 2
   ```

5. **Verify no regressions**
   ```bash
   cargo nextest run -E 'test(happy__for_)' -E 'test(happy__while)' -E 'test(happy__do_while)'
   # All should still pass
   ```

6. **Update expected if needed**
   - If output format changed (unlikely), bless with `WJSM_UPDATE_FIXTURES=1`
   - Otherwise, fixtures should now pass without update

7. **Commit**
   ```
   fix(backend): correct for loop initialization order for labeled loops
   
   Labeled break/continue were outputting 0 instead of 2 because the loop
   increment block was executing before the first iteration body. Fixed by
   ensuring the entry point jumps to the condition check, not the update block.
   
   Fixes: labeled.js, labeled_break_continue.js
   ```

**Acceptance:** Both labeled fixtures pass, all other loop fixtures still pass

---

## Phase 2: For-Of Iterator Cleanup

**Goal:** Fix `for_of_nested_break_continue.js` (early exit) and `for_of_throw_close.js` (crash on throw)

**Root cause:** `continue` in for-of does not properly re-enter iterator; `throw` does not call `iterator.return()`

**Files to modify:**
- `crates/wjsm-semantic/src/lowerer_stmt.rs` (for-of lowering, lines 561-680)
- `crates/wjsm-runtime/src/host_imports/*.rs` (iterator close implementation)

**Steps:**

1. **Read current for-of lowering**
   - Open `lowerer_stmt.rs:561-680` (`lower_for_of`)
   - Trace how `LabelContext` is set up
   - Verify `iterator_to_close` is being set
   - Check what happens on `continue` vs `break`

2. **Check continue behavior**
   - `continue` should:
     1. Call `IteratorNext(iterator)`
     2. Check done flag
     3. If not done, loop back to body
     4. If done, jump to exit
   - Currently: early exits without calling IteratorNext

3. **Fix continue target**
   - The `continue_target` in `LabelContext` should point to a block that:
     1. Calls builtin.IteratorNext
     2. Checks done
     3. Branches to body or exit
   - Currently: probably points to loop increment (wrong)

4. **Fix throw cleanup**
   - When exception is thrown in for-of body:
     1. Unwind should detect iterator_to_close in label_stack
     2. Call builtin.IteratorClose before propagating exception
   - Currently: exception propagates without cleanup (crash)

5. **Review IteratorClose builtin**
   - Ensure `builtin.IteratorClose` is implemented in runtime
   - Should call `iterator.return()` if it exists
   - Should swallow exceptions from `return()` method

6. **Test fix**
   ```bash
   cargo run -- run fixtures/happy/for_of_nested_break_continue.js
   # Should output: b\nc\n3
   
   cargo run -- run fixtures/happy/for_of_throw_close.js
   # Should output: 1\nboom\ntrue
   ```

7. **Verify no regressions**
   ```bash
   cargo nextest run -E 'test(happy__for_of)' -E 'test(happy__for_await)'
   # All should pass
   ```

8. **Commit**
   ```
   fix(semantic,runtime): implement proper iterator cleanup for for-of
   
   - continue in for-of now properly calls IteratorNext and loops
   - throw in for-of now calls iterator.return() before propagating exception
   - Added IteratorClose handling in exception unwind path
   
   Per ES §14.7.5.6 ForIn/OfBodyEvaluation and §7.4.6 IteratorClose.
   
   Fixes: for_of_nested_break_continue.js, for_of_throw_close.js
   ```

**Acceptance:** Both for-of fixtures pass, all iterator-related fixtures still pass

---

## Phase 3: Eval Exception Propagation

**Goal:** Fix `eval_exception_expression_contexts.js` — crashes instead of catching exceptions

**Root cause:** `eval()` in expression position does not propagate exceptions correctly

**Files to modify:**
- `crates/wjsm-backend-wasm/src/compiler_instructions.rs` (eval call codegen)
- Possibly `crates/wjsm-runtime/src/runtime_eval.rs` (eval implementation)

**Steps:**

1. **Trace eval call codegen**
   - Find where `CallBuiltin::Eval` is compiled to WASM
   - Check if exception flag is checked after call
   - Compare with other builtins that can throw

2. **Verify exception return convention**
   - Runtime functions return NaN-boxed values
   - Exception is encoded as TAG_EXCEPTION
   - Caller must check `is_exception` and branch to handler

3. **Fix eval call site**
   - After `call builtin.eval(...)`, emit:
     ```wasm
     local.get $result
     call $is_exception
     if
       br $exception_handler
     end
     ```
   - Ensure $exception_handler is the nearest try-catch

4. **Test fix**
   ```bash
   cargo run -- run fixtures/happy/eval_exception_expression_contexts.js
   # Should output: if\nseq\narg\nbinary\nnew\nnested
   ```

5. **Verify no regressions**
   ```bash
   cargo nextest run -E 'test(happy__eval)' -E 'test(errors__eval)'
   # All should pass
   ```

6. **Commit**
   ```
   fix(backend): propagate exceptions from eval in expression contexts
   
   eval() calls in expression position (e.g. `if (eval(...))`) were not
   checking the exception flag after return, causing crashes. Now properly
   branches to the exception handler if eval throws.
   
   Fixes: eval_exception_expression_contexts.js
   ```

**Acceptance:** eval_exception fixture passes, all other eval fixtures still pass

---

## Phase 4: Proxy Invariants

**Goal:** Fix `proxy_invariants.js` — missing PASS lines for validation

**Root cause:** Proxy constructor and trap operations don't validate invariants

**Files to modify:**
- `crates/wjsm-runtime/src/runtime_builtins.rs` or wherever Proxy is implemented
- Search for `Proxy` implementation:
  ```bash
  rg -t rust "fn.*proxy" crates/wjsm-runtime/src/
  ```

**Steps:**

1. **Find Proxy implementation**
   - Search for Proxy constructor: `rg "Proxy" crates/wjsm-runtime/src/`
   - May be in `runtime_builtins.rs` or a separate `proxy.rs` file

2. **Add constructor validation**
   - `Proxy(target, handler)` must check:
     - `target` is an object (not null, not primitive)
     - `handler` is an object (not null, not primitive)
   - If validation fails, throw `TypeError`

3. **Add revocation checks**
   - Revoked proxies have a flag or null handler
   - Every trap invocation must check revocation first
   - If revoked, throw `TypeError` immediately

4. **Add construct trap validation**
   - If construct trap returns non-object, throw `TypeError`
   - Per ES §10.5.14 [[Construct]] step 12

5. **Test fix**
   ```bash
   cargo run -- run fixtures/happy/proxy_invariants.js
   # Should output multiple PASS lines (not INFO lines)
   # Exit code: 2 (expected — final throw is intentional)
   ```

6. **Verify no regressions**
   ```bash
   cargo nextest run -E 'test(happy__proxy)'
   # All should pass
   ```

7. **Commit**
   ```
   fix(runtime): enforce Proxy invariants per ES spec
   
   - Proxy constructor now validates target and handler are objects
   - Revoked proxies now throw on all trap invocations
   - Construct trap result is validated (must be object)
   
   Per ES §28.2.1.1 Proxy(target, handler) and §10.5 Proxy Internal Methods.
   
   Fixes: proxy_invariants.js
   ```

**Acceptance:** proxy_invariants fixture passes (with expected exit code 2)

---

## Phase 5: Timer Validation

**Goal:** Fix `errors/timer_non_function.js` — crashes instead of handling non-function callback

**Root cause:** `setTimeout` doesn't validate callback is callable

**Files to modify:**
- `crates/wjsm-runtime/src/host_imports/timers.rs` or similar

**Steps:**

1. **Find setTimeout implementation**
   ```bash
   rg "setTimeout" crates/wjsm-runtime/src/
   ```

2. **Add callable check**
   - Before scheduling callback, check if it's a function
   - Use existing `is_callable` helper or implement:
     ```rust
     fn is_callable(value: i64) -> bool {
         let tag = (value >> 32) & 0x1F;
         tag == TAG_FUNCTION || tag == TAG_CLOSURE || tag == TAG_NATIVE_CALLABLE
     }
     ```
   - If not callable, return early (don't throw — spec allows silently ignoring)

3. **Test fix**
   ```bash
   cargo run -- run fixtures/errors/timer_non_function.js
   # Should output: start\nsync-throw: undefined\nno-sync-throw\nlog-caught-as-exception\nend
   # Exit code: 0
   ```

4. **Verify no regressions**
   ```bash
   cargo nextest run -E 'test(happy__timer)' -E 'test(happy__set_timeout)' -E 'test(happy__set_interval)'
   # All should pass (note: timer callbacks don't fire, that's expected)
   ```

5. **Commit**
   ```
   fix(runtime): validate setTimeout callback is callable
   
   setTimeout with non-function callback was causing crash when scheduler tried
   to invoke it. Now validates the callback is callable before scheduling.
   
   Fixes: errors/timer_non_function.js
   ```

**Acceptance:** timer_non_function fixture passes

---

## Phase 6: Eval TDZ

**Goal:** Fix `eval-tdz-let.js` — no output instead of "tdz_error"

**Root cause:** Eval doesn't check TDZ state of outer-scope let/const

**Files to modify:**
- `crates/wjsm-runtime/src/runtime_eval.rs` (eval variable resolution)
- `crates/wjsm-semantic/src/eval_scan.rs` (eval scope metadata)

**Steps:**

1. **Understand eval scope chain**
   - When eval runs, it has access to outer lexical scope
   - Must distinguish between:
     - Variable declared but uninitialized (TDZ) → throw ReferenceError
     - Variable declared and initialized → return value
     - Variable not declared → throw ReferenceError (different message)

2. **Pass TDZ state into eval**
   - When compiling eval code, metadata about outer scope must include:
     - Variable name
     - Scope depth
     - Initialized flag (TDZ bit)
   - Currently: probably only passes name + scope depth

3. **Add TDZ check in variable access**
   - When eval code reads outer variable:
     1. Look up in scope chain
     2. Check if initialized flag is true
     3. If false, throw `ReferenceError: Cannot access 'x' before initialization`
     4. If true, return value

4. **Test fix**
   ```bash
   cargo run -- run fixtures/happy/eval-tdz-let.js
   # Should output: tdz_error
   ```

5. **Verify no regressions**
   ```bash
   cargo nextest run -E 'test(happy__eval)' -E 'test(happy__tdz)'
   # All should pass
   ```

6. **Commit**
   ```
   fix(semantic,runtime): eval now respects TDZ of outer let/const
   
   eval() was not checking the TDZ state of outer-scope let/const bindings,
   allowing access before initialization. Now passes TDZ metadata through
   eval boundary and checks it on variable access.
   
   Per ES §9.4.5 GetBindingValue (throw if uninitialized).
   
   Fixes: eval-tdz-let.js
   ```

**Acceptance:** eval-tdz-let fixture passes, all TDZ fixtures still pass

---

## Phase 7: Class Private Methods (Optional)

**Goal:** Fix `class_private_method.js` — external private access doesn't throw

**Root cause:** External access to `#private` outside class is not rejected

**Files to modify:**
- `crates/wjsm-semantic/src/lowerer_classes_ts.rs` (class lowering)
- Possibly `crates/wjsm-parser` (but parser is external swc_core)

**Steps:**

1. **Assess feasibility**
   - If private identifier access is parsed by swc_core, we may not be able to reject it at parse time
   - If it reaches semantic layer, we can reject it there
   - Search for private identifier handling:
     ```bash
     rg "PrivateName" crates/wjsm-semantic/src/
     ```

2. **Add scope tracking**
   - When entering class body, push class scope onto stack
   - Record which private names are declared
   - When encountering `MemberExpr` with `PrivateName`:
     - Check if current scope is inside the declaring class
     - If not, error: "Private field '#hidden' must be declared in an enclosing class"

3. **Test fix**
   ```bash
   cargo run -- run fixtures/happy/class_private_method.js
   # Should output: secret\nfunction() { [native code] }\ndirect-private-access-error
   ```

4. **Alternative: Document limitation**
   - If fix is not feasible (requires parser changes), document it:
     - Add comment to fixture: "KNOWN LIMITATION: external private access not rejected"
     - Update `.expected` to match current behavior
     - File issue for future fix

5. **Commit** (if fix is feasible)
   ```
   fix(semantic): reject private field access outside declaring class
   
   External access to private class members (e.g. `obj.#method`) is now
   rejected at semantic analysis with a clear error message.
   
   Per ES §13.3.1.1 Static Semantics: Early Errors.
   
   Fixes: class_private_method.js
   ```

**Acceptance:** class_private_method fixture passes, OR limitation is documented and fixture expectation is updated

---

## Verification Checklist (Per Fix)

Before committing each fix:

- [ ] Failing fixture(s) now pass
- [ ] Run all fixtures in same category — no regressions
- [ ] Run full test suite — no new failures
- [ ] IR dump (if applicable) looks correct
- [ ] No compiler warnings introduced
- [ ] Commit message follows format

**Full suite verification command:**
```bash
cargo nextest run --workspace
```

**Expected result after all fixes:**
- 835 tests pass
- 0 tests fail (down from 9)

---

## Rollback Protocol

If a fix causes regressions:

1. **Identify scope:**
   ```bash
   git diff HEAD~1 --stat
   # See which files changed
   
   cargo nextest run --workspace | grep FAIL
   # See which tests broke
   ```

2. **Quick fix attempt:**
   - If obvious (typo, missed edge case), fix immediately and retest

3. **If not obvious:**
   ```bash
   git revert HEAD
   cargo nextest run --workspace
   # Confirm revert fixes regressions
   
   # Investigate offline, reapply when ready
   ```

4. **Never ship broken state:**
   - Each commit must pass its own verification checklist
   - Do not proceed to next fix if current fix breaks tests

---

## Success Criteria

**Phase complete when:**
1. All 9 failing fixtures pass
2. No regressions in existing fixtures (835 → 835 passing)
3. All 7 commits are in git history with clear messages
4. Documentation updated (if any limitations remain)

**Final verification:**
```bash
cargo nextest run --workspace
# Output: 835 tests, 0 failures

git log --oneline -7
# Shows 7 commits (one per fix category)
```

**Deliverable:** Clean git history with 7 atomic commits, each fixing 1-2 fixtures without breaking anything else.
