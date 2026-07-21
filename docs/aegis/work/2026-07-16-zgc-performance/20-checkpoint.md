# TodoCheckpointDraft

## Current todo

Task 15 V2 激活缺口修复进行中（主会话直接执行，无子代理）。all-features RED 从
103 失败降至 **39**（36 failed + 3 timed out）；默认构建面保持 1714/1714 全绿。

## Completed (this session, 2026-07-21)

- `cd5456dd` fix: managed-heap-v2 feature 统一泄漏修复落盘（删除 backend-wasm-v2 垫片、
  gc-bench opt-in、CI 显式 feature；见 32-root-cause-feature-unification.md）。
- `428df425` fix: V2 数组命名属性走 `ArrayNamedPropsStore` 侧表（heap 层数组守卫 +
  canonical key + gc_obj_get/set/delete 数组分支 + 原始值 tag 分派补齐 +
  alloc_host_null_proto_object V2 fork）。103→80。
- `9bb9ff2a` fix: `define_host_accessor_property_by_name_id_with_flags` 补 V2 fork
  （fetch Response 的 stream accessor 定义曾走 V1 grow 读垃圾容量 → 59GB 假 OOM）。80→58。
- `4e243989` fix: gc_elem_get/set_v2 的 typedarray 表分派（镜像 V1 obj_get_by_index）、
  typedarray_entry_from_value V2 句柄路径（上会话 WIP 验证收编）、SAB grow 的
  byteLength V2 写路径。58→39。

## 方法论（已验证）

1. `cargo nextest run --workspace --all-features` 取 RED 清单 → 按家族聚类。
2. 最小 fixture 复现 → 必要时 gdb **file:line 断点**（Rust 符号断点在本机 gdb 不可靠）。
3. 修复模式：V1-only host helper 补 `managed-heap-v2` fork；镜像 V1 support 模块
   的 tag/类型分派语义；数组命名属性一律侧表。
4. 每片修复后：all-features 计数 + 默认面全绿 → 独立提交。

## 已知剩余 V1ONLY 面（sweep 结果，按需修复）

`write_new_property_to_memory`（V1 分支内部，或安全）、`alloc_heap_c_string_with_env`、
runtime_values.rs：`grow_array/grow_object`（V1 分支内部）、
`read_object_property_by_name_proto_walk_with_env`、`write_object_property_by_name_id`、
`write_private_accessor_slot`；以及 `find_property_slot_by_name_id`（V1 ptr 扫描，
proxy_reflect 等处在 V2 句柄上仍可达）。

## 剩余失败家族（39）

- vm_*（6）+ async_hooks_vm（realm clone V2）
- perf_hooks（5+1，多为 3s TIMEOUT）
- cluster_ipc（5）
- fetch init/constructor（4）：fetch_data_url_init、fetch_headers_constructor_init、
  fetch_request_init、fetch_response_constructor
- streams byob + fetch body（~5）
- weakref/weak_collections/finalization/gc_map_set（4，GC 可达性）
- eval_*（3）
- 杂项：destructure_rest、for_of_conditional_break、happy_assign_toobject、
  issue135/245/255、promise_then_symbol_species、array_slice_call_arraylike、
  async_hooks_destroy_gc/load_100k、worker_threads_eval、util_events_assert、
  cluster 2×TIMEOUT、wjsm::property arithmetic（偶发）

## Blocked on

- Task 24/25 性能 GREEN 仍需 instrumented JDK 25 + 具名大堆 runner（外部资源）。

## Next step

继续按家族修复（建议顺序：fetch init/constructor → vm realm → weakref/GC →
eval → perf_hooks/cluster 超时类）。全部清零后进入 Task 15 cutover
（删除私有门与旧 dynamic heap，计划 line 443）。

## ResumeStateHint

读本文件 + `git log --oneline -12` + 最新一次 all-features 失败清单即可恢复。
环境噪声：load>10 时 3s 门禁会假超时（先 `uptime`；不要杀非本会话进程——用户明令）。

## DriftCheckDraft

- Scope：仅 Task 15 激活缺口；未动 collector 算法与 ABI 设计。
- Compatibility：无双轨 fallback 引入；V1 默认面每片验证全绿。
- Retirement：私有门与旧 heap 删除仍留在 cutover 单点（计划 Task 15 结构不变）。
