# Repository Guidelines

## Project Overview

`wjsm` is an AOT JavaScript/TypeScript runtime that compiles JS/TS to WebAssembly.
It does **not** use V8 — it uses `swc_core` for parsing, `wasm-encoder` for codegen, and `wasmtime` for execution.

Current state: implements functions, closures (lexical capture, arrow this), async/await, async generators, Promises (all combinators + microtask ordering), classes (ctor, methods, getter/setter, static, static blocks, super), objects (property descriptors, prototype chains, computed props, spread), arrays (literal + all ES Array.prototype methods), template strings (tagged), destructuring (array, object, params, defaults), control flow (if/else, switch, while/do-while/for/for-in/for-of, break/continue/labeled, try/catch/finally/throw), modules (ES import/export, CommonJS require/exports, dynamic import()), eval (direct/indirect, strict), BigInt, Symbol, RegExp, Proxy (basic), JSX, TypeScript type annotations/enum/interface/namespace/type alias/type assertions, `using` declarations (explicit resource management), JSON, Object/Array/String built-in methods, console (log/error/warn/info/debug/trace), timer APIs (setTimeout/setInterval/clearTimeout/clearInterval), fetch (data: URL), mark-sweep GC, NaN-boxed value encoding. Missing: Proxy traps beyond basic, Reflect, weak references, SharedArrayBuffer, Atomics, some newer esproposals.

## Architecture & Data Flow

Linear compilation pipeline — each stage produces output consumed by the next:

```
source (js/ts)
  → wjsm-parser: swc_core → swc_ast::Module
  → wjsm-semantic: AST lowering → wjsm_ir::Program (scope analysis, TDZ, hoisting)
  → wjsm-backend-wasm: IR → WASM bytes (wasm-encoder)
  → wjsm-runtime: wasmtime execution with host function linkage
```

For multi-file projects, the module system (`wjsm-module`) sits between semantic and backend:

```
entry.js + dep1.js + ...
  → wjsm-parser (per file) → swc_ast::Module[]
  → wjsm-module: dependency graph resolution + bundling
  → wjsm-backend-wasm: IR → WASM bytes (single binary)
  → wjsm-runtime: execution
```

Each stage has a single public function following `fn transform(input) -> Result<output>`:
- `wjsm_parser::parse_module(source: &str) -> Result<swc_ast::Module>`
- `wjsm_semantic::lower_module(module: swc_ast::Module) -> Result<Program, LoweringError>`
- `wjsm_backend_wasm::compile(program: &Program) -> Result<Vec<u8>>`
- `wjsm_runtime::execute(wasm_bytes: &[u8]) -> Result<()>` / `execute_with_writer(wasm_bytes, writer)`
- `wjsm_module::bundle(entry: &str, root_path: &Path) -> Result<Vec<u8>>`

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
| `wjsm-module` | ES module / CommonJS bundling | `bundle(entry, root_path)`, `is_es_module()`, `is_commonjs_module()` | swc_core, wjsm-parser, wjsm-semantic, wjsm-backend-wasm |
| `wjsm-test262` | test262 conformance test runner | `config`, `exec`, `read` modules | clap, serde, walkdir, rayon |
| `wjsm-cli` | CLI entry (`build`, `run` subcommands) | `main_entry()`, `execute(Cli)` | all above + clap |

Dependency graph: `parser → semantic → ir ← backend-wasm → runtime → cli(root)`, with `wjsm-module` branching off `semantic → backend-wasm`, and `wjsm-test262` standalone.

## Key Directories

