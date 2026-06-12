# Known Bugs

## A2: call_sync_iter_and_wrap exception handling

**Status**: Partially fixed (no hang; sync `for await` on custom `@@iterator` still incorrect)  
**Severity**: P1  
**Date**: 2026-06-12

### Description

Attempting to fix `call_sync_iter_and_wrap` (async-from-sync iterator adapter) to properly reject promises when the underlying sync iterator's `next()` throws synchronously causes the test to hang indefinitely.

### Location

`crates/wjsm-runtime/src/runtime_builtins.rs:2197-2261`

### Attempted Fix

```rust
let raw_result = tokio::task::block_in_place(|| {
    tokio::runtime::Handle::current().block_on(async {
        call_iterator_method_async(caller, method_to_call, iterator, call_arg).await
    })
});

// Check if next()/return() threw synchronously (TAG_EXCEPTION)
if value::is_exception(raw_result) {
    let promise = alloc_promise_from_caller(caller, PromiseEntry::pending());
    let reason = exception_reason(caller, raw_result);
    settle_promise(
        caller.data(),
        promise,
        PromiseSettlement::Reject(reason),
    );
    return promise;
}
```

### Symptom

- Test hangs indefinitely (requires timeout to kill)
- Likely caused by async-from-sync iterator state machine entering infinite loop
- May be related to `advance_async_from_sync` done flag handling or promise chain recursion

### Investigation Required

1. Trace async-from-sync iterator state machine flow when exception is converted to rejected promise
2. Check if `advance_async_from_sync` correctly handles rejected promises in the iterator result chain
3. Verify promise settlement doesn't cause re-entry into the same code path
4. Consider if sync exception → rejected promise → await → throw path creates cycle

### Original Report Context

From `report.md` (now deleted):
- Report claimed: sync `next()` returning `TAG_EXCEPTION` is wrapped as `{done: true, value}` and resolved instead of rejected
- Expected: exception should be catchable in `for await...of` try/catch
- The fix logic appears correct but triggers a runtime hang

### Workaround

Currently skipped. Other async iterator exception paths (A3: async iterator next() throws, A4: spread iterator throws) have been fixed successfully using similar patterns.
