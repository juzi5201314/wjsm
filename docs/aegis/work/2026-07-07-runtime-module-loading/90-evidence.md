# Runtime Module Loading Evidence

Date: 2026-07-07

## Design / planning evidence

- Wrote `docs/aegis/specs/2026-07-07-runtime-module-loading-design.md`.
- Wrote `docs/aegis/plans/2026-07-07-runtime-module-loading.md`.
- Updated `docs/aegis/INDEX.md` with spec and plan entries.
- Ran completion-marker scan with no matches after removing self-referential scan text.

## Implementation evidence

### Task 1 — Split CJS require analysis

- Added `crates/wjsm-module/src/cjs_require_analysis.rs` as the owner for classifying CJS `require()` sites.
- Wired `crates/wjsm-module/src/cjs_transform.rs` to consume the analyzer hoistable set and to leave runtime `require(...)` calls in the AST.
- Added targeted CJS transform tests for top-level literal, `if (false)`, `try/catch`, computed specifier, function-body, and logical short-circuit (`&&`, `||`, `??`) require classification.
- Added a transform-local temporary `var require = globalThis.require` bridge only when runtime require sites remain, so false-branch preserved calls lower without turning `require` into a semantic builtin. Retirement boundary: replace this bridge with the Task 4 module-local require binding/loader wiring.
- Updated `fixtures/modules/cjs_conditional_require_false/main.expected` after the fixture produced `undefined` without `side effect`.

### Task 2 — Runtime resolution API

- Added `crates/wjsm-module/src/runtime_resolution.rs` as the plain DTO/API owner for runtime specifier resolution and `require.resolve.paths()` search path calculation.
- Reused `ModuleResolver::resolve_specifier_with_kind` so runtime import/require condition selection goes through the existing issue #309 package exports/imports logic.
- Exposed only the runtime DTOs and two entry functions from `crates/wjsm-module/src/lib.rs`.
- Added resolver tests for relative file and JSON canonical key/path/url/format, require vs import condition selection, builtin key/url, and `require.resolve.paths()` null/search behavior.


### Task 3 — Runtime module registry and loader contract

- Added `crates/wjsm-runtime/src/runtime_module_registry.rs` as the canonical runtime module state owner with File/Json/Builtin keys, Loading/Loaded/Errored states, module-id namespace lookup, require lookup, require.cache delete semantics, and GC root extraction.
- Added `crates/wjsm-runtime/src/runtime_module_loader.rs` with runtime-owned DTOs and a `RuntimeModuleLoader` trait; it does not use `wjsm-module`, parser, semantic, or backend types.
- Replaced `RuntimeState`'s old `module_namespace_cache` field with `Arc<Mutex<RuntimeModuleRegistry>>`; existing `register_module_namespace` / `dynamic_import(module_id)` now delegate through the registry's explicit `PrecompiledModuleId` compatibility key.
- Updated runtime GC side-table roots to include registry-held module, exports, namespace, and error values.
- Added public constructors for `RuntimeResolvedModule`, `RuntimeInstantiationEnv`, and `RuntimeInstantiatedModule` so external loader implementations can keep using `#[non_exhaustive]` DTOs without struct literals.
- Added `crates/wjsm-runtime/tests/module_registry_loader_contract.rs` to prove an external-style `RuntimeModuleLoader` implementation can construct and return runtime DTOs through the public crate API.

## Verification evidence

### Task 1 commands

- `cargo nextest run -p wjsm-module -E 'test(top_level_literal_require_still_generates_import) | test(if_false_literal_require_remains_runtime_call) | test(try_literal_require_remains_runtime_call) | test(computed_require_remains_runtime_call) | test(function_body_literal_require_remains_runtime_call) | test(logical_and_require_remains_runtime_call) | test(logical_or_require_remains_runtime_call)'`
  - Result after logical short-circuit analyzer fix: passed — `Summary [   0.017s] 7 tests run: 7 passed, 204 skipped`.
- `cargo nextest run -p wjsm-module -E 'test(nullish_coalescing_require_remains_runtime_call)'`
  - Result after adding nullish coalescing coverage: passed — `Summary [   0.006s] 1 test run: 1 passed, 210 skipped`.
- `cargo nextest run -E 'test(modules__cjs_conditional_require_false)'`
  - Result after logical short-circuit analyzer fix: passed — `Summary [   0.034s] 2 tests run: 2 passed, 769 skipped`.

### Task 2 commands

- `cargo nextest run -p wjsm-module -E 'test(runtime_resolve_) | test(resolve_paths_)'`
  - RED before implementation: failed to compile because `RuntimeModuleFormat`, `RuntimeModuleKey`, `RuntimeResolveKind`, `RuntimeResolvePaths`, `resolve_runtime_paths`, and `resolve_runtime_specifier` were not exported from `wjsm-module`.
  - GREEN after implementation and final verification: passed — `Summary [   0.009s] 7 tests run: 7 passed, 211 skipped`.

