# 统一异步执行模型 — 根治实施计划

## Goal

在 `wjsm-runtime` 中彻底消除 **async Store 上的同步 WASM re-entry**：凡是运行在 `Config::async_support(true)` + `epoch_deadline_async_yield_and_update` 之后、并可能触达 WASM 的路径，必须使用 Wasmtime async API（`instantiate_async` / `call_async` / `func_wrap_async`）。

本计划的完成状态不是“减少 panic 风险”，而是：

1. `wjsm-runtime` 的公共执行入口统一为 async-only。
2. CLI 作为同步命令行边界，显式创建 Tokio runtime 并 `block_on` async runtime API。
3. async Store 可达路径中不存在 `Func::call` / `Instance::new` / sync `call_wasm_callback` / sync `resolve_and_call` / sync microtask drain。
4. 仍使用 `Func::wrap` 的宿主函数必须是纯 Rust / 纯内存 / 纯状态操作，不做任何 WASM re-entry。
5. 新增行为回归测试和源码审计测试，防止以后再次把 sync re-entry 挂回 async Store。

## Architecture

### Re-grounded corrections

本计划替代同名旧计划中的范围判断。对照当前源码重新核实后，真实情况如下：

1. **`read_object_property_by_name_with_env` 不需要 async 化。**
   `runtime_values.rs:375-449` 与 `read_object_property_by_name_proto_walk_with_env` 只读取对象 slot 和原型链内存字段，不触发 getter，不触发 Proxy trap，不调用 `call_wasm_callback`。旧 spec 中“`read_object_property_by_name` 会触发 getter/Proxy，需级联 130+ `.await`”是错误权威，执行时必须修订 spec。

2. **`register_common_bridges` 当前没有 re-entry。**
   `lib.rs:82-400` 是简单桥接；`lib.rs:402-744` 的 4 个 `call_wasm_callback` 属于 `register_complex_bridges_sync`，async 路径已经有 `register_complex_bridges_async`。旧审查报告把这 4 个点归到 `register_common_bridges` 不准确。执行时仍要删除 sync complex 版本，避免源码审计残留。

3. **直接 `call_wasm_callback` 不是完整风险集合。**
   还必须覆盖：
   - `resolve_and_call` / `resolve_callable_and_call` → `func.call` + Proxy apply trap；
   - `func_apply_impl` → `resolve_and_call`；
   - `perform_eval_from_caller` → `Instance::new` + `entry.call`；
   - `drain_microtasks_from_caller` / `call_host_function_with_args` → `func.call`；
   - `resume_async_function` → `func.call`；
   - native callable dispatch 中的 `EvalIndirect` / `EvalFunction`。

4. **`queue_microtask` 本身不是 re-entry。**
   `misc.rs:21-33` 只 enqueue，安全；真正危险点是 `drain_microtasks` import（`misc.rs:35-41`）以及 post-main/timer microtask drain。

5. **`JSON.stringify` 当前没有 toJSON callback re-entry。**
   当前 `json_stringify` import 调 `runtime_json_stringify`，未执行 `toJSON` / replacer callback。真实 JSON re-entry 是：
   - `runtime_json.rs:645`：`json_parse_to_string` 对非字符串对象做 `toString` / `valueOf` callback；
   - `runtime_json.rs:785`：JSON.parse reviver callback。

### Target model

```text
wjsm-cli (sync CLI boundary)
  -> Tokio Runtime::block_on(...)
    -> wjsm_runtime::execute(...).await
      -> async Store only
      -> register_linker(...) registers:
         - Func::wrap for non-re-entry host fns only
         - func_wrap_async for all direct/indirect WASM re-entry fns
      -> instantiate_async
      -> main.call_async
      -> drain_microtasks_async / scheduler async callbacks
```

### Async-only public contract

`wjsm-runtime` after completion exposes:

```rust
pub async fn execute(wasm_bytes: &[u8]) -> anyhow::Result<()>;
pub async fn execute_with_writer<W: std::io::Write>(wasm_bytes: &[u8], writer: W) -> anyhow::Result<W>;
```

The old sync wrappers are deleted. `wjsm-cli` is the only sync bridge in this repository.

### Re-entry inventory to eliminate

| Class | Current concrete sites | Required owner after plan |
|---|---|---|
| Direct callback helper | `call_wasm_callback` in `array_object`, `typedarray_new_methods`, `proxy_reflect`, `misc`, `core`, `runtime_host_helpers`, `runtime_json`, `runtime_values`, `register_complex_bridges_sync` | `call_wasm_callback_async(...).await`; then delete sync helper |
| Function dispatch helper | `resolve_and_call`, `resolve_callable_and_call`, `func_apply_impl` in `runtime_values.rs` and callsites in host imports | async equivalents; all async host callbacks use async equivalents; delete sync re-entry helpers |
| Eval compiled WASM | `try_compiled_eval_from_caller`, `perform_eval_from_caller`, eval direct/indirect imports, native eval callables | async eval equivalents; eval imports and native callable async path use them |
| Microtask host callback | `drain_microtasks_from_caller`, `call_host_function_with_args`, `resume_async_function` | generic async microtask/resume path usable from `Store` and `Caller`; sync versions deleted after sync runtime removal |
| Proxy/Reflect helper dispatch | `reflect_get_impl_with_receiver`, `define_property_internal`, `proxy_or_target_*`, local Proxy/Reflect helpers | async helper variants or full async conversion of owning host callbacks |
| Top-level sync execution | `execute`, `execute_with_writer`, `register_linker`, `register_complex_bridges_sync`, sync timer loop | deleted or renamed from async equivalents |

