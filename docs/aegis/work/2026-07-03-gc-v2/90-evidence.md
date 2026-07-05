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

## P3 T3.3 evidence

- `support_module.rs` 现在可 emit `GcFlavor::G1` support module；`GcFlavor::Zgc` 仍显式拒绝，避免伪 artifact。
- `support_object_helpers.rs::emit_obj_set` 与 `support_module.rs::emit_elem_set` 在 G1 flavor 下写入 24B barrier event：空间不足先 `gc_barrier_flush`，写入 flags/slot_addr/old_value/new_value，推进 `__barrier_buf_ptr`；mark-sweep flavor 不生成该序列。
- `wjsm-runtime-support` build.rs 预编译 `wjsm_support_mark_sweep.cwasm` 与 `wjsm_support_g1.cwasm`；ABI 可用 flavor 更新为 `mark-sweep,g1`，ZGC 保持 absent。
- `runtime_startup.rs` 按当前 `GcAlgorithm` 名称选择 support flavor；`embedded_support_cwasm_for(G1)` 使用 G1 embedded artifact，mark-sweep 默认路径保持不变。
- 调试修复：`WJSM_TEST_GC=g1 cargo nextest run -E 'test(happy__)'` 首次暴露 4 个 fixture 失败，根因为 G1 skeleton 在 young GC 未实现前切换后续 Eden region，破坏 mark-sweep fallback 的 contiguous heap 扫描假设；T3.3 将 `G1Collector::alloc_slow` 在 young GC 交付前保持委托 mark-sweep 连续分配，仅同步 region metadata。
- `cargo fmt` → passed。
- `cargo check -p wjsm-backend-wasm -p wjsm-runtime-support -p wjsm-runtime` → passed（zero warnings）。
- `cargo nextest run -p wjsm-backend-wasm` → 60 passed。
- `cargo nextest run -p wjsm-runtime-support` → 9 passed。
- `WJSM_TEST_GC=g1 cargo nextest run -E 'test(happy__)'` → 588 passed, 148 skipped。

## P3 T3.4 evidence

- 新增 `runtime_gc::object_walker` 作为对象引用槽遍历 owner；mark-sweep marker 改用同一 walker，避免 G1/ZGC 复制 proto/property/array/side-table-backed 引用扫描逻辑。
- 新增 `runtime_gc::g1::young`：STW young collection 从 shadow/host roots、immortal 对象引用槽、dirty card/precise slot 收集 young roots，复制 Eden/Survivor live 对象，按 age 晋升 Old 或保留 Survivor，并更新 obj_table。
- G1 allocation slow path 使用当前 Eden bump window；window 不足时先 young collect，再按需 grow region metadata/linear memory，避免 T3.3 暂态中每次 host 分配占用整 region。
- `G1RSet` 扩充 dirty card snapshot/clear 与 card re-dirty helper；young GC 在 dirty 槽仍指向 young 或 promoted destination 落在 card 上时重新标脏。
- Weak/side-table cleanup 在 freed handle 归还前执行；freed young handles 先清理 side tables，再写 obj_table=0 并归还 handle free-list。
- 新增 `fixtures/happy/gc_g1_young_churn.js`，覆盖长期 root 通过已有 property 与固定数组元素反复指向 young child；默认 mark-sweep 与 `WJSM_TEST_GC=g1` 都通过。
- 调试修复：初版 young allocation 每次 `alloc_slow` 都取新 Eden region，导致 host 分配过度消耗 region 并触发大量 fixture 失败；根因是 host 分配也走 `alloc_slow`，修正为优先在当前 Eden bump 指针内分配。
- `cargo fmt` → passed。
- `cargo check -p wjsm-runtime` → passed（zero warnings）。
- `cargo nextest run -p wjsm-runtime` → 150 passed, 2 skipped。
- `cargo nextest run -E 'test(happy__gc_g1_young_churn)'` → 1 passed, 736 skipped。
- `WJSM_TEST_GC=g1 cargo nextest run -E 'test(happy__gc_g1_young_churn)'` → 1 passed, 736 skipped。
- `cargo nextest run -E 'test(happy__)'` → 589 passed, 148 skipped。
- `WJSM_TEST_GC=g1 cargo nextest run -E 'test(happy__)'` → 589 passed, 148 skipped。
- `cargo nextest run --workspace` → 1265 passed, 2 skipped。

