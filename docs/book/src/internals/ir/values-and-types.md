# Value、变量与类型信息

这一章讲 IR 里的值怎么标识、变量名怎么编码，以及后端能从 IR 拿到哪些类型线索。

## ValueId 是 SSA 结果编号

`ValueId(u32)` 标识一条指令产出的结果，在函数内唯一，展示为 `%0`、`%1`。IR 是 SSA 形式：一个 `ValueId` 只被定义一次，控制流合并处用 `Phi` 汇聚。

`Module` / `Function` / `BasicBlock` 的 ID 各有独立命名空间与展示形式：

| 类型 | 展示 | 作用域 |
| --- | --- | --- |
| `ValueId` | `%3` | 函数内 |
| `ConstantId` | `c7` | 模块内常量池 |
| `BasicBlockId` | `bb2` | 函数内 |
| `FunctionId` | 裸数字（`@` 前缀由 caller 加） | 模块内 |
| `ModuleId` | `mod1` | Bundle 内 |

## 变量名带作用域前缀

`ValueId` 只表示临时结果。跨语句存活的绑定走 `StoreVar` / `LoadVar`，用字符串名寻址，格式是 `$<scope_id>.<name>`：

```text
store var $0.x, %21
%3 = load var $1.inner
```

`$0` 是模块顶层作用域，嵌套块和函数各有自己的编号。这个编码让同名变量在不同作用域下天然区分，后端不需要重跑作用域分析。

几个固定名不带作用域号：`$this`、`$env`、`$module_main`（模块入口函数名，常量 `MODULE_ENTRY_IR_NAME`）。它们对应 Native ABI 入口的 env / this，以及 bundle 入口函数。

> <details><summary>为什么用字符串名而不是单独的 VarId？</summary>
>
> 可选设计：
>
> 1. **每个变量一个 `VarId`**：`store var %v3, %21` 形式。后端要查 `VarId → 名字` 映射才能读 IR dump。
> 2. **名字带作用域前缀**（当前选择）：`store var $0.x, %21`，自带上下文。
>
> 选 2 的理由：
>
> - IR dump 可读性：人眼能直接读出 `$0.x` 是「作用域 0 的 x」。
> - 快照测试友好：IR 快照里能看到变量名，不需要查表。
> - 后端无歧义：同名变量在不同作用域下字符串不同，不会混淆。
>
> 代价：每次访问变量要做字符串比较而不是整数比较。IR 处理阶段（lowering、codegen）本来就在做大量字符串操作，这个开销不显著。
>
> </details>

## 运算符枚举

IR 的运算符是有限枚举，后端对每个变体给出确定的指令序列：

- `BinaryOp`：`Add`、`Sub`、`Mul`、`Div`、`Mod`、`Exp`，以及位运算 `BitAnd`、`BitOr`、`BitXor`、`Shl`、`Shr`、`UShr`。
- `UnaryOp`：`Not`、`Neg`、`Pos`、`BitNot`、`Void`、`IsNullish`、`Delete`。
- `CompareOp`：`StrictEq` / `StrictNotEq`，以及证明两侧为 Number 之后的关系比较 `Lt` / `Gt` / `LtEq` / `GtEq`（NaN → false）。未证明的关系比较仍走 `CallBuiltin(AbstractCompare)`。

`IsNullish` 是为 `??` 和 `?.` 服务的合成运算符，没有对应的 JS 语法运算符。

## 函数级类型与优化线索

`Function` 上有几个字段供后端做决策，全部由语义层填充：

- `captured_names`：闭包捕获的外层变量名，后端据此布局 `$env` 对象属性。
- `known_callee_vars`：`HashMap<String, FunctionId>`，把 `LoadVar` 读到的已知函数声明变量映射到具体函数。后端用它做 callee 的 no-GC 分析；映射为空表示保守当作 may-GC。
- `has_eval`：函数体含 direct eval，后端降低局部变量优化强度。
- `needs_prototype`：普通函数为 `true`，箭头函数、方法、类构造器为 `false`。
- `home_object`：方法的 `[[HomeObject]]`，`super` 属性访问依赖它。

IR 指令本身不标注每个 SSA 值的 JS 类型。`wjsm-ir` 的值类 fixpoint（`value_class`）在 lowering 之后证明 Number / Int32 / 可抛性，供 `typed_cfg` 折叠虚假 `is_exception` 并把 Number 关系比较改写成 `Compare`。后端 `f64_analysis` 消费这些证明做 unbox / `fadd` / `fcmp`；overlay 用反馈种子在克隆 IR 上跑同一套分析。

## 深入了解

- [NaN-boxed 值在 native 侧的位布局](../backend/value-representation.md)
- [后端如何做值类型推断与槽位分配](../backend/liveness-slots-and-spills.md)
- [闭包捕获在语义层的判定](../frontend/functions-closures-and-classes.md)
