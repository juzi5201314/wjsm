# Reflection — node:vm multi-realm

## What worked

- 以 Node 实测为真理：`strings:false` 不拦 `runIn*` / `Script` / `compileFunction`，只拦 `eval` / `Function`；`microtaskMode` 默认不在 run 边界 drain。
- free-var 路径必须与 multi-realm 一致：`eval_scope_bridge` 优先于 `$0.$global`，否则 context 会读到主 realm 全局。
- eval 表达式 completion 必须 `lower_expr_then_continue`，否则 `EvalGetBinding` 异常分叉会把 `Return` 盖在错误 BB 上（`typeof free` 恒为 `undefined`）。
- **eval 嵌套函数 durable**：compiled-eval 导入父 `__table` + 正确 `table_base` + `live_eval_instances` 保活 Instance，使 `queueMicrotask(() => { n = 1 })` 可跨 run 边界写回 sandbox。
- **Object/Promise 静态方法函数值**：`native_callable_get_property` 返回 `ObjectStatic` / `PromiseStatic` NativeCallable；eval_scope_bridge 下 `Object.keys` 仍走 Builtin 静态路径。

## Surprises

- wasmtime 对 **import 的 table** 仍会在 instantiate 时应用 active element section——不必手写 `table.set`，但必须 grow 到 `table_base + table_len` 且 Instance 不可丢。
- 仅拷贝主 global 构造器不够：`Object.keys` / `Promise.resolve` 作为 **属性** 需要 `native_callable_get_property` 合成可调用值。

## Follow-ups (out of plan)

- Promise combinator 静态方法（`all`/`race`/…）在 property 路径仍是可调用桩；完整语义继续走 `Builtin::Promise*` 编译路径。
- `Object.assign` / `fromEntries` 等属性路径可继续加深，主路径已有 Builtin。