```
crates/              # All crate source code
  wjsm-ir/docs/      # IR design docs (ir-design.md)
  wjsm-module/       # ES module / CommonJS bundler
  wjsm-test262/      # test262 runner
fixtures/
  happy/             # Happy-path JS/TS fixtures (*.js/*.ts + *.expected snapshots)
  errors/            # Error-path JS/TS fixtures (*.js/*.ts + *.expected snapshots)
  semantic/          # IR snapshot expected outputs (*.ir)
  modules/           # Module system fixtures (ESM & CJS)
tests/
  integration/       # E2E fixture runner tests (happy, errors, modules)
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

# Run test262 conformance suite
cargo run -p wjsm-test262

# Update E2E snapshots (happy-path, error, and module fixtures)
WJSM_UPDATE_FIXTURES=1 cargo test

# Watch for changes and re-run tests
cargo watch -x test

# Check for compilation errors without full build
cargo check
```

The root `Cargo.toml` defines workspace deps at shared versions; individual crates reference them with `workspace = true`.

## Code Conventions

### JavaScript Specification Compliance

**CRITICAL: wjsm must strictly follow the ECMAScript specification.**

- All language semantics must match the ECMAScript specification exactly.
- Unreachable code (e.g., statements after `return`, `throw`, `break`, `continue`) is **valid JavaScript** and must not cause compile errors. The compiler should skip unreachable statements silently or emit warnings, but never reject them.
- Early errors defined by the specification (e.g., duplicate parameter names in strict mode, `super` outside class) must be detected and reported.
- Non-standard extensions or deviations from the specification require explicit documentation and justification.
  - **No PoC compromises.** Every implementation **MUST** be spec-complete for the feature being added. The project is not a prototype — it is a runtime targeting full specification compliance. Partial stubs, deliberately skipped edge cases, "will fix later" TODOs, knowingly incomplete implementations, and speculative shortcuts are strictly prohibited. If a feature cannot be implemented correctly for all cases defined by the spec, it **MUST NOT** be merged.

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
- **NaN-boxed value encoding**: all JS values are carried as `i64`. Bits 52-63 store the IEEE 754 exponent; the quiet NaN space is used as a boxing base (`0x7FF8_0000_0000_0000`). Tags at bits 32-37 (mask `0x1F`) distinguish value types: `TAG_STRING=1`, `TAG_UNDEFINED=2`, `TAG_NULL=3`, `TAG_BOOL=4`, `TAG_EXCEPTION=5`, `TAG_ITERATOR=6`, `TAG_ENUMERATOR=7`, `TAG_OBJECT=8`, `TAG_FUNCTION=9`, `TAG_CLOSURE=0xA`, `TAG_ARRAY=0xB`, `TAG_BOUND=0xC`, `TAG_BIGINT=0xD`, `TAG_SYMBOL=0xE`, `TAG_REGEXP=0xF`, `TAG_PROXY=0x10`. Raw f64 values fall through when exponent != all-1s or quiet bit not set.
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

### Problem Resolution

When encountering an implementation challenge or semantic ambiguity, follow this escalation ladder. Each step **MUST** be exhausted before proceeding to the next.

1. **Explore the codebase first.** Search for existing patterns, reference implementations, or prior discussions using `lsp references`, `search`, `ast_grep`, and `gitnexus_query`. Understand how related features are implemented before designing a solution.

2. **Consult the ECMAScript specification.** Read the relevant section of the spec. The spec is the single source of truth — do not guess, infer behavior from other engines' bugs, or rely on secondhand knowledge.

3. **Study real-world engines.** If the spec is unclear or the implementation path is uncertain, examine how V8 (via source), Bun, or Deno handle the same feature. Use `opensrc` to fetch and study relevant source code or test suites. Document what each engine does and why.

4. **Ask the user last.** Only escalate when the first three steps have been exhausted and a genuine ambiguity remains. When asking, provide: (a) what you found in the codebase, (b) what the spec says, (c) what V8/Bun/Deno each do and why they differ (if they do), and (d) the specific open question with your analysis and recommended resolution.
## Testing & QA

Three-tier test strategy:

### 1. IR Unit Tests (`crates/wjsm-ir/tests/ir_dump.rs`)
- Programmatically construct `Module`/`Function`/`BasicBlock` objects
- Assert `dump_text()` output matches expected textual format
- Tests basic IR serialization

