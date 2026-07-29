# Support 模块与辅助函数

support module 是构建期产出的第二个 WASM 模块，提供高频 helper。这一章说明它提供什么、怎么生成、何时使用。

## 提供的 helper

`crates/wjsm-backend-wasm/src/support_module.rs` 产出 10 个真实 body 的 helper：

| Helper | Type | 作用 |
| --- | --- | --- |
| `obj_new` | 7 | 分配对象 |
| `obj_get` | 8 | 读取属性 |
| `obj_set` | 9 | 写入属性 |
| `obj_delete` | 8 | 删除属性 |
| `arr_new` | 7 | 分配数组 |
| `elem_get` | 8 | 读取索引元素 |
| `elem_set` | 9 | 写入索引元素 |
| `string_eq` | 26 | 字符串比较 |
| `to_int32` | 10 | JS 值转 i32 |
| `get_proto_from_ctor` | 3 | 构造器取原型 |

另有两个占位函数 `wjsm_bootstrap_once` / `wjsm_init_function_props`，body 是 `unreachable`，待 P2.6 迁移。它们的 type index 已预留，迁移时只替换 body 不改索引。

## 构建流程

`wjsm-host-wasm/build.rs` 在 `CARGO_FEATURE_EMBEDDED` 时调用 `emit_support_module`，按三种 `GcFlavor`（`MarkSweep` / `G1` / `Zgc`）分别产出 `.cwasm`，用 `wasmtime::Engine::precompile_module` 预编译为 `wjsm_support_<flavor>.cwasm`，再 `include_bytes!` 嵌入二进制。

运行时通过 `runtime_support::EMBEDDED_*_SUPPORT_CWASM` 静态量访问，`install_embedded_support_cwasm` 把字节装入 `wasmtime::Module`，实例化时与 user wasm 共享 linker。

## ABI 对齐

support module 的 global 索引与 user wasm 完全对齐（0..26），type section 用 `build_shared_type_section` 生成同一份。这两条对齐让 helper body 移植时无需修改 GlobalGet/GlobalSet 索引和 call_indirect type index。

support module 通过 `env` namespace import 9 个 host 函数（`gc_safepoint_poll`、`gc_take_freed_handle`、`gc_alloc_slow`、对象堆 host helpers 等），其余逻辑内联在 support body 里。

## Normal vs Eval

Normal 模式从 `wjsm_support` 导入这 10 个 helper，换取 Wasmtime 编译量的下降。Eval 模式没有独立 support instance，走内联 helper 路径——eval 产物自包含 helper body，不 import support module。这一取舍记录在 ADR 0004。

## 深入了解

- [构建期嵌入工件的生成流程](../startup/embedded-artifacts.md)
- [预编译 Support cwasm 的装载与缓存](../startup/support-cwasm.md)
- [Normal 与 Eval 模式的差异边界](normal-and-eval-modes.md)
