# Normal 与 Eval 编译模式

`CompileMode` 只有 `Normal` 和 `Eval` 两个取值，却决定了产出模块是自带资源还是寄生在父实例上。这一章说明两者的差异边界。

## 差异总览

| 方面 | Normal | Eval |
| --- | --- | --- |
| 内存 | 声明自己的 `memory` | 从 `env` 导入父实例的三块内存 |
| Global | 定义 27 个 env global | 全部作为 mutable import 导入 |
| 函数表 | 自建 table | 导入父模块 `__table` |
| 入口导出 | `main`（Type 4） | `__eval_entry`（Type 3，接收 scope env） |
| helper | 从 `wjsm_support` 导入 | 内联 helper 实现 |
| 数据段 | 从 `data_base` 起 | 从父模块已用偏移之后起 |

## env global 列表

`compiler_core.rs` 的 `ENV_GLOBAL_EXPORT_NAMES` 是这 27 个 global 的唯一名单，与 support 模块的 `abi::ENV_GLOBALS` 对齐：

```text
__func_props           __heap_ptr             __obj_table_ptr
__obj_table_count      __shadow_sp            __object_heap_start
__num_ir_functions     __shadow_stack_end     __array_proto_handle
__object_proto_handle  __eval_var_map_ptr     __eval_var_map_count
__bootstrap_done       __function_props_done  __function_props_base
__arr_proto_table_base __arr_proto_table_len  __arr_proto_table_hash
__heap_limit           __alloc_ptr            __alloc_end
__gc_alloc_bytes       __gc_trigger_bytes     __gc_phase
__good_color           __barrier_buf_ptr      __barrier_buf_end
```

Normal 模式把它们作为 mutable `env` import 再 re-export；Eval 模式按同样的类型和 mutability 导入。**两侧的 mutability 必须完全一致**，否则 wasmtime 拒绝实例化，编译 eval 会退回解释器路径。

> <details><summary>为什么 mutability 必须完全一致？</summary>
>
> WASM 的 `import`/`global` 声明是「可变性」敏感的。如果父模块声明 `global $__heap_ptr: i32 = ...`（不可变），子模块 import 写 `global $__heap_ptr: i32`（可变），wasmtime 拒绝实例化——`import` 的 mutability 必须和实际定义匹配。
>
> 两侧都用 `mut`，意味着父模块的 global 一定能写。这看似限制——「为什么不能父模块定义一个 const 给我读？」——但 wjsm 内部确实要写这些 global（GC 改 `__alloc_ptr`、ZGC 改 `__good_color`），所以必须可写。
>
> 一致性是物理层面的硬约束，不只是「约定」。任何一侧改了 mutability 都会让 eval 在 wasmtime 上跑不起来。
>
> </details>

## 内存导入形状

Eval 模式导入的三块内存必须与父模块声明一致：

- `env.memory`：min 8 页，非共享，32 位。
- `env.__shadow_memory`：min 1 页，非共享，32 位，memory index 1。
- `env.__heap_memory`：memory64 + shared，min/max 取 `wjsm_ir::HEAP_MEMORY_MIN_PAGES` / `HEAP_MEMORY_MAX_PAGES`。

## 布局基址

`compile_eval_at_data_base(program, data_base, table_base)` 的两个基址让多次 eval 在同一实例内共存：新模块的字符串数据从 `data_base` 起写，函数表下标从 `table_base` 起分配。运行时依据上一次返回的 `data_len` / `table_len` 推进这两个值。

## helper 归属

Normal 模式下 10 个 helper（`obj_new`、`obj_get`、`obj_set`、`obj_delete`、`arr_new`、`elem_get`、`elem_set`、`string_eq`、`to_int32`、`get_proto_from_ctor`）从 `wjsm_support` 导入，换取 Wasmtime 编译量的下降。Eval 模式没有独立 support instance，仍走内联 helper 路径。这一取舍记录在 ADR 0004。

## 深入了解

- [support 模块提供哪些 helper 与 ABI 常量](support-module.md)
- [eval 编译产物在运行时如何实例化](../runtime-features/dynamic-code.md)
- [ADR 0004 记录的 build-time 固化决策](../reference/adr-index.md)
