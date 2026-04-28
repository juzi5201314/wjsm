# Repository Guidelines

## Project Overview

`wjsm` is an AOT JavaScript/TypeScript runtime that compiles JS/TS to WebAssembly.
It does **not** use V8 — it uses `swc_core` for parsing, `wasm-encoder` for codegen, and `wasmtime` for execution.

Current state: PoC with basic arithmetic, string literals, `console.log`, variable declarations (var/let/const), block scoping, var hoisting, compound assignment, and semantic error diagnostics. Missing: control flow, functions, objects, arrays, modules, closures, async, most Node.js APIs.

## Architecture & Data Flow

Linear compilation pipeline — each stage produces output consumed by the next:

```
source (js/ts)
  → wjsm-parser: swc_core → swc_ast::Module
  → wjsm-semantic: AST lowering → wjsm_ir::Program (scope analysis, TDZ, hoisting)
  → wjsm-backend-wasm: IR → WASM bytes (wasm-encoder)
  → wjsm-runtime: wasmtime execution with host function linkage
```

Each stage has a single public function following `fn transform(input) -> Result<output>`:
- `wjsm_parser::parse_module(source: &str) -> Result<swc_ast::Module>`
- `wjsm_semantic::lower_module(module: swc_ast::Module) -> Result<Program, LoweringError>`
- `wjsm_backend_wasm::compile(program: &Program) -> Result<Vec<u8>>`
- `wjsm_runtime::execute(wasm_bytes: &[u8]) -> Result<()>` / `execute_with_writer(wasm_bytes, writer)`

The CLI (`wjsm-cli`) chains these: `parse → lower → compile → execute|write`.

The JIT backend (`wjsm-backend-jit`) shares the same `compile(&Program) -> Result<Vec<u8>>` signature but returns `bail!("JIT backend is not implemented yet")`.

## Workspace Structure

Cargo workspace (edition 2024, resolver 2). Root `src/main.rs` is a thin wrapper delegating to `wjsm-cli`.

| Crate | Responsibility | Public API | Deps |
|---|---|---|---|
| `wjsm-parser` | `swc_core` → `swc_ast::Module` | `parse_module(&str)` | swc_core |
| `wjsm-semantic` | AST lowering → `wjsm_ir::Program` (scope tree, TDZ, var hoisting, diagnostics) | `lower_module(swc_ast::Module)` | swc_core, thiserror, wjsm-ir |
| `wjsm-ir` | Intermediate representation | `Module`, `Function`, `BasicBlock`, `Instruction`, `value` module | none (zero external deps) |
| `wjsm-backend-wasm` | IR → WASM bytes | `compile(&Program)` | wasm-encoder, wjsm-ir |
| `wjsm-backend-jit` | Stub — `bail!("not implemented")` | `compile(&Program)` | wjsm-ir |
| `wjsm-runtime` | Execute WASM via wasmtime, provide host functions | `execute(&[u8])`, `execute_with_writer(&[u8], W)` | wasmtime, wjsm-ir |
| `wjsm-cli` | CLI entry (`build`, `run` subcommands) | `main_entry()`, `execute(Cli)` | all above + clap |

Dependency graph: `parser → semantic → ir ← backend-wasm → runtime → cli(root)`

## Key Directories

```
crates/              # All crate source code
  wjsm-ir/docs/      # IR design docs (ir-design.md)
fixtures/
  happy/             # Happy-path JS fixtures (*.js + *.expected snapshots)
  errors/            # Error-path JS fixtures (*.js + *.expected snapshots)
  semantic/          # IR snapshot expected outputs (*.ir)
  modules/           # Empty — module system not implemented yet
tests/
  integration/       # E2E fixture runner tests
  unit/              # Placeholder unit tests
  fixture_runner.rs  # FixtureRunner harness (shared across integration tests)
src/
  main.rs            # Workspace root entry point (delegates to wjsm_cli)
.config/
  nextest.toml       # Nextest configuration
```

## Development Commands

