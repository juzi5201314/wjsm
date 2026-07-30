# Import、Export 与主模块 ABI

这一章说明用户 WASM 模块的 import 与 export 面，以及它与宿主、support 模块如何对齐。这是「产物不能独立运行」的根本原因。

## 共享 type section

`crates/wjsm-backend-wasm/src/shared_types.rs` 定义了 14 个 type index，user wasm 与 support module 共用同一份 type section。wasmtime 的 `call_indirect` 要求调用方与 table 中函数的 type index 一致（不只是签名一致），因此两侧必须对齐。

| Index | 签名 | 用途 |
| --- | --- | --- |
| 0 | `(i64) -> ()` | console_log |
| 1 | `() -> ()` | main / gc_safepoint_poll / gc_barrier_flush |
| 2 | `(i64, i64) -> i64` | f64_mod, f64_pow |
| 3 | `(i64) -> i64` | get_proto_from_ctor |
| 4 | `() -> i64` | main 返回 / bootstrap |
| 6 | `(i64, i32, i32) -> i64` | JS function signature (shadow stack) |
| 7 | `(i32) -> i32` | obj_new, arr_new |
| 8 | `(i64, i32) -> i64` | obj_get, obj_delete, elem_get |
| 9 | `(i64, i32, i64) -> ()` | obj_set, elem_set |
| 10 | `(i64) -> i32` | to_int32 |
| 11 | `(i64, i64) -> i64` | string_concat |
| 12 | `(i64, i64, i32, i32) -> i64` | JS 函数签名 / native_call / call_indirect |
| 13 | `(i32, i64) -> i64` | closure_create |
| 26 | `(i32, i32) -> i32` | string_eq |

`JS_FUNC_TYPE_INDEX = 12` 是 `dump-wat` 里大量出现的 `type $#type12` 的来源。

## 三块内存

Normal 模式声明的三块内存：

| Import 名 | 类型 | 用途 |
| --- | --- | --- |
| `env.memory` | memory32, min 8 | 主线性内存（局部变量、影子栈共用） |
| `env.__shadow_memory` | memory32, min 1 | 影子栈独立段 |
| `env.__heap_memory` | memory64, shared, min 524288 max 4294967296 | ManagedHeap 对象堆 |

Eval 模式导入这三块内存而不声明，复用父实例。`HEAP_MEMORY_MIN_PAGES` / `HEAP_MEMORY_MAX_PAGES` 常量在 `wjsm-ir`，是 user wasm 与 host 的唯一对齐来源。

## env global

27 个 mutable global 从 `env` 导入，由 `ENV_GLOBAL_EXPORT_NAMES` 列出（见[Normal 与 Eval 编译模式](normal-and-eval-modes.md)）。support module 的 global 索引与 user wasm 完全对齐（0..26），使 helper body 移植时 GlobalGet/GlobalSet 索引无需修改。

## Export

Normal 模式导出 `main`（Type 4），作为宿主实例化后的调用入口。Eval 模式导出 `__eval_entry`（Type 3，接收 scope env）。其余 export 是 support module 内部的 `wjsm_bootstrap_once` / `wjsm_init_function_props`（占位 `unreachable`，待 P2.6 迁移）。

> <details><summary>为什么 `main` 导出但是 `() -> i64`？</summary>
>
> 模块入口收到一个空调用（无参数），返回一个 `i64`（NaN-box 值）。返回值是 `0`（成功）或 `TAG_EXCEPTION`（未捕获异常）。
>
> 这种形状的设计考虑：
>
> 1. **入口可被 trap 标记**。如果入口函数 panic 退出（unreachable、segment fault 等），wasmtime 会把 trap 翻译成运行时错误。如果入口是 void，trap 不可观察。
> 2. **统一的退出码逻辑**。无论正常结束还是异常，宿主都从入口返回值读到「值」，按值判断是否异常。避免两套退出路径。
> 3. **AOT 编译友好**。NaN-boxing 让任何 JS 值都能塞进 i64 返回值。入口不需要知道「用户在 main 里返回了什么类型」——通通 i64。
>
> </details>

## host import 注册

`host_import_registry/` 用六个 `specs_part*.rs` 文件定义约 507 个 host import 规格，按域分组（core ops、collections、string、async、runtime、inspector 等）。每个 spec 记录 import 名、type index 和特殊标记（如 `SpecialHostImport`）。后端在 Pass 1 之后预留这些索引，使函数体编译时直接引用。

## 深入了解

- [用户视角的产物依赖](../../user/output/wasm-artifacts.md)
- [support module 如何被预编译与嵌入](../startup/support-cwasm.md)
- [Engine 配置如何满足这些内存要求](../host-runtime/engine-configuration.md)
