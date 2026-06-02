# TaskIntentDraft

- 请求结果：完整执行 `docs/aegis/plans/2026-05-30-json-implementation.md` 的 13 个任务，实现 ES 规范合规的 `JSON.parse(text, reviver)` 和 `JSON.stringify(value, replacer, space)`，包含 SIMD 加速的递归下降解析器，替换 stub，实现所有 spec 要求，更新 22+ fixtures，无任何 TODO 占位符。
- 范围：`wjsm-semantic`、`wjsm-backend-wasm`、`wjsm-runtime`（含新建 `runtime_json.rs`）、fixtures 更新。严格按计划 13 个任务顺序执行。
- 非目标：不修改与 JSON 无关的 builtins；不引入新外部 crate 依赖；不改变 NaN-boxing 或 WASM 内存布局；不实现完整的 Error 对象 SyntaxError（runtime 限制，已在计划中记录为 deviation）；不扩展到 TypedArray/其他特性。

## BaselineReadSetHint
- `docs/aegis/plans/2026-05-30-json-implementation.md` (full plan)
- `AGENTS.md` (project conventions, esp. Chinese comments, ES spec compliance, no PoC compromises)
- `crates/wjsm-semantic/src/builtins.rs` (Builtin metadata)
- `crates/wjsm-backend-wasm/src/lib.rs` (import count table)
- `crates/wjsm-backend-wasm/src/compiler_builtins.rs` (builtin emission)
- `crates/wjsm-backend-wasm/src/host_import_registry.rs` (type indices)
- `crates/wjsm-backend-wasm/src/compiler_core.rs` (type definitions 2 and 16)
- `crates/wjsm-runtime/src/lib.rs` (module declarations)
- `crates/wjsm-runtime/src/host_imports/timers_arrays.rs` (current json_parse/json_stringify stubs)
- `crates/wjsm-runtime/src/runtime_render.rs` (current stringify logic)
- `crates/wjsm-runtime/src/runtime_*.rs` (for heap helpers: alloc_host_object, write_object_property_by_name_id, etc.)
- ECMAScript 2024 §24.5.1 (JSON.parse), §24.5.2 (JSON.stringify), §7.1.17 (Number::toString)
- Existing JSON fixtures under `fixtures/happy/json_*.js` and `.expected`

## ImpactStatementDraft
- 兼容边界：WASM host import signatures 必须匹配现有 Type 2 / Type 16；所有 22+ JSON fixtures 必须在更新后通过；不得破坏其他 builtins 的 host import 顺序或索引。
- 高风险点：
  - SIMD AVX2 运行时检测 + scalar fallback 正确性（非法指令风险）
  - `delete_property_by_name_id` swap-remove 正确性（基于 reflect_delete_property_impl）
  - Reviver walk 对数组/对象的递归 + this=holder + 直接使用返回值
  - Stringify replacer whitelist（Vec<String> vs HashSet 顺序）、toJSON 调用、space 处理
  - 现有 runtime heap 函数的正确调用（alloc_host_object, call_wasm_callback 等）
- 验证策略：每任务后 `cargo check -p <crate>`；TDD（test-driven per subagent）；spec compliance review + code quality review (two-stage)；最终 `cargo nextest run -E 'test(happy__json_)'` + 手动 eval 验证；全量 `cargo build --all`；更新 fixtures via `WJSM_UPDATE_FIXTURES=1`。
- 隔离：用户明确指示“直接在当前目录，不需要工作树，但是切换到新分支”，已切换到 `feat/json-es-compliance-simd`。使用-git-worktrees 协议因用户覆盖而部分满足（分支隔离仍提供工作隔离）。

## Rust Style Guide Application (MANDATORY per user)
- 必须使用 rust-style-guide skill 进行所有 Rust 编写、审查、重构。
- 优先仓库本地约定（AGENTS.md + 现有代码模式），其次本 guide。
- 核心：正确性第一，可维护性第二，低分配开销，显式所有权，避免不必要抽象。
- 所有新代码必须通过 rust-style-guide 审查（在 code-quality-review 阶段强制包含）。

## Subagent-Driven-Development Protocol (STRICT)
- 每个任务：新鲜 subagent (implementer) + SubagentContextPacket + TDD (aegis:test-driven-development) + 两阶段审查（先 spec compliance，再 code quality）。
- 严禁跳过任一审查；spec 未 ✅ 不得进入 code quality；发现问题必须 implementer 修复 + 重新审查直到通过。
- 每个 implementer prompt 必须包含 long-task checkpoint 引用、rust-style-guide 要求、完整任务文本、非目标、验证命令。
- 所有任务完成后：final code reviewer + finishing-a-development-branch。

## Known Facts (verified via prior reads)
- Type 16: (i64,i64,i64)->i64 exists for 3-arg; Type 2: (i64,i64)->i64 for 2-arg (from plan).
- Current JSON.parse is stub returning raw string.
- `write_object_property_by_name_id` exists and implements last-wins for duplicate keys.
- `reflect_delete_property_impl` uses swap-remove pattern for configurable props.
- SIMD design (StringBlock, NonspaceBitmap) reviewed in plan v3.

## Unsafe Assumptions (MUST be verified by implementers before relying)
- The exact function signatures and availability of: `alloc_host_object`, `alloc_array`, `write_array_elem`, `write_array_length`, `read_array_length`, `read_array_elem`, `resolve_handle`, `resolve_array_ptr`, `store_runtime_string`, `read_string`, `call_wasm_callback`, `set_runtime_error`, `eval_to_string`, `find_memory_c_string_with_env`, `alloc_heap_c_string_with_env`, `write_object_property_by_name_id`, `find_property_slot_by_name_id_with_env`, `resolve_handle_idx_with_env`.
- These must exist in current `wjsm-runtime` with the exact names and behaviors described in plan Task 9 code sketches.
- If any missing, implementer MUST report BLOCKED/NEEDS_CONTEXT immediately rather than guess.

## Stop Condition
Work is complete only when:
- All 13 tasks marked done with evidence (cargo check/build, test runs, reviews ✅).
- All JSON fixtures pass (after update).
- Manual eval commands in Task 13 produce correct output.
- No open issues from spec or quality reviews.
- finishing-a-development-branch executed.
- Drift checks passed for every slice.
- Retirement table in plan is executed (stubs replaced).
- Deviations explicitly documented (not claimed compliant).

## Non-goals (explicit)
- Do not implement full SyntaxError as Error object (runtime limitation).
- Do not add holes support to dense arrays for reviver delete.
- Do not change UTF-16 code unit counting for space (rare edge, documented deviation).
- Do not add new dependencies.
- Do not touch non-JSON builtins or IR.
