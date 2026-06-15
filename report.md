# Code Review Report — gc-framework branch uncommitted changes (CORRECTED)

**Original Reviewer**: AtomCode (automated)  
**Correction Reviewer**: Verified against live codebase + test runs  
**Date**: 2025-07-14 (corrected)  
**Scope**: 53 files changed, +1024 / −562 lines  
**Branch**: gc-framework @ 92fbc9e  
**Status**: 21 modified + 2 untracked files (+ report.md)

---

## Summary

This changeset implements **Layer 3 of the GC safepoint optimization strategy** (callee no-GC analysis) and applies a comprehensive `rustfmt`-style formatting pass across the entire codebase. The functional changes are concentrated in three areas:

1. **New `compiler_gc_analysis.rs` module** — Module-level static analysis that identifies IR functions which provably cannot trigger GC, allowing the backend to skip safepoint spills on Call instructions to those callees.
2. **IR + semantic plumbing** — `Function::known_callee_vars` field and `FunctionBuilder::record_known_callee` / `take_known_callee_vars` to propagate scope-qualified IR variable names → FunctionId mappings from the semantic layer to the backend.
3. **Backend conditional spill** — `compile_instruction` for `Instruction::Call` now checks `GcAnalysis::call_may_trigger_gc` before emitting safepoint spill prologue/epilogue; `SuperCall` and `ConstructCall` conservatively retain unconditional spill.

The remaining ~90% of the diff is mechanical formatting: indentation fixes (especially deeply nested `let`-chains in Rust 2024 `if let` guards), line-wrapping of long method chains, removal of blank lines / trailing whitespace, and `#[allow(...)]` annotations for newly-triggered clippy warnings.

---

## Findings

### Critical

None.

### Improvements

#### I1. `lowerer_stmt.rs`: Removed `_ => Err(...)` fallback in `Decl` match — verify exhaustiveness

**File**: `crates/wjsm-semantic/src/lowerer_stmt.rs` lines 11–20  
**Change**: The `_ => Err(self.error(stmt.span(), format!("unsupported declaration kind `{}`", decl_kind(decl))))` arm was removed from the `match decl` inside `lower_stmt`.

**Assessment**: The 8 `swc_ast::Decl` variants (Fn, Var, Class, TsInterface, TsTypeAlias, TsEnum, TsModule, Using) are all explicitly handled. `cargo check` produces no non-exhaustive-match warning. **This is safe and an improvement** — the previous fallback would silently produce a runtime error for any future SWC Decl variant instead of failing at compile time. The exhaustive match is the correct pattern per the project's "No PoC compromises" rule.

**Recommendation**: Approved. Consider adding a comment `// N.B.: exhaustive match — new swc_ast::Decl variants must be handled here` to guard against future SWC upgrades silently adding variants.

**Verification**: CONFIRMED. Diff shows exactly 7 lines removed (the `_ =>` arm + whitespace). Current code has 8 explicit arms matching all `swc_ast::Decl` variants.

#### I2. `compiler_gc_analysis.rs`: New module quality

**File**: `crates/wjsm-backend-wasm/src/compiler_gc_analysis.rs` (new, 269 lines)  
**Assessment**: This is well-structured and follows the project's conventions:

- **Conservative by default**: `builtin_may_trigger_gc` returns `true` for any unrecognized builtin; unknown callees are conservatively treated as may-GC; out-of-range `FunctionId` returns `true`.
- **Soundness argument**: Only single-assignment function declaration variables are entered into `known_callee_vars` (documented in code comments). This prevents stale mappings after reassignment.
- **Fixed-point iteration**: Correctly computes the transitive closure of may-GC through call edges (lines 171–199: monotone `while changed` loop over `may_gc` lattice).
- **Test coverage**: Two unit tests covering scalar and allocating builtins.

**Minor observations**:
- The `builtin_may_trigger_gc` whitelist is the complement of `builtin_returns_scalar` from `value_ty.rs`. This dual-list approach is correct but creates a maintenance risk — adding a new Builtin variant requires updating *both* lists. Currently, the `!matches!` pattern in `builtin_may_trigger_gc` acts as the "default true" catch-all, so new builtins are conservatively safe. However, if someone adds a new scalar builtin to `builtin_returns_scalar` but forgets `builtin_may_trigger_gc`, the spill optimization would be *over-conservative* (extra spill, not a soundness bug). This is acceptable per the "宁可多 spill，绝不漏 spill" principle.
- **Cross-reference verified**: The `builtin_may_trigger_gc` whitelist (lines 28–71) exactly matches the `builtin_returns_scalar` whitelist (value_ty.rs lines 127–183) in terms of Builtin variants listed. Both lists contain the same set of 50 variants.

