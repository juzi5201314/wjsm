# Issue #309 Package Resolution Enhancement — Intent

Date: `2026-07-06`
Status: `design/spec review`

## TaskIntentDraft

- Requested outcome: 使用 brainstorming 流程完成 issue #309 的设计规格，批准后进入 writing-plans。
- Scope: `wjsm-module` npm package resolution：exports/imports/type/module format/browser/self-reference/node: validation。
- Non-goals: workspace/file/link/pnpm protocols, module.createRequire, TypeScript `types` runtime condition, deno/bun conditions, general dynamic require。
- Success evidence: spec written under `docs/aegis/specs/2026-07-06-package-resolution-enhancement-design.md`, indexed, self-reviewed, and ready for user review。
- Stop condition: user approves spec and implementation plan can be written, or user requests spec changes。
- Risk hints: resolver.rs already over project file-size guidance; design must split owner files instead of growing resolver.

## BaselineReadSetHint

Required refs:

- issue #309 and its review comment
- project AGENTS.md module architecture and spec-compliance rules
- `crates/wjsm-module/src/resolver.rs`
- `crates/wjsm-module/src/graph.rs`
- `crates/wjsm-module/src/builtin_modules.rs`
- `crates/wjsm-module/src/resolver_tests.rs`
- `crates/wjsm-module/Cargo.toml`
- `docs/aegis/INDEX.md`

## BaselineUsageDraft

- Required baseline refs: listed above
- Delivered context refs: AGENTS.md, issue #309
- Acknowledged before plan refs: issue #309, resolver/graph/builtin_modules/resolver_tests/Cargo.toml, Aegis index
- Cited in design refs: spec sections 1, 3, 8, 9, 11, 12
- Missing refs: none
- Decision: continue

## ImpactStatementDraft

- Affected layers: `wjsm-module` resolver graph, package metadata handling, module format, fixtures; possible `wjsm-cli` module condition plumbing.
- Owners: keep `builtin_modules.rs` as internal built-in owner; add `exports.rs`, `package_json.rs`, `module_format.rs`, `resolution_options.rs`; keep `resolver.rs` orchestration-only.
- Invariants: compile-time deterministic resolution; target path stays in package/root; `exports` blocks legacy fallback; `node:` never searches `node_modules`.
- Compatibility: legacy `module/main/index` remains only when `exports` is absent; browser behavior explicit.
