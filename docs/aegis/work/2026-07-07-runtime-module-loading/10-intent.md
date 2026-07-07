# Runtime Module Loading Work Intent

Date: 2026-07-07
Issue: #312
Parent spec: `docs/aegis/specs/2026-07-07-runtime-module-loading-design.md`
Parent plan: `docs/aegis/plans/2026-07-07-runtime-module-loading.md`

## Requested outcome

Implement runtime module loading for issue #312: CJS runtime `require()`, dynamic `import(expr)`, `import.meta.resolve()`, `require.resolve()` / `require.cache`, JSON require, runtime module registry, CLI loader wiring, fixtures, ADR, and closeout evidence.

## Scope

- Modify `wjsm-module`, `wjsm-semantic`, `wjsm-ir`, `wjsm-backend-wasm`, `wjsm-runtime`, and `wjsm-cli` according to the plan.
- Preserve static import/export and top-level unconditional literal require fast path.
- Correct control-flow require semantics.
- Add targeted tests and module fixtures.

## Non-goals

- HMR, file watching, code splitting, remote import, package manager protocol support.
- runtime TypeScript compilation.
- `require.extensions` or full `module.createRequire`.

## BaselineReadSetHint

Required refs:

- issue #312
- `AGENTS.md`
- `docs/aegis/specs/2026-07-07-runtime-module-loading-design.md`
- `docs/aegis/plans/2026-07-07-runtime-module-loading.md`
- `docs/aegis/specs/2026-07-06-package-resolution-enhancement-design.md`
- `docs/aegis/plans/2026-07-06-package-resolution-enhancement.md`
- `crates/wjsm-module/src/resolver.rs`
- `crates/wjsm-module/src/graph.rs`
- `crates/wjsm-module/src/cjs_transform.rs`
- `crates/wjsm-module/src/resolution_options.rs`
- `crates/wjsm-semantic/src/lowerer_async_eval/async_import_promise.rs`
- `crates/wjsm-semantic/src/lowerer_jsx_objects/jsx_expressions.rs`
- `crates/wjsm-runtime/src/host_imports/misc.rs`
- `crates/wjsm-runtime/src/lib.rs`
- `crates/wjsm-runtime/src/runtime_gc/roots.rs`

## BaselineUsageDraft

- Required baseline refs: listed above.
- Acknowledged before execution: spec, plan, current CJS transform, resolver, graph, semantic dynamic import/import.meta, runtime host import/cache/roots.
- Cited in execution plan: yes.
- Missing refs: none known.
- Decision: continue.

## ImpactStatementDraft

- Affected layers: module resolver/CJS transform/graph, semantic lowerers, IR builtins, backend host imports, runtime registry/loader/host imports/GC roots, CLI loader, fixtures, ADR.
- Owners: `wjsm-module` owns resolution; runtime registry owns cache/state; CLI/orchestrator owns compile+instantiate; backend emits host calls.
- Invariants: runtime does not depend on compiler crates; canonical cache keys; `Loading` inserted before executing module; dynamic reads bounded by configured roots; issue #309 resolution rules reused.
- Compatibility: static fast paths preserved; false-branch require corrected; missing loader errors are explicit.
