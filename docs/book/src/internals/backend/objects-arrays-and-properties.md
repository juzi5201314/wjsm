# 对象、数组与属性操作

这一章说明对象分配、属性读写和数组索引在后端如何落地，以及 support helper 与内联路径的分工。

## 分配路径

对象和数组的分配走 support helper：

| 操作 | Helper | 落点 |
| --- | --- | --- |
| `new Object()` / `{}` | `obj_new` | support module |
| `new Array(n)` / `[]` | `arr_new` | support module |
| 属性读取 `o.x` / `o[k]` | `obj_get` / `elem_get` | support module |
| 属性写入 `o.x = v` / `o[k] = v` | `obj_set` / `elem_set` | support module |
| 属性删除 `delete o.x` | `obj_delete` | support module |

这些 helper 的 body 在 `support_module.rs` 里以真实 WASM 指令实现，不是 `unreachable` 占位。它们通过 `env` namespace import 9 个 host 函数（`gc_alloc_slow` 等）完成慢路径分配。

## Eval 模式的内联

Eval 模式没有 support instance，上述 10 个 helper 改为内联实现。这意味着 eval 产物体积更大，但不依赖外部 module 实例化顺序。

## 属性访问的语义拦截

语义层把已知形态的属性访问（如 `Math.max`、`arr.map`）编译成 `CallBuiltin`，跳过属性查找。后端只看到 `CallBuiltin`，不需要自己识别这些形态。这是为什么 `typeof "x".slice` 是 `undefined`——属性访问在 IR 层面就不存在，只有调用点才生成代码。

> <details><summary>support helper 是 WASM 函数，调用它有开销吗？</summary>
>
> 有，但不大。`call $obj_get` 是直接的函数调用，WASM 编译后是一条调用指令。helper 内部做的是「检查 handle 范围、查属性、走原型链」——这些都在 helper 内部，不在用户函数里。
>
> 关键收益：用户函数体保持简短。如果所有属性查找都内联进用户函数，每个属性访问会展开成 10+ 条指令，产物会膨胀。
>
> 代价是单次属性查找多一次跨函数调用。但 Cranelift 优化器可能会内联小函数——`obj_get` 在某些情况下会被自动内联到调用点，零开销。
>
> </details>

## 对象布局

对象的内存布局由 `wjsm-gc` 的 `HandleTableV2` 和堆分配器决定，后端不关心。后端只持有一个 `i64` 句柄值，通过 `TAG_OBJECT_HANDLE` 标签区分类型。GC 可以移动对象而不需要更新值里的句柄——句柄是 table index，不是指针。

## 深入了解

- [support helper 的 ABI 与 type index](support-module.md)
- [对象布局与分配的 GC 侧细节](../gc/object-layout-and-allocation.md)
- [语义层如何拦截内置方法调用](../frontend/expressions-and-statements.md)
