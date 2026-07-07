# Runtime Module Loading 实现计划（issue #312）

Goal: 实现 issue #312：支持运行时 CJS `require()`、动态 `import(expr)`、`import.meta.resolve()`、`require.resolve()` / `require.cache`、JSON require 与统一 Runtime Module Registry。

Architecture: `wjsm-module` 继续拥有 specifier 解析；`wjsm-runtime` 拥有 registry/cache/host imports/GC roots；CLI 或独立 orchestrator 安装 `RuntimeModuleLoader`，调用 `wjsm-module` + backend 编译并把动态实例接入同一 runtime env。runtime crate 不依赖 parser/semantic/backend。

Tech Stack: Rust 2024，`anyhow` / existing runtime error helpers，`wasmtime` 多实例共享 memory/table/globals，现有 `ResolutionOptions` / `ResolutionKind`，现有 fixture runner 与 nextest。

Baseline/Authority Refs:

- `docs/aegis/specs/2026-07-07-runtime-module-loading-design.md`
- issue #312
- `AGENTS.md`：AOT pipeline、crate dependency direction、Rust 2024、注释中文、文件/函数体量纪律、ECMAScript spec compliance hard rules
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

Compatibility Boundary:

- 静态 import/export 与 issue #309 package resolution 行为保持。
- 顶层无条件字面量 require 继续走编译期 bundle 快路径。
- 控制流内字面量 require 改为运行时调用；`cjs_conditional_require_false` 的旧副作用输出是旧实现偏差，需要修正。
- 运行时未安装 loader 时，静态已预注册 dynamic import 仍工作；表达式动态加载抛明确错误 / Promise reject。
- runtime 不执行 TypeScript 即时编译；`.ts/.tsx/.jsx` 只在初始编译期图里处理。

Verification:

- `cargo nextest run -p wjsm-module -E 'test(cjs_) | test(runtime_resolve_) | test(resolve_paths)'`
- `cargo nextest run -p wjsm-semantic -E 'test(dynamic_import) | test(require_runtime) | test(import_meta_resolve)'`
- `cargo nextest run -p wjsm-runtime -E 'test(module_registry) | test(require_cache) | test(dynamic_module)'`
- `cargo nextest run -E 'test(modules__runtime_loading_) | test(modules__cjs_conditional_require_false)'`
- `cargo run -- run fixtures/modules/runtime_loading/cjs_computed_require/main.js --root fixtures/modules/runtime_loading/cjs_computed_require`
- `cargo run -- run fixtures/modules/runtime_loading/esm_dynamic_import_template/main.js --root fixtures/modules/runtime_loading/esm_dynamic_import_template`

## Plan Basis

Facts:

- `cjs_transform.rs` 当前 `extract_require_specifier` 只识别字符串字面量与无插值模板字符串，并把所有收集到的 require 转成顶层 import。
- `fixtures/modules/cjs_conditional_require_false/main.expected` 当前包含 `side effect`，证明 false 分支模块被提前执行。
- `resolver.rs` 与 `lower_dynamic_import_call` 都会拒绝带表达式的 `import()`。
- runtime host import `dynamic_import(i64)` 只按 `module_namespace_cache: HashMap<u32, i64>` 查 namespace。
- runtime GC roots 已扫 `module_namespace_cache`，后续 registry values 必须进入 roots。
- issue #309 已建立 `ResolutionKind::{Import, Require}` 与 condition order，可复用为 runtime resolve 权威。

Assumptions:

- 本 issue 允许修正 false-branch require 的 fixture 预期，因为这是 Node-compatible 语义修复。
- `require(esm)` 继续沿 wjsm 当前 CJS/ESM interop 返回 namespace/default，不引入 Node 当前 `ERR_REQUIRE_ESM` 限制。
- CLI 是首个 loader 安装者；如果执行中发现单独 crate 能显著降低依赖耦合，命名为 `wjsm-runtime-loader` 并保持只被 CLI 依赖。

Unknowns:

- wasmtime 动态实例接入当前 `Store<RuntimeState>` 的具体 helper 需要在实现 Task 6 时以当前 runtime linker 代码为准细化；这不改变 owner 边界。

## BaselineUsageDraft

