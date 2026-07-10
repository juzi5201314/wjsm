# wjsm

AOT JavaScript/TypeScript runtime: compiles JS/TS to WebAssembly using `swc_core` (parsing), `wasm-encoder` (codegen), `wasmtime` (execution). No V8.

## Build & Test

```bash
cargo build                              # build workspace
cargo run -- run test.ts                 # compile + execute
cargo run -- build test.ts -o out.wasm   # compile to WASM
cargo run -- build test.ts --stage lower # stop at parse/lower/compile/execute
cargo run -- check test.ts               # check for errors, no execute
cargo run -- eval "1 + 2"                # expression → prints via console.log wrapper
cargo run -- run -e 'console.log(1+2)'   # script/statements (same eval pipeline as file stdin)
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

Fixtures run via `wjsm_cli::run_file_in_process` in-process, comparing exit code + stdout + stderr against `.expected` files. `build.rs` generates one `#[test]` per fixture into `tests/gen/` (gitignored).

## Debugging

Do **not** debug by inserting `console.log` or other temporary instrumentation in production code and deleting it afterward—that is fragile, pollutes diffs, and is disallowed unless every other path below is exhausted and you document why in the PR. For **user-level JS debugging**, use `--inspect` / `--inspect-brk` (CDP; ADR 0007). There is still no gdb-style IR/WASM single-step CLI.

**Quick JS snippets (agents)**: For ad-hoc **execution**, use inline source—**never** `echo '…' > /tmp/foo.js && cargo run -- run /tmp/foo.js`. All source-consuming commands (`run`, `eval`, `check`, `dump-ir`, `dump-ast`, `dump-wat`, `build`) accept `-e <SOURCE>` and `--script`—same inline-source compile path (`compile_source_to_pipeline_result` / `lower_module_with_source` / `compile`), not runtime `eval()` lowering (`lower_eval_module`) or backend eval mode (`compile_eval`). `run -e '<statements>'` for statements (add `--script` when script parsing is required); `eval '<expression>'` for a single expression (wrapped as `console.log((expr))`). stdin `-` still works for file/stdin input. `/tmp/*.wasm` and other build artifacts in `/tmp` are fine; do not put scratch `.js` in the project tree.

