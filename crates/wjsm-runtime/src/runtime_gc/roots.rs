//! Root 发现（spec §10，IMPL-9 fixed-point）。
//!
//! `RuntimeRoots`：impl RootProvider，扫描 shadow stack + host 侧表。
//!
//! Root 来源：
//! - shadow stack：[stack_base, sp) 每 8B 槽读 i64，解析为 handle（含 closure→env_obj）。
//! - IR function property objects（function_props_base..+num_ir_functions）：永久存活。
//! - primordial 原型（Array/Object/AsyncIterator/AsyncGenerator.prototype）：永久存活。
//! - fixed-point host 侧表（移植自 trace_runtime_side_table_roots_fixed_point）：
//!   microtask_queue（PromiseReaction/ResolveThenable/Callback/Transform/Pull/AsyncResume/
//!   CleanupFinalizationRegistry）、promise_table（state value + reactions）、
//!   continuation_table（非 completed 的 outer_promise + captured_vars）、
//!   reader_table（pending_read_promise/byob_view）、byob_request_table（view/promise）、
//!   stream_controller_table（underlying_source/pull/cancel）、timers（callback）。
//!
//! 值→handle 解析（push_value_roots）：object/array/function → low32 handle；
//! closure → host closures 表 env_obj（递归解析）；native_callable → host 表内部引用
//! （移植自 trace_native_callable_record，P4-blocker #2 同源逻辑）。
//! fixed-point：collect_with_roots 多轮注入（mark → 注入 → mark → until popcount 不变）。
//!
//! # Shadow Stack 协议与 GC 契约
//!
//! ## Shadow Stack 布局
//!
//! Shadow stack 位于 WASM 线性内存的前 64KB（由 `SHADOW_STACK_SIZE` 定义）。
//! 编译器通过 `global.set $shadow_sp` 维护栈指针（以字节为单位）。
//!
//! **不变量 INV-SP**：在任何 GC 安全点（safepoint），shadow stack 的 `[stack_base, sp)` 区间
//! 包含所有活跃的 root 值。GC 通过 `tag_needs_root` 过滤标量值（如 smallint、bool），
//! 只保留真正的 handle。
//!
//! ## Spill 策略
//!
//! 编译器在调用可能触发 GC 的函数前执行 spill prologue：
//!
//! 1. **保存 sp**：`local.set $saved_sp` 保存当前 sp 到局部变量
//! 2. **写入 root 值**：将活跃的对象/数组值写入 `[sp, sp + N*8)` 区间
//! 3. **推进 sp**：`global.set $shadow_sp` 将 sp 推进到 `sp + N*8`
//!
//! 调用返回后执行 spill epilogue：
//!
//! 1. **恢复 sp**：`global.set $shadow_sp` 将 sp 恢复为 `saved_sp`
//!
//! **不变量 INV-C（Compiler Guarantee）**：编译器保证在 GC 期间不修改 shadow stack
//! 中已 spill 的值。这意味着 GC 可以安全地读取 `[stack_base, sp)` 而不担心并发修改。
//!
//! ## 优化策略
//!
//! 编译器采用三层优化减少不必要的 spill（详见 `runtime_gc/mod.rs` 文档）：
//!
//! - **Layer 1（ValueTy 推断）**：通过固定点迭代识别标量值，避免将 number/bool 误判为 handle
//! - **Layer 2（Spill batch）**：批量写入 + immediate offset，减少指令数
//! - **Layer 3（Callee 分析）**：对不触发 GC 的 callee 省略 spill
//!
//! ## GC 期间的行为
//!
//! 当 GC 被触发时（通常在 `obj_new`/`arr_new` 分配时）：
//!
//! 1. **Runtime 调用 `gc_collect`**：通过 host function 调用
//! 2. **RuntimeRoots::for_each_shadow_stack_root**：扫描 `[stack_base, sp)` 区间
//! 3. **tag_needs_root 过滤**：跳过标量值（smallint、bool、undefined 等）
//! 4. **Mark 阶段**：标记所有从 root 可达的对象
//! 5. **Sweep 阶段**：回收未标记的对象
//!
//! **不变量 INV-NM（Non-moving）**：当前 GC 实现是 non-moving 的，不会修改 handle 值。
//! 因此编译器在 spill epilogue 后不需要 reload 值。
//!
//! ## Dead Spill 安全性
//!
//! 如果编译器 spill 了一个不再使用的值（dead spill），这是安全的：
//!
//! - **标量值**：被 `tag_needs_root` 过滤，不会作为 root
//! - **陈旧 handle**：指向已回收的对象，GC 会将其标记为 dead，不会访问
//!
//! 这允许编译器采用保守策略（宁可多 spill，也不漏 spill）。
use wjsm_ir::{value, SHADOW_STACK_SIZE};
use crate::runtime_gc::api::{Handle, RootProvider};
use crate::runtime_gc::GcContext;

