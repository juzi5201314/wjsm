# P2 host 裸写点审计清单

父计划：`docs/aegis/plans/2026-07-03-pluggable-gc-v2.md` P2。  
父规格：`docs/aegis/specs/2026-07-03-pluggable-gc-v2-design.md` §13。  
目标：所有 host 侧对象属性槽、数组元素槽、proto header 写入最终迁移到 `runtime_gc::heap_access`；只读短窗口和非 JS 对象堆写入明确标注。

## 审计命令与覆盖

- `grep` pattern `HEAP_OBJECT_PROPERTY|HEAP_ARRAY_ELEMENT|HEAP_OBJECT_PROTO_OFFSET` over `crates/wjsm-runtime/src`：命中 5 个文件族。
- `grep` pattern `copy_from_slice(&proto|copy_from_slice(&proto_handle|ptr..ptr + 4|PROTO_OFFSET` over `crates/wjsm-runtime/src`：命中 proto header 写入与只读/测试构造点。
- `grep` pattern `setPrototypeOf|Object.create|Reflect.setPrototypeOf|__proto__|prototype` over `host_imports`、`runtime_heap.rs`、`runtime_host_helpers`：命中 prototype API 入口和只读原型链遍历。

## T2.1 新 owner

- [x] `crates/wjsm-runtime/src/runtime_gc/heap_access.rs`：新增 `resolve`、`write_property_slot`、`write_element`、`write_proto`。
- [x] `HeapPtr` debug epoch 断言：raw ptr 使用时验证未跨 GC 点。
- [x] `GcContext::increment_gc_epoch()`：mark-sweep 完整/增量 sweep 已接入。

## 主 offset grep 清单

| 文件 | 行 | 分类 | 替换任务 | 状态 |
|---|---:|---|---|---|
| `runtime_heap.rs` | 117-149 | host object 分配初始化：proto 初始化迁入 `heap_access::init_proto_at_ptr`；header/count/obj_table 初始化不经 barrier | T2.3 | 已勾销 |
| `runtime_heap.rs` | 293-298 | `set_object_proto_header` 经 `heap_access::write_proto` | T2.3 | 已勾销 |
| `startup_snapshot_remap.rs` | 4-36 | 快照 remap 布局常量只读/重映射逻辑；不是运行期 mutator 写屏障点 | T2.2 | 只读/启动快照边界，保留 |
| `runtime_gc/context.rs` | 12-14 | GC layout 常量 owner，只读 | T2.2 | 保留 |
| `runtime_gc/heap_access.rs` | 97-138 | 新 canonical owner 内部 slot 地址计算 | T2.1 | 保留 |
| `runtime_host_helpers/host_helpers_alloc.rs` | 146-178 | host array 分配初始化：proto 经 `heap_access::init_proto_at_ptr`；header/handle slot 为初始化元数据写 | T2.5 | 已勾销 |
| `runtime_host_helpers/host_helpers_alloc.rs` | 187-220 | `set_array_elem_with_env` 经 `heap_access::write_element` | T2.5 | 已勾销 |
| `runtime_host_helpers/host_helpers_alloc.rs` | 223-265 | host object 分配初始化：proto 经 `heap_access::init_proto_at_ptr`；header/handle slot 为初始化元数据写 | T2.5 | 已勾销 |

## proto header 交叉清单

