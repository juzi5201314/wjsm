# Native ABI 索引

这一章汇总 generated code 与 `NativeRuntime` 之间的 ABI。契约定义在 `wjsm-native-abi`；状态与 thunk 实现在 `wjsm-host-native`。当前版本常量：`NATIVE_ABI_VERSION = 11`。

## NativeVmContext

`NativeVmContext` 是进程生命周期内由 owner 持有的热上下文。生成代码通过指针访问它，不经过线性内存导入。主要字段：

| 字段 | 用途 |
| --- | --- |
| `abi_version` / `flags` | ABI 版本；`NATIVE_FLAGS_FEEDBACK_ENABLED` 控制反馈快路径 |
| `heap_state` / `allocation_state` / `gc_state` / `barrier_state` | 指向 `NativeAgentState`、分配器、collector、`NativeBarrierState` |
| `call_arena_slots` / `call_arena_capacity` / `call_arena_active_len` | 实参槽（`CallArgs.base` 是下标） |
| `function_table` / `function_table_len` / `current_table_base` / `current_image_id` | 当前 image 的 `NativeFunctionEntry` 表 |
| `call_frame_head` / `root_frame_head` / `source_frame_head` | 调用链、GC root、源码位置 |
| `stack_low` / `stack_high` / `stack_budget_bytes` | 栈界与合作式回边预算 |
| `handle_table_base` / `ic_slots_base` / `feedback_slots_base` | 句柄表与 IC / 反馈区 |
| `heap_object_delta` | 逻辑 memory64 偏移 → 进程虚拟地址 |
| `pending_exception_kind` | gate 无法分配时的预分配异常种类 |

没有独立的 `memory` / `shadow_memory` / `heap_memory` 导入，也没有 wasm page 计数。

## 函数入口与 CallArgs

`NativeSlowEntry`：

```text
(ctx: *mut NativeVmContext, env: i64, this_value: i64, args_base: u32, args_count: u32) -> i64
```

`CallArgs`（`wjsm-host`，8 字节）是 call arena 上的一段连续槽：`base: u32` + `len: u32`。IR 形参 `$env` / `$this` 对应这里的 `env` / `this_value`。返回值是 NaN-box `i64`；异常带 `TAG_EXCEPTION`。

## Root / call / source frame

| 结构 | 用途 |
| --- | --- |
| `NativeCallFrame` | 调用链：`image_id`、`function_index`、`table_base` |
| `NativeRootFrame` | safepoint 发布的 root 视图：`slots` + `bitmap_words` |
| `NativeSourceFrame` | 错误堆栈：`image_id`、`function_index`、`source_position` |
| `NativeSourceSlot` | pending 源码位置槽 |

GC 扫描只读 bitmap 置位的 slot。`ROOT_FRAME_VERSION = 2`，`SOURCE_FRAME_VERSION = 1`，`CALL_GATE_VERSION = 1`。

## Host symbol

生成代码只能调用 `NativeHostSymbol` allowlist。叶子签名（`F64Unary` / `F64Binary` / ZGC 屏障）`may_gc` / `may_reenter` 为 false，不必发布额外 root。`HostOperationDispatcher` 会 GC / 重入，必须走完整 arena + safepoint。

| ID | 符号 | 签名 |
| --- | --- | --- |
| 0 | `wjsm_native_host_operation` | `(ctx, operation, args, args_count, feedback_slot) -> i64` |
| 1–21 | `wjsm_native_math_*` | typed f64 直连 |
| 22 | `wjsm_native_zgc_load_barrier_assist` | `(vmctx, handle) -> address` |
| 23 | `wjsm_native_zgc_store_barrier` | `(vmctx, owner, slot, value) -> status` |

`operation` 要么是 `NativeHostOp`（`Builtin::wire_id()`），要么是 `NativeRuntimeOp`（`0x1_0000` 起的同步 dispatcher ID：算术、属性、调用、`CreateException`、`CooperativePoll` 等）。

## 屏障

`NativeBarrierState` 与 collector 共享：`phase` / `access_epoch` 由 collector 在 safepoint 发布；`load_fast_events` / `store_fast_events` 由 owner mutator 写入。`BARRIER_VERSION = 2`。

## ABI hash

`native_abi_hash()` 哈希布局、关键 offset、版本常量、`NativeRuntimeOp` ID、`NativeHostSymbol` 名字与签名，以及 `wjsm-ir` 的 `value.rs` / `constants.rs`。改布局或协议会自动换 hash，旧 `.wnat` 全部 miss。不要手改一个独立的版本号来代替它。

## 深入了解

- [Import、Export 与主模块 ABI](../backend/imports-exports-and-abi.md)
- [对象、值与标签索引](layout-and-tags.md)
- [修改 Native ABI](../development/changing-wasm-abi.md)