**Recommendation**: Approved. Consider a code comment cross-referencing `builtin_returns_scalar` in `value_ty.rs` to aid future maintainers.

**Verification**: CONFIRMED with correction — file is 269 lines, not "~170 lines" as originally stated.

#### I3. `Function::known_callee_vars` — IR layer extension

**Files**: `crates/wjsm-ir/src/lib.rs`, `crates/wjsm-semantic/src/lib.rs`, `crates/wjsm-semantic/src/lowerer_function_decls.rs`  
**Assessment**: Clean addition. `known_callee_vars` is a `HashMap<String, FunctionId>` stored on both `Function` (IR) and `FunctionBuilder` (semantic). The transfer via `take_known_callee_vars()` at finalization is correct (avoids cloning). All four finalization sites (sync fn, async fn, async generator, module entry) correctly propagate the map.

**Safety of `store_function_decl_callee`**: The method is called from exactly three sites:
1. `lower_fn_decl` — sync `function` declaration (hoisted, non-reassignable)
2. Async generator declaration (hoisted, non-reassignable)
3. Async function declaration (hoisted, non-reassignable)

All three are **hoisted function declarations** which are semantically non-reassignable in JavaScript. `let`/`const` assignments of function expressions (e.g., `let f = () => {}`) go through `lower_var_decl` and **never** call `store_function_decl_callee`, so they never enter `known_callee_vars`. This matches the documented contract "仅对单次赋值的函数声明变量建映射".

**New parameter added**: `store_function_decl_callee` now takes a `callee_fn_id: wjsm_ir::FunctionId` parameter (was previously 4 params, now 5). This parameter is the `FunctionId` of the callee function, used to populate the `known_callee_vars` mapping.

**Recommendation**: Approved. The comment in `store_function_decl_callee` mentions "闭包变量也通过此路径记录" — this refers to async/async-generator wrappers (which are still function declarations), not `let`/`const` closures. The comment could be clarified to avoid confusion.

**Verification**: CONFIRMED. Three call sites verified in diff. The `callee_fn_id` parameter addition is a functional change the original report only implicitly referenced.

#### I4. `compiler_instructions.rs`: Conditional safepoint spill for Call

**File**: `crates/wjsm-backend-wasm/src/compiler_instructions.rs` lines 456–483  
**Assessment**: Correct implementation of Layer 3d. The `may_gc` check falls back to `true` when `current_function_id` or `gc_analysis` is `None`, which is the right conservative default. `SuperCall` and `ConstructCall` retain unconditional spill with added comments explaining why.

**Additional change**: Line 52 uses `.is_none_or(|t| *t == wjsm_ir::value_ty::ValueTy::Handle)` instead of the previous `.map_or(true, |t| *t == ValueTy::Handle)` — idiomatic Rust 1.82+.

**Recommendation**: Approved.

**Verification**: CONFIRMED. Diff verified: 29 insertions, 9 deletions. Line range 459–485 in original report is slightly imprecise; actual range is 456–483.

#### I5. `compiler_module.rs`: GcAnalysis computed at module level

**File**: `crates/wjsm-backend-wasm/src/compiler_module.rs` line 142–143  
**Assessment**: `self.gc_analysis = Some(GcAnalysis::analyze(module))` is computed once at the start of `compile_module`. This is correct — the analysis is module-scoped and doesn't change per-function.

**Minor**: The duplicate `// Pass 1:` comment on lines 145–146 should be cleaned up.

**Recommendation**: Approved with nitpick fix.

**Verification**: CONFIRMED. Diff shows exactly the addition of Pass 0 + the duplicate Pass 1 comment.

#### I6. Formatting changes — massive but mechanical

**Scope**: ~900 of the ~1024 added lines are formatting.  
**Assessment**: All formatting changes are consistent with `rustfmt` defaults for Rust 2024 edition. Key patterns:

- **`if let` chain indentation**: Rust 2024's `let`-chains require consistent 4-space indentation for the body block. The old code had over-indented bodies (8–12 spaces for nested guards). Fixed throughout (`lowerer_calls_eval.rs`, `lowerer_declarations.rs`, `cjs_transform_tests.rs`, `fixture_runner.rs`, `exec.rs`, etc.).
- **Method chain line-breaking**: Long `.lock().expect(...)` chains broken across multiple lines (extensively in `runtime_gc/roots.rs`, `runtime_async_fn.rs`, `streams_readable.rs`, `streams_fetch_body.rs`, `runtime_combinators.rs`, etc.).
- **`is_none_or` idiom**: `.map_or(true, |t| *t == ValueTy::Handle)` → `.is_none_or(|t| *t == ValueTy::Handle)` — idiomatic Rust 1.82+. Applied in `compiler_instructions.rs:52` and `compiler_module.rs:62`.
- **`Option` flatten**: `match dest { Some(d) => (*d, kind), None => return None }` → `((*dest)?, kind)` — cleaner. Verified in `value_ty.rs` (two sites: general case and Call/SuperCall) and `liveness.rs` (`(*dest)?` for `instr_dest`).
- **Type aliases**: `type BlockUse = HashMap<...>`, `type BlockDef = ...`, `type PhiSources = ...` in `liveness.rs` — reduces nested HashMap signature complexity.
- **Dead code annotations**: `#[allow(dead_code)]` on `stmt_kind`, `decl_kind`, `module_decl_kind` (in `ast_kinds.rs`), `check_mutable`, `FunctionBuilder::finish` (in `semantic/lib.rs`) — these are diagnostic helpers used in debug builds.
- **Clippy suppressions**: `#[allow(clippy::enum_variant_names)]` on `ErrorType` (`test262/read.rs`), `#[allow(clippy::too_many_arguments)]` on `merge_eval_completion_after_if` (`lowerer_stmt.rs`).
- **Import reordering**: Alphabetical reordering in several test files (e.g., `ir_dump.rs`, `liveness.rs`).
- **Blank line / trailing whitespace cleanup**: Throughout.
- **Test value change**: `liveness.rs` test changes `encode_f64(3.14)` → `encode_f64(3.15)` and `Constant::Number(3.14)` → `Constant::Number(3.15)` — likely to avoid confusion with π or to ensure tests use distinct constants. Cosmetic only.

**Recommendation**: Approved. These are all legitimate formatting improvements.

**Verification**: CONFIRMED. All patterns verified against actual diffs.

#### I7. `SwitchCaseRegion` struct removed

**File**: `crates/wjsm-backend-wasm/src/lib.rs`  
**Change**: Removed unused `SwitchCaseRegion` struct (had `_case_idx` and `_target_idx` fields, both prefixed with `_`).

**Assessment**: Dead code removal. Approved.

**Verification**: CONFIRMED. Diff shows 8 lines removed (struct definition + blank line).

#### I8. `gc_spill_stress.js` fixture — no `.expected` file

**File**: `fixtures/happy/gc_spill_stress.js` (new, untracked)  
**Assessment**: The fixture tests that multiple live handle locals are correctly spilled across an allocation safepoint. However, there's no corresponding `.expected` file, which means the E2E fixture runner will fail (or create one on first run with `WJSM_UPDATE_FIXTURES=1`).

**Recommendation**: Run `WJSM_UPDATE_FIXTURES=1 cargo nextest run -E 'test(happy__gc_spill_stress)'` to generate the `.expected` file, then commit it alongside the fixture.

**Verification**: CONFIRMED. `find fixtures/happy -name "gc_spill_stress*"` returns only `gc_spill_stress.js`, no `.expected` file.

#### I9. Pre-existing test failure: `fetch_http_first_read_resolves_before_end_of_body` — **CORRECTED**

**File**: `crates/wjsm-runtime/tests/fetch_http_streaming.rs`  
**Status**: **PASSING** (not failing as original report claimed)

**Correction**: The original report stated this test was "FAILING" with output `"done false\nlen 1\n"`. However, running the test twice in the current state both times produces **PASS**:

```
cargo test: 1 passed, 60 filtered out (5 suites, 0.69s)
```

The diff for this file is purely formatting (3 `assert!` calls reformatted to multi-line). No functional changes were made. The test may have been flaky previously but is **currently passing**.

**Recommendation**: No action needed. The test passes and the diff is cosmetic only.

---

### Omissions in Original Report

The following changes were present in the diff but **not covered** by the original report:

#### O1. `runtime_gc/roots.rs`: Major documentation addition (169 insertions)

**File**: `crates/wjsm-runtime/src/runtime_gc/roots.rs`  
**Change**: Added ~56 lines of comprehensive documentation covering:
- Shadow Stack layout and protocol (INV-SP invariant)
- Spill strategy (prologue/epilogue protocol)
- INV-C (Compiler Guarantee) and INV-NM (Non-moving) invariants
- Optimization strategy overview (Layer 1/2/3 cross-references)
- Dead Spill safety argument
- GC collection sequence (5-step process)

This is a **significant documentation improvement** that codifies the shadow stack contract between the compiler and GC runtime. The remaining 113 insertions are formatting (method chain line-breaking for `.lock().ok().and_then(...)` patterns and `match` arm field expansion).

**Recommendation**: This should have been called out as a positive contribution. Approved.

#### O2. `runtime_gc/mod.rs`: Safepoint optimization strategy documentation (65 lines)

The report's Summary section describes the three-layer strategy but does not explicitly note that the **documentation** for this strategy was added to `runtime_gc/mod.rs`. This 65-line doc comment (lines 10–73) is the canonical reference for the entire optimization strategy and cross-references all relevant files.

