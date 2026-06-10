# Fixture Failure Analysis & Fix Plan

**Date:** 2026-06-10  
**Status:** Draft  
**Category:** Bug fixes & spec compliance

## Executive Summary

9 fixture tests are currently failing across 7 root cause categories. All failures represent ECMAScript specification non-compliance issues requiring fixes in semantic lowering, backend codegen, or runtime layers.

## Failure Inventory

### 1. Labeled Statements (2 fixtures) — HIGH PRIORITY

**Fixtures:** `labeled.js`, `labeled_break_continue.js`

**Symptom:** Both output `0` instead of expected `2`

**Test case:**
```javascript
let total = 0;
outer: for (let i = 0; i < 5; i = i + 1) {
  if (i === 1) continue outer;  // skip i=1
  if (i === 3) break outer;     // exit at i=3
  total = total + i;            // accumulates 0, 2
}
console.log(total);  // should be 2, actual: 0
```

**Root cause:** The IR generation is correct (verified via `dump-ir`). The problem is in **WASM codegen or runtime**. The loop increment block (bb3) is being executed before the loop body on first iteration, causing `i` to be 1 before the first body execution.

**Spec reference:** ES §14.13 Labelled Statements, §14.7.4 The for Statement

**Fix location:** `wjsm-backend-wasm/src/compiler_control.rs` — loop codegen initialization order

---

### 2. For-Of Iterator Cleanup (2 fixtures) — HIGH PRIORITY

**Fixtures:** `for_of_nested_break_continue.js`, `for_of_throw_close.js`

**Symptom:**
- `for_of_nested_break_continue`: outputs `b\n2` instead of `b\nc\n3` (loop exits early)
- `for_of_throw_close`: crashes with `wasm unreachable` instead of calling `iterator.return()`

**Test case (simplified):**
```javascript
for (let ch of "abc") {
  total++;
  if (total === 1) continue;  // skip first iteration's console.log
  console.log(ch);            // should print b, c
}
console.log(total);           // should be 3, actual: 2
```

**Root cause:** `continue` in for-of loop is not re-entering the iterator properly. The semantic layer emits `IteratorClose` for break/throw (verified in `lower_break`/`lower_continue` at lowerer_branching.rs:17, 52), but the runtime or codegen is not handling the re-entry correctly for `continue`.

For `for_of_throw_close`, the iterator's `return()` method is never being called when an exception is thrown inside the loop body.

**Spec reference:** ES §14.7.5.6 ForIn/OfBodyEvaluation, §7.4.6 IteratorClose

**Fix location:**
- `wjsm-semantic/src/lowerer_stmt.rs` — for-of lowering (lines 561-680)
- `wjsm-runtime` — iterator close runtime implementation

---

### 3. Eval TDZ Checking (1 fixture) — MEDIUM PRIORITY

**Fixtures:** `eval-tdz-let.js`

**Symptom:** No output instead of `"tdz_error"`

**Test case:**
```javascript
let x;  // x is in TDZ until this line completes
try {
    eval('var r = x;');  // should throw ReferenceError (x in TDZ)
    console.log("no_error");
} catch (e) {
    console.log("tdz_error");  // expected
}
```

**Root cause:** `eval()` is not checking the TDZ state of outer-scope `let`/`const` bindings. The eval'd code can read `x` before it's initialized.

**Spec reference:** ES §9.2.1.1 [[Environment]], §9.4.5 GetBindingValue (throw if uninitialized)

**Fix location:** `wjsm-runtime/src/runtime_eval.rs` — eval variable resolution must check TDZ

---

### 4. Eval Exception Propagation (1 fixture) — HIGH PRIORITY

**Fixtures:** `eval_exception_expression_contexts.js`

**Symptom:** Crashes with `wasm unreachable` instead of catching exceptions

**Test case:**
```javascript
try {
  if (eval("throw 'if'")) {
    marker("if");
  }
} catch (e) {
  console.log(e);  // should print "if"
}
```

**Root cause:** When `eval()` throws an exception in expression position (not statement position), the exception is not being propagated to the enclosing try-catch. The WASM execution hits an unreachable instruction instead.