- Required baseline refs: design spec, issue #312, AGENTS.md, issue #309 spec/plan/current resolver code, dynamic import semantic/runtime code, GC roots。
- Delivered context refs: AGENTS.md、issue #312。
- Acknowledged before plan refs: all required refs above。
- Cited in plan refs: Goal, Architecture, Compatibility Boundary, Plan Basis, tasks below。
- Missing refs: no blocking refs; wasmtime helper signatures are task-local implementation evidence。
- Decision: continue。

## Files

Create:

- `crates/wjsm-module/src/cjs_require_analysis.rs`
- `crates/wjsm-module/src/runtime_resolution.rs`
- `crates/wjsm-runtime/src/runtime_module_loader.rs`
- `crates/wjsm-runtime/src/runtime_module_registry.rs`
- `crates/wjsm-runtime/src/host_imports/modules.rs`
- `crates/wjsm-cli/src/runtime_loader.rs` or `crates/wjsm-runtime-loader/src/lib.rs` if adding a crate is cleaner after checking dependencies
- `docs/adr/0006-runtime-module-loading-boundary.md`
- `fixtures/modules/runtime_loading/**`

Modify:

- `crates/wjsm-module/src/lib.rs`
- `crates/wjsm-module/src/cjs_transform.rs`
- `crates/wjsm-module/src/graph.rs`
- `crates/wjsm-module/src/resolver.rs`
- `crates/wjsm-semantic/src/lowerer_modules.rs`
- `crates/wjsm-semantic/src/lowerer_async_eval/async_import_promise.rs`
- `crates/wjsm-semantic/src/lowerer_jsx_objects/jsx_expressions.rs`
- `crates/wjsm-semantic/src/lowerer_types.rs`
- `crates/wjsm-ir/src/builtin.rs`
- `crates/wjsm-backend-wasm/src/compiler_builtins_async_proxy.rs`
- `crates/wjsm-backend-wasm/src/host_import_registry/specs_part*.rs`
- `crates/wjsm-runtime/src/lib.rs`
- `crates/wjsm-runtime/src/host_imports/mod.rs`
- `crates/wjsm-runtime/src/runtime_gc/roots.rs`
- `crates/wjsm-cli/src/lib.rs`
- `fixtures/modules/cjs_conditional_require_false/main.expected`
- `Cargo.toml` only if a new loader crate is introduced

## Compatibility

- Static fast path remains an optimization, not a semantic owner.
- Runtime registry is the source-of-truth for loaded module identity.
- Resolver runtime API must call the same package/exports/imports logic as compile-time graph resolution.
- `require.cache` exposes registry state, not a copied object.
- New dynamic loader reads source only inside configured root/read roots.

## Architecture Integrity Lens

- Invariant: runtime does not depend on compiler crates; module identity is canonical key, not user string or raw ModuleId.
- Canonical owner / contract: `runtime_resolution.rs` for path resolution API, `runtime_module_registry.rs` for cache/state, loader trait for compile/instantiate bridge.
- Responsibility overlap: `module_namespace_cache` must stop being the sole dynamic import cache; static dynamic import can use registry `by_module_id`.
- Higher-level path: add host imports around registry operations instead of adding scattered resolver calls in lowerer/backend.
- Retirement / falsifier: false-branch require no longer executes target; expression import no longer emits AOT-only diagnostic; deleting require.cache forces re-execution.
- Verdict: proceed with split owners.

## Plan Pressure Test

- Owner / contract / retirement: clear split; old AOT-only require/import paths are retired only after runtime path tests pass.
- Architecture integrity / higher-level path: registry + loader trait is the source-of-truth; no duplicate resolver in runtime.
- Verification scope: unit tests per owner, semantic snapshots, runtime registry tests, module fixtures, CLI smoke.
- Task executability: tasks below are ordered so each creates one owner or one wiring layer.
- Pressure result: proceed.

## Plan-Time Complexity Check

Complexity Budget:

- Artifact class: core cross-crate runtime loading architecture.
- Target files / artifacts: module resolver/CJS transform, semantic lowerers, IR/backend host imports, runtime registry/loader/host imports/GC roots, CLI loader, fixtures, ADR.
- Current pressure: `resolver.rs` 1576 lines, `cjs_transform.rs` 984 lines, runtime `lib.rs` 2115 lines.
- Projected post-change pressure: acceptable only with new owner files; direct growth in existing large files is over-budget.
- Budget result: over-budget without split; within governed bounds with owner files.
- Planned governance: new files carry new responsibilities; existing files only delegate/wire.

Plan-Time Complexity Check:

