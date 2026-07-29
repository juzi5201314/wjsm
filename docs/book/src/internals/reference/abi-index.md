# WASM 与 Host ABI 索引

这一章汇总 user wasm 与 host 之间的 ABI 要素。

## 三块内存

| 内存 | 类型 | 用途 |
| --- | --- | --- |
| `memory` | shared memory64 | 主线性内存（栈、数据段） |
| `shadow_memory` | shared memory64 | 影子栈（GC spill） |
| `heap_memory` | shared memory64 | 对象堆 |

三块都是 shared memory64，min 524288 页（32 GiB），max 4294967296 页（256 TiB）。

## 约个 27 个 env global

| Global | 用途 |
| --- | --- |
| `__heap_ptr` | 堆指针 |
| `__obj_table_ptr` | 对象表指针 |
| `__obj_table_count` | 对象表条目数 |
| `__shadow_sp` | 影子栈指针 |
| `__shadow_stack_end` | 影子栈结束 |
| `__object_proto_handle` | Object.prototype 句柄 |
| `__array_proto_handle` | Array.prototype 句柄 |
| `__object_heap_start` | 对象堆起点 |
| `__bootstrap_done` | bootstrap 完成标记 |
| `__function_props_done` | 函数属性完成标记 |
| `__function_props_base` | 函数属性基址 |
| `__num_ir_functions` | IR 函数数 |
| `__arr_proto_table_base` | 数组原型表基址 |
| `__arr_proto_table_len` | 数组原型表长度 |
| `__arr_proto_table_hash` | 数组原型表哈希 |
| `__heap_limit` | 堆上限 |
| `__alloc_ptr` | 分配指针 |
| `__alloc_end` | 分配结束 |
| `__gc_alloc_bytes` | GC 分配字节计数 |
| `__gc_trigger_bytes` | GC 触发阈值 |
| `__gc_phase` | GC 阶段 |
| `__good_color` | ZGC 着色指针状态 |
| `__barrier_buf_ptr` | 屏障缓冲区指针 |
| `__barrier_buf_end` | 屏障缓冲区结束 |

## 约个 507 个 host import

`env.*` 函数，通过 `wasmtime::Linker` 注册。按域分组的规格文件在 `host_import_registry/specs_part*.rs`。

Type 12 是函数调用约定：`(i64, i64, i32, i32) -> i64`（receiver, arg, arg_count, flags → result）。

## 导出

| 导出 | 类型 | 用途 |
| --- | --- | --- |
| `memory` | shared memory64 | 主内存 |
| `shadow_memory` | shared memory64 | 影子栈 |
| `__table` | funcref table | 函数索引表 |
| 各 global | i32/i64 | env global |

## 深入了解

- [Import、Export 与主模块 ABI](../backend/imports-exports-and-abi.md)
- [对象、值与标签索引](layout-and-tags.md)
- [修改 WASM ABI](../development/changing-wasm-abi.md)