**Default workflow** — pin the failing layer, then fix at the owner:
1. **Reproduce narrowly**: `cargo nextest run -E 'test(happy__<name>)'` or `cargo nextest run -p wjsm-semantic -E 'test(<snapshot>)'`. Use `wjsm_cli::run_file_in_process` semantics (same as `cargo run -- run`).
2. **Semantic (lowering)**: `cargo run -- dump-ir <file>` or `cargo run -- dump-ir -e '<source>'` (optional `--format dot` for CFG; `--func <NAME>` to dump one function). Compare to `fixtures/semantic/<name>.ir` or run `cargo nextest run -p wjsm-semantic -- lowering_snapshots`. If IR is wrong, fix `lowerer_*.rs` and add/update `.ir` snapshot (`WJSM_UPDATE_SNAPSHOTS=1` only after reviewing diff).
3. **Codegen (WASM)**: If IR matches the intended shape but behavior is wrong, `cargo run -- dump-wat <file>` (or `-e '<source>'`) and/or `cargo run -- build <file> -o /tmp/x.wasm && cargo run -- disasm /tmp/x.wasm`. Use `--func <NAME>` to dump one function (correlates with `dump-ir --func <NAME>`) and `--skeleton` for a body-less overview when WAT is large. Trace basic-block order, loop headers, and host calls against IR (`bbN` in dump). Fix `compiler_*.rs` / `compiler_control.rs`.
4. **Backend static analysis** (GC spill, liveness, value tags): use `wjsm-backend-wasm` helpers `infer_value_ty`, `compute_var_liveness` in crate tests—see `tests/var_slot_liveness_gc_long_loop.rs`, `tests/compiler_gc_analysis_spill.rs`. Prefer new targeted tests over runtime logging.
5. **Runtime / host**: Read trap message (`Runtime error:` exit 2). Startup snapshot issues only: `WJSM_STARTUP_SNAPSHOT_DEBUG=1`.
6. **Inspector / CDP** (`--inspect` / `--inspect-brk`, issue #313 / ADR 0007): enables statement-level `debug_break` host safepoints + wasmtime `guest_debug` (Cranelift only; Winch rejected). JS `debugger;` is a real pause under inspect; without `--inspect` it remains a compile-time no-op. Chrome DevTools connects via `ws://127.0.0.1:9229/...` (see stderr `Debugger listening on …`). `require('inspector')` exposes `url()` / `open` / `close`.
7. **node:vm multi-realm** (`require('vm')` / `node:vm`, issue #313 / ADR 0008): single Store multi-realm — pristine reachable-graph clone, `execution_realm` + `__array_proto_handle`/`__object_proto_handle` swap (no TLS), conditional GC roots, `timeout` via scoped epoch trap + interpreter `Instant` deadline. Owners: `realm.rs`, `realm_clone.rs`, `handle_remap.rs`, `runtime_node_vm.rs`, `node_vm.js`.
8. **Stage isolation**: `cargo run -- build <file> --stage parse|lower|compile` and `cargo run -- check <file>` to stop before execute.

**Evidence before fix**: state which layer failed (parse / lower / compile / runtime) with the exact command output or snapshot diff. **Tests**: lowering changes → semantic snapshots; observable behavior → `fixtures/happy` or `fixtures/errors` + `.expected` (`WJSM_UPDATE_FIXTURES=1` after review).

**Not available today** (do not assume): wasmtime/lldb native stepping UI, full CDP Profiler/Network/DOM, IR step interpreter.


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
| `wjsm-runtime-support` | build-time precompiled support cwasm variants | `embedded_support_cwasm(flavor)` |
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

**WASM contract (Normal mode)**: imports the registry-owned `env` host functions, `env.{memory, __table, <27 globals>}`, and `wjsm_support.{obj_new, obj_get, obj_set, obj_delete, arr_new, elem_get, elem_set, string_eq, to_int32, get_proto_from_ctor}`; re-exports `memory`/`__table`/the 27 globals for `WasmEnv::from_caller`; exports `main()`. Eval mode still inlines helpers while importing the same env globals. String constants in DataSection at offset 0, nul-terminated. Primordial property names (Array.prototype methods, `length`, `name`, `Symbol.toStringTag`, etc.) occupy fixed offsets at 224–493; user strings start at offset 493 (`USER_STRING_START`). GC algorithm selection is `RuntimeOptions`/CLI `--gc`/`WJSM_GC` (`mark-sweep`, `g1`, `zgc`); `WJSM_TEST_GC` remains the test-matrix override.

**Function-property handle layout**: function property objects occupy handles `function_props_base..function_props_base+num_ir_functions` (no longer `0..num_ir_functions`). GC roots must read `__function_props_base` to determine the range.

**Startup snapshot** (default on; set `WJSM_STARTUP_SNAPSHOT=0`/`false`/`off` to disable; set `WJSM_STARTUP_SNAPSHOT_DEBUG=1` for recoverable startup diagnostics): relocatable primordial heap snapshot — captures post-bootstrap object heap (after `__wjsm_bootstrap_once` and host post-bootstrap), handle table relative offsets, runtime strings, stateless NativeCallables, and seed Array.prototype method table metadata (`arr_proto_table_base`, length, ABI hash). Restore skips `__wjsm_bootstrap_once` in `main()`, verifies the current module exports the same Array.prototype table ABI, and remaps Array.prototype method function values to the current module `__arr_proto_table_base`. ABI hash inputs: format version, NaN-box constants, heap type tags, primordial string offsets **and** content, `SnapshotNativeCallable` discriminants, property slot constants — any change invalidates embedded snapshot compatibility and falls back to cold startup. New builtin/NativeCallable/primordial string must update `abi_hash()` in `wjsm-snapshot-format`; Array.prototype method table changes are guarded by backend-exported `__arr_proto_table_hash`. Runtime disk startup snapshot cache is retired; no customer-machine snapshot cache is written. See `docs/adr/0003-startup-snapshot-boundary.md` for format and current limitations.

**Build-time embedded runtime** (ADR 0004; default on): three ship-time-stable artifact families produced by `cargo build` build.rs's: (1) `wjsm-runtime-snapshot/build.rs` → `OUT_DIR/wjsm_startup_snapshot.bin` (`include_bytes!`'d into binary); (2) `wjsm-runtime-support/build.rs` → precompiled support cwasm variants for `mark-sweep`, `g1`, and `zgc` (wasmtime `precompile_module`; Normal mode user modules import 10 helpers from the selected variant: `obj_new`/`obj_get`/`obj_set`/`obj_delete`/`arr_new`/`elem_get`/`elem_set`/`string_eq`/`to_int32`/`get_proto_from_ctor`; support also exports bootstrap helpers for its own ABI); (3) `wjsm-runtime/builtin_js/manifest.rs` lists ordered `(name, source)` for snapshot-time JS extension eval (currently empty). `wjsm-cli::main_entry` installs the embedded startup snapshot and default support cwasm; runtime instantiation selects the support flavor matching the active GC algorithm, falling back to compiling the corresponding wasm bytes if cwasm deserialization is incompatible. All artifact families feed a unified ABI hash via `wjsm-snapshot-format::register_abi_hash_external_input`: combined hash of `wjsm_runtime_support::support_module_layout_hash() || builtin_js_bundle_hash()`. ABI mismatch → cold startup. Crates `wjsm-runtime-snapshot` / `wjsm-runtime-support` / `wjsm-snapshot-format` each carry an `embedded` cargo feature (default on); disabling embedded removes build-time artifacts and falls back to cold startup only. See `docs/adr/0004-build-time-embedded-runtime.md`.

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
- **Ad-hoc JS execution**: Do **not** write scratch `.js` under `/tmp` (or elsewhere) just to `cargo run -- run <file>`. Use `cargo run -- run -e '…'` or `cargo run -- eval '…'` instead. All source commands (`dump-ir`, `dump-wat`, `dump-ast`, `check`, `build`) also accept `-e '…'` (see **Debugging** → Quick JS snippets).
- **Commit**: `feat:` / `fix:` / `docs:` / `refactor:` prefixes. Keep concise.
- **Warnings**: if a build produces compiler warnings, fix them immediately before reporting the task as complete. Zero-warning builds are the baseline.

## Aegis

If Aegis is installed: match task to Aegis skill at turn start; simple/low-risk tasks skip the full workflow. Complex/diagnostic/architecture/refactor/cross-module tasks use the corresponding Aegis workflow. Confirm scope + verification before implementing; show new evidence before claiming done. Aegis is methodology, not final authority — user instructions and project rules override it.
