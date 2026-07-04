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

## P1 T1.5 evidence

- `cargo check -p wjsm-backend-wasm -p wjsm-runtime-support -p wjsm-runtime -p wjsm-cli` → passed。
- `cargo nextest run -p wjsm-runtime-support --features embedded` → 9 passed。
- `cargo nextest run -p wjsm-backend-wasm -E 'test(support_alloc_helpers_use_alloc_window_and_safepoint_poll)'` → 1 passed。
- `cargo nextest run -p wjsm-backend-wasm` → 55 passed。
- `cargo nextest run -p wjsm-runtime` → 133 passed, 2 skipped。
- `cargo nextest run -p wjsm-cli --no-tests warn` → 3 passed, 52 skipped。
- `cargo build --workspace` → passed。
- `grep` over `crates` for `support_module_layout_hash|wjsm_support_g1|wjsm_support_zgc|EMBEDDED_G1|EMBEDDED_ZGC|emit_support_module\(\)|OnceLock<regex` → no matches。
- Rule compliance fix during slice: runtime support default artifact uses `LazyLock` for fixed initializer and keeps `OnceLock` only for explicit runtime injection; CLI IR regex caches switched from `OnceLock::get_or_init` to `LazyLock`。
- Variant boundary: `wjsm_backend_wasm::GcFlavor` now names MarkSweep/G1/Zgc, but only MarkSweep emits a support module in T1.5；G1/Zgc return an error and runtime-support exposes no fake `wjsm_support_g1/zgc.cwasm` artifacts until their later phases。
- Artifact coverage: build.rs precompiles only `wjsm_support_mark_sweep.cwasm`；embedded tests deserialize mark-sweep and assert G1/Zgc artifacts are absent。

## P1 T1.6 evidence

- `cargo check -p wjsm-runtime -p wjsm-backend-wasm -p wjsm-runtime-support -p wjsm-cli` → passed。
- `cargo nextest run -E 'test(happy__typedarray_simple) | test(happy__map_set_for_each) | test(happy__error_constructor_new_target) | test(happy__symbol_prototype_methods)'` → 4 passed。
- `WJSM_STARTUP_SNAPSHOT=0 cargo nextest run -E 'test(happy__error_constructor_new_target)'` → passed。
- `WJSM_STARTUP_SNAPSHOT=0 cargo nextest run -E 'test(happy__symbol_prototype_methods)'` → passed。
- `WJSM_STARTUP_SNAPSHOT=0 cargo nextest run -E 'test(happy__typedarray_simple)'` → passed。
- `cargo nextest run --workspace` → 1242 passed, 2 skipped。
- `WJSM_STARTUP_SNAPSHOT=0 cargo nextest run --workspace` → 1242 passed, 2 skipped。
- `cargo build --workspace` → passed。
- T1.6 修复证据：fixture 验证暴露 host 侧直接 bump `__heap_ptr` 后没有同步 `__alloc_ptr`，导致后续 WASM `arr_new` fast-path 覆盖 host 分配的 property/string 区域；已在 `alloc_heap_c_string_global`、render string allocation、eval var map allocation 同步 `__alloc_ptr`。
- T1.6 修复证据：support/user helper 的 `gc_safepoint_poll` 现在同时要求 `__bootstrap_done` 与 `__function_props_done`，避免 bootstrap/function-props 构造期在没有普通 IR spill 的路径触发 GC。
- T1.6 修复证据：cold startup 期在 GC attach 前没有可靠 roots，`gc_alloc_slow` 与 host allocation 在 `dynamic_heap_start == 0` 时改为 no-GC bump/grow，避免 bootstrap/host primordial 被过早 sweep/reuse。
- T1.6 修复证据：cold startup 在 host prototype 初始化前显式执行 `__wjsm_init_function_props`，避免 main 入口首次执行时把 `obj_table_count` 回退到 `function_props_base` 并覆盖 Error/Symbol prototypes。
- T1.6 修复证据：Error constructor 使用已有 receiver 时只在 receiver 当前原型仍是 `Object.prototype` 时补设对应 Error prototype，保留 `extends TypeError` / `Reflect.construct(..., newTarget)` 已建立的自定义 receiver prototype。
