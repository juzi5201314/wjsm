# ADR 0006: Runtime Module Loading Boundary

**Status**: Accepted. issue #312 implementation landed in the working tree: runtime CJS `require()`, expression `import(expr)`, `import.meta.resolve()`, `require.resolve()`, live `require.cache`, JSON `require()`, runtime module registry, and CLI runtime loader are implemented with targeted fixture/runtime/semantic/backend verification.

**Date**: 2026-07-07

## Context

wjsm originally treated the module graph as an ahead-of-time bundle artifact. That was sufficient for static ESM imports and top-level static CommonJS `require()` calls, but it could not represent runtime module loading semantics:

- `require()` inside control flow was hoisted into top-level imports, so false branches could execute module side effects.
- Computed `require('./mods/' + name + '.js')` had no runtime resolver/loader.
- `import(expr)` was limited to compile-time-known string targets and expression/template specifiers were rejected by the resolver/lowerer.
- `import.meta.resolve()`, `require.resolve()`, `require.resolve.paths()`, and `require.cache` had no runtime source of truth.
- The runtime had only `module_namespace_cache: ModuleId -> namespace`, which could not model canonical file/builtin/JSON identity, `Loading`/`Loaded`/`Errored` states, CJS circular partial exports, live `module.exports`, or cache deletion/retry.

At the same time, the existing crate dependency direction is load-bearing:

```text
parser → semantic → ir ← backend-wasm → runtime → cli
```

`wjsm-runtime` must not grow dependencies on `wjsm-module`, parser, semantic, or backend crates. Putting the compiler inside runtime host imports would make runtime own too much and break the established boundary.

## Decision

Runtime module loading is split across three canonical owners:

1. **`wjsm-module` owns runtime resolution.**
   `crates/wjsm-module/src/runtime_resolution.rs` exposes plain runtime-resolution DTOs and reuses the issue #309 resolver/package condition logic for `Import` and `Require` resolution. It also owns `require.resolve.paths()` search path calculation.

2. **`wjsm-runtime` owns runtime module state and host semantics.**
   `crates/wjsm-runtime/src/runtime_module_registry.rs` is the runtime source of truth for module cache state. It keys modules by canonical file / JSON / builtin / precompiled identity and models `Loading`, `Loaded`, and `Errored` states. CJS circular requires read partial exports during `Loading`; loaded CJS requires read live `module.exports`; errored entries are observable and deletable through `require.cache`.

   `crates/wjsm-runtime/src/runtime_module_loader.rs` defines runtime-owned DTOs and the `RuntimeModuleLoader` trait. The trait deliberately contains no `wjsm-module`, parser, semantic, or backend types.

3. **`wjsm-cli` owns runtime compilation and instantiation glue.**
   `crates/wjsm-cli/src/runtime_loader.rs` implements `RuntimeModuleLoader` for CLI/file-backed execution. It resolves through `wjsm-module`, compiles with the existing parser/semantic/backend pipeline, and asks runtime to instantiate the compiled module with runtime-owned import-link descriptors. Backend host-import registry knowledge stays on the CLI/backend side; runtime receives only runtime-owned descriptors.

### Host and language behavior

- Top-level unconditional static CJS `require('./x')` remains an AOT fast path.
- Control-flow, function-body, short-circuit, conditional, computed, JSON, and `require.resolve.*` CJS sites stay on the runtime path.
- CJS modules receive module-local `require`, `module`, `exports`, `__filename`, and `__dirname` bindings.
- `require.cache` is a live registry-backed proxy view, not a materialized snapshot.
- `import('./literal.js')` with a compile-time module id keeps the static fast path.
- `import(expr)` evaluates the specifier under dynamic-import completion semantics: abrupt completions reject the returned Promise with the original reason instead of throwing synchronously or stringifying an exception handle.
- `import.meta.resolve(specifier)` uses Import-condition runtime resolution and propagates exceptions before enclosing expression evaluation continues.
- CJS `require('./data.json')` is supported; dynamic `import('./data.json')` without import assertions remains rejected because JSON import assertions are out of scope.
- Runtime `.ts`, `.tsx`, and `.jsx` file loads are rejected with an unsupported runtime-loader diagnostic rather than being compiled at runtime.

## Alternatives Considered

### Eagerly enumerate every possible runtime target

Rejected. It preserves a pure AOT model, but cannot cover arbitrary computed paths, optional dependencies, user-selected locale/module names, cache deletion/retry, or CJS circular loading state. It also preserves the old false-branch side-effect problem unless runtime `require()` becomes real.

