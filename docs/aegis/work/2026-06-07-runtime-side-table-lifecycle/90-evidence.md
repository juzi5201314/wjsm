# Evidence

| 命令 | 结果 |
|------|------|
| `cargo check -p wjsm-runtime` | 通过（既有 warnings） |
| `cargo nextest run -p wjsm-runtime` | 35 passed, 1 failed (`async_reentry_audit` 基线) |
| `cargo nextest run -E 'test(happy__promise_with_resolvers_second_resolve)'` | 待本回合验证 |
| `cargo nextest run --workspace -E 'not test(async_reentry_audit)'` | 待本回合验证 |

## 变更文件

- `crates/wjsm-runtime/src/lib.rs`, `runtime_promises.rs`, `runtime_builtins.rs`, `runtime_heap.rs`, `runtime_async_fn.rs`, `runtime_microtask.rs`, `runtime_combinators.rs`
- `host_imports/async_fn.rs`, `promise_combinators.rs`
- `fixtures/happy/promise_with_resolvers_second_resolve.js` + `.expected`