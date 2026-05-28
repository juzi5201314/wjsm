# TaskIntentDraft

- 请求结果：完成 `docs/aegis/plans/2026-05-27-typedarray-methods-completion.md` 的全部任务，修复现有未通过测试，达到可验证通过状态。
- 范围：`wjsm-runtime`、`wjsm-backend-wasm`、`wjsm-semantic`、`wjsm-ir` 及相关测试。
- 非目标：不扩展到 `TypedArray.from/of`，不改动与本计划无关的内置行为。

## BaselineReadSetHint

- `docs/aegis/plans/2026-05-27-typedarray-methods-completion.md`
- `crates/wjsm-runtime/src/lib.rs`
- `crates/wjsm-runtime/src/host_imports/typedarray_new_methods.rs`
- `crates/wjsm-runtime/src/host_imports/collections_buffers.rs`
- `crates/wjsm-runtime/src/runtime_render.rs`
- `crates/wjsm-backend-wasm/src/compiler_core.rs`
- `crates/wjsm-backend-wasm/src/lib.rs`
- `crates/wjsm-backend-wasm/src/compiler_builtins.rs`
- `crates/wjsm-semantic/src/builtins.rs`
- `crates/wjsm-semantic/src/lib.rs`
- `crates/wjsm-semantic/src/lowerer_async_eval.rs`
- `crates/wjsm-ir/src/builtin.rs`

## ImpactStatementDraft

- 兼容边界：不能破坏现有 9 个 TypedArray 构造器、已有 6 个方法、现有 host import 顺序。
- 高风险点：`HOST_IMPORT_NAMES` 索引错位、BigInt 64-bit 读写符号/端序错误、runtime render 与 typedarray 内容不一致。
- 验证策略：分 crate `cargo check` + 新增/更新 fixture 测试 + 全量 `cargo build` + 目标 test262 子集。
