# Proxy 13-Trap Completion + Reflect API — Design Spec

**Status:** Draft  
**Date:** 2026-05-28  
**Scope:** Fix 4 proxy trap bugs + add 9 Object.* builtins + add 2 Builtin variants

## Goal

Complete the ECMAScript Proxy internal method dispatch matrix (all 13 traps functioning) and fill the Object.* builtin gap that prevents Proxy traps from being exercised through standard Object.* calls.

## Current Baseline

All 13 Proxy traps have runtime implementations in `proxy_reflect.rs` (895 lines). All 15 Builtin enum variants exist. Reflect.* methods correctly dispatch through proxy traps with ES-spec invariant checks. However:

1. **[[Call]]/[[Construct]]**: WASM call path (`compile_call_with_new_target`) has no TAG_PROXY detection — proxy handle falls through to `call_indirect`, invoking wrong function.
2. **[[OwnPropertyKeys]]**: trap is invoked but `extract_array_like_elements` → `alloc_array` construction produces result that `JSON.stringify` renders as `null`.
3. **Descriptor serialization**: `JSON.stringify(Reflect.getOwnPropertyDescriptor(...))` renders `[object Object]` instead of proper JSON.
4. **Object.* gap**: 9 Object static methods (keys, entries, values, assign, create, getPrototypeOf, setPrototypeOf, getOwnPropertyNames, is) have Builtin enum + host_import_registry entries but zero runtime implementations. 2 more (isExtensible, preventExtensions) lack even Builtin variants.

## Design

### A. WASM Proxy Call/Construct Dispatch

**Problem**: `compile_call_with_new_target` checks `TAG_NATIVE_CALLABLE → NativeCall`, then `TAG_CLOSURE → call_indirect`, else treats callee as `TAG_FUNCTION → call_indirect`. TAG_PROXY falls through the else branch.

**Fix**: Add TAG_PROXY branch before the closure/function dispatch:

```
check TAG_NATIVE_CALLABLE → NativeCall
check TAG_PROXY → ProxyApply/ProxyConstruct (NEW)
check TAG_CLOSURE → call_indirect
else → call_indirect
```

**New SpecialHostImports**:
- `ProxyApply`: `(i64 proxy, i64 this_val, i32 args_base, i32 args_count) → i64`
- `ProxyConstruct`: `(i64 proxy, i64 this_val, i32 args_base, i32 args_count) → i64`

Both host functions:
1. Decode proxy handle → look up ProxyEntry (target, handler, revoked)
2. If revoked → `TypeError`
3. Read handler's "apply"/"construct" trap
4. If trap exists and callable → `call_wasm_callback(trap, handler, args)`
5. Else → `resolve_and_call(target, ...)` / construct_impl

**Files**: `host_import_registry.rs` (2 new variants), `compiler_instructions.rs` (TAG_PROXY branch in `compile_call_with_new_target`), `proxy_reflect.rs` (2 new host functions, reuse existing trap dispatch)

### B. Object.* Builtin Completion

**Strategy**: Object.* methods that mirror Reflect.* forward to existing Reflect impls. Independent methods (keys, entries, values, assign) implement standalone.

**New file**: `crates/wjsm-runtime/src/host_imports/object_builtins.rs`

#### Forwarding methods (Object → Reflect):

| Object method | Forwards to | Extra validation |
|---|---|---|
| `Object.getPrototypeOf(O)` | `reflect_get_prototype_of_fn_impl` | TypeError if O is not Object |
| `Object.setPrototypeOf(O, proto)` | `reflect_set_prototype_of_fn_impl` | TypeError if O is not Object; TypeError if proto not Object/null |
| `Object.getOwnPropertyDescriptor(O, P)` | `reflect_get_own_property_descriptor_impl` | TypeError if O is not Object |
| `Object.getOwnPropertyNames(O)` | `collect_own_property_names(caller, ptr, false)` | TypeError if O is not Object; returns string keys only |
| `Object.defineProperty(O, P, Attributes)` | `define_property_internal` | TypeError if O is not Object; returns O on success, throws on failure |
| `Object.create(O, Properties?)` | `alloc_host_object` + `reflect_set_prototype_of_fn_impl` | TypeError if O is not Object or null |

#### Independent methods:

| Object method | Implementation |
|---|---|
| `Object.keys(O)` | `collect_own_property_names(caller, ptr, true)` → filter enumerable → array |
| `Object.entries(O)` | Collect enumerable own string-keyed properties → `[key, value]` pairs array |
| `Object.values(O)` | Collect enumerable own string-keyed properties → values array |
| `Object.assign(target, ...sources)` | Iterate sources → for each own enumerable string/symbol key: `Reflect.set(target, key, val)` |
| `Object.is(value1, value2)` | SameValue algorithm (strict equality + NaN check) |

#### New Builtin variants:

| Builtin | import name | arity |
|---|---|---|
| `ObjectIsExtensible` | `"object.is_extensible"` | 1 |
| `ObjectPreventExtensions` | `"object.prevent_extensions"` | 1 |

Both forward to existing `is_extensible_impl` / `prevent_extensions_impl` with proxy-aware dispatch.

### C. ownKeys Return Value Fix

**Problem**: `JSON.stringify(Reflect.ownKeys(proxy))` produces `null` despite trap being invoked correctly.

**Hypothesis**: `extract_array_like_elements` returns elements, `alloc_array` + `set_array_elem` constructs new array, but the returned array's length field or memory layout is wrong — `JSON.stringify` reads 0-length array or null.

**Fix**: Debug `reflect_own_keys_fn` → verify `alloc_array` creates properly sized array → verify `set_array_elem` writes to correct offsets → verify returned array handle is valid.

### D. Descriptor JSON.stringify Fix

**Problem**: `JSON.stringify(Reflect.getOwnPropertyDescriptor(proxy, "b"))` renders `[object Object]`.

**Fix**: Verify `reflect_get_own_property_descriptor_impl` produces a correctly-formatted host object with `value`/`writable`/`enumerable`/`configurable` properties using `define_host_data_property_from_caller`. May need to check `JSON.stringify`'s object traversal path.

### E. Invariant Check Tightening

**Test failures observed** in `proxy_invariants.js`:
- `Proxy with null handler did not throw` → wjsm sets runtime_error but doesn't throw TypeError
- `construct trap returned non-object, result: object` → non-object returns should throw TypeError but wjsm wraps as object
- `set on revoked proxy returned` → should throw TypeError

**Fix**: Ensure these invariant checks throw proper `TypeError` via `bail!()` or `set_runtime_error` + early return with error sentinel.

## Files Modified/Created

| File | Action | Lines (est.) |
|------|--------|-------------|
| `crates/wjsm-runtime/src/host_imports/object_builtins.rs` | **CREATE** | ~300 |
| `crates/wjsm-runtime/src/host_imports/proxy_reflect.rs` | MODIFY | +80 |
| `crates/wjsm-runtime/src/host_imports/mod.rs` | MODIFY | +3 |
| `crates/wjsm-runtime/src/lib.rs` | MODIFY | +2 |
| `crates/wjsm-ir/src/builtin.rs` | MODIFY | +10 |
| `crates/wjsm-semantic/src/builtins.rs` | MODIFY | +6 |
| `crates/wjsm-backend-wasm/src/compiler_instructions.rs` | MODIFY | +30 |
| `crates/wjsm-backend-wasm/src/compiler_builtins.rs` | MODIFY | +10 |
| `crates/wjsm-backend-wasm/src/host_import_registry.rs` | MODIFY | +30 |
| `crates/wjsm-backend-wasm/src/lib.rs` | MODIFY | +12 |
| `fixtures/happy/proxy_traps_full.js` | MODIFY | (update expected) |
| `fixtures/happy/` new fixtures | CREATE | ~5 files |

## Non-Goals
- No changes to IR instruction set
- No changes to Proxy constructor internals (ProxyCreate host function unchanged)
- No changes to inline ProxyTrapGet/Set/Delete fast-paths
- `Object.freeze/seal` not in scope
- `Reflect` namespace not being extended (13 methods already covered)
- `JSON.stringify` general fixes not in scope (only descriptor-specific)

## Compatibility Boundary
- Existing test fixtures MUST NOT regress (10 proxy/reflect tests already passing)
- Proxy trap semantics MUST follow ES2025 §10.5
- Object.* methods MUST follow ES2025 §20.1.2

## ADR Signals
- Object.* → Reflect.* forwarding: architectural decision to avoid duplication. If Reflect impl changes, Object methods inherit the change automatically.
- `object_builtins.rs` is the owner for all Object static method implementations.
- No new dependency direction change — Object host functions call into existing Reflect helpers (same crate).
