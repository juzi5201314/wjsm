# 函数、闭包与类

这一章说明函数值如何在 IR 中表示，闭包捕获在哪一层决定，以及类体被降级成什么。

## Function 的元数据

`wjsm_ir::Function`（`crates/wjsm-ir/src/lib.rs`）除了名字、参数、基本块之外，携带若干由语义层填充、后端消费的字段：

| 字段 | 填充方 | 后端用途 |
| --- | --- | --- |
| `params` | 语义层 | Native ABI 形参顺序（含 `$env`、`$this`） |
| `captured_names` | 语义层逃逸分析 | env 对象的属性名 |
| `has_eval` | 语义层 | 函数体含 direct eval 时降低局部变量优化强度 |
| `known_callee_vars` | 语义层 | callee no-GC 分析，key 是 scope-qualified IR 名 |
| `home_object` | 语义层 | `super` 属性访问的 `[[HomeObject]]` |
| `needs_prototype` | 语义层 | 是否创建 `prototype` 对象 |
| `source_span` | 语义层 | 写入 `NativeSourceFrame`，供运行时错误映射 |

这些字段是单向的：语义层写入，后端只读。后端不回填，也不重新推导。

## Native 调用约定

生成代码入口是 `NativeSlowEntry`：`(ctx, env, this_value, args_base, args_count) -> i64`。实参落在 vmctx 的 call arena 上，用 `CallArgs { base, len }` 描述一段连续槽。IR 里普通函数的形参始终带 `$N.$env` 和 `$N.$this`：约定统一传入 env 与 this，即使函数体不用它们。

## 闭包捕获在语义层决定

捕获集合由语义层的逃逸分析算出，写进 `captured_names`。后端不做自己的捕获分析，直接按这个列表布置 env 对象的属性。

`dump-ir` 会把捕获列表打印在函数头部：

```bash
wjsm dump-ir -e 'function outer(){ let a = 1; return () => a }'
```

## 箭头函数

`lower_arrow_expr`（`lowerer_arrows.rs`）与普通函数的差别集中在 `this` 和 `super`：

- 箭头函数仍声明 `$this` 形参以符合 Native ABI，但函数体内的 `this` 通过 env 捕获读取，不用形参值。
- `lexical_home_object`、`super_allowed`、`super_call_allowed` 从外层继承，因此箭头函数体内的 `super` 沿用外层方法的绑定。
- 名字形如 `arrow_<index>`，index 取当前已生成函数数量。

`async` 箭头走 `lower_async_arrow_expr`，转入 async 降级路径。

> <details><summary>箭头函数的 this 为什么是「词法 this」？</summary>
>
> 这是 ECMAScript 规范明确的行为：箭头函数不绑定自己的 `this`，而是捕获外层词法 this。如果外层是函数方法，this 就是调用者；如果是普通函数或模块，this 是 `undefined`（严格模式）。
>
> Native ABI 要求每个函数入口都接收 env 和 this。箭头函数仍然声明 `$this` 形参，但函数体内不读这个形参——它通过 `env.$this` 读取捕获的词法 this。
>
> 每次调用箭头函数都要走 env 读 this，比普通函数的「直接用 this 形参」多一次读。词法 this 是规范要求，不能改成动态 this。
>
> </details>

## 类

类降级位于 `lowerer_classes_ts/`。要点：

- 方法是独立的 IR 函数，`home_object` 指向 `prototype` 或构造器本体，`needs_prototype` 为 false。
- 构造器名形如 `<Class>.constructor`，可在 `dump-ir` 中直接看到。
- `super()` 只允许出现在派生类构造器中，否则报 `super() is only valid inside derived constructors`；`super.x` 在非方法上下文报 `super is only valid inside methods`。

TypeScript 构造器参数属性（`constructor(public a)`）会归一成普通形参参与形参处理，并记录字段名后在 `super()` 之后、实例字段初始化器之前发射 `this.<name> = <name>`。实现位于 `class_body.rs` 的形参收集逻辑与 `decl_misc.rs` 的 `emit_param_prop_fields`。

## 深入了解

- [IR 中 Function 与 Module 的组织](../ir/program-module-function.md)
- [后端如何布置闭包与函数表](../backend/functions-closures-and-table.md)