## Tech Stack

- `wasmtime` async APIs: `Linker::func_wrap_async`, `Instance::new_async`, `TypedFunc::call_async`, `Func::call_async`.
- `tokio` runtime at CLI boundary only.
- Existing `wjsm-runtime` async scheduler (`scheduler.rs`, `drain_microtasks_async`, `call_host_function_with_args_async`) as the canonical post-main/timer owner.
- Rust 2024, default rustfmt.

## Baseline / Authority Refs

- Current plan file: `docs/aegis/plans/2026-06-02-unified-async-execution-model.md`.
- Design spec to repair during execution: `docs/aegis/specs/2026-06-02-unified-async-execution-model-design.md`.
- Current async scheduler authority to update during execution: `docs/async-scheduler.md`.
- Existing async primitives:
  - `call_wasm_callback_async` in `runtime_host_helpers.rs`.
  - `try_compiled_eval_from_caller_async` in `runtime_eval.rs`.
  - `resume_async_function_async` and `run_main_completion_block_async` in `runtime_async_fn.rs`.
  - `drain_microtasks_async`, `call_host_function_with_args_async` in `runtime_microtask.rs`.
  - `register_complex_bridges_async` in `lib.rs`.

## Compatibility Boundary

- **Behavioral compatibility:** existing fixture `.expected` outputs must not change.
- **Allowed test additions:** add targeted async re-entry runtime tests and fixture coverage for previously untested panic-prone paths.
- **Breaking API change:** `wjsm_runtime::execute` / `execute_with_writer` become async. All in-repo callers must be updated in the same plan.
- **No changes:** parser, semantic IR, backend codegen, module bundler semantics.
- **Allowed docs changes:** update `docs/async-scheduler.md` and the 2026-06-02 spec to remove stale sync-wrapper and property-read claims.

## Verification

Mandatory final evidence:

1. `cargo check -p wjsm-runtime`
2. `cargo check -p wjsm-cli`
3. `cargo build --workspace`
4. `cargo nextest run -p wjsm-runtime -E 'test(async_reentry)'`
5. `cargo nextest run -p wjsm-runtime -E 'test(async_scheduler)'`
6. `cargo nextest run --workspace`
7. `cargo clippy --workspace --all-targets`
8. Source audit tests pass:
   - no non-async `call_wasm_callback(` remains outside deleted/absent sync helper definitions;
   - no `resolve_and_call(` / `resolve_callable_and_call(` async Store callsite remains without `_async`;
   - no `Func::call` / `TypedFunc::call` / `Instance::new` remains in `wjsm-runtime/src` runtime paths;
   - `Func::wrap` remains only for non-re-entry host functions.
9. `git diff fixtures/` shows no existing `.expected` output changes unless a newly added fixture is present.

## Plan Pressure Test

```text
- Owner / contract / retirement:
  - wjsm-runtime owns Store execution and host re-entry.
  - wjsm-cli owns sync-to-async command-line bridge.
  - Old sync runtime execution path is deleted, not kept as fallback.
- Verification scope:
  - Runtime async behavior tests hit array callbacks, Function.call/apply, Proxy/Reflect traps, proxy_traps imports, eval/native eval, JSON.parse callback paths, microtasks, and timers.
  - Source audit test blocks future sync re-entry regressions.
  - Workspace build/test/clippy prove integration.
- Task executability:
  - Helper layer first, then host import modules, then public API cutover, then docs/retirement.
  - Each task has a local compile/test command before the next layer.
- Pressure result: proceed with full root-cause conversion, not the previous minimal callback-only plan.
```

## Plan-Time Complexity Check

```text
- Target files:
  - Runtime helpers: runtime_host_helpers.rs, runtime_values.rs, runtime_eval.rs, runtime_builtins.rs, runtime_microtask.rs, runtime_async_fn.rs, runtime_json.rs, runtime_render.rs only if needed by async JSON wiring.
  - Host imports: array_object.rs, typedarray_new_methods.rs, proxy_reflect.rs, proxy_traps.rs, misc.rs, primitive_core.rs, core.rs, timers_arrays.rs.
  - Runtime entry: lib.rs.
  - CLI: crates/wjsm-cli/Cargo.toml, crates/wjsm-cli/src/lib.rs.
  - Tests/docs: wjsm-runtime tests, docs/async-scheduler.md, 2026-06-02 spec.
- Existing size / shape signals:
  - proxy_reflect.rs and array_object.rs are large but single-owner host import modules.
  - runtime_eval.rs has sync/async duplicate structure already; extend it deliberately.
  - runtime_microtask.rs has async version but caller wrapper is still wrong; refactor owner rather than patching around it.
- Owner fit:
  - No new runtime abstraction owner is needed; async twins live beside existing helpers until sync helpers are retired in the same plan.
- Add-in-place risk:
  - Moderate due nested helper functions in proxy_reflect/proxy_traps. Safer than splitting because local helpers capture module invariants.
- Better file boundary:
  - Add test-only audit file; do not create new runtime module.
- Recommendation:
  - Edit in place, then delete retired sync owner code after all callsites are async.
```