### Task 3 commands

- `cargo nextest run -p wjsm-runtime -E 'test(module_registry_)'`
  - RED before implementation: failed to compile because `runtime_module_loader.rs`, registry types, and `RuntimeState::module_registry` did not exist.
  - GREEN after implementation and final verification: passed — `Summary [   0.020s] 6 tests run: 6 passed, 201 skipped`.
- Quality-blocker repair verification after DTO constructors and external-style loader coverage: passed — `Summary [   0.027s] 7 tests run: 7 passed, 201 skipped`.

### Task 4 commands

- `cargo nextest run -p wjsm-semantic -E 'test(require_runtime)'`
  - RED during implementation: failed to compile after the new test import dropped `LoweringError` and used the old five-argument `lower_modules` call.
  - GREEN after repair and formatting: passed — `Summary [   0.016s] 1 test run: 1 passed, 134 skipped`.
- `cargo nextest run -p wjsm-runtime -E 'test(require_cache) | test(require_resolve)'`
  - RED during implementation: failed to compile because `SnapshotNativeCallableBridge::try_from_native_callable` did not classify the new CJS native callable variants.
  - GREEN after repair, unavailable-loader coverage, cached-error propagation, and formatting: passed — `Summary [   0.397s] 3 tests run: 3 passed, 208 skipped`.
- `cargo nextest run -E 'test(modules__cjs_conditional_require_false)'`
  - GREEN after implementation, formatting, and cached-error propagation: passed — `Summary [   0.041s] 2 tests run: 2 passed, 769 skipped`.
- `cargo nextest run -p wjsm-module -E 'test(runtime_require_does_not_inject_global_bridge)'`
  - GREEN after bridge retirement and formatting: passed — `Summary [   0.005s] 1 test run: 1 passed, 218 skipped`.
- `cargo nextest run -p wjsm-runtime -E 'test(module_registry_)'`
  - GREEN after `require.cache` registry extensions: passed — `Summary [   0.019s] 7 tests run: 7 passed, 203 skipped`.

Task 4 boundary: runtime now accepts an injected `RuntimeModuleLoader` and focused tests cover `require.resolve`, `require.resolve.paths`, catchable missing-loader / missing-module errors, and `require.cache` on module-local CJS bindings. Full CLI filesystem module loading remains intentionally outside this slice (Task 6).

Task 4 live `module.exports` repair:

- Added `require_cache_returns_replaced_module_exports_for_loaded_cjs`, which resolves `require('./self.js')` back to `/project/main.cjs` through the existing runtime fake-loader seam after `module.exports` is replaced.
- RED before fix: `cargo nextest run -p wjsm-runtime -E 'test(require_cache_returns_replaced_module_exports_for_loaded_cjs)'` failed with `left: "undefined\nfalse\n"`, proving `require()` returned the initial `exports` alias object.
- Repair: registry `Loaded` require results now return the module object owner plus stored exports fallback; the CJS require host reads current `module.exports` from the module object for loaded modules. `Loading` still returns the initial exports object for circular partial exports.
- GREEN after fix: `cargo nextest run -p wjsm-runtime -E 'test(require_cache_returns_replaced_module_exports_for_loaded_cjs)'` passed — `Summary [   0.050s] 1 test run: 1 passed, 212 skipped`.
- Acceptance: `cargo nextest run -p wjsm-runtime -E 'test(require_cache) | test(require_resolve) | test(module_registry_)'` passed — `Summary [   0.036s] 12 tests run: 12 passed, 201 skipped`.
- Acceptance: `cargo nextest run -E 'test(modules__cjs_conditional_require_false)'` passed — `Summary [   0.030s] 2 tests run: 2 passed, 769 skipped`. 

Task 4 live `require.cache` repair:

- Added `require_cache_held_reference_reflects_delete`, proving a held `const cache = require.cache` sees registry deletion from `delete require.cache[__filename]`.
- Added `require_cache_held_reference_observes_later_loaded_entry`, proving a held cache proxy observes a later registry entry through indexed get, `Object.keys`/ownKeys, and `Object.getOwnPropertyDescriptor`.
- RED before fix: `cargo nextest run -p wjsm-runtime -E 'test(require_cache_held_reference_reflects_delete) | test(require_cache_held_reference_observes_later_loaded_entry)'` failed with stale held-cache output (`[object Object]` and `true` after deletion) and missing later-entry descriptor behavior.
- Repair: replaced the snapshot target data with a live registry-backed `require.cache` proxy (`get`, `has`, `deleteProperty`, `ownKeys`, `getOwnPropertyDescriptor`); registry deletion remains the canonical mutation path and loading entries remain protected by `RuntimeModuleRegistry::delete_cache_entry`.
- Also routed sync `Object.getOwnPropertyDescriptor` for native proxy descriptor traps so the live cache descriptor view is observable through built-ins.
- GREEN after fix: `cargo nextest run -p wjsm-runtime -E 'test(require_cache_held_reference_reflects_delete) | test(require_cache_held_reference_observes_later_loaded_entry)'` passed — `Summary [   0.032s] 2 tests run: 2 passed, 213 skipped`.
- Acceptance: `cargo nextest run -p wjsm-runtime -E 'test(require_cache) | test(module_registry_)'` passed — `Summary [   0.093s] 12 tests run: 12 passed, 203 skipped`.
- Acceptance: `cargo nextest run -E 'test(modules__cjs_conditional_require_false)'` passed — `Summary [   0.035s] 2 tests run: 2 passed, 769 skipped`.