- Target files: see Files section.
- Existing size / shape signals: resolver/CJS transform/runtime lib are already high-pressure.
- Owner fit: registry and loader do not belong in `lib.rs`; require analysis does not belong inside transform traversal.
- Add-in-place risk: high.
- Better file boundary: `cjs_require_analysis.rs`, `runtime_resolution.rs`, `runtime_module_registry.rs`, `runtime_module_loader.rs`, `host_imports/modules.rs`, CLI loader module.
- Recommendation: add owner files; edit existing files for delegation only.

## Task 1 — Split CJS require analysis and preserve runtime require sites

Files:

- Create `crates/wjsm-module/src/cjs_require_analysis.rs`
- Modify `crates/wjsm-module/src/lib.rs`
- Modify `crates/wjsm-module/src/cjs_transform.rs`
- Modify `crates/wjsm-module/src/cjs_transform_tests.rs`
- Modify `fixtures/modules/cjs_conditional_require_false/main.expected`

Why: `require()` inside control flow must execute only when that control path runs. This is the visible CJS semantic repair that unlocks runtime require without breaking top-level fast path.

Impact/Compatibility: top-level no-control-flow literal require remains import-transformed; control-flow and computed require remain in AST for runtime lowering.

Repair Track:

- Root cause: transform collector treats every statically extractable require as safe for top-level import hoisting.
- Canonical owner: new `cjs_require_analysis.rs` classifies require call sites before transform.
- Stable repair: collect only hoistable top-level require into import map; leave runtime sites as `require(...)` calls.
- Compat boundary: direct top-level `const x = require('./x')` behavior and graph resolution stay unchanged.
- Verification: cjs transform tests plus module fixture false branch.

Retirement Track:

- Old owner/fallback: `RequireCollector` recursive visitor in `cjs_transform.rs` as the sole classifier.
- Active status after task: retired for classification; transform still consumes analyzer output.
- Deletion trigger: no recursive collector path that hoists control-flow require remains.

Steps:

1. Write tests.
   - Add tests:
     - `top_level_literal_require_still_generates_import`
     - `if_false_literal_require_remains_runtime_call`
     - `try_literal_require_remains_runtime_call`
     - `computed_require_remains_runtime_call`
     - `function_body_literal_require_remains_runtime_call`
   - Tests assert transformed AST import count and remaining call expression count.
2. Verify RED.
   - Command: `cargo nextest run -p wjsm-module -E 'test(top_level_literal_require_still_generates_import) | test(if_false_literal_require_remains_runtime_call) | test(try_literal_require_remains_runtime_call) | test(computed_require_remains_runtime_call) | test(function_body_literal_require_remains_runtime_call)'`
   - Expected: new tests fail because existing collector hoists control-flow literal require and has no computed runtime classification.
3. Implement code.
   - `cjs_require_analysis.rs` defines `RequireSiteKind::{HoistableStatic, Runtime}` and `RequireAnalysis { hoistable: BTreeMap<String, String>, runtime_sites: usize }`.
   - Analyzer walks only top-level module statements as hoistable when call is not nested in control flow/function/class and the specifier is static.
   - `cjs_transform.rs` consumes `RequireAnalysis.hoistable`; `transform_call_expr` replaces only hoistable specifiers and preserves all runtime calls.
   - Existing helper `extract_static_module_specifier` moves or is re-exported crate-private from analyzer.
   - Update false-branch expected output to remove `side effect` and keep only `undefined`.
4. Verify GREEN.
   - Command: `cargo nextest run -p wjsm-module -E 'test(top_level_literal_require_still_generates_import) | test(if_false_literal_require_remains_runtime_call) | test(try_literal_require_remains_runtime_call) | test(computed_require_remains_runtime_call) | test(function_body_literal_require_remains_runtime_call)'`
   - Expected: selected tests pass.
   - Command: `cargo nextest run -E 'test(modules__cjs_conditional_require_false)'`
   - Expected: fixture passes with no side-effect output.
5. Commit.
   - Message: `fix: preserve runtime cjs require sites for #312`

## Task 2 — Add runtime resolution API in wjsm-module

Files:

- Create `crates/wjsm-module/src/runtime_resolution.rs`
- Modify `crates/wjsm-module/src/lib.rs`
- Modify `crates/wjsm-module/src/resolver.rs`
- Modify `crates/wjsm-module/src/resolver_tests.rs`

