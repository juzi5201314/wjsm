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
| `afterEvaluate`：run 边界 drain 含 nested 与 function 表达式 | `afterEvaluate_after_run 1` / `nested 2` / `fn 3` |
| context 内嵌套箭头/函数 microtask 写 free-var | 同上（主 `__table` + `live_eval_instances`） |
| `Object.keys` / `Promise.resolve` 属性可获取且可调用 | `vm_builtin_statics` |
| 既有 compileFunction / timeout / isolation / Script | 原 fixtures 仍 PASS |
| GC 根 / reclaim | runtime `realm` unit 4 passed + workspace green |

## Residual closeout (2026-07-11 后续切片)

先前「非阻塞残留」已关闭：

1. **eval 嵌套函数 durable**：compiled-eval 导入父 `__table`，`compile_eval_at_data_base(..., table_base)` 用当前 `func_table.size` 编址；instantiate 后 `live_eval_instances` 保活 Instance。  
   验证：`vm_microtask_mode` 中 `queueMicrotask(() => { n = 1 })` / `function(){ n = 3 }` 写回 sandbox。
2. **Object/Promise 静态方法函数值**：`native_callable_get_property` 对 `ObjectConstructor`/`PromiseConstructor` 返回 `ObjectStatic`/`PromiseStatic`；eval_scope_bridge 下 `Object.keys(...)` 仍优先 Builtin。  
   验证：`vm_builtin_statics`。

### Residual closeout 验证

```text
cargo nextest run -E 'test(happy__vm_) | test(errors__vm_) | test(modules__node_builtin_vm)'
Summary … 12 tests run: 12 passed

cargo nextest run --workspace
Summary [69.707s] 1602 tests run: 1602 passed, 2 skipped

WJSM_TEST_GC={mark-sweep,g1,zgc} → 各 12/12 passed
```

### 仍 open（非本 plan 阻塞）

- `#313` 大 roadmap（Network/Worker/…）；本 plan 只关闭 `node:vm multi-realm` 子范围。
- `Promise.all` 等 combinator 的 **property-path** 仍为可调用桩；完整语义走 `Builtin::Promise*`。

## Files touched (this closeout + residual)

- runtime: `runtime_eval.rs`（table_base / live instances）、`runtime_linker.rs`（Object/Promise static props）、`runtime_builtins.rs`（static dispatch）、`types.rs`、`lib.rs`
- backend: `compiler_core.rs`（eval import `__table`）、`compiler_module/module_compile.rs`（shared table elements）、`lib.rs`（`compile_eval_at_data_base` 返回 `table_len`）
- semantic: `call_expr.rs`（eval_scope_bridge 下静态 Builtin）
- fixtures: `vm_microtask_mode`、`vm_builtin_statics`、`vm_codegen_strings_false`

## ADR

`docs/adr/0008-node-vm-multi-realm.md` 已存在；本切片未改架构边界（仍 single Store multi-realm + clone_pristine）。共享主 `__table` 是该边界内的实现细节收紧，不是新 owner。