## P3 T3.5 evidence

- 新增 `runtime_gc::g1::concurrent_mark`：IHOP 以 old/humongous occupancy/heap_limit 触发；状态机包含 Idle/Mark、cycle epoch、mark bitmap、worklist、budgeted drain 与 final remark。
- `G1Collector::safepoint_step` 现在在 IHOP 触发时启动 initial mark，按 `StepBudget` drain；ready 后 flush barrier、重扫 roots fixed-point、吸收 SATB，再执行 cleanup。
- `G1RSet` 的 SATB handles 被 concurrent mark drain；host roots/immortal roots/object_walker 子引用进入同一 mark bitmap，implicit-black region 在本 cycle 中不被 cleanup 回收。
- cleanup 统计 region live bytes，释放全死 old/humongous region；dead handles 经 obj_table 清零、owner side-table reclaim、WeakRef/stream cleanup 后才发布到 handle free-list，避免 cleanup 后 handle 复用顺序错误。
- 新增 `fixtures/happy/gc_g1_concurrent_mark_churn.js`，覆盖显式 GC 后 old reference 保活与断开后的 cleanup 路径；默认与 `WJSM_TEST_GC=g1` 都通过。
- `cargo fmt` → passed。
- `cargo check -p wjsm-runtime` → passed（zero warnings）。
- `cargo nextest run -p wjsm-runtime -E 'test(g1)'` → 22 passed, 128 skipped。
- `cargo nextest run -E 'test(happy__gc_g1_concurrent_mark_churn)'` → 1 passed, 737 skipped。
- `WJSM_TEST_GC=g1 cargo nextest run -E 'test(happy__gc_g1_concurrent_mark_churn)'` → 1 passed, 737 skipped。
- `WJSM_TEST_GC=g1 cargo nextest run -E 'test(happy__weakref_gc)'` → 1 passed, 737 skipped。
- `cargo nextest run -E 'test(happy__)'` → 590 passed, 148 skipped。
- `WJSM_TEST_GC=g1 cargo nextest run -E 'test(happy__)'` → 590 passed, 148 skipped。
- `cargo nextest run --workspace` → 1272 passed, 2 skipped。

## P3 T3.6 evidence

- 新增 `runtime_gc::g1::mixed`：CSet 按 live bytes 升序选择，受 copy budget 截断，跳过 live bytes 超过 85% region。
- mixed evacuation 在 to-space 中复制 old/humongous live object 原始字节，更新 `obj_table[h]` 指向新地址；对象内部引用槽保持 handle 原值，不做 per-reference 修正。
- source region release 后清理对应 RSet card；destination object 扫描后若仍含 young handle，重新标脏 destination card。
- mixed 只压缩空间：dead handle cleanup 仍由 concurrent mark owner 负责，mixed 不清 obj_table、不发布 handle。
- `G1Collector::safepoint_step` 在 remark/cleanup 后执行 budgeted mixed step；`collect_full` 循环 mixed 到候选耗尽或 to-space 受阻。
- runtime governance 单测新增较短 G1 churn；完整 `gc_fragmentation_churn` fixture 仍由 `WJSM_TEST_GC=g1` integration test 覆盖。
- `cargo fmt` → passed。
- `cargo check -p wjsm-runtime` → passed（zero warnings）。
- `cargo nextest run -p wjsm-runtime` → 163 passed, 2 skipped。
- `cargo nextest run -p wjsm-runtime -E 'test(g1)'` → 29 passed, 136 skipped。
- `WJSM_TEST_GC=g1 cargo nextest run -E 'test(happy__gc_fragmentation_churn)'` → 1 passed, 737 skipped。
- `cargo nextest run -E 'test(happy__)'` → 590 passed, 148 skipped。
- `WJSM_TEST_GC=g1 cargo nextest run -E 'test(happy__)'` → 590 passed, 148 skipped。
- `cargo nextest run --workspace` → 1279 passed, 2 skipped。

## P3 T3.7 evidence

