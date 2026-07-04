# EvidenceBundleDraft

## P0 evidence

- `cargo check -p wjsm-runtime` → passed。
- `cargo nextest run -p wjsm-runtime` → 133 passed, 2 skipped。
- `cargo build --workspace` → passed。
- Commits:
  - `feat: T0.3 switch runtime to GC v2`
  - `feat: T0.5 add GC scheduler`
  - `feat: T0.6 finalize mark-sweep governance`

## P1 T1.1 evidence

- `cargo check -p wjsm-runtime -p wjsm-backend-wasm -p wjsm-snapshot-format` → passed。
- `cargo nextest run -p wjsm-snapshot-format` → passed。
- `cargo nextest run -p wjsm-runtime -E 'test(startup_snapshot)'` → passed。
- `cargo nextest run -p wjsm-runtime` → passed。
- `cargo nextest run -p wjsm-backend-wasm -E 'test(shadow_stack_heap_guard_layout_and_canary)'` → passed。
- `cargo nextest run -p wjsm-backend-wasm` → passed。
- `cargo build --workspace` → passed。
- Commit: `feat: T1.1 upgrade GC immortal boundary`。

## P1 T1.2 evidence

- `cargo check -p wjsm-runtime -p wjsm-backend-wasm -p wjsm-runtime-support` → passed。
- `cargo nextest run -p wjsm-backend-wasm` → passed。
- `cargo nextest run -p wjsm-runtime-support` → passed。
- `cargo nextest run -p wjsm-runtime` → passed。
- `cargo build --workspace` → passed。
- Commit: `feat: T1.2 add GC coordination globals`。

## P1 T1.3 evidence

- `cargo check -p wjsm-backend-wasm -p wjsm-runtime` → passed。
- `cargo nextest run -p wjsm-backend-wasm -E 'test(support_alloc_helpers_use_alloc_window_and_safepoint_poll)'` → passed。
- `cargo nextest run -p wjsm-backend-wasm` → 54 passed。
- `cargo nextest run -p wjsm-runtime` → 134 passed, 2 skipped。
- `cargo build --workspace` → passed。
- wasmparser proof in `crates/wjsm-backend-wasm/tests/gc_alloc_window.rs` checks support `obj_new`/`arr_new` bodies for `global.get 19` (`__alloc_ptr`), `global.get 20` (`__alloc_end`), `global.set 19`, `global.set 1` (`__heap_ptr` sync), `global.get/set 21` (`__gc_alloc_bytes`), `call gc_alloc_slow`, and absence of `call gc_maybe_collect`.
- Source confirmation: grep for backend calls to `gc_maybe_collect` now only finds the host import registry entry, not allocation helper callsites.
- Residual failure during slice: `fragmentation_churn_survivors_intact` initially panicked on `heap_type 0x02`; root cause was GC layout owner only treating OBJECT/ARRAY/ARGUMENTS as object-like, while runtime object tags PROMISE/CONTINUATION/ASYNC_GENERATOR share object header layout. Fixed in `runtime_gc/context.rs` and covered by unit test `gc_layout_treats_runtime_object_tags_as_object_like` plus runtime package run.

## P1 T1.4 evidence

- `cargo check -p wjsm-backend-wasm -p wjsm-runtime -p wjsm-runtime-support` → passed。
- `cargo nextest run -p wjsm-backend-wasm -E 'test(support_alloc_helpers_use_alloc_window_and_safepoint_poll)'` → passed。
- `cargo nextest run -p wjsm-backend-wasm -E 'test(host_imports_count_locked)'` → passed。
- `cargo nextest run -p wjsm-runtime -E 'test(fragmentation_churn_survivors_intact)'` → passed。
- `cargo nextest run -p wjsm-backend-wasm` → 54 passed。
- `cargo nextest run -p wjsm-runtime` → 133 passed, 2 skipped。
- `cargo nextest run -p wjsm-runtime-support` → 7 passed。
- `cargo build --workspace` → passed。
- `grep` over `crates` for `gc_maybe_collect|GcMaybeCollect|alloc_counter|gc_threshold|bump_alloc_counter|reset_alloc_counter|update_gc_threshold` → no matches。
- Runtime residual during slice: first T1.4 poll placement caused `fragmentation_churn_survivors_intact` out-of-bounds. Root cause was polling after `obj_new`/`arr_new` registered a fresh object but before the returned handle reached caller-visible roots. Fixed by polling at helper entry (debt from previous allocations) and before object resize allocation, not after fresh allocation.

## P1 T1.5 evidence

