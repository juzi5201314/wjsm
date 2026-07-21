# TodoCheckpointDraft

## Current todo

Task 15 V2 激活缺口修复继续。all-features RED 从 27 降至 **19**
（15 failed + 4 timed out）；默认构建面保持全绿。

## Completed (this session, 2026-07-21 cont.)

- `992c031` fix: V2 eval/proto-walk 属性读取走 HeapAccessV2
  （read_object_property_by_name_proto_walk_with_env 缺 V2 分支 →
  eval_get_binding/eval_set_binding/eval_has_binding 在 contextified sandbox
  上用 V2 handle 当 V1 ptr 扫描主内存。vm/eval 家族 8 个测试修复）。
- `c69dfd3` fix: V2 数组命名属性与函数 symbol 查找走 side table
  （get_by_name_id_sync 和 get_v_by_name_id V2 分支对数组 symbol 属性走
  get_property_slot_on_proto_chain 而非 ArrayNamedPropsStore；对函数对象
  用 decode_handle 而非 handle_index_of。issue255 / issue245 第 1 例修复）。
- `7b0f055` fix: V2 Reflect.has 和 in 操作符走 HeapAccessV2 原型链
  （reflect_has_impl 和 op_in_impl 缺 V2 分支，issue135 inherited_has 修复）。
- `f5aeae9` fix: V2 get_by_name_id_sync 非数组分支用 handle_index_of
  （函数对象 Symbol.species getter 读取，promise_then_symbol_species 修复）。

## 剩余失败家族（19）

### GC 依赖（6）—— 需 V2 GC 接入
- weakref_gc、weak_collections_gc、finalization_registry_cleanup、
  gc_map_set_owner_reachability、async_hooks_destroy_gc、perf_hooks_native_gc

**根因**：V2 feature 下 `gc_algorithm` 仍为 V1 `MarkSweepCollector`，它扫描
main memory32 的 4-byte handle table，而 V2 对象在 shared memory64 的 8-byte
handle table。V1 GC 完全扫不到 V2 对象，`freed_handles` 为空，weakref/finalization
cleanup 不触发。`MarkSweepV2`/`G1V2`/`ZgcV2` 已实现但未接入 `GcAlgorithm` trait
（接口不同：用 `RootSnapshot` 而非 `RootProvider`，用 `ManagedHeap` 而非 `GcContext`）。

### perf_hooks（4 + 1 TIMEOUT）
- api_semantics、histogram_clone、event_loop_iteration、timerify、
  native_entries (TIMEOUT)

### eval（1）
- eval_var_predeclare_parser：eval 中 class 声明导致 WASM trap，回退到解释器，
  解释器不支持 class。需排查 V2 backend 对 eval class 编译。

### to_object（3）
- happy_assign_toobject、issue245_concat 第 2 例：`to_object("ab")` 创建的
  V2 对象属性写入后读不到。`alloc_host_object` V2 分支 + `define_host_data_property`
  路径可能有 handle 不一致问题。

### 其他（2）
- array_slice_call_arraylike（call stack exhausted ×50 → 无限递归）
- cluster_worker_listen_failure_primary_exits
- async_hooks_load_100k（TIMEOUT，可能是 load 噪声）

## Next step

1. 修复 `to_object` V2 属性写入问题（影响 happy_assign_toobject + issue245 第 2 例）
2. 处理 perf_hooks 家族
3. V2 GC 接入（大任务，后续处理）

## ResumeStateHint

读本文件 + `git log --oneline -15` + 最新一次 all-features 失败清单即可恢复。
环境噪声：load>10 时 3s 门禁会假超时（先 `uptime`）。
