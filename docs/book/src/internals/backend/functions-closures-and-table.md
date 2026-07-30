# 函数、闭包与函数表

这一章说明 IR 函数如何映射到 WASM 函数索引、函数表如何承载间接调用、闭包的 env 对象如何布局。

## 函数索引三张表

`Compiler` 持有三张映射表，在 Pass 1（登记阶段）全部建立：

| 表 | key → value | 用途 |
| --- | --- | --- |
| `function_id_to_wasm_idx` | IR `FunctionId` → WASM function index | 直接 `Call` |
| `function_name_to_wasm_idx` | 函数名 → WASM function index | `call_indirect` 反查 |
| `function_table` / `function_table_reverse` | WASM function index ↔ table slot | `call_indirect` |

`FunctionRef` 常量在 `encode_function_ref_id` 中经 `function_id_to_wasm_idx` → `function_table_reverse` 两步转换为 table slot index，编码进 NaN-box 的 `TAG_FUNCTION` 值。

## 函数表

WASM `table` 是 `funcref` 类型，初始大小在 Pass 1 按函数数量确定。`element` 段把每个函数填入对应 slot。`call_indirect` 按 slot 索引调用，type index 固定为 12（`JS_FUNC_TYPE_INDEX`）。

用户函数统一用 Type 12：`(i64, i64, i32, i32) -> i64`，四个参数是 env 对象、this 值、影子栈参数基址、参数个数。这让任意元数的 JS 函数共用一个 type index。

> <details><summary>为什么所有用户函数用同一个 type index？</summary>
>
> WASM 的 `call_indirect` 在调用时检查 table 中函数的 type 是否匹配指定 type index。如果每个函数有不同 type，table 里就要存「type 不同的函数」，调用时再校验——开销大且复杂。
>
> 用同一个 type index 12 让所有 JS 函数「形状一致」：四个参数，返回 i64。具体的参数语义由函数体自行从影子栈读，不通过 WASM 形参表达。
>
> 副作用：函数调用是固定开销的（4 个参数 + 影子栈读参数），不能像 C 那样「参数少就快」。但这种开销是常数的，不随调用规模增长。
>
> </details>

## 闭包 env 布局

语义层的逃逸分析算出 `captured_names`，后端据此布局 env 对象：

- env 是普通对象（`TAG_OBJECT_HANDLE`），属性名按 `captured_names` 顺序。
- 函数被创建时 `closure_create` builtin 接收函数 table slot 和 env 对象，返回 `TAG_FUNCTION` 值。
- 函数体通过 `$env` 形参访问 env，`closure_get_func` / `closure_get_env` 用于解构。

箭头函数的 `$this` 形参占位但不用——this 通过 env 捕获读取。

## `needs_prototype` 的作用

普通函数 `needs_prototype = true`，创建 `prototype` 对象并写入函数表。箭头函数、方法、类构造器为 `false`，不创建 prototype。后端在登记阶段读这个字段决定是否生成 prototype 初始化代码。

## 深入了解

- [语义层如何决定捕获集合与 needs_prototype](../frontend/functions-closures-and-classes.md)
- [NaN-boxed 值表示中的 TAG_FUNCTION](value-representation.md)
- [Type 12 调用约定与影子栈](liveness-slots-and-spills.md)
