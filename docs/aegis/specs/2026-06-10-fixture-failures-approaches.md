# Fix Approaches & Trade-offs

**Date:** 2026-06-10  
**Parent:** 2026-06-10-fixture-failures-analysis.md

## Approach Options

### Option A: Sequential Fixes (Recommended)
Fix failures in priority order: labeled → for-of → eval-exception → proxy → timer → eval-tdz → class-private.

**Pros:**
- Lowest risk — each fix is isolated and tested independently
- Clear verification boundary after each fix
- Can pause/resume work between categories
- Easier to bisect regressions

**Cons:**
- Slower total completion time (7 sequential work items)
- May miss opportunities for shared code paths

**Estimated time:** 8-10 days

---

### Option B: Batch by Layer
Group fixes by codebase layer:
1. **Backend/codegen**: labeled, for-of iterator protocol
2. **Runtime**: eval-exception, timer validation, proxy invariants, eval-tdz
3. **Semantic**: class-private (parser boundary)

**Pros:**
- Minimizes context-switching between crates
- Shared test/build cycles within each batch
- Natural dependency ordering (backend before runtime)

**Cons:**
- Larger blast radius per batch — harder to isolate regressions
- Must complete entire batch before moving to next
- Runtime batch is heterogeneous (3 different subsystems)

**Estimated time:** 7-9 days

---

### Option C: Exception Handling First
Prioritize all exception-related failures (eval-expression, timer-crash, for-of-throw), then tackle control flow (labeled, for-of-continue), then validation (proxy, eval-tdz, class-private).

**Rationale:** Exception handling is cross-cutting — fixing it once may resolve multiple failures.

**Pros:**
- High leverage — exception propagation affects 3+ failures
- Addresses most critical safety issue (crashes)
- Cleaner verification (all exception tests pass together)

**Cons:**
- Exception handling spans multiple layers (codegen + runtime)
- May not actually share code paths (timer vs eval vs for-of are different exception sources)
- Leaves labeled statement bug unfixed longer (user-visible)

**Estimated time:** 8-10 days

---

## Recommendation: Option A (Sequential)

**Why:**
1. **Lowest risk:** Each fix is independently verifiable with its own fixture(s)
2. **Clear boundaries:** No ambiguity about when a fix is "done"
3. **Incremental progress:** Can ship partial fixes if needed
4. **Matches project discipline:** The "no PoC compromises" rule implies thorough, complete fixes — sequencing ensures each fix is fully vetted before moving on

**Trade-off accepted:** Slightly longer total time in exchange for confidence and stability.

---

## Fix Order Justification

### 1. Labeled Statements (Priority 1)
- **User-visible:** Affects basic loop control flow
- **High spec impact:** Core language feature (not edge case)
- **Isolated fix:** Likely codegen initialization order bug, no downstream dependencies
- **Quick win:** Should be straightforward once root cause is identified

### 2. For-Of Iterator Cleanup (Priority 2)
- **High spec impact:** Iterator protocol is fundamental
- **Affects multiple constructs:** for-of, destructuring, spread in future features
- **Complexity:** Requires understanding both semantic lowering and runtime iterator calls
- **Dependency:** Shares control flow logic with labeled statements — fixing labeled first may reveal insights

### 3. Eval Exception Propagation (Priority 3)
- **Safety-critical:** Crashes are unacceptable in production runtime
- **Medium complexity:** Exception handling is well-understood, just needs correct wiring
- **Independent:** Doesn't block other fixes

### 4. Proxy Invariants (Priority 4)
- **Low complexity:** Straightforward validation checks
- **Isolated:** Only affects Proxy constructor and trap invocations
- **Good momentum builder:** Easy win after complex fixes

### 5. Timer Validation (Priority 5)
- **Low complexity:** Single type check
- **Low spec impact:** Timers are host-provided, not core JS
- **Quick fix:** Can be completed in <1 hour

### 6. Eval TDZ (Priority 6)
- **Medium complexity:** Requires passing scope state through eval boundary
- **Lower priority:** Edge case (eval reading outer-scope let/const before initialization)
- **Dependency:** Best tackled after eval exception propagation is fixed