| 文件 | 行 | 入口 | 替换任务 | 状态 |
|---|---:|---|---|---|
| `runtime_heap.rs` | 134-137 | `alloc_host_object_impl` 初始化 proto 经 `heap_access::init_proto_at_ptr` | T2.3 | 已勾销 |
| `runtime_heap.rs` | 286-298 | `set_object_proto_header` 经 `heap_access::write_proto` | T2.3 | 已勾销 |
| `runtime_heap.rs` | 979-980 | 当前 proto 只读比较 | T2.3 | 短窗口只读，保留 |
| `host_imports/array_object.rs` | 156-171 | `object_write_proto_handle`（Object.create / Object.setPrototypeOf）经 `heap_access::write_proto` | T2.4 | 已勾销 |
| `host_imports/async_generator.rs` | 18-26 | AsyncGenerator prototype 安装经 `set_object_proto_header` → `heap_access::write_proto` | T2.5 | 已勾销 |
| `host_imports/generator.rs` | 17-25 | Generator prototype 安装经 `set_object_proto_header` → `heap_access::write_proto` | T2.5 | 已勾销 |
| `host_imports/object_builtins.rs` | 39-47 | Object.create proto 设置经 `heap_access::write_proto` | T2.5 | 已勾销 |
| `host_imports/object_builtins.rs` | 191-246 | Object.setPrototypeOf proto 设置经 `heap_access::write_proto` | T2.5 | 已勾销 |
| `host_imports/proxy_reflect.rs` | 671-717 | Reflect.setPrototypeOf 普通对象路径经 `heap_access::write_proto` | T2.5 | 已勾销 |
| `runtime_gc/mark_sweep/marker.rs` | 488-489 | 单元测试 buffer 构造 | T2.2 | 测试构造，保留 |
| `runtime_host_helpers/host_helpers_alloc.rs` | 163-166 | array 初始化 proto 经 `heap_access::init_proto_at_ptr` | T2.5 | 已勾销 |
| `runtime_host_helpers/host_helpers_alloc.rs` | 245-248 | object 初始化 null proto 经 `heap_access::init_proto_at_ptr` | T2.5 | 已勾销 |

## 属性槽 / 元素槽写入清单

| 文件 | 行 | 写入 | 替换任务 | 状态 |
|---|---:|---|---|---|
| `runtime_values.rs` | 327-333 | `write_array_elem_with_env` 经 `heap_access::write_element_at_ptr` | T2.3 | 已勾销 |
| `runtime_values.rs` | 721-795 | `write_object_property_by_name_id` value slot 写/新增属性 value 经 `heap_access::write_property_slot`；name/flags/count 保持元数据写 | T2.3 | 已勾销 |
| `runtime_values.rs` | 798-832+ | `write_private_accessor_slot` getter/setter/value slot 写经 `heap_access::write_property_slot` | T2.3 | 已勾销 |
| `runtime_host_helpers/host_helpers_property.rs` | 23-77 | host data property value/getter/setter slot 写经 `heap_access::write_property_slot` | T2.5 | 已勾销 |
| `runtime_host_helpers/host_helpers_property.rs` | 84-132 | host accessor value/getter/setter slot 写经 `heap_access::write_property_slot` | T2.5 | 已勾销 |
| `host_imports/array_object.rs` | 317-322 | DefineProperty existing slot value/getter/setter 经 `heap_access::write_property_slot`；flags 保持元数据写 | T2.4 | 已勾销 |
| `host_imports/collections_buffers.rs` | 2602-2604 | private field existing value slot 经 `heap_access::write_property_slot` | T2.4 | 已勾销 |
| `host_imports/object_builtins.rs` | 441-446 | descriptor flags 更新，无引用 value 写 | T2.5 | 元数据写，保留 |
| `host_imports/proxy_reflect.rs` | 358-372 | delete property slot compaction 的 value/getter/setter 移动经 `heap_access::write_property_slot`，count/name/flags 为元数据写 | T2.5 | 已勾销 |
| `runtime_host_helpers/host_helpers_proxy.rs` | 131-136 | descriptor slot value/getter/setter 写经 `heap_access::write_property_slot` | T2.5 | 已勾销 |
| `runtime_host_helpers/host_helpers_proxy.rs` | 237-247 | proxy grow object 在 alloc 后通过 handle 重新解析旧 ptr，再 copy/update obj_table | T2.6 | 已勾销 |
| `runtime_host_helpers/host_helpers_proxy.rs` | 257-269 | proxy 新增属性 value/getter/setter 写经 `heap_access::write_property_slot` | T2.5 | 已勾销 |
| `runtime_host_helpers/host_helpers_proxy.rs` | 985-995 | define host data property fallback value/getter/setter 写经 `heap_access::write_property_slot` | T2.5 | 已勾销 |