### Task 5 — ESM dynamic `import(expr)` and `import.meta.resolve()`

- Split resolver and semantic dynamic import handling: resolver-known static string specifiers still form AOT graph edges and semantic `dynamic_import(module_id)` fast-path calls; expression/template dynamic imports no longer produce AOT-only resolver/lowerer diagnostics and instead lower to `dynamic_import_runtime(referrer, specifier)`. Zero-argument malformed validation remains covered by the parser/semantic boundary, and unsupported extra options remain rejected by semantic lowering.
- Added `Builtin::DynamicImportRuntime` and `Builtin::ImportMetaResolve`, backend dispatch, and registry-owned host import specs.
- Added `import.meta.resolve` as a generated import-meta native callable that captures the current module filename as `RuntimeModuleReferrer::Path` and calls the installed loader with `RuntimeModuleResolutionKind::Import`, returning the resolved URL string.
- Added runtime dynamic import host loading through `RuntimeModuleLoader`: ToString conversion happens inside runtime, resolution/instantiation use Import conditions, registry cached namespaces fulfill the returned Promise, and conversion/resolve/instantiate failures reject it.
- Added focused module, semantic, and runtime tests for resolver diagnostic retirement, expression/template dynamic import lowering, static literal fast path preservation, import.meta.resolve loader-backed URL return, and rejected missing runtime imports. Full CLI filesystem expression import remains Task 6; Task 5 evidence uses the injected fake-loader seam.

### Task 5 commands

- `cargo nextest run -p wjsm-semantic -E 'test(dynamic_import) | test(import_meta_resolve)'`
  - GREEN after implementation/final recheck: passed — `Summary [   0.017s] 4 tests run: 4 passed, 135 skipped`.
- `cargo nextest run -p wjsm-runtime -E 'test(dynamic_module) | test(import_meta_resolve)'`
  - GREEN after implementation/final recheck: passed — `Summary [   0.026s] 3 tests run: 3 passed, 215 skipped`.
- `cargo nextest run -E 'test(modules__dynamic_import)'`
  - GREEN static dynamic import fixture preservation/final recheck: passed — `Summary [   0.033s] 3 tests run: 3 passed, 768 skipped`.
- `cargo nextest run -p wjsm-module -E 'test(dynamic_import_expression_is_runtime_not_resolver_diagnostic) | test(dynamic_import_template_expression_is_runtime_not_resolver_diagnostic) | test(dynamic_import_static_literal_still_creates_graph_edge) | test(dynamic_import_without_specifier_still_reports_malformed_call)'`
  - RED before parser-boundary test repair: zero-argument malformed call was rejected by parser before resolver extraction.
  - GREEN after resolver diagnostic retirement and parser-boundary assertion: passed — `Summary [   0.006s] 4 tests run: 4 passed, 219 skipped`.
- `cargo nextest run -p wjsm-backend-wasm -E 'test(builtin_registry_binding_count_matches_ir_contract) | test(import_names_are_unique) | test(builtin_bindings_are_unique) | test(host_imports_count_locked)'`
  - RED after initial registry-count update: failed with `left: 416`, `right: 414`; root cause was an under-updated test contract constant after adding two host-import-backed builtins on top of prior registry growth.
  - GREEN after correcting `EXPECTED_BUILTIN_REGISTRY_BINDINGS`/final recheck: passed — `Summary [   0.006s] 4 tests run: 4 passed, 59 skipped`.

Task 5 spec-compliance repair:

