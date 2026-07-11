# Evidence — node:vm multi-realm closeout

Date: 2026-07-11

## Scope closed in this slice

Plan residual Task 6.1/6.2 + deferred option semantics that checkpoint still listed:

1. `codeGeneration.strings: false` / `contextCodeGeneration` — **Node 语义**
2. `microtaskMode: "afterEvaluate"` — run 边界 drain 到稳态
3. `Function` 构造器实现 + 与 `eval` 共享 codegen 门控
4. context free-var 解析（sandbox 装主 global 内建 + eval_scope_bridge 优先）
5. eval 表达式 completion / typeof free-var IR 修正
6. 空捕获 eval 嵌套函数/箭头把 sandbox 作为闭包 env

## Commands / results

### Zero-warning build

```text
cargo build --workspace
Finished `dev` profile … (no warnings observed in tail)
```

### Focused fixtures (11/11)

```text
cargo nextest run -E 'test(happy__vm_) | test(errors__vm_) | test(modules__node_builtin_vm)'
Summary [0.235s] 11 tests run: 11 passed, 825 skipped
```

New fixtures:

- `fixtures/happy/vm_codegen_strings_false.{js,expected}`
- `fixtures/happy/vm_microtask_mode.{js,expected}`

### Workspace nextest

```text
cargo nextest run --workspace
Summary [86.828s] 1601 tests run: 1601 passed (4 slow), 2 skipped
```

### GC matrix (vm fixtures)

| GC | Result |
|---|---|
| mark-sweep (`WJSM_TEST_GC=mark-sweep`) | 11/11 passed |
| g1 | 11/11 passed |
| zgc | 11/11 passed |

### Unit / semantic

```text
cargo test -p wjsm-runtime --lib realm
4 passed

cargo nextest run -p wjsm-semantic -E 'test(eval_scope)'
1 passed (eval_scope_bridge_read_checks_exception)
```

## Behavioral contracts verified

| Contract | Evidence |
|---|---|
| `runInContext` / `Script` / `compileFunction` **不受** `strings:false` 拦截 | `vm_codegen_strings_false` |
| context 内 `eval` / `Function` 在 `strings:false` 抛 `EvalError` | 同上 + host `eval_direct`/`eval_indirect` gate + FunctionConstructor |
| `contextCodeGeneration` 别名 | 同上 |
| free `typeof Promise` / `typeof eval` 在 context 为 `function` | 同上（eval_scope_bridge + sandbox builtins） |
| default microtaskMode：run 结束不 drain | `vm_microtask_mode` `default_after_run 0` |
| `afterEvaluate`：run 边界 drain 含 nested | `afterEvaluate_after_run 1` / `nested 2` |
| 既有 compileFunction / timeout / isolation / Script | 原 9 fixtures 仍 PASS |
| GC 根 / reclaim | runtime `realm` unit 4 passed + workspace green |

## Known residual (non-blocking for plan acceptance)

1. **临时 eval Instance 上的嵌套函数**不能作为跨 `runInContext` 边界的 durable microtask 回调（table 绑定在临时 Instance）。`compileFunction`（主表 / EvalFunction 解释器路径）与主 realm 函数可跨边界。fixture 用主 `queueMicrotask` + sandbox 属性验证 drain 边界，与 Node 的 afterEvaluate 语义一致。
2. 从主 global **拷贝** 的构造器函数值，其 **静态方法表**（如 `Promise.resolve` / `Object.keys`）在部分启动路径上可能尚未挂到导出属性上；isolation / typeof / codegen / compileFunction 契约不依赖这些静态方法。若后续要严格 Node 静态方法可用性，应在 bootstrap 后保证主 global 构造器静态槽完整，或 per-realm 克隆静态表。
3. `#313` 是更大 roadmap（Network/Worker/…）；本 plan 只关闭 `node:vm multi-realm` 子范围，不 close issue。

## Files touched (this closeout)

- `crates/wjsm-runtime/src/realm.rs` — `MicrotaskMode`
- `crates/wjsm-runtime/src/runtime_node_vm.rs` — options, drain, sandbox builtins, Function/eval install
- `crates/wjsm-runtime/src/runtime_builtins.rs` — FunctionConstructor + strings gate on EvalIndirect
- `crates/wjsm-runtime/src/host_imports/reentrant_async/mod.rs` — strings gate on eval hosts
- `crates/wjsm-runtime/src/runtime_eval.rs` — object-env proto walk
- `crates/wjsm-runtime/src/runtime_host_helpers.rs` — `make_eval_error_exception`
- `crates/wjsm-semantic/...` — eval free-var bridge priority, typeof continue block, empty-capture env, eval_mode expr completion
- fixtures: `vm_codegen_strings_false`, `vm_microtask_mode`

## ADR

`docs/adr/0008-node-vm-multi-realm.md` 已存在；本切片未改架构边界（仍 single Store multi-realm + clone_pristine）。
