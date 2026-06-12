# Evidence (2026-06-08)

## Current verification (2026-06-08, worktree `.worktrees/arguments`)

| Command | Result |
|---|---|
| `cargo build -p wjsm` | OK; pre-existing warnings only |
| `cargo nextest run --workspace` | **822/822** |
| `cargo nextest run -E 'test(json_parse) \| … \| test(error_subclass)'` | **17/17** |
| `cargo nextest run -p wjsm-semantic` | **102/102** |




## Implemented behavior evidence

- `crates/wjsm-semantic/src/lowerer_declarations.rs`: `emit_arguments_init` now passes real parameter count and dispatches `CreateMappedArgumentsObject` for non-strict ordinary functions, `CreateUnmappedArgumentsObject` otherwise.
- `crates/wjsm-runtime/src/runtime_arguments.rs`: mapped arguments object defines `length` and non-strict `callee`; `Symbol.iterator` is explicitly deferred because host property helpers currently accept string keys only.
- `crates/wjsm-runtime/src/runtime_eval.rs`: compiled eval fallback now restores output/runtime_error state and runs interpreted `eval_module_items` on runtime error or compile/runtime failure; interpreted throw creates TAG_EXCEPTION entries instead of writing stdout.
- `crates/wjsm-runtime/src/runtime_eval.rs`: interpreted eval supports for/for-in/while/do-while/switch/try, array literal/member access/update/compound assignment enough for the eval fixtures and direct eval fallback path.
- `crates/wjsm-semantic/src/lowerer_arrows.rs`: non-async arrow lowering updates `is_arrow_fn_stack`, restoring lexical `this` capture (`happy__arrow_this_capture`).
- `crates/wjsm-semantic/src/builtins.rs`: `gc` is a builtin global so test262 runner can preserve `$262 = { gc: gc }` without setup-time semantic failure.
- `crates/wjsm-runtime/tests/async_reentry_audit.rs`: sync `agent_receive_broadcast` callback path documented as an allowed alive sync path; async counterpart already uses `call_wasm_callback_async`.

## Fixture/snapshot evidence

Updated/covered fixtures include:
- `fixtures/happy/eval-for-loop.*`
- `fixtures/happy/eval-switch.*`
- `fixtures/happy/eval_compiled_pipeline.*`
- `fixtures/happy/eval_exception_expression_contexts.expected`
- `fixtures/happy/arguments-callee-strict.*`
- for-of known-broken trap snapshots adjusted for wasm function index drift.

- Workspace status after convergence slice: **822/822** integration tests; targeted plan filter **17/17**.
- **2026-06-08 convergence fixes**
  - `compiler_instructions.rs`: restored closure/function `call_indirect` emission (shared path after closure if/else); prior regression left `i64` on stack inside `BlockType::Empty` → mass WASM validation failures (function 406+).
  - `compiler_helpers.rs` `$obj_get`: raw `f64` → `primitive_number_get_method` host → `NativeCallable::NumberPrimitiveMethod` (`regexp_replace_named` callback `(a+b).toString()` → `8`).
  - Semantic/runtime slice unchanged: guarded `Number#toString` fast path; `Error#toString` not via broad `CallBuiltin`.

## Diff boundary review (convergence vs worktree drift)

| Scope | Paths | Notes |
|---|---|---|
| **Convergence (this plan)** | `compiler_instructions.rs` (call `call_indirect` layout), `compiler_helpers.rs` (`$obj_get` f64 + `object_proto_handle` store), `lowerer_calls_eval.rs` (Number fast path guard; drop Error `CallBuiltin`), `host_import_registry.rs` + `math_number_error.rs` + `runtime_builtins.rs` + `lib.rs` (`PrimitiveNumberGetMethod` / `NumberPrimitiveMethod`), `reentrant_async.rs` (replace async callback), `compiler_builtins.rs` (optional radix `undefined`), blessed fixtures listed in plan filter | Required for 822/822 + targeted 17 |
| **Worktree co-drifting (not convergence-only)** | `compiler_control.rs` (Switch terminator), `runtime_eval.rs`, `runtime_arguments.rs`, `atomics.rs`, `agent_cluster.rs`, eval/arguments fixtures & `.ir` snapshots | Separate feat/eval/arguments threads; do not revert if green |
| **Dead code / follow-up** | `compiler_number_proto.rs` unused `emit_*_in_obj_get`; `builtin_from_error_proto_method` unused in semantic | Optional cleanup PR |



- **feat/arguments review (2026-06-08, worktree `.worktrees/arguments`)**
  - `lowerer_stmt.rs`: member/opt-chain expression statements now branch on `is_exception` so strict `arguments.callee` getter throws reach `try/catch` (`happy__arguments_callee_strict` → stdout `throw`).
  - `lowerer_assignments.rs`: strict undeclared assignment guard uses `eval_scope_bridge_active()` so compiled eval uses `EvalSetBinding` instead of compile-time `ReferenceError`.
  - `runtime_arguments.rs` + `NativeCallable::ArgumentsStrictCalleeGetter`: unmapped arguments expose throwing `callee` accessor in strict mode.
  - `runtime_eval.rs`: compiled eval entry syncs `new_target` from scope record meta/bindings; cache version bumped; strict eval sets `scope_records[].is_strict`.
  - `for_of_break_closes` / `arguments-callee-strict` fixture snapshots updated; `cargo nextest run -p wjsm-semantic` 102/102 after IR refresh.
  - **2026-06-08 follow-up**: `lower_eval_module_with_scope` sets `strict_mode` from eval source; `perform_eval_from_caller` returns `TAG_EXCEPTION` on compiled eval `Ok`; `eval_get_binding` for `__wjsm_new_target` skips undefined binding and uses atomic `new_target`; cache v4. `errors__eval_strict_undeclared` + `happy__eval_new_target` nextest pass.
- Direct eval test262 target estimate was ~190/286; current observed result is 165/286. Remaining failures cluster around async test262 completion, function declaration instantiation/hoisting (`undeclared identifier f`), `Test262Error`/strict equality behavior, and other non-plan runtime gaps.
- `arguments-object` remains low at 57/263, mostly class/private/async/generator/trailing-comma suites outside the small strict-callee fixture tracked here.
- `Symbol.iterator` on arguments objects remains deferred until symbol-key host properties are available.