---

## Conversion Patterns

### Pattern A: sync host import with re-entry → `func_wrap_async`

```rust
linker.func_wrap_async(
    "env",
    "host_name",
    |mut caller: Caller<'_, RuntimeState>, (arg0, arg1): (i64, i32)| {
        Box::new(async move {
            let result = call_wasm_callback_async(&mut caller, callback, this_val, &[arg0]).await;
            result.unwrap_or_else(|_| value::encode_undefined())
        })
    },
)?;
```

Rules:

- Remove `let f = Func::wrap(...); linker.define(..., f)?;` for that host function.
- Tuple all parameters in the async closure.
- Preserve return types and error behavior exactly.
- Do not convert neighboring pure `Func::wrap` callbacks.

### Pattern B: function dispatch helper async pair

Create async equivalents before converting callsites:

```rust
pub(crate) async fn resolve_and_call_async(... ) -> i64;
pub(crate) async fn resolve_callable_and_call_async(... ) -> i64;
pub(crate) async fn func_apply_impl_async(... ) -> i64;
```

Rules:

- Bound function recursion must recurse into async helper.
- Proxy apply trap must use `call_wasm_callback_async(...).await`.
- Table dispatch must use `func.call_async(...).await`.
- Native callable branch must call `call_native_callable_with_args_from_caller_async(...).await`.

### Pattern C: native callable async dispatch

Create:

```rust
pub(crate) async fn call_native_callable_with_args_from_caller_async(...) -> Option<i64>;
pub(crate) async fn call_native_callable_from_caller_async(...) -> Option<i64>;
```

Rules:

- Non-re-entry native callables keep identical logic.
- `EvalIndirect` must call `perform_eval_from_caller_async(...).await`.
- `EvalFunction` must call `call_eval_function_from_caller_async(...).await`.
- Promise resolving and table mutation branches must remain synchronous Rust inside the async function; do not spawn.

### Pattern D: eval async chain

Create async mirrors for every eval function that can reach `perform_eval_from_caller` recursively:

```rust
perform_eval_from_caller_async
call_eval_function_from_caller_async
eval_call_function_async
eval_function_block_async
eval_function_stmt_async
eval_expr_async
eval_call_async
```

Rules:

- `try_compiled_eval_from_caller_async` already exists; use it.
- Direct eval inside eval AST (`eval_call`) must call async perform eval, not sync perform eval.
- Keep parser/lowering/error behavior identical.

### Pattern E: microtask async generic owner

Refactor async microtask helpers so they work from both `Store<RuntimeState>` and `Caller<'_, RuntimeState>`:

```rust
pub(crate) async fn resume_async_function_async<C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess>(...);
pub(crate) async fn drain_microtasks_async<C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess>(...);
pub(crate) async fn drain_microtasks_from_caller_async(...);
```

Rules:

- `drain_microtasks_from_caller_async` must not delegate to sync `drain_microtasks`.
- All Promise reaction / thenable / microtask callback / AsyncResume branches use async helpers.
- Scheduler continues to call the same `drain_microtasks_async`; type signature becomes generic, behavior unchanged.

---

## Tasks

### Task 1: Add failing async re-entry regression and audit tests

**Files**:
- Modify: `crates/wjsm-runtime/tests/async_scheduler.rs`
- Create: `crates/wjsm-runtime/tests/async_reentry_audit.rs`

**Why**: Prove the bug class on the live async execution path before changing implementation, and permanently prevent forbidden sync re-entry from returning.

**Impact/Compatibility**: Test-only. Existing fixtures unchanged.

**Steps**:

- [ ] Add helper `run_async_source(source: &str) -> Result<String>` in `async_scheduler.rs` if not already available.
- [ ] Add async behavior tests with names prefixed `async_reentry_`:
  - `async_reentry_array_callbacks`: `map`, `filter`, `reduce`, `sort` callbacks.
  - `async_reentry_function_call_and_apply`: `Function.prototype.call` and `apply`.
  - `async_reentry_proxy_reflect_traps`: `Reflect.get`, `Reflect.has`, `Reflect.apply`, `Reflect.construct`, `Reflect.ownKeys`, `Reflect.defineProperty` on proxies.
  - `async_reentry_proxy_trap_imports`: property get/set/delete through proxy internal trap imports.
  - `async_reentry_eval_direct_indirect_and_native`: direct eval, indirect eval, eval function native callable.
  - `async_reentry_json_parse_callbacks`: non-string object coercion and reviver.
  - `async_reentry_microtask_and_timer_callbacks`: queueMicrotask, Promise.then, timer callback.
- [ ] Create `async_reentry_audit.rs` source audit test that recursively reads `crates/wjsm-runtime/src` and fails if non-comment source contains forbidden sync patterns after retirement:
  - `call_wasm_callback(` not followed by `_async` and not the deleted function definition;
  - `resolve_and_call(` or `resolve_callable_and_call(` without `_async` outside deleted definitions;
  - `.call(` on Wasmtime funcs;
  - `Instance::new(` in eval/runtime execution code;
  - `drain_microtasks_from_caller(` in host imports.