- Added semantic regressions for `import(path, { with: { type: 'json' } })` and resolver-known `import('./dep.js', { with: { type: 'json' } })`; the static-known case was RED before repair because it emitted the `dynamic_import(module_id)` fast path and ignored the extra argument.
- Added runtime regression for `console.log(import.meta.resolve('./missing.js'))`; it was RED before repair with output `[exception:1]` followed by `outer-observed`, proving the exception value reached the outer `console.log` instead of throwing first.
- Repair: dynamic import extra-argument validation now runs before static fast-path selection; direct `import.meta.resolve(...)` calls lower through a dedicated path that checks returned exception values before enclosing expressions continue.
- Acceptance: `cargo nextest run -p wjsm-semantic -E 'test(dynamic_import) | test(import_meta_resolve)'` passed — `Summary [   0.027s] 6 tests run: 6 passed, 135 skipped`.
- Acceptance: `cargo nextest run -p wjsm-runtime -E 'test(dynamic_module) | test(import_meta_resolve)'` passed — `Summary [   0.033s] 4 tests run: 4 passed, 215 skipped`.
- Acceptance: `cargo nextest run -p wjsm-module -E 'test(dynamic_import)'` passed — `Summary [   0.009s] 7 tests run: 7 passed, 216 skipped`.
- Acceptance: `cargo nextest run -E 'test(modules__dynamic_import)'` passed — `Summary [   0.049s] 3 tests run: 3 passed, 768 skipped`.

Task 5 quality-compliance repair — dynamic import specifier abrupt completions:

- Added semantic regressions for `import(JSON.parse('bad'))`, `import(JSON.parse('bad') + './never.js')`, `import(import.meta.resolve('./missing.js'))`, and `import(import.meta.resolve('./missing.js') + '?x')`, asserting abrupt specifier paths route through `dynamic_import_runtime` instead of generic exception unwrapping.
- Added runtime regressions proving direct and composed `JSON.parse` specifier failures return a Promise and reject with the original `SyntaxError`, and direct and composed `import.meta.resolve` specifier failures return a Promise and reject with the original missing-resolution `TypeError` reason. Also added numeric-specifier coverage to keep runtime ToString conversion for non-exception specifier values.
- Repair: dynamic import expression lowering now evaluates the specifier while collecting suppressed exception forks. Each collected TAG_EXCEPTION branch calls `dynamic_import_runtime(referrer, exception)` and merges with the normal Promise result, so composed expressions short-circuit to Promise rejection before ToString/string concatenation can consume the exception. Module specifier ToString also preserves incoming TAG_EXCEPTION values as errors instead of rendering/stringifying them.
- Acceptance: `cargo nextest run -p wjsm-semantic -E 'test(dynamic_import) | test(import_meta_resolve)'` passed — `Summary [   0.014s] 10 tests run: 10 passed, 135 skipped`.
- Acceptance: `cargo nextest run -p wjsm-runtime -E 'test(dynamic_module) | test(import_meta_resolve)'` passed — `Summary [   0.045s] 9 tests run: 9 passed, 215 skipped`.
- Acceptance: `cargo nextest run -E 'test(modules__dynamic_import)'` passed — `Summary [   0.404s] 3 tests run: 3 passed, 768 skipped`.
- Final delegated recheck: `cargo nextest run -p wjsm-semantic -E 'test(dynamic_import) | test(import_meta_resolve)'` passed — `Summary [   0.018s] 10 tests run: 10 passed, 135 skipped`.
- Final delegated recheck: `cargo nextest run -p wjsm-runtime -E 'test(dynamic_module) | test(import_meta_resolve)'` passed — `Summary [   0.050s] 9 tests run: 9 passed, 215 skipped`.
- Final delegated recheck: `cargo nextest run -E 'test(modules__dynamic_import)'` passed — `Summary [   0.027s] 3 tests run: 3 passed, 768 skipped`.
- Added sequence/comma regressions for `import((JSON.parse('bad'), './dep.js'))` and `import((sideEffect(), './dep.js'))`. The abrupt sequence case proves the intermediate `TAG_EXCEPTION` rejects with the original `SyntaxError` before the final literal can overwrite it; the normal sequence case proves a non-throwing side effect still reaches the final specifier and loads `./dep.js`.
- Final sequence recheck: `cargo nextest run -p wjsm-semantic -E 'test(dynamic_import) | test(import_meta_resolve)'` passed — `Summary [   0.013s] 12 tests run: 12 passed, 135 skipped`.
- Final sequence recheck: `cargo nextest run -p wjsm-runtime -E 'test(dynamic_module) | test(import_meta_resolve)'` passed — `Summary [   0.046s] 11 tests run: 11 passed, 215 skipped`.
- Final sequence recheck: `cargo nextest run -E 'test(modules__dynamic_import)'` passed — `Summary [   0.028s] 3 tests run: 3 passed, 768 skipped`.
- Added conditional-specifier regressions for `import((true ? JSON.parse('bad') : './dep.js') + '?x')` and `import((false ? JSON.parse('bad') : './dep.js') + '')`. The abrupt conditional case proves the selected branch's original `SyntaxError` rejects the dynamic import Promise before outer string concatenation or runtime resolution can replace it; the normal conditional case proves the selected non-throwing branch still composes and loads `./dep.js`.
- RED before fix: `cargo nextest run -p wjsm-runtime dynamic_module_import_conditional_json_parse_abrupt_rejects_original_reason` failed with `left: "function\nafter\ncaught TypeError\n"`, proving the conditional branch exception was stringified/resolved into a loader `TypeError` instead of preserving the original `SyntaxError`.
- Repair: `lower_cond` now uses `lower_expr_then_continue` for the test and each selected-arm block while dynamic-import specifier exception fork collection is active, so branch-local `TAG_EXCEPTION` values are collected before the conditional result feeds an outer `+`. Ordinary conditional lowering outside suppressed collection keeps the existing `lower_expr` path.
- Final conditional recheck: `cargo nextest run -p wjsm-semantic -E 'test(dynamic_import) | test(import_meta_resolve)'` passed — `Summary [   0.014s] 13 tests run: 13 passed, 135 skipped`.
- Final conditional recheck: `cargo nextest run -p wjsm-runtime -E 'test(dynamic_module) | test(import_meta_resolve)'` passed — `Summary [   0.117s] 13 tests run: 13 passed, 215 skipped`.
- Final conditional recheck: `cargo nextest run -E 'test(modules__dynamic_import)'` passed — `Summary [   0.068s] 3 tests run: 3 passed, 768 skipped`.
- Ordinary conditional guard: `cargo nextest run -p wjsm-semantic -E 'test(ternary_phi_fixture_matches_ir_snapshot) | test(ternary_nested_fixture_matches_ir_snapshot)'` passed — `Summary [   0.014s] 2 tests run: 2 passed, 146 skipped`.

