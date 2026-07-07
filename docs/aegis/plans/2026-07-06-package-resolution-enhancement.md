# Package Resolution Enhancement 实现计划（issue #309）

Goal: 实现 issue #309：让 `wjsm-module` 支持现代 package resolution（`exports` / `imports` / self-reference / `type` / `.mjs` / `.cjs` / `browser` / `node:` 验收）。

Architecture: 在 `wjsm-module` 内拆分 package metadata、exports/imports target 解析、module format 与 resolver 编排职责。`resolver.rs` 不再承载全部逻辑；`builtin_modules.rs` 继续作为内置模块唯一 owner。

Tech Stack: Rust 2024，`anyhow` 错误上下文，`serde_json` 读取 package.json，现有 `wjsm-parser` / `cjs_transform` / `ModuleGraph` 流程。

Baseline/Authority Refs:

- `docs/aegis/specs/2026-07-06-package-resolution-enhancement-design.md`
- issue #309
- `AGENTS.md`：wjsm-module 架构、Rust 2024、注释中文、文件/函数大小纪律、spec-compliance hard rules
- `crates/wjsm-module/src/resolver.rs`
- `crates/wjsm-module/src/graph.rs`
- `crates/wjsm-module/src/builtin_modules.rs`
- `crates/wjsm-module/src/resolver_tests.rs`
- `crates/wjsm-module/Cargo.toml`

Compatibility Boundary:

- 无 `exports` 的包继续支持 `module/main/index.*`。
- 有 `exports` 的包必须阻断 `module/main/index` 回退。
- `node:` 永远解析到内置模块注册表，不查 `node_modules`。
- 公共 API 默认不启用 `browser`；CLI 显式 `--browser` / `--condition browser` 才启用。
- Runtime/WASM ABI 不变。

Verification:

- `cargo nextest run -p wjsm-module`
- `cargo nextest run -p wjsm-cli -E 'test(cli_)'`（若新增 CLI 参数测试名不同，则用实际新增测试表达式）
- `cargo nextest run -E 'test(modules__package_resolution_)'`
- 关键 fixture 手动冒烟：`cargo run -- run fixtures/modules/package_resolution/<case>/main.js --root fixtures/modules/package_resolution/<case>`

## Plan Basis

Facts:

- `resolver.rs` 当前 1114 行，已包含 path/package entry/module loading/import extraction/export extraction/TS syntax validation。
- `builtin_modules.rs` 已支持 bare builtins 与 `node:` 前缀，并对未知 `node:` 报错。
- `graph.rs` 当前对所有静态 import/re-export/dynamic import 调用同一个 `resolver.resolve(specifier, parent)`，没有 import/require 条件上下文。
- CJS 转换会把静态 `require()` 转成 import；父模块 `is_cjs` 可作为静态 require 边条件来源。动态 `import()` 始终按 import 条件。

Assumptions:

- issue #309 的默认条件优先级按用户选择采用 `wjsm > node > import/require > default`。
- `browser` 是显式条件，不默认启用。

Unknowns:

- 无阻塞 unknown。若执行期发现 CLI 参数框架中 `--condition` 名称冲突，使用 `--module-condition`，并同步更新测试与 help 文本。

## BaselineUsageDraft

- Required baseline refs: approved design spec, issue #309, `AGENTS.md`, resolver/graph/builtin_modules/resolver_tests/Cargo.toml
- Acknowledged before plan refs: all required refs
- Cited in plan refs: Goal, Architecture, Compatibility Boundary, tasks below
- Missing refs: none
- Decision: continue

## Architecture Integrity Lens

- Invariant: specifier resolution remains compile-time deterministic and owned by `wjsm-module`.
- Canonical owner / contract: `exports.rs` for package target semantics; `package_json.rs` for package metadata; `module_format.rs` for file format; `resolver.rs` for orchestration.
- Responsibility overlap: no new builtin list; reuse `builtin_modules.rs`.
- Higher-level path: carry `ResolutionKind` through graph instead of encoding conditions in target parser.
- Retirement / falsifier: old package entry logic becomes legacy-only and is tested unreachable when `exports` exists.
- Verdict: proceed.