- [ ] Run and confirm RED before implementation:
  - `cargo nextest run -p wjsm-runtime -E 'test(async_reentry)'`
  - `cargo nextest run -p wjsm-runtime -E 'test(async_reentry_audit)'`
- [ ] Commit tests after RED evidence is captured in the commit message.

**Verification**: The behavior tests or audit test fail before implementation; failure is the async Store sync re-entry issue, not unsupported JS syntax.

---

### Task 2: Complete async helper layer and eliminate helper-level sync recursion

**Files**:
- Modify: `crates/wjsm-runtime/src/runtime_host_helpers.rs`
- Modify: `crates/wjsm-runtime/src/runtime_values.rs`
- Modify: `crates/wjsm-runtime/src/runtime_builtins.rs`

**Why**: Host import conversion needs safe async primitives before callsites can move.

**Impact/Compatibility**: Internal helper additions first; no public API change yet.

**Steps**:

- [ ] Fix `call_wasm_callback_async` bound branch: line 286 currently calls sync `call_wasm_callback`; replace with recursive `call_wasm_callback_async(...).await`.
- [ ] Add `resolve_callable_and_call_async` mirroring `resolve_callable_and_call`:
  - proxy apply trap uses `call_wasm_callback_async(...).await`;
  - recursive proxy target forwarding uses `_async`;
  - native callable branch uses `call_native_callable_with_args_from_caller_async(...).await`;
  - table dispatch uses `func.call_async(...).await`.
- [ ] Add `resolve_and_call_async` mirroring `resolve_and_call` and using `resolve_callable_and_call_async(...).await` for bound and non-bound branches.
- [ ] Add `func_apply_impl_async` and keep argument behavior identical to sync `func_apply_impl`.
- [ ] Add `call_native_callable_with_args_from_caller_async` and `call_native_callable_from_caller_async` in `runtime_builtins.rs`; only eval branches await, all other branches remain identical.
- [ ] Run `cargo check -p wjsm-runtime`.
- [ ] Commit.

**Verification**: `cargo check -p wjsm-runtime` passes; audit test still fails because callsites remain sync.

---

### Task 3: Complete async eval chain

**Files**:
- Modify: `crates/wjsm-runtime/src/runtime_eval.rs`
- Modify: `crates/wjsm-runtime/src/runtime_builtins.rs` if async native callable signatures need adjustment

**Why**: Eval is a compiled WASM re-entry path. Existing `try_compiled_eval_from_caller_async` is unused by sync `perform_eval_from_caller` and native callable eval branches.

**Impact/Compatibility**: Internal async mirror functions; sync functions remain until retirement.

**Steps**:

- [ ] Add `perform_eval_from_caller_async` with identical validation/parsing/error behavior to `perform_eval_from_caller`, but call `try_compiled_eval_from_caller_async(...).await`.
- [ ] Add async mirrors for interpreted eval function execution:
  - `call_eval_function_from_caller_async`
  - `eval_call_function_async`
  - `eval_function_block_async`
  - `eval_function_stmt_async`
  - `eval_expr_async`
  - `eval_call_async`
- [ ] In `eval_call_async`, direct nested `eval(...)` calls must call `perform_eval_from_caller_async(...).await`.
- [ ] Keep sync eval functions untouched until public sync runtime path is deleted.
- [ ] Run `cargo check -p wjsm-runtime`.
- [ ] Run `cargo nextest run -p wjsm-runtime -E 'test(async_reentry_eval)'` and confirm failures now move from compile/panic risk toward remaining callsites only.
- [ ] Commit.

**Verification**: `cargo check -p wjsm-runtime` passes; async eval helpers compile and are used by async native callable dispatch.

---

### Task 4: Make microtask and async resume helpers truly async from Caller and Store

**Files**:
- Modify: `crates/wjsm-runtime/src/runtime_microtask.rs`
- Modify: `crates/wjsm-runtime/src/runtime_async_fn.rs`
- Modify: `crates/wjsm-runtime/src/scheduler.rs` only for signature fallout

**Why**: `drain_microtasks_from_caller_async` currently delegates to sync `drain_microtasks`, which still calls `func.call`. That is unsafe if the `drain_microtasks` host import runs on async Store.

**Impact/Compatibility**: Internal signature refactor; behavior stays identical.

**Steps**:

- [ ] Change `resume_async_function_async` to generic `C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess` so it accepts `Store` and `Caller`.
- [ ] Change `drain_microtasks_async` to generic over the same bound.
- [ ] Replace the body of `drain_microtasks_from_caller_async` so it calls `drain_microtasks_async(caller, &env).await`, not sync `drain_microtasks`.
- [ ] Ensure `call_host_function_async` / `call_host_function_with_args_async` remain generic and use `func.call_async` only.
- [ ] Update `scheduler.rs` callsites for the new generic signatures.
- [ ] Run `cargo check -p wjsm-runtime`.
- [ ] Commit.

**Verification**: `cargo check -p wjsm-runtime` passes; source audit no longer flags async microtask wrapper delegating to sync drain.

---

### Task 5: Convert `array_object.rs` re-entry callbacks, including Function call/apply

**Files**:
- Modify: `crates/wjsm-runtime/src/host_imports/array_object.rs`

**Why**: Array callbacks and Function call/apply are direct user callback dispatch from async Store.