- `cargo check -p wjsm-backend-wasm -p wjsm-runtime-support -p wjsm-runtime -p wjsm-cli` → passed。
- `cargo nextest run -p wjsm-runtime-support --features embedded` → 9 passed。
- `cargo nextest run -p wjsm-backend-wasm -E 'test(support_alloc_helpers_use_alloc_window_and_safepoint_poll)'` → 1 passed。
- `cargo nextest run -p wjsm-backend-wasm` → 55 passed。
- `cargo nextest run -p wjsm-runtime` → 133 passed, 2 skipped。
- `cargo nextest run -p wjsm-cli --no-tests warn` → 3 passed, 52 skipped。
- `cargo build --workspace` → passed。
- `grep` over `crates` for `support_module_layout_hash|wjsm_support_g1|wjsm_support_zgc|EMBEDDED_G1|EMBEDDED_ZGC|emit_support_module\(\)|OnceLock<regex` → no matches。
- Rule compliance fix during slice: runtime support default artifact uses `LazyLock` for fixed initializer and keeps `OnceLock` only for explicit runtime injection; CLI IR regex caches switched from `OnceLock::get_or_init` to `LazyLock`。
- Variant boundary: `wjsm_backend_wasm::GcFlavor` now names MarkSweep/G1/Zgc, but only MarkSweep emits a support module in T1.5；G1/Zgc return an error and runtime-support exposes no fake `wjsm_support_g1/zgc.cwasm` artifacts until their later phases。
- Artifact coverage: build.rs precompiles only `wjsm_support_mark_sweep.cwasm`；embedded tests deserialize mark-sweep and assert G1/Zgc artifacts are absent。

## P1 T1.6 evidence

- `cargo check -p wjsm-runtime -p wjsm-backend-wasm -p wjsm-runtime-support -p wjsm-cli` → passed。
- `cargo nextest run -E 'test(happy__typedarray_simple) | test(happy__map_set_for_each) | test(happy__error_constructor_new_target) | test(happy__symbol_prototype_methods)'` → 4 passed。
- `WJSM_STARTUP_SNAPSHOT=0 cargo nextest run -E 'test(happy__error_constructor_new_target)'` → passed。
- `WJSM_STARTUP_SNAPSHOT=0 cargo nextest run -E 'test(happy__symbol_prototype_methods)'` → passed。
- `WJSM_STARTUP_SNAPSHOT=0 cargo nextest run -E 'test(happy__typedarray_simple)'` → passed。
- `cargo nextest run --workspace` → 1242 passed, 2 skipped。
- `WJSM_STARTUP_SNAPSHOT=0 cargo nextest run --workspace` → 1242 passed, 2 skipped。
- `cargo build --workspace` → passed。
- T1.6 修复证据：fixture 验证暴露 host 侧直接 bump `__heap_ptr` 后没有同步 `__alloc_ptr`，导致后续 WASM `arr_new` fast-path 覆盖 host 分配的 property/string 区域；已在 `alloc_heap_c_string_global`、render string allocation、eval var map allocation 同步 `__alloc_ptr`。
- T1.6 修复证据：support/user helper 的 `gc_safepoint_poll` 现在同时要求 `__bootstrap_done` 与 `__function_props_done`，避免 bootstrap/function-props 构造期在没有普通 IR spill 的路径触发 GC。
- T1.6 修复证据：cold startup 期在 GC attach 前没有可靠 roots，`gc_alloc_slow` 与 host allocation 在 `dynamic_heap_start == 0` 时改为 no-GC bump/grow，避免 bootstrap/host primordial 被过早 sweep/reuse。
- T1.6 修复证据：cold startup 在 host prototype 初始化前显式执行 `__wjsm_init_function_props`，避免 main 入口首次执行时把 `obj_table_count` 回退到 `function_props_base` 并覆盖 Error/Symbol prototypes。
- T1.6 修复证据：Error constructor 使用已有 receiver 时只在 receiver 当前原型仍是 `Object.prototype` 时补设对应 Error prototype，保留 `extends TypeError` / `Reflect.construct(..., newTarget)` 已建立的自定义 receiver prototype。

## P2 T2.1 evidence

- 新增 `crates/wjsm-runtime/src/runtime_gc/heap_access.rs`：`HeapPtr` debug epoch、`SlotPart`、`resolve`、`write_property_slot`、`write_element`、`write_proto`，并在 `runtime_gc/mod.rs` 挂载。
- `GcContext::increment_gc_epoch()` 已加入；mark-sweep 完整 sweep 与 lazy sweep progress/complete 路径递增 epoch，保证 host `HeapPtr` debug 断言能发现跨 GC 点 raw ptr。
- `cargo check -p wjsm-runtime` → passed（zero warnings）。
- `cargo nextest run -p wjsm-runtime` → 135 passed, 2 skipped。