## Plan-Time Complexity Check

Complexity Budget:

- Artifact class: core resolver architecture change
- Target files: `resolver.rs`, `graph.rs`, `lib.rs`, new `package_json.rs`, `exports.rs`, `module_format.rs`, `resolution_options.rs`, tests, fixtures, possible `wjsm-cli` option plumbing
- Current pressure: `resolver.rs` 1114 lines; direct growth is over-budget
- Projected post-change pressure: split owner files each under focused responsibility; resolver stays orchestration-only
- Budget result: at-risk but governed by file split
- Planned governance: implement pure modules first with tests, then wire resolver

Plan Pressure Test:

- Owner / contract / retirement: clear owner split; legacy fallback explicitly gated by absence of exports
- Architecture integrity / higher-level path: `ResolutionKind` is the higher-level resolver contract
- Verification scope: unit + module fixtures + CLI option tests
- Task executability: each task has concrete files and commands
- Pressure result: proceed

## Task 1 — Add package metadata and module format owners

Files:

- Create `crates/wjsm-module/src/package_json.rs`
- Create `crates/wjsm-module/src/module_format.rs`
- Modify `crates/wjsm-module/src/lib.rs`
- Create tests in the new modules

Why: package `type`, `browser`, name, imports/exports metadata must be read once and shared; module format must not be inferred only from AST syntax.

Impact/Compatibility: no resolver behavior changes until Task 3 wires these modules. Existing tests should keep passing after module declarations compile.

Repair Track:

- Root cause: resolver currently mixes package.json reading with path fallback and cannot answer nearest package format.
- Canonical owner: `package_json.rs` and `module_format.rs`.
- Stable repair: typed package metadata + deterministic format function.
- Compat boundary: `.ts/.tsx/.jsx` retain existing AST/CJS detection until resolver integration.

Retirement Track:

- Old owner: `resolve_package_entry` local ad-hoc package.json read.
- Active status after this task: still active; Task 3 retires direct JSON read.
- Deletion trigger: resolver uses `PackageInfo` for legacy entry.

Steps:

1. Write tests first.
   - In `package_json.rs`, add unit tests using temp dirs and `std::fs::write`:
     - `read_package_info_reads_name_type_exports_imports_browser`
     - `read_package_info_defaults_to_commonjs_without_type`
     - `find_nearest_package_stops_at_root`
   - In `module_format.rs`, add unit tests:
     - `mjs_is_esm`
     - `cjs_is_commonjs`
     - `js_uses_package_type_module`
     - `js_package_without_type_defaults_to_commonjs`
     - `js_without_package_uses_ast_detection_marker`
     - `tsx_uses_ast_detection_marker`
2. Verify RED:
   - Command: `cargo nextest run -p wjsm-module -E 'test(read_package_info_) | test(find_nearest_package_) | test(mjs_is_esm) | test(cjs_is_commonjs) | test(js_uses_package_type_module) | test(js_package_without_type_defaults_to_commonjs) | test(js_without_package_uses_ast_detection_marker)'`
   - Expected: compile failure or failing tests because modules do not exist.
3. Implement code:
   - `package_json.rs` defines `PackageInfo`, `PackageType`, `BrowserField`, `read_package_info`, `find_nearest_package`.
   - Use `anyhow::{Context, Result, bail}`; every JSON read/parse error includes package path.
   - Store raw `serde_json::Value` for `exports` and `imports` so Task 2 can parse full shapes.
   - `browser` object maps string keys to `Option<String>` where JSON `false` becomes `None`.
   - `module_format.rs` defines `ModuleFormat` and `detect_module_format(path, package, ast_is_cjs)`; `.mjs`/`.cjs` override, `.js` uses package type when a package exists, no-package `.js` and other extensions use `ast_is_cjs` to preserve existing wjsm entry/fixture behavior.
   - Add `mod package_json; mod module_format;` in `lib.rs`.