Why: runtime host imports need the exact same resolver semantics as compile-time graph resolution, without copying resolver logic into runtime.

Impact/Compatibility: no WASM/runtime behavior changes until host imports call this API.

Repair Track:

- Root cause: resolver entry points return `ModuleId` / loaded AST modules, not a plain canonical runtime target.
- Canonical owner: `runtime_resolution.rs` wraps `ModuleResolver` package/path logic and returns plain DTO.
- Stable repair: one resolver API for `require`, `import`, `import.meta.resolve`, and `require.resolve.paths`.
- Compat boundary: compile-time `resolve_with_kind` behavior remains unchanged.
- Verification: resolver unit tests.

Retirement Track:

- Old owner/fallback: no runtime resolve owner exists.
- Active status: new owner becomes canonical for runtime resolution.

Steps:

1. Write tests.
   - Add resolver tests:
     - `runtime_resolve_relative_file_returns_file_key`
     - `runtime_resolve_package_uses_require_condition`
     - `runtime_resolve_import_meta_uses_import_condition`
     - `runtime_resolve_builtin_returns_builtin_key`
     - `resolve_paths_for_bare_package_lists_node_modules_parents`
     - `resolve_paths_for_relative_returns_null_marker`
2. Verify RED.
   - Command: `cargo nextest run -p wjsm-module -E 'test(runtime_resolve_) | test(resolve_paths_)'`
   - Expected: compile failure or failing tests because runtime API does not exist.
3. Implement code.
   - Define `RuntimeResolveKind::{Import, Require}` mirroring internal `ResolutionKind` without exposing private internals.
   - Define `RuntimeResolvedModule { key, path, url, format }` and `RuntimeModuleKey::{File, Json, Builtin}`.
   - Add `resolve_runtime_specifier(specifier, referrer_path, root, options, kind) -> Result<RuntimeResolvedModule>`.
   - Add `resolve_runtime_paths(specifier, referrer_path, root) -> RuntimeResolvePaths` where `RuntimeResolvePaths::Null` maps to JS null and `Search(Vec<PathBuf>)` maps to array.
   - Reuse resolver package/builtin/path functions; do not duplicate package target logic.
   - `.json` is classified as `RuntimeModuleKey::Json`.
4. Verify GREEN.
   - Command: `cargo nextest run -p wjsm-module -E 'test(runtime_resolve_) | test(resolve_paths_)'`
   - Expected: selected tests pass.
5. Commit.
   - Message: `feat: expose runtime module resolution for #312`

## Task 3 — Add Runtime Module Registry and loader contract

Files:

- Create `crates/wjsm-runtime/src/runtime_module_registry.rs`
- Create `crates/wjsm-runtime/src/runtime_module_loader.rs`
- Modify `crates/wjsm-runtime/src/lib.rs`
- Modify `crates/wjsm-runtime/src/runtime_gc/roots.rs`

Why: `module_namespace_cache: ModuleId -> namespace` cannot represent require.cache, partial loading, errored modules, JSON, or delete semantics.

Impact/Compatibility: static dynamic import continues to work after compatibility wiring; registry values become GC roots.

Repair Track:

- Root cause: module cache has the wrong key and no state machine.
- Canonical owner: `RuntimeModuleRegistry`.
- Stable repair: path/builtin/json key + Loading/Loaded/Errored states + cache view operations.
- Compat boundary: existing `register_module_namespace` and `dynamic_import(module_id)` can delegate through `by_module_id`.
- Verification: runtime registry unit tests and GC roots tests.

Retirement Track:

- Old owner/fallback: `module_namespace_cache` as full dynamic import cache.
- Active status after task: retained only as transition field if needed, with registry as canonical owner.
- Deletion trigger: all host imports read registry; roots scan registry; no direct cache reads outside registry adapter.

Steps:

1. Write tests.
   - Add runtime tests:
     - `module_registry_returns_loaded_namespace_by_module_id`
     - `module_registry_returns_loading_exports_for_circular_require`
     - `module_registry_delete_loaded_file_entry`
     - `module_registry_refuses_delete_loading_entry`
     - `module_registry_errored_entry_is_rooted`
     - `module_registry_roots_include_exports_namespace_and_error`
2. Verify RED.
   - Command: `cargo nextest run -p wjsm-runtime -E 'test(module_registry_)'`
   - Expected: compile failure or failing tests because registry owner does not exist.
