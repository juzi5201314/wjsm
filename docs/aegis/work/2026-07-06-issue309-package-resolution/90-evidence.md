# Issue #309 Package Resolution Enhancement — Evidence

Date: `2026-07-06`

## Commands / tool evidence

- `issue://309`: issue body and review comment read.
- Code context reads:
  - `crates/wjsm-module/src/resolver.rs`
  - `crates/wjsm-module/src/graph.rs`
  - `crates/wjsm-module/src/builtin_modules.rs`
  - `crates/wjsm-module/src/resolver_tests.rs`
  - `crates/wjsm-module/Cargo.toml`
  - `docs/aegis/INDEX.md`
- `wc -l crates/wjsm-module/src/*.rs`:
  - `resolver.rs`: 1114 lines
  - `cjs_transform.rs`: 983 lines
  - total `wjsm-module/src`: 5443 lines

## Artifacts

- Spec: `docs/aegis/specs/2026-07-06-package-resolution-enhancement-design.md`
- Work intent: `docs/aegis/work/2026-07-06-issue309-package-resolution/10-intent.md`
- Checkpoint: `docs/aegis/work/2026-07-06-issue309-package-resolution/20-checkpoint.md`
- Plan: `docs/aegis/plans/2026-07-06-package-resolution-enhancement.md`
- Aegis index updated: `docs/aegis/INDEX.md`

## Verification performed

Design-only verification:

- The spec explicitly covers every issue #309 implementation item: exports, imports, `node:`, module format markers, browser, self-reference, package resolution rules, tests, and non-goals.
- The spec accounts for the review comment additions: `node:` prefix, `exports` null, and condition priority including `node` per user selection.
- Placeholder scan completed with no `TBD`, `TODO`, or `implement later` text in the spec.
- The implementation plan maps approved spec requirements into eight execution tasks with owner files, tests, exact verification commands, compatibility boundaries, and issue-closing steps.
- Placeholder scan completed with no `TBD`, `TODO`, `implement later`, `待定`, `后续实现`, `类似`, or `appropriate` text in the plan.

## Implementation evidence

Task 1 — package metadata and module format owners:

- Files changed: `crates/wjsm-module/src/package_json.rs`, `crates/wjsm-module/src/module_format.rs`, `crates/wjsm-module/src/lib.rs`.
- Targeted verification: `cargo nextest run -p wjsm-module -E 'test(read_package_info_) | test(find_nearest_package_) | test(mjs_is_esm) | test(cjs_is_commonjs) | test(js_uses_package_type_module) | test(js_defaults_to_commonjs) | test(tsx_uses_ast_detection_marker)'` → 10 tests run, 10 passed.
- Spec review: initial REQUEST_CHANGES for missing `module/main` and directory-based optional `read_package_info`; fixed and re-reviewed PASS.
- Code quality review: PASS; minor forward notes for Task 3 path canonicalization/cache invariant and removal/narrowing of temporary dead_code expectations.

Task 2 — exports/imports target parser:

- Files changed: `crates/wjsm-module/src/exports.rs`, `crates/wjsm-module/src/lib.rs`, `Cargo.toml`, `Cargo.lock`, `crates/wjsm-test262/src/main.rs`.
- Targeted verification after fixes: `cargo nextest run -p wjsm-module -E 'test(exports_) | test(imports_)'` → 47 tests passed; `cargo test -p wjsm-test262 by_feature_json_map_sorts_feature_names` → 1 test passed.
- Spec review: initial REQUEST_CHANGES for manifest-order pattern tie-break and export subpath normalization; fixed and re-reviewed PASS.
- Code quality review: initial REQUEST_CHANGES for workspace `serde_json/preserve_order` deterministic-output fallout, backslash target escape, and missing package-context diagnostics; fixed and re-reviewed PASS.

Task 3 — resolver algorithm wiring:

- Files changed: `crates/wjsm-module/src/resolver.rs`, `crates/wjsm-module/src/resolver_tests.rs`, `crates/wjsm-module/src/resolution_options.rs`, `crates/wjsm-module/src/lib.rs`, `crates/wjsm-module/src/package_json.rs`, `crates/wjsm-module/src/exports.rs`.
- Targeted verification after fixes: resolver package-resolution suite → 21 passed; package_json/module-format adjacent suite → 9 passed; direct changed-test check (`exports_null_blocks_subpath`, `resolve_rejects_path_outside_root`) → 2 passed.
- Spec review: initial REQUEST_CHANGES for exports directory target reaching legacy entry, self-reference without exports, and missing get_id consistency regression; fixed and re-reviewed PASS.
- Code quality review: initial REQUEST_CHANGES for resolver_tests scratch leakage, duplicate nearest-package owner, and missing parent/specifier error context; fixed and re-reviewed PASS.
- Retirement closure: direct package.json parse/read in resolver retired; duplicate `package_json::find_nearest_package` removed; exports-present fallback to `module/main/index` retired; no source-of-truth data risk.