4. Verify GREEN:
   - Command: `cargo nextest run -p wjsm-module -E 'test(read_package_info_) | test(find_nearest_package_) | test(mjs_is_esm) | test(cjs_is_commonjs) | test(js_uses_package_type_module) | test(js_package_without_type_defaults_to_commonjs) | test(js_without_package_uses_ast_detection_marker)'`
   - Expected: all selected tests pass.
5. Commit:
   - Message: `feat: add package metadata owners for #309`

## Task 2 — Implement exports/imports target parser

Files:

- Create `crates/wjsm-module/src/exports.rs`
- Modify `crates/wjsm-module/src/lib.rs`

Why: exports/imports conditional target parsing is the highest-risk semantic owner and should be testable without filesystem traversal.

Impact/Compatibility: pure module only; no resolver behavior changes until Task 3.

Repair Track:

- Root cause: resolver currently cannot distinguish package subpath availability from file existence.
- Canonical owner: `exports.rs`.
- Stable repair: pure target parser with Node-style error codes.
- Compat boundary: arrays are rejected in this phase instead of implicit fallback.

Retirement Track:

- Old owner: none; current resolver lacks this capability.
- Active status: new owner becomes canonical immediately after creation.

Steps:

1. Write tests first in `exports.rs`:
   - `exports_string_resolves_main`
   - `exports_condition_prefers_wjsm_then_node_then_import_then_default`
   - `exports_condition_prefers_require_for_require_edges`
   - `exports_subpath_resolves_exact_key`
   - `exports_pattern_replaces_star`
   - `exports_null_reports_not_exported`
   - `exports_rejects_absolute_target`
   - `exports_rejects_parent_traversal`
   - `exports_rejects_mixed_condition_and_subpath_keys`
   - `imports_hash_alias_resolves`
   - `imports_hash_pattern_resolves`
   - `imports_missing_reports_import_not_defined`
2. Verify RED:
   - Command: `cargo nextest run -p wjsm-module -E 'test(exports_) | test(imports_)'`
   - Expected: compile failure or failing tests.
3. Implement code:
   - Define `PackageTarget { relative_path: String }`.
   - Define `PackageResolutionError` with `thiserror` or return `anyhow::Error` carrying exact Node-style code prefix; keep functions crate-private.
   - Implement condition recursion over `serde_json::Value::Object` by iterating the provided conditions in order and selecting matching keys.
   - Implement subpath map matching: exact key first, then patterns sorted by longest prefix and stable original order.
   - Validate targets: must start with `./`; reject `..`, absolute paths, URL-like strings, and any segment equal to `node_modules`.
   - `null` maps to not exported/not defined depending on caller.
   - Add `mod exports;` to `lib.rs`.
4. Verify GREEN:
   - Command: `cargo nextest run -p wjsm-module -E 'test(exports_) | test(imports_)'`
   - Expected: all selected tests pass.
5. Commit:
   - Message: `feat: parse package exports and imports for #309`

## Task 3 — Wire resolver algorithm, self-reference, imports, exports, and legacy fallback retirement

Files:

- Modify `crates/wjsm-module/src/resolver.rs`
- Modify `crates/wjsm-module/src/resolver_tests.rs`
- Modify `crates/wjsm-module/src/lib.rs` if module visibility needs internal imports

Why: this is the behavioral cutover from legacy package entry lookup to Node-style package resolution.

Impact/Compatibility: affects all package bare specifier resolution. Existing no-exports packages must stay green.

Repair Track:

- Root cause: `resolve_bare_specifier` directly joins package subpaths and `resolve_package_entry` always tries `module/main`.
- Canonical owner: resolver orchestration calling package/exports owners.
- Stable repair: one `resolve_specifier_to_path(specifier, parent, kind, options)` path used by `resolve` and `get_id_for_specifier`.
- Compat boundary: legacy fallback only when package has no `exports`.

Retirement Track:

- Old owner: direct `std::fs::read_to_string(package.json)` in `resolve_package_entry`.
- Active status after task: retired; any package.json access goes through `package_json.rs`.
- Deletion trigger: remove direct package.json JSON parsing from `resolver.rs`.