**Impact/Compatibility**: Only re-entry host functions switch to `func_wrap_async`; pure Array methods stay `Func::wrap`.

**Steps**:

- [ ] Convert these direct callback imports to `func_wrap_async` and `call_wasm_callback_async(...).await`: `arr_proto_sort`, `arr_proto_for_each`, `arr_proto_map`, `arr_proto_filter`, `arr_proto_reduce`, `arr_proto_reduce_right`, `arr_proto_find`, `arr_proto_find_index`, `arr_proto_some`, `arr_proto_every`, `arr_proto_flat_map`.
- [ ] Convert `func_call` host import to `func_wrap_async` and use `resolve_and_call_async(...).await`.
- [ ] Convert `func_apply` host import to `func_wrap_async` and use `func_apply_impl_async(...).await`.
- [ ] Leave non-re-entry methods (`push`, `pop`, `includes`, `slice`, `splice`, etc.) as `Func::wrap`.
- [ ] Run `cargo check -p wjsm-runtime`.
- [ ] Run `cargo nextest run -p wjsm-runtime -E 'test(async_reentry_array_callbacks) or test(async_reentry_function_call_and_apply)'`.
- [ ] Commit.

**Verification**: Targeted async array/function tests pass.

---

### Task 6: Convert `typedarray_new_methods.rs` re-entry callbacks

**Files**:
- Modify: `crates/wjsm-runtime/src/host_imports/typedarray_new_methods.rs`

**Why**: TypedArray callback methods have the same async Store re-entry risk as Array methods.

**Impact/Compatibility**: Only callback methods convert.

**Steps**:

- [ ] Convert `forEach`, `map`, `filter`, `reduce`, `reduceRight`, `find`, `findIndex`, `some`, `every`, `sort` to `func_wrap_async`.
- [ ] Replace every callback dispatch with `call_wasm_callback_async(...).await`.
- [ ] Preserve TypedArray element read/write and SameValueZero behavior.
- [ ] Run `cargo check -p wjsm-runtime`.
- [ ] Add TypedArray coverage to `async_reentry_array_callbacks` if not already present.
- [ ] Commit.

**Verification**: TypedArray callback coverage passes under async runtime.

---

### Task 7: Convert `proxy_reflect.rs` fully, including local helper recursion

**Files**:
- Modify: `crates/wjsm-runtime/src/host_imports/proxy_reflect.rs`
- Modify: `crates/wjsm-runtime/src/runtime_host_helpers.rs` if shared helpers are used by Object builtins too

**Why**: Proxy and Reflect are the densest source of direct and indirect callback dispatch. Partial conversion leaves invariant paths unsafe.

**Impact/Compatibility**: Convert every Proxy/Reflect host import that can dispatch a trap or call target. `proxy_create` and `proxy_revocable` remain sync if they only allocate/state-mutate.

**Steps**:

- [ ] Convert direct trap callbacks to `func_wrap_async`: `reflect_get`, `reflect_set`, `reflect_has`, `reflect_delete_property`, `reflect_apply`, `reflect_construct`, `reflect_set_prototype_of`, `reflect_get_own_property_descriptor`, `reflect_define_property`, `reflect_own_keys`, `proxy.apply`, `proxy.construct`.
- [ ] Convert indirect prototype/extensibility callbacks to async: `reflect_get_prototype_of`, `reflect_is_extensible`, `reflect_prevent_extensions`.
- [ ] Add async local helpers inside the file where local helpers currently call sync re-entry:
  - `reflect_apply_impl_async`
  - `reflect_construct_impl_async`
  - `reflect_get_prototype_of_impl_async`
- [ ] Use shared async helpers where appropriate:
  - `reflect_get_impl_with_receiver_async`
  - `define_property_internal_async`
  - `proxy_or_target_is_extensible_impl_async`
  - `proxy_or_target_prevent_extensions_impl_async`
- [ ] Ensure recursive proxy forwarding awaits async helper, not sync helper.
- [ ] Run `cargo check -p wjsm-runtime`.
- [ ] Run `cargo nextest run -p wjsm-runtime -E 'test(async_reentry_proxy_reflect_traps)'`.
- [ ] Commit.

**Verification**: Proxy/Reflect async tests pass; source audit no longer flags `proxy_reflect.rs`.

---

### Task 8: Convert `proxy_traps.rs` internal trap imports

**Files**:
- Modify: `crates/wjsm-runtime/src/host_imports/proxy_traps.rs`

**Why**: `proxy_trap_get`, `proxy_trap_set`, and `proxy_trap_delete` call local `call_trap_with_args`, which uses `resolve_and_call` and `func.call` transitively.

**Impact/Compatibility**: Only proxy trap imports convert.

**Steps**:

- [ ] Add `call_trap_with_args_async` using `resolve_and_call_async(...).await`.
- [ ] Add async local helpers:
  - `proxy_internal_get_async`
  - `proxy_internal_set_async`
  - `proxy_internal_delete_async`
  - async ordinary recursive branches for proxy targets.
- [ ] Convert `proxy_trap_get`, `proxy_trap_set`, `proxy_trap_delete` to `func_wrap_async`.
- [ ] Preserve existing TypeError messages and boolean return semantics.
- [ ] Run `cargo check -p wjsm-runtime`.
- [ ] Run `cargo nextest run -p wjsm-runtime -E 'test(async_reentry_proxy_trap_imports)'`.
- [ ] Commit.

