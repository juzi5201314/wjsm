# TodoCheckpointDraft

## Current todo

Task 15 GREEN 完成。all-features / default workspace / 三种 collector happy 矩阵 / 三 CLI smoke 全绿；私有 `managed-heap-v2` feature 与全部 `.github/workflows` 已删除。

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

## 历史记录（2026-07-21，已过时）

以下 19-failure 家族描述对应 cutover 中途状态，已被 2026-07-23 Task 15 GREEN 证据推翻；保留仅供追溯，不再作为 resume 依据。

## Next step

1. 进入 Task 16（colored barriers）或计划后续 Phase D 任务。
2. Task 26 继续源码级退役：`HANDLE_TABLE_ENTRY_SIZE=4` 字面常量、legacy `MarkSweepCollector`/`G1Collector`/`ZgcCollector` 实现体、`emit_support_module_with_heap_mode(..., false)` 等 dead code。

## ResumeStateHint

读本文件 + `git log --oneline -15` + Task 15 GREEN 证据段即可恢复。
环境噪声：load>10 时 3s 门禁会假超时（先 `uptime`）。

## Checkpoint 2026-07-23：Task 15 GREEN 完成

### TodoCheckpointDraft

- 当前 todo：Task 15 已完成；下一任务是 Phase D Task 16 或计划后续项。
- 已完成：all-features、default workspace、三种 `WJSM_TEST_GC` happy 矩阵、三 CLI smoke、active-tree 负审计、完成前核验、删除全部 GitHub Actions workflows。
- 本切片代码改动：仅删除 `.github/workflows/{zgc-capability-matrix,zgc-nightly,test262}.yml`；运行时/后端无新增源码改动。

### Evidence

- `cargo nextest run --workspace --all-features` → 1837 passed / 17 skipped
- `cargo nextest run --workspace` → 1837 passed / 17 skipped
- `WJSM_TEST_GC=mark-sweep|g1|zgc cargo nextest run -E 'test(happy__)'` → 各 666 passed / 227 skipped
- `cargo run -- run --gc mark-sweep|g1|zgc -e 'const x={a:[1,2,3]}; gc(); console.log(x.a[1])'` → 均输出 `2`
- WAT skeleton 含 shared `env.__heap_memory` memory64，并 import `wjsm_support.{obj_new,obj_get,obj_set,...}`
- 负审计：`managed-heap-v2 =` 与 `cfg(feature="managed-heap-v2")` 均不存在；`.github/workflows` 三文件已删
- 完成前回归：`modules__async_local_worker_main | happy__weakref_gc | happy__finalization_registry_cleanup` → 3 passed
- `cargo check -p wjsm-runtime -p wjsm-backend-wasm -p wjsm-cli` → Finished

### BaselineUsageDraft

- 已读取：主计划 Task 15、检查点、证据、runtime_startup/active_v2/host GC 路径、WAT skeleton、workflow 文件。
- 缺失：无 Task 15 阻塞项。

### DriftCheckDraft

- 范围：仍在 Task 15 单一 active V2 cutover。
- 兼容边界：未改 ECMAScript / 公开 `--gc` 选择；用户明确要求删除 CI workflows 且暂不重建。
- 退役轨迹：private feature 与 CI 入口已删；legacy collector 源码残留交给 Task 26。
- 决策：`continue`（计划可进入 Task 16）。

### Risk / Unknown

- 旧 4-byte collector 实现与 `HANDLE_TABLE_ENTRY_SIZE=4` 字面常量仍在树中，但 active 对象路径已走 `HeapAccessV2` + shared memory64；源码级清扫属于 Task 26。
- CI 已按用户要求完全删除；后续若恢复 CI 需重新设计，不可回灌 `managed-heap-v2` feature。
