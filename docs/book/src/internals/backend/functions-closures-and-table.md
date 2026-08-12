# 闭包与函数表

这一章说明函数值和闭包在 native 后端如何表示。

## 函数表

IR 的 `functions: Vec<Function>` 在后端编译为一张函数指针表。每个 `FunctionId` 对应表中的一个条目，指向该函数的 native code 入口。

| 来源 | 函数表条目 |
| --- | --- |
| 语义层 lower 出的函数 | 对应 native code |
| direct_callable 优化的函数 | 入口不变，但调用点直接引用 |
| eval 编译的函数 | 独立 image，运行时挂载 |

`direct_call` pass 在 lowering 后把不可变函数声明绑定的 `LoadVar`/`GetProp` 替换为 `Const(FunctionRef)`，后端据此对调用点发射直接 `call`，省掉 callee 求值和类型分派。

## 闭包环境

闭包捕获的外层变量在语义层已由逃逸分析算出（`captured_names`）。后端为每个需要 env 的函数创建一个 env 对象，属性名按 `captured_names` 顺序布置。

调用约定统一传入 `$env` 和 `$this` 参数：

- 普通函数：`$env` 是闭包环境（可能为空），`$this` 是调用者传入的 this。
- 箭头函数：`$this` 形参占位但函数体内不读——它从 `$env.$this` 读取词法 this。
- 方法：`home_object` 指向 `prototype` 或构造器本体，`super` 属性访问依赖它。

## 函数属性

`Function` 上的元数据字段由语义层填充，后端只读：

| 字段 | 后端用途 |
| --- | --- |
| `captured_names` | 布置 env 对象的属性 |
| `has_eval` | 降低局部变量优化强度 |
| `known_callee_vars` | callee no-GC 分析 |
| `home_object` | `super` 属性访问 |
| `needs_prototype` | 决定是否创建 `prototype` 对象 |
| `source_span` | 编码进 debug 段，供运行时错误映射 |

## 深入了解

- [函数、闭包与类](../frontend/functions-closures-and-classes.md)
- [Function 的元数据](../ir/program-module-function.md)
- [活跃性、槽位与 GC Spill](liveness-slots-and-spills.md)