### Put parser/semantic/backend directly in `wjsm-runtime`

Rejected. This would make host imports straightforward, but it reverses the current dependency direction and turns runtime into a compiler owner. That would make snapshot/support/runtime boundaries harder to audit and would duplicate ownership already held by module/semantic/backend crates.

### Keep `module_namespace_cache` as the main cache

Rejected. `ModuleId -> namespace` is only enough for static dynamic import. It cannot represent file/JSON/builtin identity, live CommonJS exports, `Loading`/`Errored` state, `require.cache`, or deletion/retry semantics. It is replaced by `RuntimeModuleRegistry`; precompiled module ids are retained only as a compatibility key for the static fast path.

## Consequences

### Positive

- Runtime module loading now has one state owner: `RuntimeModuleRegistry`.
- Resolver rules remain shared with compile-time package resolution instead of being copied into runtime.
- Runtime remains compiler-agnostic; CLI supplies compilation/instantiation glue through the loader trait.
- CJS runtime semantics are closer to Node-compatible behavior: false branches do not execute, circular requires observe partial exports, `module.exports` reassignment is live, cache deletion is observable, and optional missing dependencies can be caught.
- Dynamic import expression semantics are Promise-owned, including specifier abrupt completions.

### Negative / Risks

- CLI runtime loading now instantiates modules after initial execution starts, so the runtime instantiation boundary is more complex than the previous static bundle-only model.
- `RuntimeModuleLoader` is a public extension point; DTO constructors and error codes are compatibility surfaces.
- Runtime CJS/ESM goal detection must preserve explicit resolver formats (`.mjs`, package `type: module`) while still probing ambiguous extensionless / no-package `.js` files that contain CommonJS-only constructs.
- JSON support is intentionally asymmetric: CJS `require()` works, but ESM JSON import without assertions remains rejected until import assertions are designed.

## Compatibility Boundary

- Existing static import/export and static dynamic import fixtures remain supported.
- Top-level unconditional literal CJS require remains eligible for AOT import transformation.
- Runtime loader is installed by CLI/file-backed execution. Library/runtime execution without a loader returns explicit loader-unavailable errors for true runtime loads.
- Runtime crate dependency direction remains intact: no runtime dependency on `wjsm-module`, parser, semantic, or backend crates.
- Runtime TypeScript/JSX compilation is not supported.

## Implementation Notes

- `RuntimeModuleRegistry` values are included in GC root collection.
- `require.cache` uses registry-backed traps for get/has/delete/ownKeys/descriptor behavior.
- Runtime CJS module lifecycle is `Loading -> Loaded` on success and `Loading -> Errored` on body failure, so thrown partial exports do not become stale loaded entries.
- Dynamic import lowers runtime specifiers through a completion-propagating path so intermediate abrupt completions inside sequence/conditional/composed expressions are preserved as Promise rejections.
- CLI runtime loader maps backend host-import specs to runtime-owned import-link descriptors before calling runtime instantiation helpers.

## Status of Implementation

| Slice | Status | Evidence |
|---|---|---|
| CJS require analysis split | ✅ | top-level/static vs runtime-site tests; `cjs_conditional_require_false` no longer prints side effect |
| Runtime resolution API | ✅ | `wjsm-module` runtime resolver tests for file/JSON/package/builtin/paths |
| Runtime registry + loader contract | ✅ | `wjsm-runtime` module registry tests; GC root coverage; external-style loader DTO test |
| CJS require/resolve/cache host behavior | ✅ | runtime require/resolve/cache tests; live `module.exports`; live `require.cache` |
| Dynamic import + import.meta.resolve | ✅ | semantic/runtime/module dynamic import tests; abrupt-completion regressions |
| CLI runtime loader | ✅ | computed CJS require and dynamic import variable fixtures; loader-unavailable tests; TS/TSX/JSX rejection |
| Fixture matrix | ✅ | JSON require, optional missing dependency, cache delete/retry, errored cache retry, circular partial exports, resolve.paths, explicit ESM preservation, extensionless CJS probing |

## References

- issue #312
- Spec: `docs/aegis/specs/2026-07-07-runtime-module-loading-design.md`
- Plan: `docs/aegis/plans/2026-07-07-runtime-module-loading.md`
- Work evidence: `docs/aegis/work/2026-07-07-runtime-module-loading/90-evidence.md`
- ADR 0002: `docs/adr/0002-runtimestate-stays-flat.md`
- ADR 0004: `docs/adr/0004-build-time-embedded-runtime.md`