Steps:

1. Write tests first in `resolver_tests.rs`:
   - `exports_blocks_main_fallback`
   - `exports_resolves_package_main_dot`
   - `exports_resolves_subpath`
   - `exports_resolves_pattern`
   - `exports_null_blocks_subpath`
   - `imports_resolves_hash_alias_within_parent_package`
   - `imports_missing_reports_import_not_defined`
   - `self_reference_uses_own_exports_before_node_modules`
   - `legacy_main_still_works_without_exports`
   - `node_prefix_still_resolves_builtin_without_node_modules`
2. Verify RED:
   - Command: `cargo nextest run -p wjsm-module -E 'test(exports_blocks_main_fallback) | test(exports_resolves_) | test(imports_) | test(self_reference_) | test(legacy_main_still_works_without_exports) | test(node_prefix_still_resolves_builtin_without_node_modules)'`
   - Expected: new tests fail or do not compile.
3. Implement code:
   - Add `ResolutionKind::{Import, Require}` and `ResolutionOptions` in `resolution_options.rs`; default conditions are `wjsm`, `node`, edge condition, `default`.
   - Add `mod resolution_options;` in `lib.rs`.
   - Add `ModuleResolver` fields:
     - `options: ResolutionOptions`
     - `package_cache: HashMap<PathBuf, Option<PackageInfo>>`
   - Keep `ModuleResolver::new(root)` defaulting options to normal non-browser resolution; add crate-private `with_options(root, options)` for CLI tests later.
   - Replace `resolve_path(specifier, parent)` internals with a crate-private kind-aware path resolver. Preserve the existing public `resolve_path` as a compatibility wrapper using `ResolutionKind::Import` and default options for existing tests.
   - Implement `resolve_package_imports` for `#` specifiers using nearest package only.
   - Implement package self-resolution before `node_modules` lookup.
   - For bare package with exports, call `exports.rs`; join validated target to package dir and resolve file/directory; do not fallback on miss.
   - For bare package without exports, preserve old subpath and entry behavior through `legacy_package_entry`.
   - For browser object mapping, apply only when options enable browser.
4. Verify GREEN:
   - Command: `cargo nextest run -p wjsm-module -E 'test(exports_blocks_main_fallback) | test(exports_resolves_) | test(imports_) | test(self_reference_) | test(legacy_main_still_works_without_exports) | test(node_prefix_still_resolves_builtin_without_node_modules)'`
   - Expected: all selected tests pass.
5. Commit:
   - Message: `feat: resolve package exports imports and self references for #309`

## Task 4 — Carry import/require resolution kind through graph and module loading

Files:

- Modify `crates/wjsm-module/src/graph.rs`
- Modify `crates/wjsm-module/src/resolver.rs`
- Modify `crates/wjsm-module/src/resolver_tests.rs` or `graph.rs` tests

Why: condition maps need `import` versus `require`; CJS-generated static imports must resolve with `require`, while dynamic `import()` remains `import`.

Impact/Compatibility: only packages using conditional exports with both `import` and `require` observe behavior change.

Repair Track:

- Root cause: graph currently calls one conditionless resolver for every edge.
- Canonical owner: graph determines edge kind; resolver applies package conditions.
- Stable repair: static edge kind based on parent module `is_cjs`; dynamic import always `Import`.
- Compat boundary: ESM static import remains `import`; CJS generated require remains `require`.

Retirement Track:

- Old owner: implicit import-only condition.
- Active status after task: retired; all graph resolution calls include kind.

Steps:

1. Write tests first:
   - Add graph/resolver integration test `cjs_require_uses_require_condition` with package exports `{ ".": { "import": "./esm.js", "require": "./cjs.js" } }` and CJS parent `const x = require('pkg')`.
   - Add test `dynamic_import_from_cjs_uses_import_condition` with CJS parent `import('pkg')`.