**Spec reference:** ES §19.2.1.1 eval(x) — exceptions must propagate

**Fix location:** `wjsm-backend-wasm/src/compiler_instructions.rs` — eval call codegen must handle exceptions

---

### 5. Class Private Method Access (1 fixture) — LOW PRIORITY

**Fixtures:** `class_private_method.js`

**Symptom:** Missing `"direct-private-access-error"` line

**Test case:**
```javascript
class Secret {
  #hidden() { return "secret"; }
  reveal() { return this.#hidden(); }
}
const s = new Secret();
console.log(s.reveal());       // works: "secret"
console.log(s.#hidden);         // should throw SyntaxError or TypeError
// but currently prints "function() { [native code] }"
```

**Root cause:** External access to private methods via `obj.#method` should be a **SyntaxError** (caught at parse time per ES spec), but if it somehow reaches runtime, it should throw `TypeError`. Currently it succeeds and returns the function reference.

**Note:** The test fixture itself acknowledges this is documenting current behavior, not asserting correct behavior. The syntax `s.#hidden` outside the class is invalid per spec and should be rejected by the parser.

**Spec reference:** ES §13.3.1.1 Static Semantics: Early Errors — private identifier outside class

**Fix location:** `wjsm-parser` or `wjsm-semantic` — reject private identifier access outside class scope

---

### 6. Proxy Invariants (1 fixture) — MEDIUM PRIORITY

**Fixtures:** `proxy_invariants.js`

**Symptom:** Missing several PASS lines about invariant enforcement

**Test case:**
```javascript
new Proxy(null, {});          // should throw TypeError (target not object)
new Proxy(, null);          // should throw TypeError (handler not object)
let {proxy, revoke} = Proxy.revocable({}, {});
revoke();
proxy.foo;                    // should throw TypeError (revoked proxy)
```

**Root cause:** Proxy constructor and trap operations are not validating invariants defined in ES §10.5 (Proxy Object Internal Methods). Specifically:
- Constructor must validate target and handler are objects
- Revoked proxies must throw on any trap invocation
- Construct trap must return an object

**Spec reference:** ES §28.2.1.1 Proxy(target, handler), §10.5.14 [[Construct]]

**Fix location:** `wjsm-runtime/src/host_imports/proxy.rs` or wherever Proxy is implemented

---

### 7. Timer Validation (1 fixture) — MEDIUM PRIORITY

**Fixtures:** `errors/timer_non_function.js`

**Symptom:** Crashes with `wasm unreachable` instead of gracefully handling non-function callback

**Test case:**
```javascript
console.log("start");
try {
  setTimeout("not a function", 0);  // should not crash
} catch (e) {
  console.log("sync-throw:", e.message);
}
console.log("end");  // should reach here
```

**Root cause:** `setTimeout` does not validate that the callback is a function before scheduling. When the scheduler tries to invoke a string, it hits an unreachable instruction.

**Spec reference:** HTML spec §8.1.7.1 setTimeout(handler, timeout) — handler should be IsCallable check

**Fix location:** `wjsm-runtime/src/host_imports/timers.rs` — validate callback is callable

---

## Fix Priority Matrix

| Category | Severity | Spec Impact | Implementation Complexity | Fix Order |
|----------|----------|-------------|---------------------------|-----------|
| Labeled statements | High | High | Medium (codegen bug) | 1 |
| For-of iterator cleanup | High | High | High (protocol flow) | 2 |
| Eval exception propagation | High | High | Medium (exception handling) | 3 |
| Proxy invariants | Medium | Medium | Low (validation checks) | 4 |
| Timer validation | Medium | Low | Low (type check) | 5 |
| Eval TDZ | Medium | Medium | Medium (scope tracking) | 6 |
| Class private method | Low | Low | Low (parser/semantic) | 7 |

---

## Common Patterns Across Failures

1. **Exception handling gaps:** 3 failures involve exceptions not propagating correctly (eval expression contexts, timer non-function, for-of throw)
2. **Control flow edge cases:** 2 failures involve loop control flow (labeled, for-of continue)
3. **Validation missing:** 3 failures involve missing input validation (proxy, timer, eval TDZ)