```bash
# Build entire workspace
cargo build

# Build release
cargo build --release

# Compile JS/TS to WASM
cargo run -- build test.ts -o out.wasm

# Compile and execute immediately
cargo run -- run test.ts

# Run all tests
cargo test

# Run tests for a specific crate
cargo test -p wjsm-semantic

# Run with nextest (configured in .config/nextest.toml)
cargo nextest run

# Run specific test function
cargo test -p wjsm-semantic -- hello_fixture_matches_ir_snapshot

# Update E2E snapshots (happy-path and error fixtures)
WJSM_UPDATE_FIXTURES=1 cargo test

# Watch for changes and re-run tests
cargo watch -x test

# Check for compilation errors without full build
cargo check
```

The root `Cargo.toml` defines workspace deps at shared versions; individual crates reference them with `workspace = true`.

## Code Conventions

### Rust Edition & Style
- Rust 2024 edition; edition is set once in `[workspace.package]` and inherited by all crates.
- No explicit `rustfmt` config — use default `rustfmt` style.

### Error Handling
- **CLI/runtime/backend crates**: `anyhow::Result` everywhere. Use `bail!()` for early exits and `.with_context(||)`) for context on errors crossing crate boundaries.
- **Semantic analysis**: `thiserror` for crate-specific enum errors (`LoweringError::Diagnostic(Diagnostic { start, end, message })`). Structured errors preserve span info for diagnostics.
- Pattern: thin layer of `anyhow` at the API boundary, `thiserror` for domain errors.

### Naming
- Types: `UpperCamelCase`
- Functions and variables: `snake_case`
- IR identifiers use `PascalCase` (e.g. `BasicBlockId`, `ValueId`)
- IR variable names in dumps: `${scope_id}.{name}` (e.g. `$0.x`). Declared names in the semantic layer follow this pattern — new scopes increment the prefix.
- Constant pool: `c0`, `c1`, ... in dumps
- Basic blocks: `bb0`, `bb1`, ... in dumps

### Comments
- Code comments are written in **Chinese** (项目主力语言是中文注释).
- IR design docs are in Chinese.
- README is bilingual (Chinese primary, English secondary).
- AGENTS.md is in English (for AI tooling).

### Architecture Patterns
- **Single-function public API per crate**: each crate exposes one or two public functions; the rest is private.
- **SSA-like IR**: instructions produce `ValueId` outputs consumed by later instructions. Not full SSA (no phi nodes), but the `dest`-based naming is SSA-style.
- **NaN-boxed value encoding**: all JS values are carried as `i64`. Bits 52-63 store the IEEE 754 exponent; the quiet NaN space is used as a boxing base (`0x7FF8_0000_0000_0000`). Tags at bits 32-34 distinguish string pointers (tag=1) and undefined (tag=2). Raw f64 values fall through when exponent != all-1s or quiet bit not set.
- **Two-phase lowering**: `wjsm-semantic` processes statements in two passes — (1) pre-declare: hoist `var` to function scope (initialised with `undefined`), register `let`/`const` in block scope (TDZ, uninitialised), (2) lower: walk AST emitting IR instructions. This ensures TDZ checks and hoisting semantics work correctly.
- **Scope tree**: lexical scope tree with `ScopeKind::Block` / `ScopeKind::Function`, `VarKind::Var` / `Let` / `Const`. Names are scope-qualified in IR. Lookup walks the scope chain upward.
- **WASM contract** (generated module): imports `env.console_log(i64)`, exports `main()`, exports `memory` (1 page initial, no max). String constants are embedded in a DataSection at offset 0, nul-terminated.
- **Textual IR dump** is the stable snapshot format (not AST pretty-printing). Format:
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

## Testing & QA

Three-tier test strategy:

### 1. IR Unit Tests (`crates/wjsm-ir/tests/ir_dump.rs`)
- Programmatically construct `Module`/`Function`/`BasicBlock` objects
- Assert `dump_text()` output matches expected textual format
- Tests basic IR serialization

### 2. Semantic Snapshot Tests (`crates/wjsm-semantic/tests/lowering_snapshots.rs`)
- 12 snapshot tests: read `fixtures/happy/<name>.js`, parse + lower, compare `dump_text()` against `fixtures/semantic/<name>.ir`
- 5 inline error diagnostic tests: assert `LoweringError::Diagnostic` message content
- **No auto-bless** for `.ir` files — update them manually when lowering changes
- Pattern: `assert_snapshot("name")` helper reads from `fixtures/happy/{name}.js` and compares against `fixtures/semantic/{name}.ir`

