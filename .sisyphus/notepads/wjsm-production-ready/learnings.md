# wjsm-production-ready Learnings

## Project Conventions

### Current Architecture (335 lines)
- Single crate structure with `src/main.rs`, `src/compiler/`, `src/runtime.rs`
- SWC parser for JS/TS → AST
- Direct AST → WASM codegen using wasm-encoder
- wasmtime runtime with `console.log` host function
- NaN boxing for value encoding (numbers, strings)

### Code Style
- Rust 2024 edition
- Error handling: `anyhow::Result` for app, `thiserror` for libs
- Chinese comments preferred

### Key Files
- `src/main.rs`: CLI with `build` and `run` commands
- `src/compiler/codegen.rs`: AST → WASM compiler (203 lines)
- `src/compiler/value.rs`: NaN boxing value encoding (35 lines)
- `src/runtime.rs`: wasmtime execution (43 lines)

## Design Decisions

### Phase 1: Infrastructure
- Must use `cargo nextest` instead of `cargo test`
- Must establish workspace crates structure before IR design
- IR is the foundation for both AOT (WASM) and JIT backends

## References

### Existing Implementation Patterns
- Compiler: Single-pass AST → WASM using wasm-encoder
- Value encoding: NaN boxing with 64-bit tagged values
- Runtime: wasmtime with host function imports

## Phase 1 Task 1.1

- `cargo nextest` 已安装并可通过 `cargo nextest run --color=never` 作为标准测试入口执行。
- `tests/unit.rs` 需要作为 Cargo 自动发现的测试入口，子模块可放在 `tests/unit/mod.rs` 中复用目录结构。
- `nextest` 仓库级配置负责并发与报告行为；默认无彩色输出通过仓库级 `CARGO_TERM_COLOR=never` 提供。

## Phase 1 Task 1.2

- fixture 测试入口可沿用 `tests/*.rs` 作为根测试 crate，再通过 `#[path = "integration/mod.rs"]` 组织 `tests/integration/` 子模块，这样 nextest 过滤名会保持 `integration::fixtures::*`。
- 对当前单 crate 二进制项目，fixture runner 直接调用 `CARGO_BIN_EXE_wjsm`（或 `target/debug/wjsm` 回退路径）比在集成测试中复用内部模块更稳妥，不需要引入 `lib.rs`。
- snapshot 文件采用 `*.expected`，首轮缺失时自动生成；后续通过 `WJSM_UPDATE_FIXTURES=1` 更新，可覆盖 happy/error fixture 的 stdout、stderr 与 exit code。