/// 运行时 root 提供者：扫描 shadow stack + host 侧表（fixed-point 友好）。
pub struct RuntimeRoots;

impl RootProvider for RuntimeRoots {
    fn for_each_shadow_stack_root(&mut self, ctx: &mut GcContext, visit: &mut dyn FnMut(Handle)) {
        let sp = ctx.shadow_sp();
        let end = ctx.shadow_stack_end();
        // shadow stack 区间：[stack_base, sp)；位于 handle table 之后，非从地址 0 起扫。
        if end == 0 {
            return;
        }
        let stack_base = end.saturating_sub(SHADOW_STACK_SIZE as usize);
        if sp <= stack_base || sp > end {
            return;
        }
        // 先快照所有 raw 值（with_memory 借用周期短），再解析（解析可能 with_state）。
        let vals: Vec<i64> = ctx.with_memory(|_caller, data| {
            let mut out = Vec::new();
            let mut addr = stack_base;
            while addr + 8 <= sp.min(data.len()) {
                let val = i64::from_le_bytes([
                    data[addr],
                    data[addr + 1],
                    data[addr + 2],
                    data[addr + 3],
                    data[addr + 4],
                    data[addr + 5],
                    data[addr + 6],
                    data[addr + 7],
                ]);
                out.push(val);
                addr += 8;
            }
            out
        });
        for val in vals {
            push_value_roots(ctx, val, visit);
        }
    }

    fn for_each_host_table_root(&mut self, ctx: &mut GcContext, visit: &mut dyn FnMut(Handle)) {
        // 稳定 root：IR function property objects，永久存活。
        // startup snapshot 拆分 bootstrap 后，primordial 原型先于函数属性对象分配，
        // 占据更低 handle，故函数属性对象从 __function_props_base 起算，区间为
        // function_props_base..+num_ir_functions（不再是 0..num_ir_functions）。
        let base = ctx.function_props_base();
        let n = ctx.num_ir_functions();
        let count = ctx.obj_table_count();
        for h in base..base + n {
            if h < count {
                visit(h as Handle);
            }
        }
        // 稳定 root：primordial 原型对象。这些在 bootstrap / host post-bootstrap 创建，
        // handle 低于 function_props_base，不被上面的区间扫描覆盖，必须显式作顶层 root，
        // 否则被 sweep 回收 → 原型链断裂 → 属性查找读到 garbage（P4 T4.5 发现）。
        if let Some(h) = ctx.array_proto_handle() {
            visit(h);
        }
        if let Some(h) = ctx.object_proto_handle() {
            visit(h);
        }
        // %AsyncIteratorPrototype% / AsyncGenerator.prototype 同样位于 function_props_base
        // 之下，且仅由 RuntimeState 字段持有；旧布局下靠 0..num_ir_functions 扫描被顺带 root，
        // 区间改为 base.. 后失去覆盖，必须显式 root。
        let (async_iterator_proto, async_gen_proto) =
            ctx.with_state(|st| (st.async_iterator_prototype, st.async_gen_prototype));
        push_value_roots(ctx, async_iterator_proto, visit);
        push_value_roots(ctx, async_gen_proto, visit);
        let protos = ctx.with_state(|st| st.error_prototypes);
        if protos.is_initialized() {
            for proto in [
                protos.error,
                protos.type_error,
                protos.range_error,
                protos.syntax_error,
                protos.reference_error,
                protos.uri_error,
                protos.eval_error,
                protos.aggregate_error,
            ] {
                push_value_roots(ctx, proto, visit);
            }
        }
        // 动态 root：host 侧表快照 → 解析每个 raw 值为 handle。
        let snapshot = collect_host_table_values(ctx);
        for val in snapshot {
            push_value_roots(ctx, val, visit);
        }
    }
}