- 审计 `G1Collector` v2 hooks：`alloc_slow` flush barrier → young alloc/collect → mixed fallback；`safepoint_step` 派发 concurrent mark、remark、mixed；`collect_full` 执行 young + full mark + mixed 到候选耗尽或 to-space 受阻；`on_host_write`/`barrier_flush` 均接入 RSet/SATB。
- `runtime_gc::registry::create(G1)` 已返回 `G1Collector::new()`；`Zgc` 保持显式未实现错误并列出当前 available `mark-sweep, g1`。
- `cargo check -p wjsm-runtime` → passed（zero warnings）。
- `cargo nextest run -p wjsm-runtime -E 'test(g1)'` → 29 passed, 136 skipped。
- `WJSM_TEST_GC=g1 cargo nextest run -E 'test(happy__)'` → 590 passed, 148 skipped。

## P3 T3.8 evidence

- P3 G1 阶段 closure：T3.0-T3.7 已完成并验证，默认 mark-sweep 与 `WJSM_TEST_GC=g1` 路径均保持绿。
- `cargo nextest run -E 'test(gc_)'` → 14 passed, 724 skipped。
- `WJSM_TEST_GC=g1 cargo nextest run --workspace` → 1279 passed, 2 skipped。
- `cargo build` → passed（zero warnings）。
- DriftCheckDraft：Scope=P3 G1；Compatibility=默认 mark-sweep 未变且 G1 workspace 全绿；Retirement=G1 registry 拒绝路径、单 support cwasm 假设、host/WASM 无 barrier 记录假设、young-only/old-never-compact 临时限制均已退休；Decision=continue to P4。

## P4 T4.1 evidence

- 新增 `runtime_gc::zgc::color`：`ZEntry` 以低 2 bit 承载 `Empty/Marked0/Marked1/Remapped`，`ZColorState` 支持 Marked0/Marked1 双 good 切换与 relocate 期 `good=11`。
- 新增 `runtime_gc::zgc::page`：`ZPageSpace` 使用 host-side page metadata，按 dynamic heap start 计算 page index/grow，metadata 不写入 wasm dynamic heap；支持 live bytes、relocation set 与全死 page immediate reclaim。
- `recolor_live_obj_table_entries` 作为 attach/restore owner helper，将非空 obj_table entry 统一 recolor 到当前 good，保证 live entry 不保持 `00`。
- T4.1 未打开 ZGC registry：计划 T4.2 生成 ZGC support cwasm 后再执行 `WJSM_TEST_GC=zgc` 冒烟，避免在无 load barrier support 时提供伪运行路径。
- `cargo fmt` → passed。
- `cargo check -p wjsm-runtime` → passed（zero warnings）。
- `cargo nextest run -p wjsm-runtime -E 'test(zgc)'` → 11 passed, 164 skipped。
- `cargo nextest run -p wjsm-runtime` → 173 passed, 2 skipped。

## P4 T4.2 evidence