### Task 6 — CLI runtime filesystem loader

- Added `crates/wjsm-cli/src/runtime_loader.rs` as the CLI-owned runtime module loader. It uses `wjsm-module` runtime resolution, rejects runtime `.ts/.tsx/.jsx` loads with an explicit unsupported-loader error, lowers runtime-loaded file entries through the existing module pipeline, compiles them with relocatable data/table bases, and asks `wjsm-runtime` to instantiate into the current Store/env.
- Extended `wjsm-runtime` with `RuntimeModuleInstantiationContext` so external loaders can reserve shared table/data ranges and instantiate compiled WASM without giving runtime a parser/semantic/backend dependency.
- Added runtime compile layout support in `wjsm-backend-wasm` so dynamically instantiated modules place active element segments and string data outside the already-running module's table/data ranges.
- Added `wjsm-module::lower_runtime_entry_bundle_with_options` so runtime-loaded ESM entries register a namespace object for dynamic import fulfillment.
- Wired CLI file and in-process fixture execution to install the loader automatically with inferred/explicit root, sandbox read roots, and CLI package-resolution options.
- Added fixtures `fixtures/modules/runtime_loading/cjs_computed_require` and `fixtures/modules/runtime_loading/esm_dynamic_import_variable`, plus focused unavailable-loader runtime tests `require_loader_unavailable` and `dynamic_module_loader_unavailable`.

### Task 6 commands

- RED before loader wiring: `cargo nextest run -E 'test(modules__runtime_loading__cjs_computed_require) | test(modules__runtime_loading__esm_dynamic_import_variable)'` failed with missing-loader runtime output for both fixtures.
- GREEN after implementation: `cargo nextest run -E 'test(modules__runtime_loading__cjs_computed_require) | test(modules__runtime_loading__esm_dynamic_import_variable)'` passed — `Summary [   0.070s] 2 tests run: 2 passed, 773 skipped`.
- Unavailable-loader guard: `cargo nextest run -p wjsm-runtime -E 'test(dynamic_module_loader_unavailable) | test(require_loader_unavailable)'` passed — `Summary [   0.137s] 2 tests run: 2 passed, 228 skipped`.
- CLI smoke: `cargo run -- run fixtures/modules/runtime_loading/cjs_computed_require/main.js --root fixtures/modules/runtime_loading/cjs_computed_require` printed `computed-cjs-loaded` and `true` and exited successfully.
- Build guard: `cargo check -p wjsm-runtime -p wjsm-cli` passed with no warnings after the final source changes.


Task 6 spec-compliance repair — runtime/backend import boundary and unsupported extension coverage:

- Moved runtime-loaded WASM host-import name knowledge out of `wjsm-runtime`: `RuntimeModuleInstantiationContext` now accepts runtime-owned `RuntimeModuleImportLink` descriptors, while `wjsm-cli` maps `wjsm_backend_wasm::host_import_registry::host_import_specs()` to those descriptors before instantiation.
- Boundary check: `grep` for `wjsm_backend_wasm::host_import_registry`, `host_import_registry`, and `host_import_specs(` in `crates/wjsm-runtime/src/runtime_module_loader.rs` returned no matches.
- Added `modules__runtime_loading__rejects_runtime_ts_tsx_jsx`, which runtime-loads computed `require('./dep.' + ext)` targets for `.ts`, `.tsx`, and `.jsx` and asserts the unsupported runtime-loader error text is observable.
- Acceptance: `cargo check -p wjsm-runtime -p wjsm-cli` passed after final source/test edits — `Finished dev profile [unoptimized + debuginfo] target(s) in 0.25s`.
- Acceptance: `cargo nextest run -E 'test(modules__runtime_loading__cjs_computed_require) | test(modules__runtime_loading__esm_dynamic_import_variable)'` passed — `Summary [   0.054s] 2 tests run: 2 passed, 774 skipped`.
- Acceptance: `cargo nextest run -p wjsm-runtime -E 'test(dynamic_module_loader_unavailable) | test(require_loader_unavailable)'` passed — `Summary [   0.041s] 2 tests run: 2 passed, 228 skipped`.
- New extension coverage: `cargo nextest run -E 'test(modules__runtime_loading__rejects_runtime_ts_tsx_jsx)'` passed after final test-source edit — `Summary [   0.175s] 1 test run: 1 passed, 775 skipped`.
- CLI smoke: `cargo run -- run fixtures/modules/runtime_loading/cjs_computed_require/main.js --root fixtures/modules/runtime_loading/cjs_computed_require` printed `computed-cjs-loaded` and `true` and exited successfully.

Task 6 quality-review repair — runtime loader diagnostics:

- RED before diagnostic owner fix: `cargo nextest run -p wjsm-runtime -E 'test(dynamic_module_loader_unavailable) | test(require_loader_unavailable)'` failed because both paths emitted `TypeError\nfalse\ntrue\n`, proving the shared loader-unavailable boundary still said `CommonJS` and did not include the generic `runtime module loader is not installed` wording.
- RED before diagnostic owner fix: `cargo nextest run -E 'test(modules__runtime_loading__rejects_runtime_ts_tsx_jsx)'` failed with `true\ntrue\nfalse\n`, proving unsupported `.ts/.tsx/.jsx` runtime loads were still wrapped in `Cannot find module`.
- Repair: `crates/wjsm-runtime/src/host_imports/modules.rs` now emits a generic `runtime module loader is not installed` unavailable-loader diagnostic and renders `RuntimeModuleLoadErrorCode::NotFound` with `Cannot find module` while preserving unsupported/invalid/instantiate loader messages directly.
- Acceptance: `cargo nextest run -p wjsm-runtime -E 'test(dynamic_module_loader_unavailable) | test(require_loader_unavailable)'` passed — `Summary [   0.090s] 2 tests run: 2 passed, 228 skipped`.
- Acceptance: `cargo nextest run -E 'test(modules__runtime_loading__rejects_runtime_ts_tsx_jsx)'` passed — `Summary [   0.885s] 1 test run: 1 passed, 775 skipped`.
- Acceptance: `cargo nextest run -E 'test(modules__runtime_loading__cjs_computed_require) | test(modules__runtime_loading__esm_dynamic_import_variable)'` passed — `Summary [   0.054s] 2 tests run: 2 passed, 774 skipped`.
- NotFound guard: `cargo nextest run -p wjsm-runtime -E 'test(require_resolve_uses_installed_loader_and_paths)'` passed — `Summary [   0.057s] 1 test run: 1 passed, 229 skipped`.

### Task 7 — Runtime module loading fixture matrix

- Added runtime-loading fixtures for `cjs_try_optional_missing`, `cjs_require_json`, `cjs_require_cache_delete`, `cjs_circular_partial_exports`, and `require_resolve_paths`; wired them into `tests/integration/fixtures.rs` so they exercise the real CLI filesystem runtime loader.
- RED before implementation: `cargo nextest run -E 'test(modules__runtime_loading__) | test(modules__cjs_conditional_require_false)'` failed for JSON require because top-level `require('./data.json')` was hoisted into the static module graph and parsed as JS, for circular CJS because transformed synthetic exports hid partial runtime `exports`, and for `require.resolve.paths` because `require.resolve.paths(...)` did not put a runtime-loaded file in CommonJS goal.
- Repair: JSON CJS requires now stay on the runtime path; CLI runtime loading compiles CommonJS runtime entries as CommonJS source instead of the CJS-to-ESM graph transform; CLI JSON runtime entries parse through the runtime JSON parser and create a `module`/`exports` cache entry; `require.resolve.*` member access marks files as CommonJS for runtime lowering.
- Added focused module/runtime guards for JSON require hoisting, `require.resolve.paths` CommonJS detection, loaded JSON cache deletion, and errored file cache deletion.
- Reviewed the new expected outputs manually after the fixture matrix passed; no `WJSM_UPDATE_FIXTURES=1` update was used.
- Transform guard: `cargo nextest run -p wjsm-module -E 'test(top_level_json_require_remains_runtime_call) | test(detects_cjs_via_require_resolve_paths_member)'` passed — `Summary [   0.015s] 2 tests run: 2 passed, 223 skipped`.
- Acceptance: `cargo nextest run -E 'test(modules__runtime_loading__) | test(modules__cjs_conditional_require_false)'` passed — `Summary [   0.267s] 10 tests run: 10 passed, 776 skipped`.
- Acceptance: `cargo nextest run -p wjsm-runtime -E 'test(require_cache) | test(module_registry_) | test(require_resolve)'` passed — `Summary [   0.257s] 16 tests run: 16 passed, 216 skipped`.