/// 把一个 NaN-boxed value 解析为它引用的 obj_table handle（含 closure/native_callable）。
/// 对每个解析出的 handle 调 visit（递归收敛，受 host 表大小约束）。
fn push_value_roots(ctx: &mut GcContext, val: i64, visit: &mut dyn FnMut(Handle)) {
    if !value::tag_needs_root(val) {
        return;
    }
    let count = ctx.obj_table_count();
    if value::is_object(val) || value::is_array(val) {
        let h = value::decode_object_handle(val);
        if (h as usize) < count {
            visit(h);
        }
        return;
    }
    if value::is_function(val) {
        // 函数值低 32 位是函数表索引；其属性对象 handle 从 function_props_base 起算。
        let base = ctx.function_props_base();
        let h = (val as u32 as usize).saturating_add(base);
        if h < count {
            visit(h as Handle);
        }
        return;
    }
    if value::is_closure(val) {
        let closure_idx = value::decode_closure_idx(val) as usize;
        let env_obj = ctx.with_state(|st| {
            st.closures
                .lock()
                .ok()
                .and_then(|g| g.get(closure_idx).map(|e| e.env_obj))
        });
        if let Some(env) = env_obj {
            push_value_roots(ctx, env, visit);
        }
        return;
    }
    if value::is_native_callable(val) {
        let idx = value::decode_native_callable_idx(val) as usize;
        let refs = ctx.with_state(|st| collect_native_callable_refs(st, idx));
        for r in refs {
            push_value_roots(ctx, r, visit);
        }
    }
    if value::is_bound(val) {
        let idx = value::decode_bound_idx(val) as usize;
        let refs = ctx.with_state(|st| {
            crate::runtime_gc::side_table_refs::collect_bound_refs(st, idx)
        });
        for r in refs {
            push_value_roots(ctx, r, visit);
        }
        return;
    }
    if value::is_proxy(val) {
        let idx = value::decode_proxy_handle(val) as usize;
        let refs = ctx.with_state(|st| {
            crate::runtime_gc::side_table_refs::collect_proxy_refs(st, idx)
        });
        for r in refs {
            push_value_roots(ctx, r, visit);
        }
        return;
    }
    if value::is_iterator(val) {
        let idx = value::decode_handle(val) as usize;
        let refs = ctx.with_state(|st| {
            crate::runtime_gc::side_table_refs::collect_iterator_refs(st, idx)
        });
        for r in refs {
            push_value_roots(ctx, r, visit);
        }
        return;
    }
    if value::is_scope_record(val) {
        let handle = value::decode_scope_record_handle(val);
        let refs = ctx.with_state(|st| {
            crate::runtime_gc::side_table_refs::collect_scope_record_refs(st, handle)
        });
        for r in refs {
            push_value_roots(ctx, r, visit);
        }
        return;
    }
    // 其余 tag（bigint/symbol/regexp/enumerator/runtime_string/exception）：
    // 侧表不含 obj_table 引用，不需追踪。
}

/// 从 native_callable 表项提取其内部持有的对象引用。
/// 委托给 `runtime_gc::native_callable_refs` 的共享实现。
fn collect_native_callable_refs(st: &mut crate::RuntimeState, idx: usize) -> Vec<i64> {
    crate::runtime_gc::native_callable_refs::collect_native_callable_refs(st, idx)
}