3. Implement code.
   - `runtime_module_loader.rs` defines plain DTOs and `RuntimeModuleLoader` trait using runtime-owned types only.
   - `runtime_module_registry.rs` defines `RuntimeModuleKey`, `RuntimeModuleState`, `RuntimeModuleRegistry`, and methods `begin_loading`, `finish_loaded`, `finish_errored`, `get_for_require`, `get_namespace_by_module_id`, `delete_cache_entry`, `roots`.
   - `RuntimeState` owns `Arc<Mutex<RuntimeModuleRegistry>>`.
   - GC roots call `registry.roots()`.
   - Keep old field only if host import migration requires staged wiring; mark it internal transition in code comments if retained.
4. Verify GREEN.
   - Command: `cargo nextest run -p wjsm-runtime -E 'test(module_registry_)'`
   - Expected: selected tests pass.
5. Commit.
   - Message: `feat: add runtime module registry for #312`

## Task 4 — Wire CJS require, require.resolve, and require.cache host behavior

Files:

- Create `crates/wjsm-runtime/src/host_imports/modules.rs`
- Modify `crates/wjsm-runtime/src/host_imports/mod.rs`
- Modify `crates/wjsm-runtime/src/lib.rs`
- Modify `crates/wjsm-semantic/src/lowerer_modules.rs`
- Modify `crates/wjsm-semantic/src/lowerer_types.rs`
- Modify `crates/wjsm-ir/src/builtin.rs`
- Modify `crates/wjsm-backend-wasm/src/compiler_builtins_async_proxy.rs`
- Modify `crates/wjsm-backend-wasm/src/host_import_registry/specs_part*.rs`

Why: preserved runtime `require(...)` calls need module-local bindings and host imports that route through registry/loader.

Impact/Compatibility: CJS modules gain real `require`, `require.resolve`, `require.cache`, `module`, and `exports` semantics; static fast path remains.

Repair Track:

- Root cause: CJS transform can leave `require(...)` calls, but semantic currently has no CJS require binding.
- Canonical owner: CJS scope initialization in semantic + runtime module host imports.
- Stable repair: create module-local native callable capturing module id/referrer and expose properties on that callable.
- Compat boundary: `__filename`/`__dirname` existing behavior stays.
- Verification: semantic snapshots, runtime tests, CJS fixtures.

Retirement Track:

- Old owner/fallback: unbound `require` identifier after transform preservation.
- Active status: retired once CJS scope binding is installed.

Steps:

1. Write tests.
   - Add semantic snapshots for CJS module scope containing runtime require.
   - Add runtime tests for `require.resolve`, `require.resolve.paths`, `require.cache` delete, and missing module thrown inside try/catch.
   - Add fixtures `runtime_loading/cjs_try_optional_missing`, `runtime_loading/cjs_require_cache_delete`, `runtime_loading/cjs_computed_require`.
2. Verify RED.
   - Command: `cargo nextest run -p wjsm-semantic -E 'test(require_runtime)'`
   - Expected: compile failure or snapshot failure.
   - Command: `cargo nextest run -p wjsm-runtime -E 'test(require_cache) | test(require_resolve)'`
   - Expected: compile failure or failing tests.
3. Implement code.
   - Add IR builtins for creating module-local require callable and runtime require invocation if direct native callable dispatch is not enough.
   - In semantic CJS module setup, declare and initialize `require`, `module`, `exports`, `__filename`, `__dirname`.
   - Runtime native callable performs ToString, calls loader resolve, uses registry states, executes JSON/module load, and returns exports or throws.
   - Attach `resolve`, `resolve.paths`, and `cache` properties to the require function.
   - Implement cache view as host-backed object or proxy trap; it must support get, delete, and ownKeys for path keys.
4. Verify GREEN.
   - Command: `cargo nextest run -p wjsm-semantic -E 'test(require_runtime)'`
   - Expected: selected snapshots/tests pass.
   - Command: `cargo nextest run -p wjsm-runtime -E 'test(require_cache) | test(require_resolve)'`
   - Expected: selected runtime tests pass.
   - Command: `cargo nextest run -E 'test(modules__runtime_loading__cjs_) | test(modules__cjs_conditional_require_false)'`
   - Expected: CJS runtime loading fixtures pass.
5. Commit.
   - Message: `feat: implement runtime cjs require for #312`

## Task 5 — Wire dynamic import expressions and import.meta.resolve