**Verification**: Proxy internal trap async tests pass.

---

### Task 9: Convert `misc.rs` native call, drain, and eval imports

**Files**:
- Modify: `crates/wjsm-runtime/src/host_imports/misc.rs`

**Why**: `native_call`, `drain_microtasks`, `eval_direct`, and `eval_indirect` can reach WASM on async Store. `queue_microtask` only enqueues and remains sync.

**Impact/Compatibility**: Mixed sync/async imports in one module.

**Steps**:

- [ ] Leave `queue_microtask` as `Func::wrap`.
- [ ] Convert `drain_microtasks` import to `func_wrap_async` and call `drain_microtasks_from_caller_async(...).await`.
- [ ] Convert `native_call` to `func_wrap_async`:
  - Proxy construct/apply traps use `call_wasm_callback_async(...).await`.
  - Proxy target forwarding uses `resolve_and_call_async(...).await`.
  - Native callable branch uses `call_native_callable_with_args_from_caller_async(...).await`.
  - `new_target` restoration remains identical on every return path.
- [ ] Convert `eval_direct` and `eval_indirect` imports to `func_wrap_async` and call `perform_eval_from_caller_async(...).await`.
- [ ] Run `cargo check -p wjsm-runtime`.
- [ ] Run `cargo nextest run -p wjsm-runtime -E 'test(async_reentry_eval_direct_indirect_and_native) or test(async_reentry_microtask_and_timer_callbacks)'`.
- [ ] Commit.

**Verification**: Eval/native/microtask targeted tests pass.

---

### Task 10: Convert `primitive_core.rs` callable replacer path

**Files**:
- Modify: `crates/wjsm-runtime/src/host_imports/primitive_core.rs`

**Why**: RegExp/String replacement with callable replacer uses `resolve_and_call`.

**Impact/Compatibility**: Only host import callback path converts; primitive pure operations stay sync.

**Steps**:

- [ ] Locate the host import containing `primitive_core.rs:1105` `resolve_and_call`.
- [ ] Convert that host import to `func_wrap_async`.
- [ ] Replace `resolve_and_call(...)` with `resolve_and_call_async(...).await`.
- [ ] Preserve argument materialization order for match, captures, offset, original string, and named groups object.
- [ ] Run `cargo check -p wjsm-runtime`.
- [ ] Add replacer coverage to `async_reentry_function_call_and_apply` or a dedicated `async_reentry_string_replace_callback` test and run it.
- [ ] Commit.

**Verification**: Callable replacer passes under async runtime.

---

### Task 11: Convert `core.rs` proxy `in` trap path

**Files**:
- Modify: `crates/wjsm-runtime/src/host_imports/core.rs`

**Why**: `op_in` dispatches Proxy `has` trap via sync `call_wasm_callback`.

**Impact/Compatibility**: Convert only `op_in`; pure core imports remain sync.

**Steps**:

- [ ] Convert `op_in` host import to `func_wrap_async`.
- [ ] Replace Proxy `has` trap dispatch with `call_wasm_callback_async(...).await`.
- [ ] If `op_in_impl` recurses into proxy targets and can call re-entry, add `op_in_impl_async` and use it from async host import.
- [ ] Preserve non-proxy behavior and TypeError/runtime error messages.
- [ ] Run `cargo check -p wjsm-runtime`.
- [ ] Add `"x" in proxy` coverage to proxy async test and run it.
- [ ] Commit.

**Verification**: Proxy `in` trap passes under async runtime.

---

### Task 12: Convert JSON.parse callback/coercion path and timers_arrays JSON import

**Files**:
- Modify: `crates/wjsm-runtime/src/runtime_json.rs`
- Modify: `crates/wjsm-runtime/src/host_imports/timers_arrays.rs`

**Why**: JSON.parse can call user code via object-to-string coercion and reviver.

**Impact/Compatibility**: JSON.stringify remains sync unless implementation grows actual callback support; JSON.parse import becomes async.

**Steps**:

- [ ] Add `json_parse_to_string_async` mirroring `json_parse_to_string`; object `toString` / `valueOf` uses `call_wasm_callback_async(...).await`.
- [ ] Add `apply_reviver_async` with recursive await on array/object traversal and `call_wasm_callback_async(...).await` for reviver.
- [ ] Add `json_parse_to_wasm_async` using both async helpers.
- [ ] Convert `json_parse` import in `timers_arrays.rs` to `func_wrap_async`.
- [ ] Keep `json_stringify` import as `Func::wrap` because current implementation has no callback dispatch.
- [ ] Run `cargo check -p wjsm-runtime`.
- [ ] Run `cargo nextest run -p wjsm-runtime -E 'test(async_reentry_json_parse_callbacks)'`.
- [ ] Commit.

**Verification**: JSON.parse callback/coercion tests pass under async runtime.

---

### Task 13: Delete sync complex bridges and collapse runtime linker naming

**Files**:
- Modify: `crates/wjsm-runtime/src/lib.rs`

**Why**: Async path already has `register_complex_bridges_async`; sync complex bridge copy keeps forbidden source patterns alive and confuses audits.

**Impact/Compatibility**: Internal runtime wiring only.