## P2 T2.2 evidence

- 新增 `docs/aegis/work/2026-07-03-gc-v2/bare-write-audit.md`。
- `grep` pattern `HEAP_OBJECT_PROPERTY|HEAP_ARRAY_ELEMENT|HEAP_OBJECT_PROTO_OFFSET` over `crates/wjsm-runtime/src` → 已归类到主 offset 清单。
- `grep` pattern `copy_from_slice(&proto|copy_from_slice(&proto_handle|ptr..ptr + 4|PROTO_OFFSET` over `crates/wjsm-runtime/src` → 已归类到 proto header 交叉清单。
- `grep` pattern `setPrototypeOf|Object.create|Reflect.setPrototypeOf|__proto__|prototype` over runtime host paths → 已归类 prototype API 入口与只读原型链遍历。
- 清单明确区分待替换写点、只读短窗口、对象初始化/元数据写、非 JS 对象堆写入。

## P2 T2.3 evidence

- `runtime_heap.rs`: `set_object_proto_header` 改经 `heap_access::write_proto`；host object 初始化 proto 改经 `heap_access::init_proto_at_ptr`（对象发布前无 barrier）。
- `runtime_values.rs`: `write_array_elem_with_env` 改经 `heap_access::write_element_at_ptr`；`write_object_property_by_name_id` 与 `write_private_accessor_slot` 的 value/getter/setter 子槽改经 `heap_access::write_property_slot`，并在 grow 后重新 resolve 当前 object ptr。
- `runtime_builtins.rs` / `host_imports/core.rs` 本批审计无对象槽裸写替换点。
- `cargo check -p wjsm-runtime` → passed（zero warnings）。
- `cargo nextest run -p wjsm-runtime` → 135 passed, 2 skipped。
- `cargo nextest run --workspace` → 1244 passed, 2 skipped。

## P2 T2.4 evidence

- `host_imports/array_object.rs`: `object_write_proto_handle` 改经 `heap_access::write_proto`；DefineProperty existing-slot 的 value/getter/setter 改经 `heap_access::write_property_slot`，flags 保持元数据写。
- `host_imports/collections_buffers.rs`: private existing-slot value 写改经 `heap_access::write_property_slot`。
- typedarray/streams 审计命中为 backing store、ArrayBufferEntry 或 RuntimeState 侧表写，不属于 JS 对象 heap slot。
- `cargo check -p wjsm-runtime` → passed（zero warnings）。
- `cargo nextest run --workspace` → 1244 passed, 2 skipped。

## P2 T2.5 evidence

- `runtime_host_helpers/host_helpers_alloc.rs`: array/object 初始化 proto 改经 `heap_access::init_proto_at_ptr`；`set_array_elem_with_env` 改经 `heap_access::write_element`。
- `host_imports/generator.rs` / `async_generator.rs` / `object_builtins.rs` / `proxy_reflect.rs`: proto header 写改经 `heap_access::write_proto` 或 `set_object_proto_header`。
- `runtime_host_helpers/host_helpers_property.rs` / `host_helpers_proxy.rs`: data/accessor 属性 value/getter/setter 写改经 `heap_access::write_property_slot`。
- `runtime_values.rs`: descriptor object、object rest/spread 的引用槽写改经 `heap_access::write_property_slot` 或复用 `write_object_property_by_name_id`。
- 剩余 slot/proto grep 命中为只读访问、`heap_access` 内部、测试 buffer 构造或 resize/obj_table 更新（T2.6）。
- `cargo check -p wjsm-runtime` → passed（zero warnings）。
- `cargo nextest run --workspace` → 1244 passed, 2 skipped。

## P2 T2.6 evidence

- `compiler_helpers/helpers_object.rs` / `support_object_helpers.rs`: object resize 在 allocation helper 返回后、`memory.copy` 前，从 `obj_table[handle]` 重新读取 old_ptr，避免 slow-path GC/move 后复用旧 raw ptr。
- `runtime_values.rs`: `grow_array` / `grow_object` 在 `alloc_heap_region_for_host` 后通过 handle 重新解析旧 ptr，并用重新解析的 ptr 执行 copy/abandon。
- `runtime_host_helpers/host_helpers_proxy.rs`: proxy grow object 在 host allocation 后通过 handle 重新解析旧 ptr，再 copy/update obj_table。
- `crates/wjsm-backend-wasm/tests/gc_alloc_window.rs`: 新增 support `obj_set` 结构测试，断言 resize `memory.copy` 前存在 obj_table re-resolve 序列。
- `cargo check -p wjsm-runtime -p wjsm-backend-wasm` → passed（zero warnings）。
- `cargo nextest run -p wjsm-backend-wasm` → 56 passed。
- `cargo nextest run -p wjsm-runtime` → 135 passed, 2 skipped。

