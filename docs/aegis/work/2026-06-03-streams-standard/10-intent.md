# TaskIntentDraft

- 请求结果：继续执行 `docs/aegis/plans/2026-06-03-streams-standard.md`，从当前已做一半的工作树状态恢复并完成 WHATWG Streams Standard 实现。
- 范围：`wjsm-runtime` streams/fetch/native callable/accessor 相关实现、`wjsm-ir`/`wjsm-semantic`/`wjsm-backend-wasm` streams builtins、streams/fetch fixtures，以及必要的回归验证。
- 非目标：不使用 git worktree（用户明确要求）；不做与 Streams 无关的运行时重构；不引入新依赖；不削弱 ECMAScript/WHATWG Streams 语义；不用 stub/TODO/占位实现冒充完成。

## BaselineReadSetHint

- `docs/aegis/plans/2026-06-03-streams-standard.md`
- `docs/aegis/specs/2026-06-03-streams-standard-design.md`
- `AGENTS.md`（项目约束：Rust 2024、中文注释、ECMAScript/WHATWG spec 合规、禁止 PoC 妥协）
- `crates/wjsm-runtime/src/lib.rs`
- `crates/wjsm-runtime/src/runtime_heap.rs`
- `crates/wjsm-runtime/src/runtime_host_helpers.rs`
- `crates/wjsm-runtime/src/runtime_builtins.rs`
- `crates/wjsm-runtime/src/host_imports/streams_readable.rs`
- `crates/wjsm-runtime/src/host_imports/streams_writable.rs`
- `crates/wjsm-runtime/src/host_imports/streams_transform.rs`
- `crates/wjsm-runtime/src/host_imports/fetch.rs`
- `crates/wjsm-runtime/src/host_imports/fetch_core.rs`
- `crates/wjsm-runtime/src/host_imports/collections_buffers.rs`
- `crates/wjsm-ir/src/builtin.rs`
- `crates/wjsm-semantic/src/builtins.rs`
- `crates/wjsm-backend-wasm/src/lib.rs`
- `crates/wjsm-backend-wasm/src/host_import_registry.rs`

## Known Facts

- 用户明确说明本次没有使用工作树；当前工作在 `master`，且已有未提交 streams 相关改动。
- `cargo nextest run -E 'test(streams__)'` 当前无法编译：`streams_readable.rs:693` 使用 `args`，函数参数名仍为 `_args`。
- 当前代码已出现 accessor helper、Map/Set size getter、Readable/Writable/Transform 初步实现和部分 streams fixtures，但完成度必须以测试和两阶段审查为准。

## Stop Condition

- 计划中所有适用任务完成，或明确记录哪些任务因已有实现被验证为完成。
- 每个执行切片都有：实现者结果、spec compliance review、code quality review、控制器验证证据。
- `cargo nextest run -E 'test(streams__)'`、`cargo nextest run -E 'test(fetch__)'`、`cargo nextest run -E 'test(happy__)'` 和最终 workspace 回归达到可交付状态，或阻塞点有完整证据。
- 无 open Critical/Important review issue。