**Steps**:

- [ ] Delete `register_complex_bridges_sync` entirely.
- [ ] Rename `register_complex_bridges_async` to `register_complex_bridges`.
- [ ] Confirm `register_common_bridges` remains sync and has no re-entry.
- [ ] Keep `register_common_bridges` only if source audit proves it contains no forbidden patterns.
- [ ] Run `cargo check -p wjsm-runtime`.
- [ ] Commit.

**Verification**: `lib.rs` has no sync complex bridge copy and no sync `call_wasm_callback` in runtime execution wiring.

---

### Task 14: Delete sync execution path and rename async API as default

**Files**:
- Modify: `crates/wjsm-runtime/src/lib.rs`
- Modify: `crates/wjsm-runtime/tests/async_scheduler.rs`
- Modify: `crates/wjsm-runtime/tests/timer_timing.rs`

**Why**: Keeping sync wrappers contradicts the async-only model and keeps sync Wasmtime calls in the crate.

**Impact/Compatibility**: Public `wjsm-runtime` API becomes async. In-repo tests update in the same task.

**Steps**:

- [ ] Delete sync `register_linker` and rename `register_linker_async` to `register_linker`.
- [ ] Delete sync `execute` and sync `execute_with_writer`.
- [ ] Rename `execute_async` to `execute`.
- [ ] Rename `execute_with_writer_async` to `execute_with_writer`.
- [ ] Update runtime tests to `Runtime::new()?.block_on(async { execute_with_writer(...).await })` or use `#[tokio::test]`.
- [ ] Update `timer_timing.rs` to call async runtime through a Tokio runtime.
- [ ] Run `cargo check -p wjsm-runtime`.
- [ ] Run `cargo nextest run -p wjsm-runtime`.
- [ ] Commit.

**Verification**: Runtime crate compiles and tests use only async public API.

---

### Task 15: CLI integration with explicit Tokio bridge

**Files**:
- Modify: `crates/wjsm-cli/Cargo.toml`
- Modify: `crates/wjsm-cli/src/lib.rs`

**Why**: CLI remains a synchronous command-line program but must call async runtime API.

**Impact/Compatibility**: CLI behavior unchanged; dependency on Tokio becomes direct.

**Steps**:

- [ ] Add direct dependency:
  ```toml
  tokio = { version = "1", features = ["rt-multi-thread"] }
  ```
- [ ] Add a small helper in `wjsm-cli/src/lib.rs`:
  ```rust
  fn execute_runtime(wasm: &[u8]) -> anyhow::Result<()> {
      let rt = tokio::runtime::Runtime::new()
          .context("Failed to create tokio runtime")?;
      rt.block_on(wjsm_runtime::execute(wasm))
  }
  ```
- [ ] Replace both CLI `runtime::execute(...)` callsites with `execute_runtime(...)`.
- [ ] Ensure no CLI path calls `wjsm_runtime::execute_with_writer` directly unless it awaits through the same runtime boundary.
- [ ] Run `cargo check -p wjsm-cli`.
- [ ] Run a targeted CLI fixture through nextest: `cargo nextest run -E 'test(happy__hello)'`.
- [ ] Commit.

**Verification**: CLI check passes and a CLI-backed fixture still executes.

---

### Task 16: Retire unused sync re-entry helpers

**Files**:
- Modify: `crates/wjsm-runtime/src/runtime_host_helpers.rs`
- Modify: `crates/wjsm-runtime/src/runtime_values.rs`
- Modify: `crates/wjsm-runtime/src/runtime_eval.rs`
- Modify: `crates/wjsm-runtime/src/runtime_microtask.rs`
- Modify: `crates/wjsm-runtime/src/runtime_async_fn.rs`
- Modify: `crates/wjsm-runtime/src/runtime_builtins.rs`

**Why**: A clean cutover prevents future callsites from accidentally choosing sync helpers.

**Impact/Compatibility**: Internal deletions after all callsites have moved.

**Steps**:

- [ ] Delete sync `call_wasm_callback`.
- [ ] Delete sync `resolve_and_call`, `resolve_callable_and_call`, and `func_apply_impl` if no sync callsite remains.
- [ ] Delete sync `try_compiled_eval_from_caller`, `perform_eval_from_caller`, and sync eval interpreter helpers if no sync callsite remains. If a sync eval helper remains for pure AST code without Wasm re-entry, keep it only if source audit proves it cannot call `Instance::new`, `entry.call`, or `perform_eval_from_caller`.
- [ ] Delete sync `drain_microtasks`, `drain_microtasks_from_caller`, `call_host_function`, `call_host_function_with_args`, and sync `resume_async_function` if no sync callsite remains.
- [ ] Delete sync native callable dispatch helpers or keep only pure wrappers that cannot reach eval/WASM; async Store callsites must use async native callable helpers.
- [ ] Remove `Func` imports from host import files that no longer use `Func::wrap`; keep `Func` where pure sync imports remain.
- [ ] Run `cargo check -p wjsm-runtime`.
- [ ] Run `cargo nextest run -p wjsm-runtime -E 'test(async_reentry_audit)'`.
- [ ] Commit.

**Verification**: Source audit test passes for retired sync helper patterns.

---

### Task 17: Update authority docs and spec

