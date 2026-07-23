# TodoCheckpointDraft

## Current todo

**Task 26 后残余清零完成。** 下一任务是 Task 27（最终全量验证与 ADR/AGENTS 闭环）。

## Completed (this session, 2026-07-23)

### Task 26 residual purge — dual-path / dead helpers / memory32 obj_table 回落
- Backend：删除 `compile_object_helpers`、`compiler_array_helpers`、`helpers_bounds`、
  孤儿 `support_object_helpers`；`helpers_object` 仅保留 `bind_v2_support_helpers`。
- support_module：删除未调用 emit 辅助（barrier/resolve/bounds/resize 等）；
  HOST_IMPORTS 瘦身为 safepoint + take_freed_handle + V2 host helpers 并重编号；
  删除 `gc_alloc_slow` import；`emit_handle_table_alloc_check` 改为 V2 handle 上限检查
  （禁止 main-memory 4-byte entry 布局）。
- host registry / runtime：删除 `GcAllocSlow` / `env.gc_alloc_slow` 定义。
- Runtime grow/resolve：`grow_array` → `ensure_v2_array_capacity`；
  `grow_object` → `HeapAccessV2::grow_object_capacity`；删除 main-memory
  `obj_table` 写与 `abandon_region`/`abandoned_regions`。
- 属性写/数组扩容 dual-path fallback 删除；handle 解析失败不再 silent memory32 写。
- snapshot build.rs：折叠 V1 分支，仅嵌入 V2 artifact ABI。
- 残余清零：`compile_object_helpers` / `compile_array_helpers` / `support_object_helpers` /
  `helpers_bounds` / 错误 4-byte handle stride / grow 路径 main-memory obj_table → **0**。

### 此前已关闭
- Task 26 V1 collectors / dyn GcAlgorithm / criterion 退役
- Active concurrent ZGC wiring（`active_zgc`）
- Task 15–25 协议与 cutover

## Next step — Task 27

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo nextest run --workspace
WJSM_TEST_GC=mark-sweep cargo nextest run -E 'test(happy__)'
WJSM_TEST_GC=g1 cargo nextest run -E 'test(happy__)'
WJSM_TEST_GC=zgc cargo nextest run -E 'test(happy__)'
```

## ResumeStateHint

读本文件 + 提交 `refactor: purge remaining V1 GC dual-path residuals`。
Task 24/25 性能/大堆矩阵仍 `needs-verification`。
Task 27 负责 ADR 0010 状态文案与全量闭环。

## DriftCheckDraft

- 范围：Task 26 后 dual-path / 死代码 / memory32 对象堆回落清零。
- 兼容：公开 `--gc` / `WJSM_GC` / `gc()` 不变；support ABI 仅 V2 host imports。
- 退役：inline object helpers、gc_alloc_slow、abandon_region、main-memory grow。
- 决策：`continue` → Task 27。
