# wjsm

AOT JavaScript/TypeScript runtime: compiles JS/TS to WebAssembly using `swc_core` (parsing), `wasm-encoder` (codegen), `wasmtime` (execution). No V8.

## Build & Test

```bash
cargo build                              # build workspace
cargo run -- run test.ts                 # compile + execute
cargo run -- build test.ts -o out.wasm   # compile to WASM
cargo run -- build test.ts --stage lower # stop at parse/lower/compile/execute
cargo run -- check test.ts               # check for errors, no execute
cargo run -- eval "1 + 2"                # evaluate expression
cargo run -- dump-ir test.ts             # IR dump (--format dot for graphviz)
cargo run -- dump-wat test.ts            # WAT text output

# Testing (prefer nextest over cargo test; per-test timeout ~9s via .config/nextest.toml)
cargo nextest run --workspace                       # all tests
cargo nextest run -E 'test(happy__hello)'           # specific fixture
cargo nextest run -E 'test(happy__)'                # all happy-path fixtures
cargo nextest run -p wjsm-semantic                  # per-crate

WJSM_UPDATE_FIXTURES=1 cargo nextest run            # update E2E .expected snapshots
WJSM_UPDATE_SNAPSHOTS=1 cargo nextest run -p wjsm-semantic  # update .ir snapshots
```

Fixtures run via `wjsm run <file>` subprocess, comparing exit code + stdout + stderr against `.expected` files. `build.rs` generates one `#[test]` per fixture into `tests/gen/` (gitignored).

## Architecture

Linear pipeline, each stage a `fn transform(input) -> Result<output>`:

```
source → wjsm-parser (swc_core → AST)
       → wjsm-semantic (AST → IR: scope analysis, TDZ, hoisting)
       → wjsm-backend-wasm (IR → WASM bytes)
       → wjsm-runtime (wasmtime execution + host functions)
```

Multi-file: `wjsm-module` sits between semantic and backend (dependency graph + bundling).

| Crate | Role | Public API |
|---|---|---|
| `wjsm-parser` | swc_core → AST | `parse_module(&str)` |
| `wjsm-semantic` | AST → IR (scope tree, TDZ, hoisting) | `lower_module`, `lower_modules`, `lower_eval_module` |
| `wjsm-ir` | IR types (zero deps) | `Module`, `Function`, `BasicBlock`, `Instruction`, `value` |
| `wjsm-backend-wasm` | IR → WASM | `compile(&Program)`, `compile_eval(&Program)` |
| `wjsm-backend-jit` | Stub (not implemented) | `compile(&Program)` |
| `wjsm-runtime` | wasmtime execution + host functions | `execute(&[u8])`, `execute_with_writer` |
| `wjsm-module` | ESM/CJS bundling | `bundle(entry, root_path)` |
| `wjsm-test262` | test262 conformance runner | — |
| `wjsm-cli` | CLI: build/run/check/eval/dump-ir/dump-ast/dump-wat/disasm/fmt/init/size/validate | `main_entry()` |
| `wjsm-runtime-snapshot` | build-time embedded snapshot bytes | `EMBEDDED_STARTUP_SNAPSHOT` |
| `wjsm-runtime-support` | build-time precompiled support cwasm | `EMBEDDED_SUPPORT_CWASM` |
| `wjsm-snapshot-format` | snapshot byte format + ABI hash (zero wasmtime) | `decode_snapshot`, `abi_hash` |

Dep graph: `parser → semantic → ir ← backend-wasm → runtime → cli`. `wjsm-module` branches off `semantic → backend-wasm`. Build-time support crates: `wjsm-runtime-snapshot` / `wjsm-runtime-support` → `OUT_DIR` artifacts consumed by `wjsm-runtime` → `wjsm-cli` via `install_embedded_*`; `wjsm-snapshot-format` is dependency-free and consumed by snapshot build.rs + runtime.

### Key directories

```
crates/wjsm-semantic/src/lowerer_*.rs        # lowering submodules (per AST category)
crates/wjsm-backend-wasm/src/compiler_*.rs   # codegen submodules
crates/wjsm-ir/docs/ir-design.md             # IR design doc (Chinese)
crates/wjsm-runtime/src/runtime_*.rs         # runtime submodules
crates/wjsm-runtime/src/host_imports/        # host import implementations
fixtures/{happy,errors,semantic,modules}/    # test fixtures + snapshots
tests/fixture_runner.rs                      # E2E harness
```

### Load-bearing conventions