**Files**:
- Modify: `docs/async-scheduler.md`
- Modify: `docs/aegis/specs/2026-06-02-unified-async-execution-model-design.md`


**Why**: Current docs contradict the new async-only contract and stale property-read scope.

**Impact/Compatibility**: Documentation only.

**Steps**:

- [ ] Update `docs/async-scheduler.md`:
  - replace sync wrapper section with async-only runtime API;
  - state CLI owns the sync `block_on` bridge;
  - remove “CLI migration excluded” claim;
  - preserve run-to-completion and scheduler owner semantics.
- [ ] Update the 2026-06-02 spec:
  - correct `read_object_property_by_name` claim;
  - correct `register_common_bridges` claim;
  - add `resolve_and_call`, eval, native callable, microtask drain, and proxy_traps to risk inventory;
  - update acceptance criteria to source audit + targeted async tests.
- [ ] Do not modify `docs/aegis/INDEX.md`; existing entries already point to this plan and spec.
- [ ] Run `cargo nextest run -p wjsm-runtime -E 'test(async_reentry_audit)'` to ensure docs did not replace technical verification.
- [ ] Commit.

**Verification**: Docs no longer conflict with code plan or public API.

---

### Task 18: Final verification and no-regression proof

**Files**: None, unless verification exposes a real issue.

**Steps**:

- [ ] Run `cargo check -p wjsm-runtime`.
- [ ] Run `cargo check -p wjsm-cli`.
- [ ] Run `cargo build --workspace`.
- [ ] Run `cargo nextest run -p wjsm-runtime -E 'test(async_reentry)'`.
- [ ] Run `cargo nextest run -p wjsm-runtime -E 'test(async_scheduler)'`.
- [ ] Run `cargo nextest run --workspace`.
- [ ] Run `cargo clippy --workspace --all-targets`.
- [ ] Run `git diff -- fixtures` and confirm no existing `.expected` files changed.
- [ ] If any verification fails, fix the source issue and rerun the failed command plus the directly related targeted test.
- [ ] Commit final verification/doc cleanup if any files changed.

**Verification**: Every command above passes; targeted audit proves no sync re-entry owner remains in async runtime paths.

---

## Risks

| Risk | Mitigation |
|---|---|
| Async closure lifetime errors with `Caller<'_, RuntimeState>` | Follow existing `register_complex_bridges_async` pattern; convert one module at a time with `cargo check -p wjsm-runtime`. |
| Eval async mirror becomes large | Keep a 1:1 async mirror first; delete sync mirror only after all callsites move; do not redesign eval semantics during this plan. |
| `drain_microtasks_async` generic refactor breaks scheduler | Refactor signature first, run runtime scheduler tests before host import conversion. |
| Native callable async path silently keeps sync eval | Source audit test bans `perform_eval_from_caller` from async/native callsites and targeted eval tests cover direct/indirect/native eval. |
| Pure `Func::wrap` accidentally converted broadly | Source audit validates risk; only convert re-entry callbacks. Mixed sync/async host imports are allowed. |
| Public API break misses a caller | Workspace build and search-backed source audit catch in-repo callers; docs mark the external breaking boundary. |
| Existing fixture output changes | Final fixture diff check; any changed existing `.expected` is a regression unless separately justified by a spec bug fix, which is outside this plan. |

## Retirement

| Old owner/fallback | Final status | Deletion trigger |
|---|---|---|
| sync `execute` / `execute_with_writer` | Deleted | Task 14 |
| sync `register_linker` | Deleted | Task 14 |
| `register_complex_bridges_sync` | Deleted | Task 13 |
| sync `call_wasm_callback` | Deleted | Task 16 |
| sync `resolve_and_call` / `resolve_callable_and_call` / `func_apply_impl` | Deleted or proven unreachable pure sync-only absent from async Store | Task 16 source audit |
| sync eval compiled-WASM helpers | Deleted or no Wasm re-entry remains | Task 16 source audit |
| sync microtask drain / host function callback helpers | Deleted | Task 16 |
| sync `resume_async_function` | Deleted | Task 16 |
| stale `async-scheduler.md` sync wrapper contract | Replaced by async-only contract | Task 17 |
| stale 2026-06-02 spec property-read/common-bridge claims | Corrected | Task 17 |

## ADR Signal

This remains an architecture decision: execution model single-owner async-only, Tokio is a hard dependency at CLI/runtime integration boundary, and `wjsm-runtime` public API changes from sync to async. Completion should trigger ADR/baseline backfill documenting:

- why mixed `Func::wrap` / `func_wrap_async` is allowed only when sync callbacks do not re-enter WASM;
- why sync runtime wrappers were deleted instead of retained;
- why `read_object_property_by_name` stayed sync;
- why CLI, not runtime, owns `block_on`.

## Self-review checklist

- [x] Every stale claim from the prior review was re-grounded against current source.
- [x] `register_common_bridges` false positive corrected.
- [x] `read_object_property_by_name` scope corrected.
- [x] `resolve_and_call`, eval, native callable, microtask, proxy_traps, JSON.parse, and Function call/apply are covered.
- [x] Verification includes behavior tests and source audit tests.
- [x] Public API and CLI migration are explicit.
- [x] Docs/spec conflict is a task, not a hidden follow-up.
- [x] No compatibility fallback or sync runtime owner remains after retirement.
