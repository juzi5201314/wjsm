# Proxy 13-Trap + Reflect API — Implementation Plan

**Goal:** Fix 4 proxy trap bugs + add 9 Object.* builtins + 2 new Builtin variants

**Architecture:** Object.* → Reflect.* forwarding. WASM proxy call/construct via SpecialHostImport dispatch. No new IR instructions.

**Baseline:** `docs/aegis/specs/2026-05-28-proxy-reflect-complete-design.md`

**Compatibility:** 10 existing proxy/reflect fixtures must not regress. All 372 fixtures pass.

**Verification:** `cargo nextest run --workspace`

---

## Phase 1: IR + Semantic + WASM Backend Registration

### Task 1.1: Add ObjectIsExtensible + ObjectPreventExtensions Builtin variants

**Files:** `crates/wjsm-ir/src/builtin.rs`

**Change:** Add variants after `ObjectGroupBy`:
```rust
    ObjectIsExtensible,
    ObjectPreventExtensions,
```
Add Display: `Self::ObjectIsExtensible => "object.is_extensible"`, `Self::ObjectPreventExtensions => "object.prevent_extensions"`

**File:** `crates/wjsm-semantic/src/builtins.rs` — In `builtin_from_static_member` under `"Object"`:
```rust
    "isExtensible" => Some(Builtin::ObjectIsExtensible),
    "preventExtensions" => Some(Builtin::ObjectPreventExtensions),
```

**Verify:** `cargo check -p wjsm-ir -p wjsm-semantic`

---

### Task 1.2: Register WASM imports + builtin_arity + compiler dispatch

**Files:**
- `crates/wjsm-backend-wasm/src/host_import_registry.rs`: Add `ProxyApply`, `ProxyConstruct` to `SpecialHostImport` enum. Add 4 `HostImportSpec` entries (2 Builtin + 2 Special, type_idx 3 for Builtins, type_idx 12 for proxy call).
- `crates/wjsm-backend-wasm/src/lib.rs`: Add `builtin_arity` entries: `ObjectIsExtensible => ("object.is_extensible", 1)`, `ObjectPreventExtensions => ("object.prevent_extensions", 1)`
- `crates/wjsm-backend-wasm/src/compiler_builtins.rs`: Add `ObjectIsExtensible | ObjectPreventExtensions` to the 1-arg Object methods dispatch arm

**Verify:** `cargo check -p wjsm-backend-wasm`

**Commit:** `feat: IR/semantic/backend for Object.isExtensible/Object.preventExtensions + Proxy apply/construct imports`

---

## Phase 2: Object.* Runtime

### Task 2.1: Create object_builtins.rs (all 11 host functions)

**File:** CREATE `crates/wjsm-runtime/src/host_imports/object_builtins.rs`

Implement these host functions, each validating args then forwarding to existing helpers:

| Function | Import name | Logic |
|----------|-----------|-------|
| Object.getPrototypeOf | `"object.get_prototype_of"` | `TypeError` if non-object → `reflect_get_prototype_of_impl` |
| Object.setPrototypeOf | `"object.set_prototype_of"` | `TypeError` if non-object; proto not Object/null → `reflect_set_prototype_of_fn_impl` |
| Object.getOwnPropertyDescriptor | `"object.get_own_property_descriptor"` | `TypeError` if non-object → `reflect_get_own_property_descriptor_impl` |
| Object.getOwnPropertyNames | `"object.get_own_property_names"` | `TypeError` if non-object; if proxy: ownKeys trap → filter symbols → array; else: `collect_own_property_names(caller, ptr, false)` |
| Object.keys | `"object.keys"` | Same pattern as getOwnPropertyNames but `collect_own_property_names(caller, ptr, true)` |
| Object.values | `"object.values"` | Iterate enumerable own string keys, read values, return array |
| Object.entries | `"object.entries"` | Iterate enumerable own string keys, create `[key, val]` pairs, return array |
| Object.assign | `"object.assign"` | Shadow-stack based; iterate sources, copy own enumerable properties via `reflect_get_impl` + `reflect_set_impl` |
| Object.create | `"object.create"` | `alloc_host_object` → set prototype via memory write; `TypeError` if proto not Object/null |
| Object.is | `"object.is"` | SameValue algorithm: `===` + NaN check + +0/-0 distinction |
| Object.isExtensible | `"object.is_extensible"` | `TypeError` if non-object; proxy: trap dispatch + invariant → `is_extensible_impl` |
| Object.preventExtensions | `"object.prevent_extensions"` | `TypeError` if non-object; proxy: trap dispatch + invariant → `prevent_extensions_impl`; returns obj |