## P2 T2.7 evidence

- P2 scope closed: `bare-write-audit.md` 已勾销 host 引用槽裸写迁移与 resize re-resolve 项，剩余裸写分类为 `heap_access` owner 内部、初始化/元数据写、只读短窗口或非 JS 对象堆写入。
- `cargo nextest run --workspace` → 1245 passed, 2 skipped。
- `WJSM_STARTUP_SNAPSHOT=0 cargo nextest run -E 'test(happy__)'` → 588 passed, 148 skipped。
- `cargo build` → passed（zero warnings）。

## P3 T3.0 evidence

- `RuntimeOptions` 新增 `gc_algorithm`，默认 `mark-sweep`；`gc_algorithm_from_env` 读取 `WJSM_TEST_GC`，未知值复用 registry 错误并列出 `mark-sweep, g1, zgc`。
- `RuntimeState::new_with_shared_and_options` 按 `RuntimeOptions.gc_algorithm` 创建 registry 算法；未实现的 `g1`/`zgc` 仍由 registry 显式拒绝，不提供行为 stub。
- CLI 与 in-process fixture runner 均从运行时 env snapshot/overrides 写入 `RuntimeOptions.gc_algorithm`。
- `cargo fmt` → passed。
- `cargo check -p wjsm-cli -p wjsm-runtime` → passed（zero warnings）。
- `cargo nextest run -p wjsm-runtime -E 'test(gc_algorithm_env)'` → 2 passed, 137 skipped。
- `WJSM_TEST_GC=mark-sweep cargo nextest run -E 'test(happy__hello)'` → 1 passed, 735 skipped。
- `WJSM_TEST_GC=bogus cargo run -- run -e 'console.log(1)'` → exited 1 with `unknown GC algorithm \`bogus\`; expected one of: mark-sweep, g1, zgc`。

## P3 T3.1 evidence

- 新增 `runtime_gc::g1::{mod,region}`：`RegionSpace` 以 `object_heap_start` 为基准维护 host-side region metadata，支持 immortal/free/eden/humongous 显式状态、region/card index、grow 扩展与 metadata footprint 计算。
- `G1Collector` 接入 v2 `GcAlgorithm` 生命周期：`attach_heap` 建立 region 域并安装首个 Eden 分配窗口；slow alloc/safepoint/full collect 暂时复用 mark-sweep 行为并同步 region metadata，不实现 RSet/barrier/young GC。
- `registry::create(G1)` 改为创建 `G1Collector`；ZGC 保持显式未实现错误。
- `cargo fmt` → passed。
- `cargo check -p wjsm-runtime` → passed（zero warnings）。
- `cargo nextest run -p wjsm-runtime -E 'test(g1)'` → 4 passed, 139 skipped。
- `WJSM_TEST_GC=g1 cargo nextest run -E 'test(happy__hello)'` → 1 passed, 735 skipped。

## P3 T3.2 evidence

- 新增 `runtime_gc::g1::rset`：`BarrierEvent` 固定 24B `{flags:u32, slot_addr:u32, old_value:i64, new_value:i64}` 编解码；SATB 记录旧槽位 NaN-boxed value 解出的 handle；dirty card 使用 sparse set，热点 card 升级为 precise-slot set。
- `G1Collector::on_host_write` 经 `heap_access` hook 记录 host 写；按 target handle / slot_addr / new_value 解析 owner 与 young edge，使用 event 自带值而不是重读当前 slot。
- `G1Collector::barrier_flush` 解码 `[barrier_base, __barrier_buf_ptr)` 的 24B event，通过 slot_addr 反查 owner 对象范围并记录 RSet/SATB，最后把 `__barrier_buf_ptr` 重置到 runtime 记录的 buffer base。
- T3.2 范围内未修改 support emitter；WASM 侧 event 生成留给 T3.3。
- `cargo fmt` → passed。
- `cargo check -p wjsm-runtime` → passed（zero warnings）。
- `cargo nextest run -p wjsm-runtime -E 'test(g1)'` → 11 passed, 139 skipped。
- `WJSM_TEST_GC=g1 cargo nextest run -E 'test(happy__hello)'` → 1 passed, 735 skipped。
