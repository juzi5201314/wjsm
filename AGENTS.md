# wjsm

AOT JavaScript/TypeScript runtime: SWC AST → semantic IR → WebAssembly → Wasmtime. No V8. The Wasm backend is production-capable; the JIT target is an unimplemented extension point.

## Non-negotiable rules

- Rust 2024, default rustfmt, Chinese source comments, and zero compiler warnings.
- ECMAScript is the semantic source of truth. Do not ship partial semantics, skipped edge cases, or invalid early-error behavior.
- Keep backend boundaries from ADRs 0011–0013: Wasm dependencies belong only in `wjsm-backend-wasm` and `wjsm-host-wasm`; `wjsm-builtins`, `wjsm-host`, `wjsm-gc`, and `wjsm-module` stay backend-independent.
- Preserve the unified ManagedHeap path. Never reintroduce a memory32 object heap, a dual-heap fallback, or a second runtime owner.
- Put generated artifacts in `/tmp`. For ad-hoc JS/TS, use `-e`; do not create scratch source files.
- Fix the owning layer and remove obsolete paths. Do not hide failures by weakening fixtures or snapshots.

## Commands

```bash
cargo build
cargo run -- run -e 'console.log(1 + 2)'
cargo run -- build -e 'console.log(1)' -o /tmp/out.wasm
cargo run -- check -e 'const x = 1'
cargo run -- dump-ir -e 'const x = 1'
cargo run -- dump-wat -e 'const x = 1'
cargo nextest run --workspace
cargo nextest run -E 'test(happy__hello)'
cargo nextest run -p wjsm-semantic
WJSM_UPDATE_FIXTURES=1 cargo nextest run -E 'test(happy__<name>)'
WJSM_UPDATE_SNAPSHOTS=1 cargo nextest run -p wjsm-semantic -- lowering_snapshots
```

## Workflow

- Diagnose the failing stage first: parse → lower → module graph → codegen → host/runtime.
- Use `dump-ast`, `dump-ir`, `dump-wat`, and `disasm` to compare adjacent stages; do not add temporary production logging unless those paths cannot expose the failure.
- Lowering changes require semantic IR snapshots. Observable behavior requires `fixtures/happy` or `fixtures/errors` plus `.expected`. Module behavior belongs in `fixtures/modules`.
- Review generated fixture/snapshot changes before accepting them. Run the narrow test first and the workspace suite for cross-crate changes.
- Use exact spec text for language questions; after the local code and spec, inspect real-engine source before asking the user to decide semantics.
- Keep files responsibility-focused (target ≤500 lines) and functions cohesive (target ≤30 lines); split by semantic/backend/host domain rather than adding parallel conventions.

## Source of truth

- User-facing behavior and CLI: [README.md](README.md) and `wjsm --help`.
- Architecture boundaries and invariants: [docs/adr/](docs/adr/), especially ADRs 0010–0013.
- New backend contract: [docs/backend-implementation-guide.md](docs/backend-implementation-guide.md).
- Fixture and test mechanics: `build.rs`, `tests/`, `fixtures/`, and `.config/nextest.toml`.