#### O3. `store_function_decl_callee` signature change

The report mentions the semantic plumbing but does not explicitly call out that `store_function_decl_callee` gained a new `callee_fn_id: wjsm_ir::FunctionId` parameter. This is the key integration point where the semantic layer communicates callee identity to the IR.

#### O4. `runtime_gc/api.rs` and `runtime_gc/context.rs` formatting

These files have minor formatting changes (6 insertions, 6 deletions) — function signature wrapping and `if/else` collapsing. Pure formatting, not functionally significant.

#### O5. `compiler_gc_analysis.rs` line count error

The original report states "~170 lines" for the new module. The actual file is **269 lines** (238 lines of implementation + 31 lines of tests).

---

## Nitpicks

| # | File | Issue |
|---|------|-------|
| N1 | `compiler_module.rs:145-146` | Duplicate `// Pass 1:` comment — remove one |
| N2 | `runtime_json.rs:1114-1115` | Stray blank-line change between `#[test]` and `fn test_parse_numbers` — cosmetic only |
| N3 | `runtime_gc/mod.rs:1` | Spec reference changed from `§6` to `§2` — **should verify this matches project spec**; not resolvable from diff alone |
| N4 | `wjsm-cli/src/lib.rs:246` | `source` field on `PipelineResult` is dead — `cargo check` confirms: `warning: field 'source' is never read`. Consider removing or marking `#[allow(dead_code)]` |
| N5 | `liveness.rs` tests | `encode_f64(3.14)` → `encode_f64(3.15)` — cosmetic test constant change, no functional impact |

---

## Test Results

| Suite | Result |
|-------|--------|
| `cargo check` (full workspace) | Pass (81 warnings, 0 errors) |
| `wjsm-ir` (25 tests) | All pass |
| `wjsm-backend-wasm` (15 tests) | All pass (including new `compiler_gc_analysis` tests) |
| `wjsm-semantic` (102 tests) | All pass |
| `wjsm-module` (127 tests) | All pass |
| `wjsm-runtime` | **All pass** (including `fetch_http_first_read_resolves_before_end_of_body`) |

---

## Conclusion

**Recommendation: Approved** (with minor suggestions)

The changeset is well-structured and the Layer 3 GC safepoint optimization is a meaningful performance improvement with a sound conservative design. The formatting cleanup is thorough and consistent. All functional changes are correct:

- `known_callee_vars` is only populated for hoisted function declarations (not `let`/`const` assignments), matching the documented safety contract.
- `GcAnalysis` uses conservative defaults throughout (unknown callee → may-GC, out-of-range FunctionId → may-GC).
- `compiler_instructions.rs` correctly gates safepoint spill on `call_may_trigger_gc` for `Call`, while preserving unconditional spill for `SuperCall`/`ConstructCall`.
- The `lowerer_stmt.rs` exhaustive match on `swc_ast::Decl` is correct (8/8 variants covered, `Decl` is not `#[non_exhaustive]`).
- The `builtin_may_trigger_gc` whitelist exactly mirrors `builtin_returns_scalar`, ensuring consistency.
- Shadow stack protocol is now thoroughly documented in `roots.rs`.

### Corrections to Original Report

| # | Original Claim | Correction |
|---|---------------|------------|
| E1 | `compiler_gc_analysis.rs` is "~170 lines" | Actually **269 lines** (238 impl + 31 tests) |
| E2 | `fetch_http_first_read_resolves_before_end_of_body` is FAILING | Test **PASSES** in current state (verified twice) |
| E3 | `compiler_instructions.rs` "lines 459–485" | Actual range is **456–483** |
| E4 | Major documentation additions to `roots.rs` (~56 lines) and `mod.rs` (65 lines) were not mentioned | These are significant positive contributions |
| E5 | `store_function_decl_callee` signature change not explicitly noted | New `callee_fn_id` parameter is a key integration point |

**Suggested follow-ups** (non-blocking):

| Priority | Item | Detail |
|----------|------|--------|
| Low | N1 | Remove duplicate `// Pass 1:` comment in `compiler_module.rs:145-146` |
| Low | I8 | Generate `.expected` for `gc_spill_stress.js` fixture |
| Low | I1 | Add exhaustiveness comment on `match decl` in `lowerer_stmt.rs` |
| Low | I3 | Clarify "闭包变量也通过此路径记录" comment — refers to async wrappers, not `let`/`const` closures |
| Low | I2 | Add cross-reference comment between `builtin_may_trigger_gc` and `builtin_returns_scalar` |
| Low | N4 | `PipelineResult::source` is dead code — remove or annotate |
| Low | N3 | Verify `§6` → `§2` spec reference change in `runtime_gc/mod.rs` is intentional |
