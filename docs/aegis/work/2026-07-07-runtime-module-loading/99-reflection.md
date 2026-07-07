# Runtime Module Loading Reflection

Date: 2026-07-07
Issue: #312

## Goal Closure

- Goal status: satisfied pending final fresh verification and final review.
- Success evidence: Tasks 1-7 each have targeted implementation evidence; ADR 0006 records the runtime/compiler boundary; final verification commands remain the last closeout step.
- Stop state: needs-verification until the final targeted commands and final review pass.
- Non-goals respected: no JSON import assertions, no runtime TypeScript/JSX compilation, no HMR/code splitting, no runtime dependency on compiler crates.

## Repair Track

- Repaired object: runtime module loading across CJS `require()`, dynamic `import(expr)`, `import.meta.resolve()`, `require.resolve()`, `require.cache`, JSON require, CLI runtime loader.
- Action: introduced runtime resolution API, runtime registry/loader contract, CJS module-local bindings, dynamic import runtime path, CLI loader, and fixture matrix.
- Impact: runtime module loading now uses canonical resolver/cache owners while preserving AOT static fast paths.
- Verification: targeted module/semantic/runtime/backend/integration/CLI checks recorded in `90-evidence.md`.

## Retirement Track

- Retired object: all-static recursive CJS require hoisting, transform-local `globalThis.require` bridge, AOT-only expression dynamic import diagnostics, `module_namespace_cache` as canonical module cache.
- Action: moved ownership to `cjs_require_analysis`, semantic CJS bindings, runtime registry, and injected loader.
- Retained boundary: `PrecompiledModuleId` compatibility key remains inside registry for static dynamic import fast path.
- Future trigger: if static dynamic imports are fully keyed by canonical runtime module keys, retire `PrecompiledModuleId` compatibility.

## Drift Check

- Scope: still issue #312 runtime module loading.
- Compatibility: static import/export, static dynamic import, and top-level unconditional literal require fast paths preserved; runtime TypeScript/JSX and JSON import assertions remain out of scope.
- Owner integrity: resolver logic in `wjsm-module`, runtime state in `wjsm-runtime`, compilation/instantiation glue in `wjsm-cli`.
- Evidence: implementation evidence exists for every planned slice; final fresh verification still required before completion claim.
- Decision: continue to final verification and review.

## Residual Risk

- `PrecompiledModuleId` compatibility key remains as an intentional bridge for static dynamic import.
- Runtime module loading has targeted evidence rather than a full workspace suite.
- Runtime loader goal detection for ambiguous JS relies on resolver format plus AST CJS probe and has focused fixtures for explicit ESM and extensionless CJS boundaries.
