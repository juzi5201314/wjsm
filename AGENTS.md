# wjsm

AOT JavaScript/TypeScript runtime: SWC AST → verified semantic IR → portable `.wjsm` → direct Cranelift native image → `NativeRuntime`. No V8, Wasm, Wasmtime, or execution-backend fallback.

## Non-negotiable rules

- Rust 2024, default rustfmt, Chinese source comments, and zero compiler warnings.
- ECMAScript is the semantic source of truth. Do not ship partial semantics, skipped edge cases, or invalid early-error behavior.
- Keep backend boundaries from ADR 0014: Cranelift/object/platform dependencies belong only in `wjsm-backend-native` and `wjsm-host-native`; `wjsm-builtins`, `wjsm-host`, `wjsm-gc`, and `wjsm-module` stay backend-independent.
- Tests verify correctness only: deterministic, fast, in-process; never heavy-load/stress tasks, never randomness-dependent assertions, never real network I/O (TCP/UDP/HTTP/TLS/DNS), never real child-process/PTY spawning in the automated suite.
- Put generated artifacts in `/tmp`. For ad-hoc JS/TS, use `-e`; do not create scratch source files.
- Fix the owning layer and remove obsolete paths. Do not hide failures by weakening fixtures or snapshots.

## Tests

- 测试只验证正确性：确定性、快速、进程内。禁止在测试里放重负载/压力任务（GC churn、大循环、大量分配）、依赖随机结果的断言、真实网络 I/O（TCP/UDP/HTTP/TLS/DNS）或真实子进程/PTY 启动。
- 重负载、随机性质、真实网络/进程行为属于 `crates/*-bench`、`fuzz/`、`bench/` 或手工验证命令；确需保留的慢用例只能放 `slow`/`full` profile，默认 `cargo nextest run --workspace` 必须全为快速正确性测试。
- 需要验证网络/进程协议时，在宿主层提供确定性的测试替身/transport 钩子（如 `WJSM_TEST_*` 环境开关），测试只断言协议状态机，不做真实 I/O。
- 新增测试如果冷启动就超过默认 profile 的 30s 硬门禁，说明测试本身过重：拆小、替换测试替身、或移出手工验证，而不是加 nextest 黑名单。


## Commands

```bash
cargo build
cargo run -- run -e 'console.log(1 + 2)'
cargo run -- build -e 'console.log(1)' -o /tmp/out.wjsm
cargo run -- check -e 'const x = 1'
cargo run -- dump-ir -e 'const x = 1'
cargo run -- dump-clif -e 'const x = 1'
cargo nextest run --workspace
cargo nextest run -E 'test(happy__hello)'
cargo nextest run -p wjsm-semantic
WJSM_UPDATE_FIXTURES=1 cargo nextest run -E 'test(happy__<name>)'
WJSM_UPDATE_SNAPSHOTS=1 cargo nextest run -p wjsm-semantic -- lowering_snapshots
```

## Workflow

- Diagnose the failing stage first: parse → lower → module graph → codegen → host/runtime.
- Use `dump-ast`, `dump-ir`, `dump-clif`, and `disasm` to compare adjacent stages; do not add temporary production logging unless those paths cannot expose the failure.
- Lowering changes require semantic IR snapshots. Observable behavior requires `fixtures/happy` or `fixtures/errors` plus `.expected`. Module behavior belongs in `fixtures/modules`.
- Review generated fixture/snapshot changes before accepting them. Run the narrow test first and the workspace suite for cross-crate changes.
- Use exact spec text for language questions; after the local code and spec, inspect real-engine source before asking the user to decide semantics.
- Keep files responsibility-focused (target ≤500 lines) and functions cohesive (target ≤30 lines); split by semantic/backend/host domain rather than adding parallel conventions.

## Source of truth

- User-facing behavior and CLI: [README.md](README.md) and `wjsm --help`.
- Architecture boundaries and invariants: [docs/adr/](docs/adr/), especially ADRs 0010 and 0014.
- Direct native backend contract: [docs/backend-implementation-guide.md](docs/backend-implementation-guide.md).
- Fixture and test mechanics: `build.rs`, `tests/`, `fixtures/`, and `.config/nextest.toml`.