Task 4 — import/require edge kind propagation:

- Files changed: `crates/wjsm-module/src/graph.rs`, `crates/wjsm-module/src/resolver.rs`, `crates/wjsm-module/src/resolution_options.rs`, `crates/wjsm-module/src/resolver_tests.rs`.
- Targeted verification: `cargo nextest run -p wjsm-module -E 'test(cjs_require_uses_require_condition) | test(dynamic_import_from_cjs_uses_import_condition)'` → 2 passed; related resolver consistency tests → 3 passed.
- Spec review: PASS.
- Code quality review: PASS; minor graph test temp-dir hygiene was fixed with RAII `TestProject` and re-reviewed PASS.
- Retirement closure: conditionless graph package edge resolution retired; existing resolver wrappers retained as import/default compatibility entry points.

Task 5 — module format semantics:

- Files changed: `crates/wjsm-module/src/resolver.rs`, `crates/wjsm-module/src/resolver_tests.rs`, `crates/wjsm-module/src/module_format.rs`, plus wjsm-module test setup adjustments in `graph.rs`, `bundler.rs`, `semantic.rs`, `lib.rs`.
- Targeted verification: required five tests (`mjs_forces_esm`, `cjs_forces_commonjs`, `type_module_js_is_esm`, `type_commonjs_js_is_commonjs`, `commonjs_goal_rejects_static_import_syntax`) → 5 passed; additional affected wjsm-module narrow suites → 54 + 5 + 4 + 12 + 3 + 11 passed.
- Spec review: PASS.
- Code quality review: PASS.
- Retirement closure: syntax-only JS module format detection retired for `.js/.mjs/.cjs`; `.ts/.tsx/.jsx` AST-based behavior retained as planned.

Task 6 — browser condition plumbing:

- Files changed: `crates/wjsm-module/src/resolution_options.rs`, `resolver.rs`, `resolver_tests.rs`, `bundler.rs`, `lib.rs`, `package_json.rs`, and `crates/wjsm-cli/src/lib.rs` / tests.
- Targeted verification after fixes: `cargo nextest run -p wjsm-module -E 'test(browser_) | test(browser_condition)'` → 12 passed; `cargo nextest run -p wjsm-cli -E 'test(cli_browser_flag_enables_browser_condition) | test(cli_condition_adds_custom_condition)'` → 2 passed; `cargo fmt -p wjsm-module -- --check` passed.
- Spec review: initial REQUEST_CHANGES for `--condition browser` not enabling browser field semantics; fixed and re-reviewed PASS.
- Code quality review: REQUEST_CHANGES for legacy package-entry browser object mapping after extension resolution and extensionless `module` coverage; fixed and re-reviewed PASS.
- Compatibility closure: default resolver/bundler/CLI behavior remains non-browser; browser semantics are opt-in via `--browser` or `browser` condition.

Task 7 — package resolution fixtures:

- Files added: eight cases under `fixtures/modules/package_resolution/` covering exports condition import/require, exports subpath pattern, imports private alias, package `type: module`, `.cjs` override, self-reference exports, and default non-browser boundary.
- Targeted verification: `cargo nextest run -E 'test(modules__package_resolution_)'` → 8 passed.
- Spec review: PASS.
- Code quality review: initial REQUEST_CHANGES for selector docs and browser fixture naming clarity; fixed and re-reviewed PASS.
- Fixture boundary: generated FixtureRunner uses default resolution options, so `browser_condition_default_boundary` proves browser behavior is opt-in; explicit browser behavior remains covered by Task 6 resolver/CLI tests.

Task 8 — final verification:

- `cargo fmt -- --check` → passed.
- `cargo nextest run -p wjsm-module` → 204 passed.
- `cargo nextest run -p wjsm-cli -E 'test(cli_)'` → 2 passed.
- `cargo nextest run -E 'test(modules__package_resolution_)'` → 8 passed.
- `cargo nextest run -E 'test(modules__)'` → 78 passed.
- Manual smoke: `cargo run -- run fixtures/modules/package_resolution/exports_condition_import/main.js --root fixtures/modules/package_resolution/exports_condition_import` → `exports-import`.
- Manual smoke: `cargo run -- run fixtures/modules/package_resolution/exports_condition_require/main.js --root fixtures/modules/package_resolution/exports_condition_require` → `exports-require`.
- Final code review: PASS, no Critical/Important/Minor findings.
- Commit: `0266021 feat: complete package resolution enhancement for #309`.
- GitHub issue comment: `https://github.com/juzi5201314/wjsm/issues/309#issuecomment-4900391418`.
- GitHub issue #309 closed with reason `completed`.

No runtime/build verification remains deferred for issue #309 implementation closure.