**NaN-boxed values**: all JS values are `i64`. Boxing base `0x7FF8_0000_0000_0000`; tags at bits 32-37 (mask `0x1F`): `NATIVE_CALLABLE=0x0, STRING=1, UNDEFINED=2, NULL=3, BOOL=4, EXCEPTION=5, ITERATOR=6, ENUMERATOR=7, OBJECT=8, FUNCTION=9, CLOSURE=0xA, ARRAY=0xB, BOUND=0xC, BIGINT=0xD, SYMBOL=0xE, REGEXP=0xF, PROXY=0x10, SCOPE_RECORD=0x11`. Heap type tags: `OBJECT=0x00, ARRAY=0x01, PROMISE=0x02, CONTINUATION=0x03, ASYNC_GENERATOR=0x04, ARGUMENTS=0x05`. Raw f64 falls through when exponent != all-1s or quiet bit unset.

**Two-phase lowering**: (1) pre-declare — hoist `var` to function scope (init `undefined`), register `let`/`const` in block scope (TDZ, uninit); (2) lower — walk AST emitting IR. Ensures TDZ + hoisting semantics.

**Scope tree**: `ScopeKind::Block`/`Function`, `VarKind::Var`/`Let`/`Const`. Names scope-qualified in IR as `${scope_id}.{name}` (e.g. `$0.x`); new scopes increment the prefix. Lookup walks scope chain upward.

**WASM contract (Normal mode)**: imports `env.{memory, __table, <14 host funcs>, <19 globals>}` + `wjsm_support.{obj_new, obj_get, obj_set, obj_delete, arr_new, elem_get, elem_set, string_eq, to_int32, get_proto_from_ctor}`; re-exports `memory`/`__table`/globals for `WasmEnv::from_caller`；exports `main()`。Eval mode 仍 inline 所有 helpers。String constants in DataSection at offset 0, nul-terminated. Primordial property names (Array.prototype methods, `length`, `name`, `Symbol.toStringTag`, etc.) occupy fixed offsets at 224–493; user strings start at offset 493 (`USER_STRING_START`).

**Function-property handle layout**: function property objects occupy handles `function_props_base..function_props_base+num_ir_functions` (no longer `0..num_ir_functions`). GC roots must read `__function_props_base` to determine the range.

**Startup snapshot** (default on; set `WJSM_STARTUP_SNAPSHOT=0`/`false`/`off` to disable; set `WJSM_STARTUP_SNAPSHOT_DEBUG=1` for recoverable startup diagnostics): relocatable primordial heap snapshot — captures post-bootstrap object heap (after `__wjsm_bootstrap_once` and host post-bootstrap), handle table relative offsets, runtime strings, stateless NativeCallables, and seed Array.prototype method table metadata (`arr_proto_table_base`, length, ABI hash). Restore skips `__wjsm_bootstrap_once` in `main()`, verifies the current module exports the same Array.prototype table ABI, and remaps Array.prototype method function values to the current module `__arr_proto_table_base`. ABI hash inputs: format version, NaN-box constants, heap type tags, primordial string offsets **and** content, `SnapshotNativeCallable` discriminants, property slot constants — any change invalidates embedded snapshot compatibility and falls back to cold startup. New builtin/NativeCallable/primordial string must update `abi_hash()` in `wjsm-snapshot-format`; Array.prototype method table changes are guarded by backend-exported `__arr_proto_table_hash`. Runtime disk startup snapshot cache is retired; no customer-machine snapshot cache is written. See `docs/adr/0003-startup-snapshot-boundary.md` for format and current limitations.

**Build-time embedded runtime** (ADR 0004; default on): three ship-time-stable artifacts produced by `cargo build` build.rs's: (1) `wjsm-runtime-snapshot/build.rs` → `OUT_DIR/wjsm_startup_snapshot.bin` (`include_bytes!`'d into binary); (2) `wjsm-runtime-support/build.rs` → `OUT_DIR/wjsm_support.cwasm` (wasmtime `precompile_module` → cwasm; Normal mode user modules import 10 helpers from this: `obj_new`/`obj_get`/`obj_set`/`obj_delete`/`arr_new`/`elem_get`/`elem_set`/`string_eq`/`to_int32`/`get_proto_from_ctor`; bootstrap functions stay inline); (3) `wjsm-runtime/builtin_js/manifest.rs` lists ordered `(name, source)` for snapshot-time JS extension eval (currently empty). `wjsm-cli::main_entry` calls `wjsm_runtime::install_embedded_startup_snapshot` + `install_embedded_support_cwasm` at startup. All three feed a unified ABI hash via `wjsm-snapshot-format::register_abi_hash_external_input`: combined hash of `wjsm_runtime_support::support_module_layout_hash() || builtin_js_bundle_hash()`. ABI mismatch → cold startup. Crates `wjsm-runtime-snapshot` / `wjsm-runtime-support` / `wjsm-snapshot-format` each carry an `embedded` cargo feature (default on); disabling embedded removes build-time artifacts and falls back to cold startup only. See `docs/adr/0004-build-time-embedded-runtime.md`.