### 2. Semantic Snapshot Tests (`crates/wjsm-semantic/tests/lowering_snapshots.rs`)
- 95+ snapshot tests: read `fixtures/happy/<name>.js`, parse + lower, compare `dump_text()` against `fixtures/semantic/<name>.ir`
- 6 inline error diagnostic tests: assert `LoweringError::Diagnostic` message content
- **No auto-bless** for `.ir` files — update them manually when lowering changes
- Pattern: `assert_snapshot("name")` helper reads from `fixtures/happy/{name}.js` and compares against `fixtures/semantic/{name}.ir`

### 3. E2E Fixture Runner Tests (`tests/integration/fixtures.rs`)
- Discovers all `.js`/`.ts` files under `fixtures/{happy,errors,modules}/`
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
**Happy path** (210+ fixtures): covers functions, closures, classes, async/await, promises, modules, objects, arrays, control flow, try/catch/finally, template strings, destructuring, eval, BigInt, Symbol, RegExp, Object/Array/String built-in methods, typeof, operators, TypeScript type annotations/enum/interface/namespace/type alias/type assertions, JSX, `using` declarations, proxy.

**Error path** (30 fixtures): undeclared_var, const_reassign, tdz, let_redeclare, redeclare combinations, unsupported statements/expressions, syntax_error, await/yield/for-await outside valid contexts, break/continue outside loop, unknown/duplicate labels, with statement, for-in/for-of bad LHS, for-of non-iterable, bigint JSON, regex_invalid, get_own_property_descriptor non-object, define_property_accessor non-callable, eval errors (strict var leak, syntax, throw, lexical redeclare, const reassign).

### Not yet tested
Full test262 conformance suite (via `wjsm-test262` crate), Proxy traps beyond basic, Reflect API, weak references, SharedArrayBuffer, Atomics.

## Important Files

| File | Purpose |
|---|---|
| `src/main.rs` | Workspace entry point (2 lines: `wjsm_cli::main_entry()`) |
| `Cargo.toml` | Workspace root, shared dependency versions |
| `crates/wjsm-ir/src/lib.rs` | IR types: `Module`, `Function`, `BasicBlock`, `Instruction`, `Terminator`, `Constant` |
| `crates/wjsm-ir/src/value.rs` | NaN-boxed value encoding (`i64`): encode/decode for all JS value types |
| `crates/wjsm-ir/docs/ir-design.md` | Detailed IR design rationale and format specification |
| `crates/wjsm-semantic/src/lib.rs` | Scope tree (`ScopeTree`, `Scope`, `VarInfo`), lowering logic (`Lowerer`), diagnostics (`LoweringError`) |
| `crates/wjsm-backend-wasm/src/lib.rs` | WASM codegen: two-pass local assignment, f64 reinterpretation, string data segments |
| `crates/wjsm-runtime/src/lib.rs` | wasmtime execution: imports `console_log`, reads strings from WASM memory, renders values |
| `crates/wjsm-cli/src/lib.rs` | CLI entry: `build`/`run` subcommands, pipeline orchestration |
| `crates/wjsm-module/src/lib.rs` | Module bundler: ES import/export + CommonJS require support |
| `crates/wjsm-test262/src/` | test262 conformance test runner |
| `tests/fixture_runner.rs` | Shared `FixtureRunner` harness for E2E snapshot tests |
| `fixtures/semantic/*.ir` | Stable IR snapshots — must be manually updated when lowering changes |
| `todo.md` | Gap analysis of missing language features |

## Adding a New Language Feature

The typical flow:

0. **Module considerations** (`wjsm-module`): If the feature involves cross-file interaction (import/export, module namespace), update the bundler, dependency graph, and/or CJS transform.

1. **IR layer** (`wjsm-ir`): Add instructions/variants to `Instruction` enum and/or `Constant` enum if a new constant kind is needed. Update `Display` impls for dump format. Add helpers in `value.rs` if a new JS value type (e.g. boolean) is needed.