- `emit_support_module(GcFlavor::Zgc)` 已生成 ZGC support；`wjsm-runtime-support` 现在产出并暴露 `wjsm_support_zgc.cwasm`，ABI 可用 flavor 为 `mark-sweep,g1,zgc`。
- ZGC support helper 的 handle 解引用经统一 load barrier 序列：检查 low-bit color、必要时调用 `gc_load_barrier_slow`，并在 helper 内去除 low 2 bit 后作为真实 ptr 使用。
- `obj_set` / `elem_set` 在 ZGC flavor 下复用统一 24B barrier buffer 记录 SATB event；结构测试证明无独立 `__satb_ptr`。
- `obj_new` / `arr_new` 在 ZGC support 下写入 `ptr | __good_color`，runtime `gc_load_barrier_slow` 和 `runtime_values::resolve_handle_idx_with_env` 使用 `heap_access::resolve` 修复 host colored ptr。
- `registry::create(Zgc)` 已打开，runtime startup 按当前 GC flavor 选择 mark-sweep / G1 / ZGC embedded support artifact。
- 调试修复：`WJSM_TEST_GC=zgc cargo nextest run -E 'test(happy__)'` 首次暴露 class/new 与 eval 失败；根因是 compiler inline/eval helper 仍直接使用 colored obj_table ptr。已在 `SetProto`、inline array helpers、inline object helpers中对 obj_table entry 去色，保持 eval mode 可运行。
- 调试修复：support flavor 改变导致 stack-overflow synthetic wasm frame source-map 行号随 flavor 漂移；测试 harness 现在仅归一化无列号的 generated anonymous wasm frame，保留真实 JS frame（如 `recurse:2:1`）。
- `cargo fmt` → passed。
- `cargo check -p wjsm-backend-wasm -p wjsm-runtime-support -p wjsm-runtime` → passed（zero warnings）。
- `cargo nextest run -p wjsm-backend-wasm -E 'test(support)'` → 14 passed, 49 skipped。
- `cargo nextest run -p wjsm-runtime-support --features embedded` → 9 passed。
- `WJSM_TEST_GC=zgc cargo nextest run -E 'test(happy__hello)'` → 1 passed, 737 skipped。
- `WJSM_TEST_GC=zgc cargo nextest run -E 'test(happy__)'` → 590 passed, 148 skipped。
- `cargo nextest run --workspace` → 1292 passed, 2 skipped。
- `WJSM_TEST_GC=zgc cargo nextest run --workspace` → 1292 passed, 2 skipped。
- `WJSM_TEST_GC=g1 cargo nextest run -E 'test(happy__)'` → 590 passed, 148 skipped。
- `cargo build` → passed（zero warnings）。

## P4 T4.3 evidence

- 新增 `runtime_gc::zgc::mark`：维护 mark bitmap、worklist、SATB handles、dead_handle_set 与 page live-bytes 统计；MarkStart 切换当前 mark good color并 snapshot roots，MarkEnd flush SATB + fixed-point 重扫 host/side-table roots。
- `ZgcCollector::safepoint_step` 按 `StepBudget` drain；`load_barrier_slow` 在 Mark phase repair bad color 到当前 good 并把 handle 入 mark worklist，实现 load barrier assisted marking。
- `barrier_flush` / `on_host_write` 消费统一 24B barrier buffer 与 host old value，作为 SATB 输入；flush 只 drain/reset，不触发 collect/grow/move。
- MarkEnd 生成 dead_handle_set 并执行 cleanup：清 obj_table slot、owner side-table reclaim、WeakRef/FinalizationRegistry enqueue、stream side-table cleanup 之后才发布 handles 到 free-list。
- `object_walker::resolve_handle` 去除低 2 bit color，保证共享 root/object 扫描能读取 ZGC colored obj_table entries。
- 调试修复：第一轮 ZGC mark 不能继续使用 attach 后的 Marked0 good；`ZColorState` 默认 `good=Marked0,next_mark=Marked1`，否则未访问对象因旧色等于本周期 good 被误保活，WeakRef/FinalizationRegistry fixture 不会 cleanup。
- `cargo fmt` → passed。
- `cargo check -p wjsm-runtime` → passed（zero warnings）。
- `cargo nextest run -p wjsm-runtime -E 'test(zgc)'` → 15 passed, 165 skipped。
- `WJSM_TEST_GC=zgc cargo nextest run -E 'test(happy__hello)'` → 1 passed, 737 skipped。
- `WJSM_TEST_GC=zgc cargo nextest run -E 'test(happy__weakref_gc) + test(happy__finalization_registry_cleanup) + test(happy__gc_map_set_owner_reachability) + test(happy__weak_collections_gc)'` → 4 passed, 734 skipped。
- `WJSM_TEST_GC=zgc cargo nextest run -E 'test(happy__)'` → 590 passed, 148 skipped。
- `cargo nextest run -p wjsm-runtime` → 178 passed, 2 skipped。
- `cargo nextest run --workspace` → 1297 passed, 2 skipped。
- `WJSM_TEST_GC=zgc cargo nextest run --workspace` → 1297 passed, 2 skipped。
- `cargo build` → passed（zero warnings）。

## P4 T4.4 evidence

