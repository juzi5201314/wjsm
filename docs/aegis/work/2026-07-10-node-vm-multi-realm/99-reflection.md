# Reflection — node:vm multi-realm

## What worked

- 以 Node 实测为真理：`strings:false` 不拦 `runIn*` / `Script` / `compileFunction`，只拦 `eval` / `Function`；`microtaskMode` 默认不在 run 边界 drain。
- free-var 路径必须与 multi-realm 一致：`eval_scope_bridge` 优先于 `$0.$global`，否则 context 会读到主 realm 全局。
- eval 表达式 completion 必须 `lower_expr_then_continue`，否则 `EvalGetBinding` 异常分叉会把 `Return` 盖在错误 BB 上（`typeof free` 恒为 `undefined`）。

## Surprises

- 临时 compiled-eval Instance 上的嵌套函数不能跨 run 当 microtask 回调（func table 生命周期）。`compileFunction`/主 realm 回调不受此限。fixture 用主 `queueMicrotask` 验证 afterEvaluate 边界，而不是伪造成“context 内箭头写 free var”全绿。
- 主 global 拷贝构造器不一定带齐静态方法属性；`typeof Promise` 与 isolation 仍可验收。

## Follow-ups (out of plan)

- 持久化 eval 嵌套函数表项或改为 EvalFunction 解释路径，使 context 内 `queueMicrotask(() => { n = 1 })` 可写回 sandbox。
- 保证 `Promise.resolve` / `Object.keys` 等静态槽在主 global 与 context 拷贝后可用。