2. **Semantic layer** (`wjsm-semantic`): Handle the new SWC AST node in the appropriate `lower_*` method. Update `ScopeTree` methods if new scoping rules apply. Add diagnostic variants to `LoweringError` if new error conditions exist. Update `stmt_kind`/`expr_kind` helpers.

3. **Backend** (`wjsm-backend-wasm`): Emit corresponding WASM instructions in `compile_instruction`. Add any new import/export signatures. Update `encode_constant` for new constant types.

4. **Runtime** (`wjsm-runtime`): Add new host function imports (in `Linker` or `Func::wrap`). Update `render_value` for new value display.

5. **Tests**: Add fixture to `fixtures/happy/` or `fixtures/errors/` with `.js` + `.expected`. Add snapshot test to `fixtures/semantic/<name>.ir` if lowering produces new IR shapes. Add integration test entry in `tests/integration/fixtures.rs`. If the feature has test262 coverage, ensure the test262 runner works.

## Commit Conventions

- `feat: ` 新功能
- `fix: ` 修复
- `docs: ` 文档更新
- `refactor: ` 重构
- 保持简洁清晰

<!-- gitnexus:start -->
# GitNexus — Code Intelligence

This project is indexed by GitNexus as **wjsm** (270,283 nodes, 384,564 edges, 300 execution flows). Use the GitNexus MCP tools to understand code, assess impact, and navigate safely.

> 索引过期时，运行 `bunx gitnexus analyze --skip-agents-md --embeddings` 更新。
> 初次索引或在大型影响分析前，建议先重建索引保证数据新鲜。

## Always Do

- **MUST run impact analysis before editing any symbol.** Before modifying a function, class, or method, run `gitnexus_impact({target: "symbolName", direction: "upstream"})` and report the blast radius (direct callers, affected processes, risk level) to the user.
- **MUST run `gitnexus_detect_changes()` before committing** to verify your changes only affect expected symbols and execution flows.
- **MUST run `bunx gitnexus analyze --skip-agents-md --embeddings` after `git commit` or `git merge`** to update the index before proceeding with analysis or editing.
- **MUST warn the user** if impact analysis returns HIGH or CRITICAL risk before proceeding with edits.
- When exploring unfamiliar code, use `gitnexus_query({query: "concept"})` to find execution flows instead of grepping. It returns process-grouped results ranked by relevance.
- When you need full context on a specific symbol — callers, callees, which execution flows it participates in — use `gitnexus_context({name: "symbolName"})`.

## Never Do

- NEVER edit a function, class, or method without first running `gitnexus_impact` on it.
- NEVER ignore HIGH or CRITICAL risk warnings from impact analysis.
- NEVER rename symbols with find-and-replace — use `gitnexus_rename` which understands the call graph.
- NEVER commit changes without running `gitnexus_detect_changes()` to check affected scope.

## Resources

| Resource | Use for |
|---|---|
| `gitnexus://repo/wjsm/context` | Codebase overview, check index freshness |
| `gitnexus://repo/wjsm/clusters` | All functional areas |
| `gitnexus://repo/wjsm/processes` | All execution flows |
| `gitnexus://repo/wjsm/process/{name}` | Step-by-step execution trace |

## CLI

| Task | Read this skill file |
|---|---|
| Understand architecture / "How does X work?" | `.claude/skills/gitnexus/gitnexus-exploring/SKILL.md` |
| Blast radius / "What breaks if I change X?" | `.claude/skills/gitnexus/gitnexus-impact-analysis/SKILL.md` |
| Trace bugs / "Why is X failing?" | `.claude/skills/gitnexus/gitnexus-debugging/SKILL.md` |
| Rename / extract / split / refactor | `.claude/skills/gitnexus/gitnexus-refactoring/SKILL.md` |
| Tools, resources, schema reference | `.claude/skills/gitnexus/gitnexus-guide/SKILL.md` |
| Index, status, clean, wiki CLI commands | `.claude/skills/gitnexus/gitnexus-cli/SKILL.md` |

<!-- gitnexus:end -->
