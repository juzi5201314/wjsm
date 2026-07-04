# EvidenceBundleDraft

## P0 evidence

- `cargo check -p wjsm-runtime` → passed。
- `cargo nextest run -p wjsm-runtime` → 133 passed, 2 skipped。
- `cargo build --workspace` → passed。
- Commits:
  - `feat: T0.3 switch runtime to GC v2`
  - `feat: T0.5 add GC scheduler`
  - `feat: T0.6 finalize mark-sweep governance`

## P1 T1.1 evidence

- `cargo check -p wjsm-runtime -p wjsm-backend-wasm -p wjsm-snapshot-format` → passed。
- `cargo nextest run -p wjsm-snapshot-format` → passed。
- `cargo nextest run -p wjsm-runtime -E 'test(startup_snapshot)'` → passed。
- `cargo nextest run -p wjsm-runtime` → passed。
- `cargo nextest run -p wjsm-backend-wasm -E 'test(shadow_stack_heap_guard_layout_and_canary)'` → passed。
- `cargo nextest run -p wjsm-backend-wasm` → passed。
- `cargo build --workspace` → passed。
- Commit: `feat: T1.1 upgrade GC immortal boundary`。

## P1 T1.2 evidence

- `cargo check -p wjsm-runtime -p wjsm-backend-wasm -p wjsm-runtime-support` → passed。
- `cargo nextest run -p wjsm-backend-wasm` → passed。
- `cargo nextest run -p wjsm-runtime-support` → passed。
- `cargo nextest run -p wjsm-runtime` → passed。
- `cargo build --workspace` → passed。
- Commit: `feat: T1.2 add GC coordination globals`。

## P1 T1.3 evidence

- `cargo check -p wjsm-backend-wasm -p wjsm-runtime` → passed。
- `cargo nextest run -p wjsm-backend-wasm -E 'test(support_alloc_helpers_use_alloc_window_and_safepoint_poll)'` → passed。
- `cargo nextest run -p wjsm-backend-wasm` → 54 passed。
- `cargo nextest run -p wjsm-runtime` → 134 passed, 2 skipped。
- `cargo build --workspace` → passed。
- wasmparser proof in `crates/wjsm-backend-wasm/tests/gc_alloc_window.rs` checks support `obj_new`/`arr_new` bodies for `global.get 19` (`__alloc_ptr`), `global.get 20` (`__alloc_end`), `global.set 19`, `global.set 1` (`__heap_ptr` sync), `global.get/set 21` (`__gc_alloc_bytes`), `call gc_alloc_slow`, and absence of `call gc_maybe_collect`.
- Source confirmation: grep for backend calls to `gc_maybe_collect` now only finds the host import registry entry, not allocation helper callsites.
- Residual failure during slice: `fragmentation_churn_survivors_intact` initially panicked on `heap_type 0x02`; root cause was GC layout owner only treating OBJECT/ARRAY/ARGUMENTS as object-like, while runtime object tags PROMISE/CONTINUATION/ASYNC_GENERATOR share object header layout. Fixed in `runtime_gc/context.rs` and covered by unit test `gc_layout_treats_runtime_object_tags_as_object_like` plus runtime package run.

## P1 T1.4 evidence

- `cargo check -p wjsm-backend-wasm -p wjsm-runtime -p wjsm-runtime-support` → passed。
- `cargo nextest run -p wjsm-backend-wasm -E 'test(support_alloc_helpers_use_alloc_window_and_safepoint_poll)'` → passed。
- `cargo nextest run -p wjsm-backend-wasm -E 'test(host_imports_count_locked)'` → passed。
- `cargo nextest run -p wjsm-runtime -E 'test(fragmentation_churn_survivors_intact)'` → passed。
- `cargo nextest run -p wjsm-backend-wasm` → 54 passed。
- `cargo nextest run -p wjsm-runtime` → 133 passed, 2 skipped。
- `cargo nextest run -p wjsm-runtime-support` → 7 passed。
- `cargo build --workspace` → passed。
- `grep` over `crates` for `gc_maybe_collect|GcMaybeCollect|alloc_counter|gc_threshold|bump_alloc_counter|reset_alloc_counter|update_gc_threshold` → no matches。
- Runtime residual during slice: first T1.4 poll placement caused `fragmentation_churn_survivors_intact` out-of-bounds. Root cause was polling after `obj_new`/`arr_new` registered a fresh object but before the returned handle reached caller-visible roots. Fixed by polling at helper entry (debt from previous allocations) and before object resize allocation, not after fresh allocation.