2. Verify RED:
   - Command: `cargo nextest run -p wjsm-module -E 'test(cjs_require_uses_require_condition) | test(dynamic_import_from_cjs_uses_import_condition)'`
   - Expected: `require` condition test fails by selecting import/default or compile failure.
3. Implement code:
   - Add `ModuleResolver::resolve_with_kind(specifier, parent, kind)` and `get_id_for_specifier_with_kind`.
   - In `graph.rs` BFS static imports, compute `let kind = if module.is_cjs { ResolutionKind::Require } else { ResolutionKind::Import };`.
   - Re-export sources always use `Import`.
   - Dynamic imports always use `Import`.
   - During final graph edge construction, use the same kind rules as BFS.
4. Verify GREEN:
   - Command: `cargo nextest run -p wjsm-module -E 'test(cjs_require_uses_require_condition) | test(dynamic_import_from_cjs_uses_import_condition)'`
   - Expected: both tests pass.
5. Commit:
   - Message: `feat: apply import and require package conditions for #309`

## Task 5 — Enforce `.mjs/.cjs/type` module format semantics

Files:

- Modify `crates/wjsm-module/src/resolver.rs`
- Modify `crates/wjsm-module/src/resolver_tests.rs`
- Possibly modify `crates/wjsm-module/src/cjs_transform.rs` only if forced-CJS transform needs a public helper; do not grow it with resolver logic

Why: package `type` and file extensions must control ESM/CJS behavior independently of syntax detection.

Impact/Compatibility: `.js` in packages without `type` becomes CJS by default, matching Node. Files with ESM syntax in CommonJS goal should be rejected instead of silently treated as ESM.

Repair Track:

- Root cause: `is_cjs` currently follows syntax detection only.
- Canonical owner: `module_format.rs`; resolver only consumes result.
- Stable repair: choose format before transform; validate mismatched syntax.
- Compat boundary: `.ts/.tsx/.jsx` retain existing detection behavior.

Retirement Track:

- Old owner: `crate::cjs_transform::is_commonjs_module(&ast)` as sole format source.
- Active status after task: only used for syntax detection and non-js extensions.

Steps:

1. Write tests first in `resolver_tests.rs`:
   - `mjs_forces_esm_even_with_package_commonjs`
   - `cjs_forces_commonjs_even_with_package_module`
   - `type_module_js_is_esm`
   - `type_commonjs_js_is_commonjs`
   - `commonjs_goal_rejects_static_import_syntax`
2. Verify RED:
   - Command: `cargo nextest run -p wjsm-module -E 'test(mjs_forces_esm) | test(cjs_forces_commonjs) | test(type_module_js_is_esm) | test(type_commonjs_js_is_commonjs) | test(commonjs_goal_rejects_static_import_syntax)'`
   - Expected: failures under old syntax-only detection.
3. Implement code:
   - In `load_resolved_module`, find nearest package for `path` before transform.
   - Compute `ast_is_cjs = cjs_transform::is_commonjs_module(&ast)`.
   - Compute `format = detect_module_format(&path, package.as_ref(), ast_is_cjs)`.
   - If format is CommonJs and AST has static import/export module declarations, `bail!` with `SyntaxError: Cannot use import/export syntax in CommonJS module <path>`.
   - If format is CommonJs, run `transform_with_prefix` even when `ast_is_cjs` is false so metadata is CommonJS.
   - If format is Esm, do not transform because of CJS-looking identifiers.
4. Verify GREEN:
   - Command: `cargo nextest run -p wjsm-module -E 'test(mjs_forces_esm) | test(cjs_forces_commonjs) | test(type_module_js_is_esm) | test(type_commonjs_js_is_commonjs) | test(commonjs_goal_rejects_static_import_syntax)'`
   - Expected: all selected tests pass.
5. Commit:
   - Message: `feat: honor package module format markers for #309`

## Task 6 — Add browser condition plumbing and CLI options

Files:

- Modify `crates/wjsm-module/src/resolver.rs`
- Modify `crates/wjsm-module/src/bundler.rs`
- Modify `crates/wjsm-module/src/lib.rs`
- Modify `crates/wjsm-cli/src/lib.rs`
- Add/modify CLI tests where current CLI test conventions live