/// 快照所有 host 侧表持有的 raw 引用值（移植自 trace_runtime_side_table_roots_fixed_point）。
/// 返回 raw i64 列表，由调用方经 push_value_roots 解析为 handle。
/// 注：promise_table 的 reactions 只对 marked promise 有意义（fixed-point 二轮起生效），
/// 但此处全量快照简化——push_value_roots 的越界/空检查兜底。
fn collect_host_table_values(ctx: &mut GcContext) -> Vec<i64> {
    use crate::{Microtask, PromiseReactionKind, PromiseState};
    let mut out = Vec::new();

    ctx.with_state(|st| {
        // microtask_queue
        let microtasks = st
            .microtask_queue
            .lock()
            .ok()
            .map(|g| g.clone())
            .unwrap_or_default();
        for task in microtasks {
            match task {
                Microtask::PromiseReaction {
                    promise,
                    handler,
                    argument,
                    ..
                } => {
                    out.extend([promise, handler, argument]);
                }
                Microtask::PromiseResolveThenable {
                    promise,
                    thenable,
                    then,
                } => {
                    out.extend([promise, thenable, then]);
                }
                Microtask::MicrotaskCallback { callback } => out.push(callback),
                Microtask::TransformStreamTransform {
                    callback,
                    this_val,
                    chunk,
                    controller,
                    write_promise,
                } => {
                    out.extend([callback, this_val, chunk, controller, write_promise]);
                }
                Microtask::TransformStreamFlush {
                    callback,
                    this_val,
                    controller,
                    close_promise,
                    ..
                } => {
                    out.extend(callback);
                    out.extend([this_val, controller, close_promise]);
                }
                Microtask::AsyncResume {
                    continuation,
                    resume_val,
                    ..
                } => {
                    out.extend([continuation, resume_val]);
                    // continuation 若是 object handle → 解析 continuation_table.captured_vars
                    if value::is_object(continuation) {
                        let cont_idx = value::decode_object_handle(continuation) as usize;
                        if let Some(entry) = st
                            .continuation_table
                            .lock()
                            .ok()
                            .and_then(|g| g.get(cont_idx).cloned())
                        {
                            out.push(entry.outer_promise);
                            out.extend(entry.captured_vars);
                        }
                    }
                }
                Microtask::CleanupFinalizationRegistry {
                    callback,
                    held_value,
                } => {
                    out.extend([callback, held_value]);
                }
                Microtask::ReadableStreamPull {
                    callback,
                    this_val,
                    controller,
                } => {
                    out.extend([callback, this_val, controller]);
                }
            }
        }

        // promise_table：state value + reactions（handler/target_promise）。
        let promises = st
            .promise_table
            .lock()
            .ok()
            .map(|g| g.clone())
            .unwrap_or_default();
        for entry in promises.iter() {
            if !entry.is_promise {
                continue;
            }
            match &entry.state {
                PromiseState::Fulfilled(v) | PromiseState::Rejected(v) => out.push(*v),
                PromiseState::Pending => {}
            }
            for reaction in entry
                .fulfill_reactions
                .iter()
                .chain(entry.reject_reactions.iter())
            {
                match &reaction.kind {
                    PromiseReactionKind::Normal { handler } => {
                        out.push(reaction.target_promise);
                        out.push(*handler);
                    }
                    PromiseReactionKind::AsyncResume { .. } => {
                        // target_promise 是 continuation object handle → fixed-point 下轮经
                        // AsyncResume 路径覆盖 captured_vars。这里先 push target。
                        out.push(reaction.target_promise);
                        if value::is_object(reaction.target_promise) {
                            let cont_idx =
                                value::decode_object_handle(reaction.target_promise) as usize;
                            if let Some(ce) = st
                                .continuation_table
                                .lock()
                                .ok()
                                .and_then(|g| g.get(cont_idx).cloned())
                            {
                                out.push(ce.outer_promise);
                                out.extend(ce.captured_vars);
                            }
                        }
                    }
                }
            }
        }

        // reader_table：pending_read_promise / pending_byob_view
        let readers = st
            .reader_table
            .lock()
            .ok()
            .map(|g| g.clone())
            .unwrap_or_default();
        for r in readers.iter() {
            if let Some(v) = r.pending_read_promise {
                out.push(v);
            }
            if let Some(v) = r.pending_byob_view {
                out.push(v);
            }
            if let Some(v) = r.closed_promise {
                out.push(v);
            }
        }

        // byob_request_table：view / promise
        let byobs = st
            .byob_request_table
            .lock()
            .ok()
            .map(|g| g.clone())
            .unwrap_or_default();
        for e in byobs.iter() {
            out.push(e.view);
            out.push(e.promise);
        }

        // stream_controller_table：underlying_source / pull / cancel
        let ctrls = st
            .stream_controller_table
            .lock()
            .ok()
            .map(|g| g.clone())
            .unwrap_or_default();
        for c in ctrls.iter() {
            out.extend(c.underlying_source);
            out.extend(c.pull_callback);
            out.extend(c.cancel_callback);
            out.extend(c.strategy_size);
            out.extend(c.abort_reason);
            for chunk in c.chunk_queue.iter() {
                out.push(*chunk);
            }
        }

        // timers：callback（TimerEntry 非 Clone，在 guard 内直接取字段）
        if let Ok(timers) = st.timers.lock() {
            for t in timers.iter() {
                out.push(t.callback);
            }
        }

        crate::array_named_props::ArrayNamedPropsStore::trace_roots(
            &st.array_named_props,
            &mut out,
        );

        // map_table: keys + values（全量扫描——Map 对象的 __map_handle__ 属性是数字，
        // collect_child_raw_values 的 tag_needs_root 对数字返回 false，无法从 mark phase 追踪）
        if let Ok(table) = st.map_table.lock() {
            for entry in table.iter() {
                out.extend(entry.keys.iter().copied());
                out.extend(entry.values.iter().copied());
            }
        }
        // set_table: values（同 map_table 理由）
        if let Ok(table) = st.set_table.lock() {
            for entry in table.iter() {
                out.extend(entry.values.iter().copied());
            }
        }
        // async_generator_table: continuation + active_request + queue + waiting_resume_promise
        if let Ok(table) = st.async_generator_table.lock() {
            for entry in table.iter() {
                out.push(entry.continuation);
                if let Some(v) = entry.waiting_resume_promise {
                    out.push(v);
                }
                if let Some(req) = &entry.active_request {
                    out.push(req.value);
                    out.push(req.promise);
                }
                for req in entry.queue.iter() {
                    out.push(req.value);
                    out.push(req.promise);
                }
            }
        }
        // async_from_sync_iterators: sync_iterator + outer_iter
        if let Ok(table) = st.async_from_sync_iterators.lock() {
            for entry in table.iter() {
                out.push(entry.sync_iterator);
                out.push(entry.outer_iter);
            }
        }
        // writable_stream_table: error + abort_signal
        if let Ok(table) = st.writable_stream_table.lock() {
            for entry in table.iter() {
                if let Some(v) = entry.error {
                    out.push(v);
                }
                if let Some(v) = entry.abort_signal {
                    out.push(v);
                }
            }
        }
        // writer_table: closed_promise + ready_promise
        if let Ok(table) = st.writer_table.lock() {
            for entry in table.iter() {
                if let Some(v) = entry.closed_promise {
                    out.push(v);
                }
                if let Some(v) = entry.ready_promise {
                    out.push(v);
                }
            }
        }
        // transform_stream_table: transform_callback + flush_callback + transformer_this + readable_obj + writable_obj
        if let Ok(table) = st.transform_stream_table.lock() {
            for entry in table.iter() {
                if let Some(v) = entry.transform_callback {
                    out.push(v);
                }
                if let Some(v) = entry.flush_callback {
                    out.push(v);
                }
                if let Some(v) = entry.transformer_this {
                    out.push(v);
                }
                if let Some(v) = entry.readable_obj {
                    out.push(v);
                }
                if let Some(v) = entry.writable_obj {
                    out.push(v);
                }
            }
        }
    });

    out
}
