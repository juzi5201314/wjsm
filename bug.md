# Known Bugs

## A2: for-await GetIterator exceptions not catchable

**Status**: ✅ RESOLVED (2026-06-12)
**Severity**: P1

### Symptom (before fix)

`for await...of` over an iterable whose async/sync iterator acquisition throws
synchronously (`@@asyncIterator`/`@@iterator` non-callable, or the iterator method
returns a non-object / object without callable `next`) printed an unbounded run of
`undefined` and never ran the surrounding `try/catch` or the function's
`.then`/`.catch`. Failing fixtures: `errors/for_await_non_callable_sync_iterator`,
`errors/for_await_non_callable_async_iterator`.

### Root cause

Two layers, both off-spec:

1. **`async_iterator_from` fallthrough** (`crates/wjsm-runtime/src/lib.rs`) returned a
   *bare* `create_error_object(...)` (a normal `TAG_OBJECT` handle) instead of a
   catchable `TAG_EXCEPTION` when GetIterator(obj, async) failed.
2. **`iterator_next_async`** (`crates/wjsm-runtime/src/host_imports/core_async.rs`)
   did not recognise a `TAG_EXCEPTION` iterator handle: `decode_handle` indexed the
   iterator table with the error-table index, missed, and returned `undefined` — so
   the for-await loop spun on `{value: undefined, done: undefined}` and its
   continuation never completed.

The runtime's `GetMethod` (added in 096155f) *did* correctly return `TAG_EXCEPTION`
for the non-callable cases; the "symbol key reading" suspicion in that commit was a
misdiagnosis — the exception was produced but then silently consumed downstream.

### Fix

Route the `TAG_EXCEPTION` iterator handle through the for-await loop's **existing
suspend/resume rejection path** rather than inventing a new control-flow path:

- `async_iterator_from` fallthrough now returns `make_type_error_exception(...)`.
- `iterator_next_async` converts a `TAG_EXCEPTION` handle into a **rejected promise**
  at entry (same shape as the A3 sync-throw path). `await` of that promise hits
  `is_rejected` → `emit_throw_value`, which the loop's `try/catch` catches (else it
  rejects the async function's own promise — so `.then`/`.catch` chains complete).

Safe for sync `for...of`: sync `IteratorFrom` never yields `TAG_EXCEPTION` (always a
`TAG_ITERATOR`/`Error` handle), so the new guard never fires there.

### Rejected approach (important)

Adding a semantic-layer `IsException` fork after `AsyncIteratorFrom` in
`lower_for_await_of` (via `lower_value_exception_branch`) **regresses**
`for_await_sync_iter_throw` / `for_await_next_throws`. The fork's exception arm jumps
to the shared `catch_entry` from the **entry state-machine segment**, while the
in-loop rejection path reaches the same `catch_entry` from a **resume segment**; the
wasm relooper miscompiles this cross-segment edge (the catch body becomes unreachable
from the resume path). This is the hazard behind `expr_exception_fork_allowed()`
returning `false` inside async bodies — keep exception handling in the runtime/resume
path for for-await, not in an IR fork.

---

## C1: user-defined `main` collides with synthesized module entry → wasm validation fail

**Status**: ✅ RESOLVED (2026-06-12)
**Severity**: P1

### Symptom

A function **declaration** named `main` (async or sync) miscompiles:
```js
async function main(){ await Promise.resolve(1); } main();   // WASM-FAIL
function main(){ console.log("hi"); } main();                // WASM-FAIL
```
`run` reports `WASM validation failed ... type mismatch: expected i32, found i64`.
Renaming to anything else works (`notmain`, `run`, …); `const main = async()=>{}` also
works (only `function`-declaration `main` collides). Deterministic; identical wasm bytes.
Note `build` emits the (invalid) wasm without complaint — only `run`/`validate` catches it.

### Root cause

The module top-level entry is synthesized as an IR function literally named `"main"`
(`lowerer_core.rs:17,796`, `lib.rs:1205`, async wrapper `lowerer_async_eval.rs:962`). The
wasm backend identifies the entry purely by `function.name() == "main"`
(`compiler_module.rs:104,288,534,576`) to apply the entry calling-convention and export it
as `"main"`/`"__eval_entry"`. A user-declared `main` produces a second IR function with the
same name, so the backend treats it as the entry (i32 signature) while its JS call sites use
the normal i64 calling convention → signature/type mismatch.

### Fix (2026-06-12)

Synthesized module entry IR name is `$module_main` (`MODULE_ENTRY_IR_NAME` / `is_module_entry_ir_function()`). Wasm export remains `main` / `__eval_entry`. Fixtures: `fixtures/happy/user_main_async.js`, `user_main_sync.js`.