**IR dump format** (stable snapshot, not AST pretty-print):
```
module {
  constants:
    c0 = number(1.0)
  fn @main [entry=bb0]:
    bb0:
      %0 = const c0
      call builtin.console.log(%0)
      return
}
```
Constants: `c0, c1, …`. Basic blocks: `bb0, bb1, …`. IR identifiers: `PascalCase` (`BasicBlockId`, `ValueId`).

## Conventions

- **Rust 2024**, default rustfmt. Types `UpperCamelCase`, functions/vars `snake_case`.
- **Comments in Chinese**. IR design docs in Chinese. README bilingual (Chinese primary). AGENTS.md in English.
- **Error handling**: `anyhow::Result` in CLI/runtime/backend (`bail!` for early exit, `.with_context()` at crate boundaries). `thiserror` in semantic (`LoweringError::Diagnostic { start, end, message }` preserves span).
- **Single-function public API per crate**: one or two public functions, rest private.
- **Modular lowering/backend**: `lowerer_*.rs` / `compiler_*.rs` submodules per AST/codegen category.
- **File size**: a source file should generally fit in one screenful of code. Target ≤500 lines; anything approaching 1000+ lines is a strong signal to refactor into smaller, more focused units. When a file exceeds this threshold, prefer splitting by responsibility (e.g. per AST category, per host import family) rather than growing it further.
- **Function size**: a function should do one thing and its name should fully describe that thing. Target ≤30 lines; anything approaching 50–100 lines is a strong signal to extract meaningful sub-functions. Length is a proxy for cohesion — if you can't summarize the body in one sentence, it's too long.

## Spec compliance (hard rules)

- All semantics **must** match ECMAScript spec exactly. Spec is the single source of truth.
- Unreachable code is valid JS — never reject it (skip silently or warn).
- Early errors (duplicate strict-mode params, `super` outside class, etc.) must be detected.
- **No partial implementations.** No stubs, no skipped edge cases, no "will fix later" TODOs, no MVP compromises. If a feature can't be spec-complete, it must not be merged. "能跑" is not acceptable.

## Problem resolution (escalation ladder)

Exhaust each step before the next:

1. **Explore codebase**: `search`, `ast_grep`, codegraph tools. Understand existing patterns first.
2. **Read the ECMAScript spec**: the relevant section. Don't guess or infer from engine bugs.
3. **Study real engines**: V8 (source via `opensrc`), Bun, Deno. Document what each does and why.
4. **Ask the user**: only after 1-3 are exhausted. Provide: (a) codebase findings, (b) spec text, (c) engine behaviors, (d) open question + your analysis.

## Adding a feature

0. **Module** (`wjsm-module`): update bundler/deps/CJS transform if cross-file.
1. **IR** (`wjsm-ir`): add `Instruction`/`Constant` variants + `Display` impls. Add `value.rs` helpers if new value type.
2. **Semantic** (`wjsm-semantic`): handle AST node in the right `lowerer_*.rs`. Update `ScopeTree` if scoping changes. Add `LoweringError` variants if new error conditions.
3. **Backend** (`wjsm-backend-wasm`): emit WASM in the right `compiler_*.rs`. Update imports/exports + `encode_constant`.
4. **Runtime** (`wjsm-runtime`): add host function imports. Update `render_value`.
5. **Tests**: add `fixtures/{happy,errors}/<name>.js` + `.expected`. Add `fixtures/semantic/<name>.ir` if IR shape changes.

## Rules

- **Temporary files** (`*.wasm`, `*.o`, caches, test data) go in `/tmp`, never in the project dir. If accidentally committed: `git rm --cached <file>` then commit.
- **Commit**: `feat:` / `fix:` / `docs:` / `refactor:` prefixes. Keep concise.

## Aegis

If Aegis is installed: match task to Aegis skill at turn start; simple/low-risk tasks skip the full workflow. Complex/diagnostic/architecture/refactor/cross-module tasks use the corresponding Aegis workflow. Confirm scope + verification before implementing; show new evidence before claiming done. Aegis is methodology, not final authority — user instructions and project rules override it.
