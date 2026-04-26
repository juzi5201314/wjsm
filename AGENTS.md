# Repository Guidelines

## Project Overview

`wjsm` is an AOT JavaScript/TypeScript runtime that compiles JS/TS to WebAssembly.
It does **not** use V8 — it uses `swc_core` for parsing, `wasm-encoder` for codegen, and `wasmtime` for execution.

## Workspace Structure

This is a Cargo workspace. The root `src/main.rs` is only a thin wrapper; all logic lives in `crates/`:

| Crate | Responsibility |
|---|---|
| `wjsm-parser` | `swc_core` → `swc_ast::Module` |
| `wjsm-semantic` | AST lowering → `wjsm_ir::Program` (scope tree, TDZ, var hoisting) |
| `wjsm-ir` | Intermediate representation (`Module`, `Function`, `BasicBlock`, `Instruction`) |
| `wjsm-backend-wasm` | IR → WASM bytes (`wasm-encoder`) |
| `wjsm-backend-jit` | Stub — not implemented yet |
| `wjsm-runtime` | Execute WASM bytes via `wasmtime` |
| `wjsm-cli` | CLI entry (`build`, `run` subcommands) |

Compilation pipeline: `parser → semantic → backend-wasm → runtime`.

## Build & Run

```bash
# Build entire workspace
cargo build

# Compile JS/TS to WASM
cargo run -- build test.ts -o out.wasm

# Compile and execute immediately
cargo run -- run test.ts
```

## Testing

```bash
# Run all tests across workspace
cargo test

# Run tests for a single crate
cargo test -p wjsm-semantic
```

- Unit tests live in `src/lib.rs` under `#[cfg(test)]`.
- Snapshot tests in `crates/wjsm-semantic/tests/lowering_snapshots.rs` compare lowered IR against `.ir` files in `fixtures/semantic/`.
- Happy-path JS fixtures are in `fixtures/happy/` with `.expected` files for expected stdout.
- Error fixtures are in `fixtures/errors/`.
- If you change lowering logic, update `fixtures/semantic/*.ir` manually — there is no auto-bless.

## Code Conventions

- Rust 2024 edition.
- Error handling: `anyhow::Result` for CLI/runtime, `thiserror` for crate-specific errors (e.g. `LoweringError`).
- Code comments are written in Chinese.
- IR variable names are scope-qualified: `${scope_id}.{name}` (e.g. `$0.x`).

## Architecture Notes

- **Value encoding**: NaN boxing in `crates/wjsm-ir/src/value.rs`. `i64` carries JS values: numbers as raw `f64` bits, strings via tagged pointer into WASM memory, `undefined` with a dedicated tag.
- **Scope tree**: `wjsm-semantic` implements lexical scoping with TDZ. `var` is hoisted to function scope and initialised with `undefined`; `let`/`const` enter TDZ until their initializer runs.
- **WASM contract**: The generated module imports `env.console_log(i64)`, exports `main()`, and exports `memory`.
- **JIT backend**: `wjsm-backend-jit` currently returns `bail!("JIT backend is not implemented yet")`.

## Commit Guidelines

- `feat:` 新功能
- `fix:` 修复
- `docs:` 文档更新
- `refactor:` 重构
- 保持简洁清晰