---

## Implementation Approach

### Phase 1: Control Flow Fixes (Labeled + For-Of)
Both labeled statements and for-of iterator cleanup are control flow issues that likely share code paths. Fix together.

**Steps:**
1. Verify semantic IR is correct (done — IR looks correct)
2. Trace WASM codegen for loop initialization order
3. Fix loop entry point generation
4. Add iterator cleanup on continue (not just break/throw)
5. Test both labeled and for-of fixtures together

**Estimated complexity:** 2-3 days

---

### Phase 2: Exception Propagation (Eval Expressions)
Eval in expression contexts must propagate exceptions correctly.

**Steps:**
1. Review `eval` builtin call codegen
2. Ensure exception flag is checked after eval returns
3. Add branch to exception handler if flag is set
4. Verify with `eval_exception_expression_contexts` fixture

**Estimated complexity:** 1 day

---

### Phase 3: Validation Checks (Proxy, Timer, Eval TDZ)
Straightforward validation additions.

**Steps:**
1. Proxy: Add constructor validation (target/handler must be objects)
2. Proxy: Add revocation checks on trap invocations
3. Timer: Add IsCallable check before scheduling
4. Eval TDZ: Pass outer scope TDZ state to eval context

**Estimated complexity:** 1-2 days

---

### Phase 4: Class Private Methods (Low Priority)
Parser or semantic error for external private access.

**Steps:**
1. Add scope tracking for private identifiers in semantic layer
2. Reject private identifier access outside owning class
3. Or: reject at parse time (cleaner, but parser is external `swc_core`)

**Estimated complexity:** 1 day (or defer if parser-level fix is impractical)

---

## Non-Goals

- **Timer callbacks not firing:** Known architectural limitation (synchronous execution model). Requires async runtime refactor (out of scope).
- **Class setters bypassed:** Known limitation documented in fixtures (out of scope).
- **Computed methods undefined:** Known limitation (out of scope).

---

## Verification Strategy

For each fix:
1. Run the specific failing fixture(s)
2. Run all fixtures in the same category (happy/errors)
3. Run full test suite to catch regressions
4. Update `.expected` if behavior changes intentionally

Use `WJSM_UPDATE_FIXTURES=1 cargo nextest run` to bless new expectations only after confirming behavior is spec-correct.

---

## ADR Signal

This work touches:
- **Control flow generation** (WASM backend) — potential impact on other loop constructs
- **Exception handling** (backend + runtime) — must preserve existing try-catch behavior
- **Iterator protocol** (semantic + runtime) — affects all for-in/for-of loops
- **Eval isolation boundary** (runtime) — TDZ and exception propagation affect eval security model

No new public API changes. Internal fixes only.

---

## Appendix: Failure Details

### Test Output Comparison

#### Labeled (expected vs actual)
```
Expected: 2
Actual:   0
```

#### For-of nested (expected vs actual)
```
Expected: b\nc\n3
Actual:   b\n2
```

#### For-of throw close (expected vs actual)
```
Expected: 1\nboom\ntrue
Actual:   (crash: wasm trap: unreachable)
```

#### Eval TDZ (expected vs actual)
```
Expected: tdz_error
Actual:   (no output)
```

#### Eval exception contexts (expected vs actual)
```
Expected: if\nseq\narg\nbinary\nnew\nnested
Actual:   (crash: wasm trap: unreachable)
```

#### Class private method (expected vs actual)
```
Expected: secret\nfunction() { [native code] }\ndirect-private-access-error
Actual:   secret\nfunction() { [native code] }
```

#### Proxy invariants (expected vs actual)
```
Expected: Multiple PASS lines for validation
Actual:   Multiple INFO lines (checks skipped)
```

#### Timer non-function (expected vs actual)
```
Expected: start\nsync-throw: undefined\nno-sync-throw\nlog-caught-as-exception\nend
Actual:   (crash: wasm trap: unreachable)
```