### Task 7 spec-review blocker repairs — JSON import and errored runtime CJS cache

- Added CLI runtime-loading fixtures `esm_dynamic_import_json_rejected` and `cjs_errored_cache_delete_retry`; wired both into `tests/integration/fixtures.rs`. The JSON fixture uses a computed dynamic import target so it exercises the runtime loader boundary, while the existing `cjs_require_json` fixture remains in the same selector to guard CJS JSON `require()`.
- RED before repair: `cargo nextest run -E 'test(modules__runtime_loading__esm_dynamic_import_json_rejected) | test(modules__runtime_loading__cjs_errored_cache_delete_retry)'` failed with JSON dynamic import resolving successfully and errored CJS retry observing a stale loaded partial cache entry (`require.cache[id] === undefined` was `false`, second require loaded `stale-partial`).
- Repair: runtime dynamic import now rejects `RuntimeModuleFormat::Json` with `runtime JSON import is unsupported without import assertions`; runtime-loaded CommonJS modules are preseeded as `Loading`, the CJS prologue refreshes that loading entry instead of marking it loaded, successful body completion promotes it to `Loaded`, and body exceptions mark an `Errored` entry that is hidden from `require.cache` but removable for retry.
- Targeted guard: `cargo nextest run -E 'test(modules__runtime_loading__esm_dynamic_import_json_rejected) | test(modules__runtime_loading__cjs_errored_cache_delete_retry)'` passed — `Summary [   0.124s] 2 tests run: 2 passed, 788 skipped`.
- Acceptance after final helper cleanup: `cargo nextest run -E 'test(modules__runtime_loading__) | test(modules__cjs_conditional_require_false)'` passed — `Summary [   0.280s] 12 tests run: 12 passed, 778 skipped`.
- Acceptance after final helper cleanup: `cargo nextest run -p wjsm-runtime -E 'test(require_cache) | test(module_registry_) | test(require_resolve)'` passed — `Summary [   0.144s] 16 tests run: 16 passed, 216 skipped`.

Task 7 quality-review repair — explicit ESM runtime format preservation:

- Added `fixtures/modules/runtime_loading/explicit_esm_require_resolve_paths`, covering runtime-loaded `.mjs` and package `type: module` `.js` files that contain `require.resolve.paths('pkg')`; the final fixture keeps the AST member access present with a local `var require` and asserts ESM execution does not receive CommonJS `module`/`exports` bindings.
- RED before repair, using the same runtime-loaded explicit ESM targets with an initial `typeof require` assertion: `cargo nextest run -E 'test(modules__runtime_loading__explicit_esm_require_resolve_paths)'` failed with actual stdout showing `mjs require type: function` and `type-module require type: function`, proving the runtime loader downgraded explicit ESM targets to CommonJS when the CJS detector saw `require.resolve.paths`.
- Repair: `crates/wjsm-cli/src/runtime_loader.rs` now keeps resolver-selected explicit extension/package formats authoritative and runs the CJS AST probe only for no-package `.js` files whose resolver fallback is ESM, preserving the existing ambiguous `.js` `require.resolve.paths` CommonJS path.
- Explicit ESM regression: `cargo nextest run -E 'test(modules__runtime_loading__explicit_esm_require_resolve_paths)'` passed — `Summary [   0.082s] 1 test run: 1 passed, 791 skipped`.
- Ambiguous `.js` guard: `cargo nextest run -E 'test(modules__runtime_loading__require_resolve_paths)'` passed — `Summary [   0.075s] 1 test run: 1 passed, 791 skipped`.
- Acceptance: `cargo nextest run -E 'test(modules__runtime_loading__) | test(modules__cjs_conditional_require_false)'` passed — `Summary [   0.329s] 13 tests run: 13 passed, 779 skipped`.

Task 7 quality-review repair — extensionless runtime CommonJS probe:

- Added `fixtures/modules/runtime_loading/extensionless_cjs_require`, covering a runtime `require('./child')` inside a non-hoistable branch that resolves an exact extensionless file with `require.resolve.paths('pkg')`, `exports.*`, and `module.exports === exports` CommonJS binding checks; wired it into `tests/integration/fixtures.rs`.
- RED before repair: `cargo nextest run -p wjsm-cli -E 'test(runtime_commonjs_probe_includes_extensionless_no_package)'` failed because `should_probe_runtime_commonjs` returned false for a no-package extensionless path.
- Repair: `crates/wjsm-cli/src/runtime_loader.rs` now runs the CommonJS AST probe for ambiguous no-package `.js` and extensionless files only; `.mjs`, already-CommonJS fallbacks, and package-manifest `.js` files remain outside the second-pass probe.
- Helper guard: `cargo nextest run -p wjsm-cli -E 'test(runtime_commonjs_probe_)'` passed — `Summary [   0.009s] 2 tests run: 2 passed, 2 skipped`.
- Explicit ESM regression: `cargo nextest run -E 'test(modules__runtime_loading__explicit_esm_require_resolve_paths)'` passed — `Summary [   0.077s] 1 test run: 1 passed, 793 skipped`.
- Ambiguous `.js` guard: `cargo nextest run -E 'test(modules__runtime_loading__require_resolve_paths)'` passed — `Summary [   0.058s] 1 test run: 1 passed, 793 skipped`.
- Extensionless runtime CJS guard: `cargo nextest run -E 'test(modules__runtime_loading__extensionless_cjs_require)'` passed — `Summary [   0.087s] 1 test run: 1 passed, 793 skipped`.
- Acceptance: `cargo nextest run -E 'test(modules__runtime_loading__) | test(modules__cjs_conditional_require_false)'` passed — `Summary [   0.325s] 14 tests run: 14 passed, 780 skipped`.

### Task 8 — ADR and closeout records

- ADR creation gate source files requested by `recording-architecture-decisions` (`docs/adr/ADR-CREATION-GATE.md` and `docs/current/AEGIS_ADR_AUTO_BACKFILL.md`) are not present in this repository, so the decision used the existing project ADR style in `docs/adr/0002-*`, `0003-*`, and `0004-*` plus the approved issue #312 spec/plan as authority.
- ADR gate result: create. The runtime module loading boundary is durable, surprising without context, and has a real trade-off: injected CLI loader + runtime registry vs eager-only loading vs compiler-in-runtime.
- Wrote `docs/adr/0006-runtime-module-loading-boundary.md` documenting canonical owners, dependency boundary, alternatives, compatibility, implementation state, and references.
- Updated `docs/aegis/INDEX.md` Baselines with ADR 0006.
- Baseline sync: required and satisfied by ADR 0006 plus the Aegis index baseline entry; no separate baseline snapshot was created because this repository already uses project `docs/adr/` as the architecture baseline owner for runtime boundaries.

### Final targeted verification — 2026-07-07

- `cargo check -p wjsm-runtime -p wjsm-cli` passed — `cargo build (0 crates compiled)` and `Finished dev profile [unoptimized + debuginfo] target(s) in 0.49s`.
- `cargo nextest run -p wjsm-module -E 'test(cjs_) | test(runtime_resolve_) | test(resolve_paths_) | test(dynamic_import) | test(runtime_commonjs_probe_)'` passed — `Summary [   0.169s] 106 tests run: 106 passed, 119 skipped`.
- `cargo nextest run -p wjsm-semantic -E 'test(dynamic_import) | test(import_meta_resolve) | test(require_runtime)'` passed — `Summary [   0.024s] 14 tests run: 14 passed, 134 skipped`.
- `cargo nextest run -p wjsm-runtime -E 'test(dynamic_module) | test(import_meta_resolve) | test(require_cache) | test(module_registry_) | test(require_resolve) | test(dynamic_module_loader_unavailable) | test(require_loader_unavailable)'` passed — `Summary [   0.163s] 31 tests run: 31 passed, 201 skipped`.
- `cargo nextest run -p wjsm-backend-wasm -E 'test(builtin_registry_binding_count_matches_ir_contract) | test(import_names_are_unique) | test(builtin_bindings_are_unique) | test(host_imports_count_locked)'` passed — `Summary [   0.021s] 4 tests run: 4 passed, 59 skipped`.
- `cargo nextest run -E 'test(modules__runtime_loading__) | test(modules__cjs_conditional_require_false) | test(modules__dynamic_import)'` passed — `Summary [   0.406s] 17 tests run: 17 passed, 777 skipped`.
- CLI smoke `cargo run -- run fixtures/modules/runtime_loading/cjs_computed_require/main.js --root fixtures/modules/runtime_loading/cjs_computed_require` printed `computed-cjs-loaded` and `true`.
- CLI smoke `cargo run -- run fixtures/modules/runtime_loading/esm_dynamic_import_variable/main.js --root fixtures/modules/runtime_loading/esm_dynamic_import_variable` printed `dynamic-esm-loaded`.