- 新增 `runtime_gc::zgc::relocate`：Relocate phase 使用 `good=Remapped`，按 page live bytes/fragmentation 选择 relocation set，主动搬迁与 load-barrier/host-resolve 协助搬迁共享同一 copy/heal owner。
- relocation set 跳过当前 active allocation page，避免 mutator 继续向被搬迁源页写入；候选对象在 cycle start 缓存，`alloc_slow` 仅按小 budget 补步进，避免每次分配重新全表扫描导致 `gc_fragmentation_churn` 超时。
- 搬迁路径复制对象原始 bytes 到目标 zPage，写 `obj_table[h]=new_ptr|Remapped`；对象内部引用槽保持 handle，不做 per-reference rewrite。
- source page reclaim 只在扫描确认没有 live obj_table entry 落在源页时释放；RelocateStep 不发布 dead handles、不调用 WeakRef/side-table cleanup。
- 调试修复：T4.4 初版 `WJSM_TEST_GC=zgc happy__gc_typedarray_dataview_side_refs` 输出 `undefined`，根因是 relocation 选中了当前分配页并在 mutator 仍写该页时回收源页；跳过 active allocation page 后 fixture 恢复。
- 调试修复：T4.4 初版 `WJSM_TEST_GC=zgc happy__gc_fragmentation_churn` 超时，根因是 alloc slow path 对 relocation 全量 drain 且每轮重新扫描 obj_table；改为缓存候选 + allocation budgeted step 后恢复。
- `cargo fmt` → passed。
- `cargo check -p wjsm-runtime` → passed（zero warnings）。
- `cargo nextest run -p wjsm-runtime -E 'test(zgc)'` → 22 passed, 165 skipped。
- `WJSM_TEST_GC=zgc cargo nextest run -E 'test(happy__gc_typedarray_dataview_side_refs)'` → 1 passed, 737 skipped。
- `WJSM_TEST_GC=zgc cargo nextest run -E 'test(happy__gc_fragmentation_churn)'` → 1 passed, 737 skipped。
- `WJSM_TEST_GC=zgc cargo nextest run -E 'test(happy__)'` → 590 passed, 148 skipped。
- `cargo nextest run --workspace` → 1304 passed, 2 skipped。
- `WJSM_TEST_GC=zgc cargo nextest run --workspace` → 1304 passed, 2 skipped。
- `WJSM_TEST_GC=g1 cargo nextest run -E 'test(happy__)'` → 590 passed, 148 skipped。
- `cargo build` → passed（zero warnings）。

## P4 T4.5 evidence

- 审计 `ZgcCollector` v2 hooks：`alloc_slow` flush barrier → Mark/Relocate budgeted progress → fallback allocation；`safepoint_step` 派发 Mark/Relocate；`collect_full` 同步完整 mark+relocate 周期；`on_host_write`/`barrier_flush` 进入 SATB；`load_barrier_slow` 在 Mark phase 协助标记、Relocate phase 协助 heal。
- `runtime_gc::registry::create(Zgc)` 已返回 `ZgcCollector::new()`；无残余 ZGC 未实现路径。
- `cargo check -p wjsm-runtime` → passed（zero warnings）。
- `cargo nextest run -p wjsm-runtime -E 'test(zgc)'` → 22 passed, 165 skipped。
- `WJSM_TEST_GC=zgc cargo nextest run -E 'test(happy__)'` → 590 passed, 148 skipped。

## P4 T4.6 evidence

- P4 ZGC 阶段 closure：T4.1-T4.5 已完成并验证；默认 mark-sweep、G1 与 ZGC 路径均保持绿。
- `cargo nextest run -E 'test(gc_)'` → 14 passed, 724 skipped。
- `WJSM_TEST_GC=zgc cargo nextest run -E 'test(gc_)'` → 14 passed, 724 skipped。
- `cargo nextest run --workspace` → 1304 passed, 2 skipped。
- `WJSM_TEST_GC=zgc cargo nextest run --workspace` → 1304 passed, 2 skipped。
- `WJSM_TEST_GC=g1 cargo nextest run -E 'test(happy__)'` → 590 passed, 148 skipped。
- `cargo build` → passed（zero warnings）。
- DriftCheckDraft：Scope=P4 ZGC；Compatibility=默认 mark-sweep、G1 happy、ZGC workspace 全绿；Retirement=ZGC registry 拒绝路径、无 support 变体、无 mark、无 relocate/heal 的临时限制均已退休；Decision=continue to P5。