### 3. E2E Fixture Runner Tests (`tests/integration/fixtures.rs`)
- Discovers all `.js`/`.ts` files under `fixtures/{happy,errors}/`
- Runs `wjsm run <file>` as a subprocess
- Compares exit code + stdout + stderr against `.expected` snapshot files
- Snapshots format:
  ```
  exit_code: 0
  --- stdout ---
  Hello, World!
  --- stderr ---
  ```
- Auto-update: `WJSM_UPDATE_FIXTURES=1 cargo test` writes new `.expected` files

### Covered by fixtures
**Happy path** (13 fixtures): hello, arithmetic, let_decl, block_scope, assignment, compound_assign, compound_assign_nested, var_hoist, var_hoist_read_before_decl, var_no_init_redeclare, block_var_hoist_before_block, (plus tests/integration ones)

**Error path** (6 fixtures): undeclared_var, const_reassign, tdz, let_redeclare, unsupported_stmt, syntax_error

### Not yet tested
Modules (`fixtures/modules/` is empty), `.ts` type annotations, loops, conditionals, function definitions/calls, string operations beyond literals, objects/arrays, WASM backend in isolation (has inline tests), JIT backend, CLI error paths.

## Important Files

| File | Purpose |
|---|---|
| `src/main.rs` | Workspace entry point (2 lines: `wjsm_cli::main_entry()`) |
| `Cargo.toml` | Workspace root, shared dependency versions |
| `crates/wjsm-ir/src/lib.rs` | IR types: `Module`, `Function`, `BasicBlock`, `Instruction`, `Terminator`, `Constant` |
| `crates/wjsm-ir/src/value.rs` | NaN-boxed value encoding (`i64`): `encode_f64`, `encode_string_ptr`, `encode_undefined`, and their `is_*`/`decode_*` counterparts |
| `crates/wjsm-ir/docs/ir-design.md` | Detailed IR design rationale and format specification |
| `crates/wjsm-semantic/src/lib.rs` | Scope tree (`ScopeTree`, `Scope`, `VarInfo`), lowering logic (`Lowerer`), diagnostics (`LoweringError`) |
| `crates/wjsm-backend-wasm/src/lib.rs` | WASM codegen: two-pass local assignment, f64 reinterpretation, string data segments |
| `crates/wjsm-runtime/src/lib.rs` | wasmtime execution: imports `console_log`, reads strings from WASM memory, renders values |
| `crates/wjsm-cli/src/lib.rs` | CLI entry: `build`/`run` subcommands, pipeline orchestration |
| `tests/fixture_runner.rs` | Shared `FixtureRunner` harness for E2E snapshot tests |
| `fixtures/semantic/*.ir` | Stable IR snapshots — must be manually updated when lowering changes |
| `todo.md` | Gap analysis of missing language features |

## Adding a New Language Feature

The typical flow:

1. **IR layer** (`wjsm-ir`): Add instructions/variants to `Instruction` enum and/or `Constant` enum if a new constant kind is needed. Update `Display` impls for dump format. Add helpers in `value.rs` if a new JS value type (e.g. boolean) is needed.

2. **Semantic layer** (`wjsm-semantic`): Handle the new SWC AST node in the appropriate `lower_*` method. Update `ScopeTree` methods if new scoping rules apply. Add diagnostic variants to `LoweringError` if new error conditions exist. Update `stmt_kind`/`expr_kind` helpers.

3. **Backend** (`wjsm-backend-wasm`): Emit corresponding WASM instructions in `compile_instruction`. Add any new import/export signatures. Update `encode_constant` for new constant types.

4. **Runtime** (`wjsm-runtime`): Add new host function imports (in `Linker` or `Func::wrap`). Update `render_value` for new value display.

5. **Tests**: Add fixture to `fixtures/happy/` or `fixtures/errors/` with `.js` + `.expected`. Add snapshot test to `fixtures/semantic/<name>.ir` if lowering produces new IR shapes. Add integration test entry in `tests/integration/fixtures.rs`.

## Commit Conventions

- `feat: ` 新功能
- `fix: ` 修复
- `docs: ` 文档更新
- `refactor: ` 重构
- 保持简洁清晰