Helper functions needed (define as private `fn` in the module):
- `extract_array_like_elements_internal` — copy from proxy_reflect.rs
- `proxy_get_target` — get proxy target handle
- `reflect_own_keys_fn_impl_local` — ownKeys logic for reuse

**File:** `crates/wjsm-runtime/src/host_imports/mod.rs` — Add `mod object_builtins;` + `pub(crate) use object_builtins::define_object_builtins;`

**File:** `crates/wjsm-runtime/src/lib.rs` — Add `define_object_builtins(&mut linker, &mut store)?;` (line ~231, near other `define_*` calls)

**Verify:** `cargo check -p wjsm-runtime`

**Commit:** `feat(runtime): implement 11 Object.* static host functions`

---

## Phase 3: Proxy [[Call]] / [[Construct]] Fix

### Task 3.1: WASM call path TAG_PROXY detection

**File:** `crates/wjsm-backend-wasm/src/compiler_instructions.rs`

In `compile_call_with_new_target`, after the `TAG_NATIVE_CALLABLE` block and before the closure/function dispatch, insert a `TAG_PROXY` check:

```
// After line ~801 (emitting TAG_NATIVE_CALLABLE If block's End + Else):
// Add TAG_PROXY check before existing closure/function dispatch

// Check tag == TAG_PROXY
emit LocalGet(callee_local)
emit I64Const(32); I64ShrU; I64Const(0x1F); I64And
emit I64Const(TAG_PROXY as i64); I64Eq
emit If(BlockType::Result(ValType::I64))
  // Branch to ProxyApply or ProxyConstruct
  emit LocalGet(callee_local)
  emit LocalGet(this_val_local)
  emit LocalGet(shadow_sp_scratch)
  emit I32Const(args.len())
  emit Call(SpecialHostImport::ProxyApply or ProxyConstruct per new_target presence)
  emit LocalSet(result_scratch)
  // Restore shadow_sp + new_target, return result
  emit LocalGet(shadow_sp_scratch); GlobalSet(shadow_sp_global)
  emit LocalGet(saved_new_target); Call(NewTargetSet); Drop
  emit LocalGet(result_scratch); Return
emit End
```

**Verify:** `cargo check -p wjsm-backend-wasm`

---

### Task 3.2: ProxyApply + ProxyConstruct host functions

**File:** `crates/wjsm-runtime/src/host_imports/proxy_reflect.rs`

Add 2 host functions before `Ok(())`:

**`proxy_apply_call`** (signature: `(i64 proxy, i64 this, i32 base, i32 count) → i64`):
1. Decode proxy handle → ProxyEntry (revoked? → TypeError)
2. Read shadow stack args into Vec
3. Look up handler's "apply" trap
4. If trap exists: pack args into array, `call_wasm_callback(trap, handler, [target, this, arr])`
5. Else: `reflect_apply_impl(caller, target, this, &args)`

**`proxy_construct_call`** (signature: `(i64 proxy, i64 this, i32 base, i32 count) → i64`):
1. Same pattern for construct: look up "construct" trap
2. If trap: `call_wasm_callback(trap, handler, [target, arr, target])`
3. Else: `reflect_construct_impl(caller, target, &args, target)`

**Verify:** `cargo check -p wjsm-runtime -p wjsm-backend-wasm`

**Commit:** `fix: proxy [[Call]] and [[Construct]] dispatch through WASM call path`

---

## Phase 4: Bug Fixes

### Task 4.1: Fix ownKeys result null

**File:** `crates/wjsm-runtime/src/host_imports/proxy_reflect.rs`

**Root cause:** `reflect_own_keys_fn` uses `alloc_array` + `set_array_elem` (writes to raw offset `ptr + 16 + i * 8`), but this doesn't update the array's length field at offset 8-11. The WASM-level `json_stringify` reads length → 0 → returns null.

**Fix:** After the `for` loop writing elements, ensure length is set. Add after line ~883 (before `return arr`):
```rust
// Ensure array length is set
if let Some(arr_ptr) = resolve_handle(&mut caller, arr) {
    write_array_length(&mut caller, arr_ptr, keys.len() as u32);
}
```

