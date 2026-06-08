# Evidence (2026-06-08)

## Current verification

| Command | Result |
|---|---|
| `cargo build -p wjsm` | OK; existing warnings only |
| `cargo nextest run --workspace` | **822/822 passed** |
| `cargo run -p wjsm-test262 -- run --suite test/language/eval-code/direct --all --plain` | **165/286 passed**, 121 failed, 57.69% |
| `cargo run -p wjsm-test262 -- run --suite test/language/arguments-object --all --plain` | **57/263 passed**, 206 failed, 21.67% |

Artifacts observed in session:
- workspace nextest: `artifact://104`
- test262 direct eval JSON/plain run: `artifact://100`, JSON written to `/tmp/test262-eval-results.json`
- test262 arguments run: `artifact://102`

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

`cargo nextest run --workspace` confirms all generated fixture tests and crate tests pass after these updates.

## Remaining known gaps

- Direct eval test262 target estimate was ~190/286; current observed result is 165/286. Remaining failures cluster around async test262 completion, function declaration instantiation/hoisting (`undeclared identifier f`), `Test262Error`/strict equality behavior, and other non-plan runtime gaps.
- `arguments-object` remains low at 57/263, mostly class/private/async/generator/trailing-comma suites outside the small strict-callee fixture tracked here.
- Strict `arguments.callee` remains KNOWN-BROKEN; fixture records current non-conformant behavior rather than claiming spec compliance.
- `Symbol.iterator` on arguments objects remains deferred until symbol-key host properties are available.