## P5 T5.1 evidence

- `RuntimeOptions` 新增 `with_gc_algorithm` 与 `set_gc_algorithm` builder/mutator；`RuntimeOptions.gc_algorithm` 继续作为 runtime 选择 source of truth。
- env 选择链改为 `WJSM_TEST_GC`（测试矩阵兼容）→ `WJSM_GC` → 默认 `mark-sweep`。
- CLI 新增全局 `--gc <mark-sweep|g1|zgc>`，运行时优先级为 CLI `--gc` > `WJSM_TEST_GC`/`WJSM_GC` > 默认。
- `cargo fmt` → passed。
- `cargo check -p wjsm-cli -p wjsm-runtime` → passed（zero warnings）。
- `cargo nextest run -p wjsm-runtime -E 'test(gc_algorithm)'` → 5 passed, 185 skipped。
- `cargo run -- --gc g1 run -e 'console.log(1)'` → stdout `1`。
- `WJSM_GC=zgc cargo run -- run -e 'console.log(2)'` → stdout `2`。
- `WJSM_GC=bogus cargo run -- --gc mark-sweep run -e 'console.log(3)'` → stdout `3`，证明 CLI 覆盖 env。
- `WJSM_TEST_GC=bogus cargo run -- --gc zgc run -e 'console.log(5)'` → stdout `5`，证明 CLI 覆盖测试 env。
- `WJSM_GC=bogus cargo run -- run -e 'console.log(4)'` → exit 1 with `unknown GC algorithm \`bogus\`; expected one of: mark-sweep, g1, zgc`。
- `WJSM_GC=g1 cargo nextest run -E 'test(happy__hello)'` → 1 passed, 737 skipped。
- `WJSM_GC=zgc cargo nextest run -E 'test(happy__hello)'` → 1 passed, 737 skipped。
- `WJSM_GC=bogus WJSM_TEST_GC=g1 cargo nextest run -E 'test(happy__hello)'` → 1 passed, 737 skipped。

## P5 T5.2 evidence

- `GcStats` 扩展 spec §17 字段：cycle_kind、pause_ns_max/total/count、relocated_bytes/objects、committed_pages、free_bytes_reusable、region/page/RSet/SATB/load-barrier 计数；保留既有 mark/sweep/free/fragmentation 字段。
- mark-sweep/G1/ZGC 均从真实 owner 填充统计：mark-sweep barrier/relocation/load-barrier 为 0；G1 从 RSet/barrier/region/mixed owner 填充；ZGC 从 page/mark/relocate/load-barrier owner 填充。
- `RuntimeState` 新增最近 256 次 pause 环形缓冲；`store_last_gc_stats` 在观测到 pause 时推进 hist，并在 `WJSM_GC_LOG=1` 时输出 algorithm/cycle/pause/relocated/barrier/rset/load_barrier 摘要。
- `cargo fmt` → passed。
- `cargo check -p wjsm-runtime` → passed（zero warnings）。
- `cargo nextest run -p wjsm-runtime -E 'test(gc_stats) | test(pause_hist) | test(zgc) | test(g1)'` → 56 passed, 139 skipped。
- `WJSM_GC_LOG=1 WJSM_GC=mark-sweep cargo run -- run -e 'gc(); console.log("ok")'` → 输出 GC 摘要并 stdout `ok`。
- `WJSM_GC_LOG=1 WJSM_GC=g1 cargo run -- run -e 'gc(); console.log("ok")'` → 输出 GC 摘要（含 barrier_events=601）并 stdout `ok`。
- `WJSM_GC_LOG=1 WJSM_GC=zgc cargo run -- run -e 'gc(); console.log("ok")'` → 输出 GC 摘要并 stdout `ok`。
- `cargo nextest run --workspace` → 1312 passed, 2 skipped。
- `WJSM_GC=g1 cargo nextest run -E 'test(happy__)'` → 590 passed, 148 skipped。
- `WJSM_GC=zgc cargo nextest run -E 'test(happy__)'` → 590 passed, 148 skipped。
- `cargo build` → passed（zero warnings）。