Why: issue #309 includes browser field, but it must be explicit so normal Node-like resolution is not changed.

Impact/Compatibility: default CLI/API unchanged; new flags opt into browser mappings and extra conditions.

Repair Track:

- Root cause: no way to pass resolution conditions into bundling.
- Canonical owner: CLI parses options; `wjsm-module` owns options semantics.
- Stable repair: crate-private/default options plus public wrapper kept unchanged.
- Compat boundary: `Target` remains backend target, not browser platform selector.

Retirement Track:

- Old owner: none.
- Active status: new options path becomes canonical for future condition flags.

Steps:

1. Write tests first:
   - wjsm-module test `browser_string_replaces_package_entry_when_enabled` using `ModuleBundler::with_resolution_options` or crate-visible constructor.
   - wjsm-module test `browser_mapping_replaces_relative_dependency_when_enabled`.
   - CLI test `cli_browser_flag_enables_browser_condition`.
   - CLI test `cli_condition_adds_custom_condition`.
2. Verify RED:
   - Commands:
     - `cargo nextest run -p wjsm-module -E 'test(browser_)'`
     - `cargo nextest run -p wjsm-cli -E 'test(cli_browser_flag_enables_browser_condition) | test(cli_condition_adds_custom_condition)'`
   - Expected: tests fail or do not compile.
3. Implement code:
   - Add `ModuleBundler::with_resolution_options(root_path, options)` crate-public or public only if CLI needs it across crate boundary. Prefer public `ModuleResolutionOptions` only if `wjsm-cli` cannot otherwise pass options; document it if public.
   - Keep existing `ModuleBundler::new` and `bundle/lower_bundle/parse_entry_ast` defaulting to normal options.
   - Add CLI flags:
     - `--browser` sets browser option.
     - `--condition <name>` appends custom condition.
   - Thread options through compile plan only for bundle mode; single-source inline mode ignores module resolver conditions.
   - Implement browser object replacement in resolver for package-relative file keys.
   - `false` mapping reports `ERR_PACKAGE_PATH_DISABLED_BY_BROWSER`.
4. Verify GREEN:
   - Commands:
     - `cargo nextest run -p wjsm-module -E 'test(browser_)'`
     - `cargo nextest run -p wjsm-cli -E 'test(cli_browser_flag_enables_browser_condition) | test(cli_condition_adds_custom_condition)'`
   - Expected: all selected tests pass.
5. Commit:
   - Message: `feat: add browser package resolution options for #309`

## Task 7 — Add end-to-end module fixtures and update snapshots

Files:

- Create `fixtures/modules/package_resolution/exports_condition_import/main.js` plus package files and `.expected`
- Create `fixtures/modules/package_resolution/exports_condition_require/`
- Create `fixtures/modules/package_resolution/exports_subpath_pattern/`
- Create `fixtures/modules/package_resolution/imports_private_alias/`
- Create `fixtures/modules/package_resolution/type_module_js/`
- Create `fixtures/modules/package_resolution/cjs_extension_override/`
- Create `fixtures/modules/package_resolution/self_reference_exports/`
- Create `fixtures/modules/package_resolution/browser_condition_default_boundary/`（generated FixtureRunner 不传 `--browser`，此 fixture 固定默认非 browser 边界；显式 browser 行为由 Task 6 CLI/resolver tests 覆盖）

Why: unit tests prove resolver pieces; fixtures prove complete bundling/lowering/execution path.

Impact/Compatibility: adds coverage only. Never weaken existing expected files to hide logic errors.

Repair Track:

- Root cause: package resolution currently lacks E2E coverage.
- Canonical owner: fixtures/modules package resolution suite.
- Stable repair: one fixture per externally observable behavior.
- Compat boundary: expected stdout/stderr/exit code reflect actual behavior.

Retirement Track:

- Old owner: resolver unit tests alone.
- Active status after task: E2E fixtures become regression guard.

Steps:

1. Write fixture inputs first:
   - Each case has a `main.js` and a local `node_modules/pkg/package.json` or package self-reference setup.
   - Each success case prints a short deterministic string such as `exports-wjsm`, `require-branch`, `pattern-ok`, `imports-ok`, `type-module-ok`, `self-ok`, `non-browser`.
   - Error fixtures, if added, assert error code prefixes instead of absolute paths.
2. Verify RED:
   - Command: `cargo nextest run -E 'test(modules__package_resolution_)'`
   - Expected: generated tests fail due missing expected snapshots or unresolved package behavior.
3. Generate expected snapshots only after reviewing stdout/stderr manually:
   - Command: `WJSM_UPDATE_FIXTURES=1 cargo nextest run -E 'test(modules__package_resolution_)'`
   - Expected: `.expected` files are created/updated.
4. Verify GREEN:
   - Command: `cargo nextest run -E 'test(modules__package_resolution_)'`
   - Expected: all package resolution fixtures pass.
5. Commit:
   - Message: `test: add package resolution fixtures for #309`

## Task 8 — Final verification, cleanup, and issue closure

Files:

- Modify only files required by compiler warnings, formatter, or failing tests from this issue.
- Update `docs/aegis/work/2026-07-06-issue309-package-resolution/20-checkpoint.md` and `90-evidence.md` with final evidence.

Why: issue #309 is complete only when targeted tests and E2E fixtures prove behavior, warnings are clean, and GitHub issue is closed with evidence.

Impact/Compatibility: no new feature scope; cleanup only.

Repair Track:

- Root cause: final integration can expose warnings or missed callsites.
- Canonical owner: touched package owners from prior tasks.
- Stable repair: fix warnings at source; no suppressions.
- Compat boundary: no fixture weakening.

Retirement Track:

- Old owner/fallback: legacy package entry fallback remains only as no-exports path.
- Active status: documented by tests.

Steps:

1. Run formatter:
   - Command: `cargo fmt`
   - Expected: exits 0.
2. Run targeted tests:
   - Command: `cargo nextest run -p wjsm-module`
   - Expected: exits 0, no failures.
   - Command: `cargo nextest run -p wjsm-cli -E 'test(cli_)'`
   - Expected: exits 0 for relevant CLI tests.
   - Command: `cargo nextest run -E 'test(modules__package_resolution_)'`
   - Expected: exits 0, package resolution fixtures pass.
3. Run one manual smoke per critical branch:
   - Command: `cargo run -- run fixtures/modules/package_resolution/exports_condition_import/main.js --root fixtures/modules/package_resolution/exports_condition_import`
   - Expected stdout contains the selected `wjsm` or `node` branch from that fixture.
   - Command: `cargo run -- run fixtures/modules/package_resolution/exports_condition_require/main.js --root fixtures/modules/package_resolution/exports_condition_require`
   - Expected stdout contains `require` branch.
4. Update work evidence:
   - Add exact commands and observed pass/fail output to `90-evidence.md`.
   - Add final checkpoint/drift decision to `20-checkpoint.md`.
5. Commit and close issue:
   - Commit message: `feat: complete package resolution enhancement for #309`
   - GitHub comment on #309 summarizes implemented exports/imports/type/browser/node/self-reference coverage and verification commands.
   - Close #309.

## Risks

- `exports` arrays are rejected by design; if real packages require fallback arrays during implementation validation, return to spec before broadening behavior.
- `browser` false-to-empty-module semantics is intentionally not implemented; if fixtures or target packages require it, design an explicit empty-module owner rather than returning a silent fake module.
- Public `ModuleResolutionOptions` may be necessary for CLI cross-crate plumbing. If made public, document it and keep fields non-exhaustive or constructor-based.

## Retirement

- Direct package.json parsing inside `resolver.rs` retires after Task 3.
- Conditionless graph resolution retires after Task 4.
- Syntax-only JS format detection retires after Task 5 for `.js/.mjs/.cjs`.
- Legacy `module/main/index` remains as an explicit compatibility path only when `exports` is absent.
