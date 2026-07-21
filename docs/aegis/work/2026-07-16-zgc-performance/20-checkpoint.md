# TodoCheckpointDraft

## Current todo

Task 15 V2 激活缺口修复继续（主会话直接执行）。all-features RED 从 27 降至
**20**（17 failed + 3 timed out）；默认构建面保持全绿。

## Completed (this session, 2026-07-21 cont.)

- `992c031` fix: V2 eval/proto-walk 属性读取走 HeapAccessV2
  （read_object_property_by_name_proto_walk_with_env 缺 V2 分支 →
  eval_get_binding/eval_set_binding/eval_has_binding 在 contextified sandbox
  上用 V2 handle 当 V1 ptr 扫描主内存。vm/eval 家族 8 个测试修复）。
- `c69dfd3` fix: V2 数组命名属性与函数 symbol 查找走 side table
  （get_by_name_id_sync 和 get_v_by_name_id V2 分支对数组 symbol 属性走
  get_property_slot_on_proto_chain 而非 ArrayNamedPropsStore；对函数对象
  用 decode_handle 而非 handle_index_of。issue255 / issue245 第 1 例修复）。

## 方法论（已验证）

1. `cargo nextest run --workspace --all-features` 取 RED 清单 → 按家族聚类。
2. 最小 fixture 复现 → 必要时 eprintln 临时调试（已移除）。
3. 修复模式：V1-only host helper 补 `managed-heap-v2` fork；镜像 V1 support 模块
   的 tag/类型分派语义；数组命名属性一律侧表；函数对象 handle 用 handle_index_of。
4. 每片修复后：all-features 计数 + 默认面全绿 → 独立提交。

## 剩余失败家族（20）

### GC 依赖（6）—— 需 V2 GC 接入
- weakref_gc、weak_collections_gc、finalization_registry_cleanup、
  gc_map_set_owner_reachability、async_hooks_destroy_gc、perf_hooks_native_gc

**根因**：V2 feature 下 `gc_algorithm` 仍为 V1 `MarkSweepCollector`，它扫描
main memory32 的 4-byte handle table，而 V2 对象在 shared memory64 的 8-byte
handle table。V1 GC 完全扫不到 V2 对象，`freed_handles` 为空，weakref/finalization
cleanup 不触发。`MarkSweepV2`/`G1V2`/`ZgcV2` 已实现但未接入 `GcAlgorithm` trait
（接口不同：用 `RootSnapshot` 而非 `RootProvider`，用 `ManagedHeap` 而非 `GcContext`）。

### perf_hooks（5 + 1 TIMEOUT）
- event_loop_iteration、api_semantics、histogram_clone、timerify、
  native_gc、native_entries (TIMEOUT)

### eval（1）
- eval_var_predeclare_parser：eval 中 class 声明导致 WASM trap，回退到解释器，
  解释器不支持 class。需排查 V2 backend 对 eval class 编译。

### misc（5）
- array_slice_call_arraylike（call stack exhausted ×50 → 无限递归）
- happy_assign_toobject（"undefined undefined undefined" → ToObject 失败）
- issue135_reflect_set_has（inherited_has: false → 原型链 has 查找缺 V2 分支）
- issue245_concat（第 2 例：普通对象索引属性 spread）
- promise_then_symbol_species（第 4 个 true 应为 false → Symbol.species 查找）

### 其他（2）
- cluster_worker_listen_failure_primary_exits
- modules__node_builtin_util_events_assert（第 1 个 false → 同 issue135 原型链 has）

## Blocked on

- Task 24/25 性能 GREEN 仍需 instrumented JDK 25 + 具名大堆 runner（外部资源）。
- V2 GC 接入是 weakref/finalization 家族的前置阻断。

## Next step

继续 misc 家族（不依赖 GC）：issue135 原型链 has → issue245 对象索引 spread →
promise_then Symbol.species → happy_assign_toobject → array_slice 递归。
然后处理 perf_hooks。GC 依赖家族需 V2 GC adapter（大任务，后续处理）。

## ResumeStateHint

读本文件 + `git log --oneline -15` + 最新一次 all-features 失败清单即可恢复。
环境噪声：load>10 时 3s 门禁会假超时（先 `uptime`）。