Also check the loop uses `define_host_data_property_from_caller` pattern instead of raw `set_array_elem` for consistency.

**Verify:** `cargo run -- run fixtures/happy/proxy_traps_full.js` — confirm ownKeys line shows array, not null

---

### Task 4.2: Fix descriptor JSON.stringify (`[object Object]`)

**File:** `crates/wjsm-runtime/src/host_imports/proxy_reflect.rs`

**Root cause:** `reflect_get_own_property_descriptor_impl` constructs a host object with `define_host_data_property_from_caller`. The object has correct properties set but `JSON.stringify` on the runtime may not traverse host object properties correctly.

**Fix:** Check if `JSON.stringify` processes host objects. If not, the fix is in the JSON stringify path. As a workaround, use `render_value` to produce a string representation.

Actually verify: run isolated test `console.log(JSON.stringify({value:42,writable:true,enumerable:true,configurable:true}))` — if this works, the issue is specific to the descriptor object construction.

**Verify:** `cargo run -- run -e 'console.log(JSON.stringify(Reflect.getOwnPropertyDescriptor({a:1}, "a")))'` — expect proper JSON

---

### Task 4.3: Tighten invariant checks

**File:** `crates/wjsm-runtime/src/host_imports/proxy_reflect.rs`

Issues from `proxy_invariants.expected`:
1. `Proxy with null handler did not throw` — In `proxy_create_fn`, change `*caller.data().runtime_error.lock()... = Some(...)` to `set_runtime_error(...)` with proper TypeError
2. `construct trap returned non-object, result: object` — In `reflect_construct_fn`, after `call_wasm_callback` for construct trap, the check `if !value::is_js_object(res)` should throw TypeError AND return undefined (not coerce to object)
3. `set on revoked proxy returned` — Revoked proxy checks should use `set_runtime_error` consistently

**Verify:** `cargo nextest run -E 'test(proxy_invariants)'` — exit_code should remain 2 (by design) but FAIL assertions should decrease

---

## Phase 5: Fixtures + Full Verification

### Task 5.1: Update proxy_traps_full expected output

**Files:** `fixtures/happy/proxy_traps_full.expected`

Update expected output to match fixed behavior:
- `apply call:` should show `apply trap: [5,6]` then `apply call: 22`
- `constructed name: Alice`, `constructed flag: true`
- `ownKeys: ["y","z"]`
- `instanceof proxyPerson: true`, `instanceof Person: true`

Run `WJSM_UPDATE_FIXTURES=1 cargo nextest run -E 'test(proxy_traps_full)'` to auto-update.

---

### Task 5.2: Add comprehensive test fixtures

Create new fixtures:
1. `fixtures/happy/proxy_apply_construct.js` — direct proxy() and new proxy() with apply/construct traps
2. `fixtures/happy/object_methods_proxy.js` — Object.keys/entries/values/getOwnPropertyNames/getOwnPropertyDescriptor on proxies
3. `fixtures/happy/object_static_methods.js` — all new Object.* methods on plain objects
4. `fixtures/happy/proxy_ownkeys_trap.js` — ownKeys trap with filtered keys, invariant checks
5. `fixtures/happy/reflect_full.js` — comprehensive Reflect.* test covering all 13 methods

**Verify:** `cargo nextest run -E 'test(proxy_) | test(object_) | test(reflect_)'`

---

### Task 5.3: Full regression

```bash
cargo nextest run --workspace
```

All 372+ existing tests must pass. No regressions.

**Commit:** `test: comprehensive proxy/reflect/Object.* fixtures`

---

## Risks

1. **`reflect_apply_impl` / `reflect_construct_impl` accessibility** — these are local `fn` inside `define_proxy_reflect`. The new ProxyApply/ProxyConstruct host functions added to the same function can reference them directly. Verified safe.

2. **Object.keys/values/entries proxy dispatch** — these use ownKeys trap + target property reads. Need to ensure `reflect_get_impl` returns correct values for proxy-wrapped targets (it does — already tested).

3. **JSON.stringify on descriptor objects** — may require deeper JSON.stringify fix if the issue is in how host objects' properties are enumerated.

## Retirement

- No old code to retire. All additions are net-new or bug fixes on existing code.
- `docs/superpowers/plans/2026-05-13-es-builtins-phase6-proxy-reflect.md` should be marked as superseded or deleted.