### 7. Class Private Methods (Priority 7)
- **Low priority:** Test fixture acknowledges it's documenting existing behavior, not a blocker
- **Ambiguous scope:** May require parser changes (external to wjsm)
- **Deferrable:** Can be dropped if other fixes take longer

---

## Verification Strategy Per Fix

Each fix follows this workflow:

1. **Read relevant code:** Understand current implementation before changing
2. **Write failing test:** Confirm fixture reproduces the issue
3. **Implement fix:** Minimal change to address root cause
4. **Verify fixture:** Failing fixture now passes
5. **Run category tests:** All fixtures in same category still pass
6. **Run full suite:** No regressions in unrelated tests
7. **Update `.expected`:** Only if behavior intentionally changes (should be rare)
8. **Commit with clear message:** `fix: labeled break/continue now work correctly`

**No exception:** Do not skip any verification step. Do not commit a fix that causes regressions.

---

## Risk Mitigation

### Control Flow Changes (Labeled, For-Of)
**Risk:** Breaking other loop constructs (while, do-while, for-in)

**Mitigation:**
- Review IR diff before and after change
- Run all loop-related fixtures (not just labeled)
- Check `switch` statement behavior (shares break/continue logic)

### Exception Handling Changes (Eval, Timer)
**Risk:** Breaking existing try-catch behavior

**Mitigation:**
- Run all error-path fixtures before and after
- Test nested try-catch (ensure propagation still works)
- Verify async exceptions still work (Promise rejection)

### Iterator Protocol Changes (For-Of)
**Risk:** Breaking destructuring, spread, or async iteration

**Mitigation:**
- Run all iterator-consuming fixtures (destructuring, spread, Array.from, etc.)
- Test both sync and async iterators
- Verify generator cleanup still works

---

## Scope Boundaries

**In scope:**
- Fixing the 9 identified failing fixtures
- Ensuring ECMAScript spec compliance for affected features
- Maintaining existing test coverage (no regressions)

**Out of scope:**
- Timer callbacks firing (architectural limitation — synchronous execution model)
- Class setter bypass (known limitation, documented)
- Class computed methods (known limitation, documented)
- Performance optimization (only correctness matters here)
- Refactoring unrelated code (unless blocking the fix)

**Explicit non-goals:**
- Do not refactor the entire control flow system "while we're here"
- Do not add new features (only fix broken ones)
- Do not change public API (these are internal bug fixes)

---

## Dependency Graph

```
Labeled statements (no dependencies)
  ↓
For-of iterator cleanup (may share control flow insights)
  ↓
Eval exception propagation (independent path)
  ↓
Proxy invariants (independent path)
  ↓
Timer validation (independent path)
  ↓
Eval TDZ (depends on eval infrastructure being stable)
  ↓
Class private methods (optional — can drop if time-constrained)
```

**Parallel work opportunity:** After completing Labeled + For-of, the remaining fixes are independent and could be parallelized if multiple contributors are available. However, for single-contributor work, sequential is safer.

---

## Rollback Plan

If a fix causes regressions:

1. **Identify scope:** Which tests broke? Same category or different?
2. **Attempt quick fix:** If regression is obvious (typo, missed case), fix it immediately
3. **If not obvious:** Revert the commit, investigate offline, re-apply when ready
4. **Do not ship partial fixes:** A fix that breaks other tests is not a fix

**Rule:** Every commit must leave the test suite in a passing state (modulo the remaining known failures).

---

## Alternative: Drop Low-Priority Fixes

If time is constrained, we can drop Priority 6-7 (Eval TDZ, Class Private Methods) and ship the first 5 fixes. This still addresses:
- All HIGH priority failures (labeled, for-of, eval-exception)
- All safety-critical failures (crashes)
- Most spec-impactful failures

**Trade-off:** Leaves 2 edge-case failures unfixed, but delivers 7/9 fixes with lower risk.

**Recommendation:** Only exercise this option if a deadline forces prioritization. Otherwise, complete all 9 fixes for full spec compliance.
