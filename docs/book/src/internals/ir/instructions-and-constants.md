# Instruction 与 Constant

这一章说明 IR 指令集的组织原则和常量池的内容。

## 指令集的粒度选择

`Instruction` 是个大枚举（`crates/wjsm-ir/src/lib.rs`），设计取向是**贴 JS 语义，不贴机器**。例如属性访问是一条 `GetProp`/`SetProp` 而不是「算哈希 + 查槽 + 走原型链」的指令序列；`CallBuiltin` 直接携带 `Builtin` 枚举而不是函数地址。

这么切的理由：IR 的消费者是多个后端（见 [多后端边界](../backend/multi-backend-boundary.md)）。属性查找的具体策略属于后端与堆布局的决定，把它固化进 IR 会强迫每个新后端复刻同一套实现细节。

代表性变体：

| 变体 | 用途 |
| --- | --- |
| `Const { dest, constant }` | 从常量池取值 |
| `Binary` / `Unary` / `Compare` | 运算，op 由 `BinaryOp`/`UnaryOp`/`CompareOp` 指定 |
| `Phi { dest, sources }` | 控制流汇合 |
| `CallBuiltin { dest, builtin, args }` | 调用宿主 builtin |
| `StringConcatVa { dest, parts }` | 变长字符串拼接，避免 N-1 条二元 concat |

`CallBuiltin` 的 `dest` 是 `Option<ValueId>`——有些 builtin 只有副作用，不产生值，用 `None` 表达比强行分配一个被丢弃的 `ValueId` 干净。

## 运算符枚举

`BinaryOp`：`Add`、`Sub`、`Mul`、`Div`、`Mod`、`Exp`，位运算 `BitAnd`、`BitOr`、`BitXor`、`Shl`、`Shr`、`UShr`。

`UnaryOp`：`Not`、`Neg`、`Pos`、`BitNot`、`Void`、`IsNullish`、`Delete`。

`CompareOp` 只有 `StrictEq` 和 `StrictNotEq`。**抽象相等不在这里**——`==` 需要完整的 ES 强制转换算法，lowering 成 `CallBuiltin` 走宿主实现。同理，`<`/`>` 等关系比较也走 builtin（`abstract_compare`）。IR 只保留能直接映射到机器指令的严格比较。

> <details><summary>为什么 `==` 不在 IR 里？</summary>
>
> ES 规范里 `==` 的语义是「抽象相等比较」：先把两边做 ToPrimitive 类型转换，再比较。比如 `1 == "1"` 为 `true`，因为 `"1"` ToPrimitive 后变成 number `1`。
>
> 这种语义没办法直接映射到机器指令——`i64.eq` 是位比较，不会做类型转换。强行让后端实现完整的 ToPrimitive 算法会把这部分语义固化到 IR。
>
> 当前的做法：抽象相等 lowering 成 `CallBuiltin(abstract_compare)`，由 builtins 层（Rust 实现）做完整的算法。这种 builtin 调用有「函数调用 + 内存读」开销，但把语义从 IR 抽离出来——后端只需要「调用某个 builtin」，不用理解 `==` 的全部规则。
>
> 性能上：纯 `===` 路径仍是 `i64.eq`（机器指令），开销极低。`==` 路径走 builtin 调用，比 `===` 慢几倍。这是「正确性换性能」的取舍，符合 ES 规范的真实成本。
>
> </details>

## Constant

```rust
pub enum Constant {
    Number(f64), String(String), Bool(bool), Null, Undefined,
    FunctionRef(FunctionId),
    NativeCallableEval,
    BigInt(String),
    RegExp { pattern: String, flags: String },
    ModuleId(ModuleId),
}
```

几个值得说明的变体：

- `BigInt(String)` 存十进制字符串而非数值。IR 零依赖，不引入 bignum 库；解析推给宿主。
- `RegExp` 存 pattern 与 flags 原文，正则编译在运行时由 `regress` 完成。
- `NativeCallableEval` 用于 `eval` 被当作值读取（而非直接调用）的场合。
- `ModuleId(ModuleId)` 是 AOT 期解析出的模块 id，供动态 `import()` 使用。

`Display` 实现给出稳定文本形式（`number(1)`、`string("x")`、`functionref(@0)`），这是 IR dump 的基础。

## 深入了解

- [dump 文本格式为什么必须稳定](identifiers-and-display.md)
- [Builtin 枚举与宿主实现的对应关系](../host-runtime/host-imports.md)
- [语义层在什么条件下发射 CallBuiltin](../frontend/expressions-and-statements.md)