Files:

- Modify `crates/wjsm-semantic/src/lowerer_async_eval/async_import_promise.rs`
- Modify `crates/wjsm-semantic/src/lowerer_jsx_objects/jsx_expressions.rs`
- Modify `crates/wjsm-semantic/src/lowerer_types.rs`
- Modify `crates/wjsm-ir/src/builtin.rs`
- Modify `crates/wjsm-backend-wasm/src/compiler_builtins_async_proxy.rs`
- Modify `crates/wjsm-backend-wasm/src/host_import_registry/specs_part*.rs`
- Modify `crates/wjsm-runtime/src/host_imports/modules.rs`
- Modify `crates/wjsm-runtime/src/host_imports/misc.rs`

Why: `import(expr)` and `import.meta.resolve()` are issue #312 ESM requirements and should share registry/loader semantics with CJS require.

Impact/Compatibility: static `import('./literal')` stays fast; expression import returns Promise rather than compile-time error.

Repair Track:

- Root cause: semantic dynamic import lowerer only accepts compile-time string target.
- Canonical owner: dynamic import lowerer chooses static vs runtime path; runtime host import resolves/loads through registry.
- Stable repair: runtime host import accepts referrer + specifier value and fulfills/rejects Promise.
- Compat boundary: existing static dynamic import fixtures stay green.
- Verification: semantic tests and ESM runtime_loading fixtures.

Retirement Track:

- Old owner/fallback: AOT-only diagnostics in resolver/semantic for expression `import()`.
- Active status: diagnostics only remain for malformed zero-arg import; expression path becomes runtime.

Steps:

1. Write tests.
   - Add semantic snapshots for `import(path)` and `` import(`./locale/${lang}.js`) ``.
   - Add fixtures `runtime_loading/esm_dynamic_import_template`, `runtime_loading/esm_dynamic_import_variable`, `runtime_loading/esm_import_meta_resolve`.
   - Add runtime test for dynamic import rejection on missing module.
2. Verify RED.
   - Command: `cargo nextest run -p wjsm-semantic -E 'test(dynamic_import) | test(import_meta_resolve)'`
   - Expected: expression import still fails before implementation.
3. Implement code.
   - In resolver dynamic import extraction, static strings continue to populate graph; expression imports are recorded as runtime sites without compile-time resolution.
   - In semantic, static string + target mapping uses existing module-id host call; non-static argument lowers expression, ToString, and calls runtime import host import with current module id/referrer.
   - `import.meta` object gains `resolve` method bound to current module metadata.
   - Runtime host import creates Promise, calls registry/loader, fulfills namespace or rejects Error value.
   - Static `dynamic_import(i64)` delegates to registry by module id.
4. Verify GREEN.
   - Command: `cargo nextest run -p wjsm-semantic -E 'test(dynamic_import) | test(import_meta_resolve)'`
   - Expected: selected semantic tests pass.
   - Command: `cargo nextest run -E 'test(modules__dynamic_import) | test(modules__runtime_loading__esm_)'`
   - Expected: existing and new dynamic import fixtures pass.
5. Commit.
   - Message: `feat: support runtime dynamic import for #312`

## Task 6 — Install CLI runtime loader and shared-instance instantiation

Files:

- Create `crates/wjsm-cli/src/runtime_loader.rs` or create `crates/wjsm-runtime-loader/src/lib.rs`
- Modify `crates/wjsm-cli/src/lib.rs`
- Modify `crates/wjsm-runtime/src/lib.rs`
- Modify `crates/wjsm-runtime/src/runtime_linker.rs` if linker env helpers are needed
- Modify workspace `Cargo.toml` only if adding a new crate

Why: runtime needs an injected owner that can call parser/module/backend without making runtime depend on compiler crates.

Impact/Compatibility: normal `cargo run -- run file.js` installs loader automatically for file-backed modules; `execute(&[u8])` library path without loader still executes static modules and rejects true runtime dynamic loads clearly.

Repair Track:

- Root cause: host imports can request loads, but no orchestrator compiles and instantiates requested modules.
- Canonical owner: CLI runtime loader module or dedicated loader crate.
- Stable repair: loader composes `wjsm-module` runtime resolution, lower/compile, and runtime shared env instantiation.
- Compat boundary: public runtime API can keep loader optional.
- Verification: CLI smoke fixtures.

Retirement Track:

- Old owner/fallback: none; dynamic runtime load previously impossible.
- Active status: new loader becomes default for CLI run/build-execute path.

Steps:

1. Write tests.
   - Add CLI/integration fixtures that require true runtime loader: computed require and dynamic import variable.
   - Add runtime test for loader unavailable producing `ERR_DYNAMIC_MODULE_LOADER_UNAVAILABLE`.
2. Verify RED.
   - Command: `cargo nextest run -E 'test(modules__runtime_loading__cjs_computed_require) | test(modules__runtime_loading__esm_dynamic_import_variable)'`
   - Expected: fails because loader is not installed or instantiation path is missing.
3. Implement code.
   - Add `RuntimeOptions::module_loader` or equivalent setter using `Arc<dyn RuntimeModuleLoader>`.
   - CLI builds loader with entry root, `ResolutionOptions`, read roots, and current GC/support settings.
   - Loader resolves with `wjsm-module::runtime_resolution`, compiles JS modules through existing lower/compile path, and instantiates with the current runtime env.
   - Add runtime helper to expose shared env imports for dynamic instances without creating a second heap.
   - Loader rejects runtime `.ts/.tsx/.jsx` paths with a clear error.
4. Verify GREEN.
   - Command: `cargo nextest run -E 'test(modules__runtime_loading__cjs_computed_require) | test(modules__runtime_loading__esm_dynamic_import_variable)'`
   - Expected: selected fixtures pass.
   - Command: `cargo run -- run fixtures/modules/runtime_loading/cjs_computed_require/main.js --root fixtures/modules/runtime_loading/cjs_computed_require`
   - Expected: stdout matches fixture expected output.
5. Commit.
   - Message: `feat: install runtime module loader for #312`

## Task 7 — Complete JSON require, circular cache, and fixture matrix

Files:

- Modify `crates/wjsm-runtime/src/runtime_module_registry.rs`
- Modify `crates/wjsm-runtime/src/host_imports/modules.rs`
- Modify `crates/wjsm-module/src/runtime_resolution.rs`
- Add fixtures under `fixtures/modules/runtime_loading/`
- Update generated expected files with fixture update command after reviewing output

Why: issue #312 explicitly includes module registry, circular references, try/catch optional require, and review comment adds JSON require.

Impact/Compatibility: extends runtime loader to non-JS JSON and validates cache semantics end to end.

Repair Track:

- Root cause: registry needs observable Node-like state transitions, not only successful load cache.
- Canonical owner: runtime registry + host imports.
- Stable repair: exercise JSON, delete cache, circular partial exports, optional missing module.
- Compat boundary: JSON dynamic import with no assertion remains outside this issue.
- Verification: fixture matrix plus runtime tests.

Retirement Track:

- Old owner/fallback: existing eager circular CJS fixture does not prove runtime partial exports.
- Active status: retained as static interop fixture; new runtime fixture owns dynamic circular behavior.

Steps:

1. Write fixtures.
   - `cjs_try_optional_missing`
   - `cjs_require_json`
   - `cjs_require_cache_delete`
   - `cjs_circular_partial_exports`
   - `require_resolve_paths`
2. Verify RED.
   - Command: `cargo nextest run -E 'test(modules__runtime_loading__)'`
   - Expected: new fixtures fail before complete JSON/cache/circular wiring.
3. Implement code.
   - JSON path reads text through approved read root and parses to JS value.
   - `begin_loading` inserts module object and exports before executing JS module body.
   - `delete require.cache[path]` removes loaded/errored file/json entries.
   - `require.resolve.paths` returns JS array/null from runtime resolution API.
4. Verify GREEN.
   - Command: `WJSM_UPDATE_FIXTURES=1 cargo nextest run -E 'test(modules__runtime_loading__) | test(modules__cjs_conditional_require_false)'`
   - Expected: fixtures regenerate expected outputs; review changed `.expected` files manually.
   - Command: `cargo nextest run -E 'test(modules__runtime_loading__) | test(modules__cjs_conditional_require_false)'`
   - Expected: selected fixtures pass without update env.
5. Commit.
   - Message: `feat: complete runtime module cache cases for #312`

## Task 8 — ADR, full targeted verification, and issue closeout evidence

Files:

- Create `docs/adr/0006-runtime-module-loading-boundary.md`
- Modify `docs/aegis/INDEX.md`
- Modify `docs/aegis/work/2026-07-07-runtime-module-loading/10-intent.md`
- Modify `docs/aegis/work/2026-07-07-runtime-module-loading/20-checkpoint.md`
- Modify `docs/aegis/work/2026-07-07-runtime-module-loading/90-evidence.md`

Why: this issue changes runtime/compiler boundary and introduces a new execution-time source-of-truth; the architecture decision must be durable.

Impact/Compatibility: docs only, but required for future maintainers and issue closeout.

Repair Track:

- Root cause: no ADR currently records runtime module loading boundary.
- Canonical owner: ADR 0006 plus Aegis evidence docs.
- Stable repair: document chosen loader/registry boundary and rejected alternatives.
- Compat boundary: no code behavior change.
- Verification: doc scan plus targeted test suite from this plan.

Retirement Track:

- Old owner/fallback: design ambiguity around putting compiler into runtime.
- Active status: retired by ADR.

Steps:

1. Write docs.
   - ADR title: `Runtime Module Loading Boundary`.
   - Record alternatives A eager-only, B runtime owns compiler, C injected loader + registry.
   - Record decision C and consequences for runtime API, CLI, registry, snapshots/support ABI.
   - Create work evidence files with commands and observed outputs.
2. Verify docs are complete.
   - Command: use the built-in `grep` tool with the completion-scan pattern from the active review checklist over new docs.
   - Expected: no completion-scan matches.
3. Run targeted verification.
   - Command: `cargo nextest run -p wjsm-module -E 'test(cjs_) | test(runtime_resolve_) | test(resolve_paths)'`
   - Command: `cargo nextest run -p wjsm-semantic -E 'test(dynamic_import) | test(require_runtime) | test(import_meta_resolve)'`
   - Command: `cargo nextest run -p wjsm-runtime -E 'test(module_registry) | test(require_cache) | test(dynamic_module)'`
   - Command: `cargo nextest run -E 'test(modules__runtime_loading_) | test(modules__cjs_conditional_require_false)'`
   - Command: `cargo run -- run fixtures/modules/runtime_loading/cjs_computed_require/main.js --root fixtures/modules/runtime_loading/cjs_computed_require`
   - Command: `cargo run -- run fixtures/modules/runtime_loading/esm_dynamic_import_template/main.js --root fixtures/modules/runtime_loading/esm_dynamic_import_template`
   - Expected: all commands pass and produce fixture-matching stdout.
4. Update issue evidence.
   - Comment on #312 with implemented behavior, test commands, and mention ADR 0006.
   - Close #312 only after targeted verification passes.
5. Commit.
   - Message: `docs: record runtime module loading boundary for #312`

## Risks

- Wasmtime dynamic instance sharing may require refactoring runtime linker helpers; keep that inside Task 6 and do not change registry/semantic contracts.
- CJS `module.exports = value` plus `exports` alias needs exact object identity handling; Task 4/7 fixtures must include reassignment and property mutation cases if current CJS transform exposes gaps.
- Cache view backed by host object may need Proxy trap extensions; if existing Proxy support cannot own keys/delete, implement a dedicated host-backed cache object rather than copying cache state.
- GC roots must include every registry-held value before fixtures stress cache deletion and circular modules.

## Retirement

- Retire AOT-only dynamic import diagnostics for expression arguments; keep zero-argument validation.
- Retire recursive all-static require hoisting; keep top-level fast path.
- Retire `module_namespace_cache` as the canonical cache; registry becomes source-of-truth.
- Retire false-branch require side-effect expectation in fixtures.

## Self-review

- Spec coverage: every issue #312 item maps to at least one task: dynamic require Tasks 1/4/6/7; import expr Task 5/6; import.meta.resolve Task 5; require.resolve/cache Task 4/7; registry Task 3; JSON require Task 7; ADR Task 8.
- Completeness scan: no incomplete-section markers are intentionally present.
- Type consistency: plan uses runtime-owned DTOs for trait boundary and `wjsm-module` DTOs for resolver boundary; implementation may rename types but must preserve owner split.
- Compatibility: static import/export and top-level require fast path are preserved; corrected false-branch fixture is documented.
- Complexity: all new responsibilities have owner files; existing large files are wiring-only.
- Architecture integrity: runtime/compiler dependency direction remains intact.
- Verification: each task has exact nextest or CLI command and expected result.
- Dual-track: repair and retirement tracks are included for behavior-changing tasks.
