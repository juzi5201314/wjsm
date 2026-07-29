# 函数、闭包与类

这一章说明函数值如何在 IR 中表示，闭包捕获在哪一层决定，以及类体被降级成什么。

## Function 的元数据

`wjsm_ir::Function`（`crates/wjsm-ir/src/lib.rs`）除了名字、参数、基本块之外，携带若干由语义层填充、后端消费的字段：

| 字段 | 填充方 | 后端用途 |
| --- | --- | --- |
| `params` | 语义层 | WASM 调用约定的形参顺序 |
| `captured_names` | 语义层逃逸分析 | env 对象的属性名 |
| `has_eval` | 语义层 | 函数体含 direct eval 时降低局部变量优化强度 |
| `known_callee_vars` | 语义层 | callee no-GC 分析，key 是 scope-qualified IR 名 |
| `home_object` | 语义层 | `super` 属性访问的 `[[HomeObject]]` |
| `needs_prototype` | 语义层 | 是否创建 `prototype` 对象 |
| `source_span` | 语义层 | 编码进 custom section，供运行时错误映射 |

这些字段是单向的：语义层写入，后端只读。后端不回填，也不重新推导。

## 闭包捕获在语义层决定

捕获集合由语义层的逃逸分析算出，写进 `captured_names`。后端不做自己的捕获分析，直接按这个列表布置 env 对象的属性。

`dump-ir` 会把捕获列表打印在函数头部：

```bash
wjsm dump-ir -e 'function outer(){ let a = 1; return () => a }'
```

普通函数的形参里能看到 `$N.$env` 和 `$N.$this`：调用约定统一传入 env 与 this，即使函数不使用它们。

## 箭头函数

`lower_arrow_expr`（`lowerer_arrows.rs`）与普通函数的差别集中在 `this` 和 `super`：

- 箭头函数声明 `$this` 形参占位以满足 WASM 调用约定，但函数体内的 `this` 通过 env 捕获读取，不用形参值。
- `lexical_home_object`、`super_allowed`、`super_call_allowed` 从外层继承，因此箭头函数体内的 `super` 沿用外层方法的绑定。
- 名字形如 `arrow_<index>`，index 取当前已生成函数数量。

`async` 箭头走 `lower_async_arrow_expr`，转入 async 降级路径。

## 类

类降级位于 `lowerer_classes_ts/`（`mod.rs` 约 914 行，`class_body.rs` 约 716 行）。要点：

- 方法是独立的 IR 函数，`home_object` 指向 `prototype` 或构造器本体，`needs_prototype` 为 false。
- 构造器名形如 `<Class>.constructor`，可在 `dump-ir` 中直接看到。
- `super()` 只允许出现在派生类构造器中，否则报 `super() is only valid inside derived constructors`；`super.x` 在非方法上下文报 `super is only valid inside methods`。

TypeScript 构造器参数属性（`constructor(public a)`）目前不生成字段赋值，`class_body.rs` 在收集形参时对 `ParamOrTsParamProp::TsParamProp` 返回 `None`。用户侧影响记录在[限制章](../../user/runtime/limitations.md)。

## 深入了解

- [IR 中 Function 与 Module 的组织](../ir/program-module-function.md)
- [后端如何布置闭包与函数表](../backend/functions-closures-and-table.md)
