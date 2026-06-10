# Evidence (final)

## Baseline

| Command / Ref | Result |
|---|---|
| Starting point | 826/835 passing |
| Final state | 829/835 passing |

## Verification Runs

| Command | Result |
|---|---|
| `cargo run -- run fixtures/happy/labeled.js` | 2 |
| `cargo run -- run fixtures/happy/labeled_break_continue.js` | 2 |
| `cargo run -- run fixtures/happy/for_of_nested_break_continue.js` | b/c/3 |
| `cargo nextest run --workspace` | 829/835 passed |
| `cargo nextest run -p wjsm-semantic` | 102/102 passed (snapshots updated) |

## Remaining Failures (pre-existing)

| Fixture | Status |
|---|---|
| errors/timer_non_function | Unchanged - WASM trap |
| happy/class_private_method | Unchanged - missing external access error |
| happy/eval_exception_expression_contexts | Unchanged - WASM trap |
| happy/eval-tdz-let | Unchanged - no TDZ error |
| happy/for_of_throw_close | Unchanged - WASM trap |
| happy/proxy_invariants | Unchanged - missing validations |

## Changes

| File | Change |
|---|---|
| `crates/wjsm-backend-wasm/src/compiler_control.rs` | +25 lines: new rule in compile_branch_body_with_context to inline loop update blocks (Jump-terminated, within loop, can reach header) |
| `fixtures/semantic/*.ir` | Updated snapshots to reflect current IR shape |

## Subagent Attempts (reverted)

All subagent changes to semantic, runtime, and proxy layers were reverted after causing regressions. The only surviving change is the backend loop update inline rule.

## Drift Notes

No unexpected drift. The fix is scoped to the backend structured control flow for loop body blocks. The remaining 6 failures are genuine architecture gaps requiring focused single-fixture work.