## 明确非 JS 对象堆写入

以下命中不属于 `heap_access` 迁移目标：

- `host_imports/atomics.rs`：SharedArrayBuffer/TypedArray backing store 原子写，不是 JS 对象 heap slot。
- `host_imports/collections_buffers.rs` DataView / TypedArray backing buffer 写入，不是 JS 对象 heap slot。
- `host_imports/streams_*`、`runtime_host_helpers` 中 `ArrayBufferEntry.data` 写入，不是 JS 对象 heap slot。
- `shadow stack` 参数写入（`memory.write(... saved_sp ...)`）是 safepoint root 协议，不是对象属性槽。
- `RuntimeState` 侧表写入（promise/continuation/stream table）按 spec §13 明确不经 `heap_access`。

## T2.2 DriftCheckDraft

- Scope：本清单覆盖主 offset grep、proto 写交叉 grep、prototype API 入口 grep。
- Compatibility：未改运行时行为，仅建立迁移核对表。
- Retirement：后续 T2.3-T2.5 逐项勾销；P2 末复跑 grep，剩余命中必须为 `heap_access` 内部、只读短窗口、初始化/元数据写或非对象堆写。
- Decision：continue。

## T2.3 DriftCheckDraft

- Scope：完成 `runtime_values.rs` 和 `runtime_heap.rs` 核心写点迁移；`runtime_builtins.rs` / `host_imports/core.rs` 本批无对象槽裸写需替换。
- Compatibility：`cargo nextest run --workspace` 已通过；默认 mark-sweep 输出语义未改。
- Retirement：核心属性/元素/proto 写点已从裸写切到 `heap_access`；对象分配初始化通过 `heap_access::init_proto_at_ptr` 归入 owner，但仍不触发 barrier，因为对象尚未发布给 mutator。
- Decision：continue。

## T2.4 DriftCheckDraft

- Scope：完成 `host_imports/array_object.rs` 的 proto/DefineProperty existing-slot 写，以及 `collections_buffers.rs` private existing-slot 写；typedarray/streams 命中均为 backing store 或 RuntimeState 侧表，不属于 JS 对象 heap slot。
- Compatibility：`cargo nextest run --workspace` 已通过。
- Retirement：集合/对象 builtin 族待迁移写点已勾销；新增属性路径复用 `runtime_host_helpers::write_new_property_to_memory`，归入 T2.5。
- Decision：continue。

## T2.5 DriftCheckDraft

- Scope：完成其余 host imports、runtime_host_helpers 与 runtime_values 补充写点迁移；剩余 grep 命中为只读 getter/proto 读取、测试 buffer 构造、`heap_access` 内部或非对象堆写入。
- Compatibility：`cargo nextest run --workspace` 已通过。
- Retirement：host 侧对象属性/元素/proto 引用槽写入已收敛到 `heap_access`，obj_table ptr 更新与 resize re-resolve 留给 T2.6。
- Decision：continue。

## T2.6 DriftCheckDraft

- Scope：完成 resize re-resolve；`compiler_helpers/helpers_object.rs` 与 `support_object_helpers.rs` 的 object resize 在 `gc_alloc_slow` 后、`memory.copy` 前重新从 `obj_table[handle]` 读取 old_ptr；`runtime_values::{grow_array,grow_object}` 与 `runtime_host_helpers/host_helpers_proxy.rs` 的 host resize 在 `alloc_heap_region_for_host` 后重新解析旧 ptr 再拷贝。
- Compatibility：`cargo check -p wjsm-runtime -p wjsm-backend-wasm`、`cargo nextest run -p wjsm-backend-wasm`、`cargo nextest run -p wjsm-runtime` 已通过。
- Retirement：resize 旧 ptr 不再跨 GC/alloc 点复用；新增 backend 结构测试锁定 support module 的 `obj_set` resize copy 前 re-resolve 序列。
- Decision：continue。
