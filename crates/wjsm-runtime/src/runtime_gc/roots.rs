//! Root 发现（spec §10，IMPL-9 fixed-point）。
//!
//! `RuntimeRoots`：impl RootProvider，扫描 shadow stack + host 侧表。
//!
//! Root 来源：
//! - shadow stack：[0, sp) 每 8B 槽读 i64，解析为 handle（含 closure→env_obj）。
//! - IR function property objects（0..num_ir_functions）：永久存活。
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
use crate::runtime_gc::api::{GcContext, Handle, RootProvider};
use crate::NativeCallable;
use wjsm_ir::value;

/// 运行时 root 提供者：扫描 shadow stack + host 侧表（fixed-point 友好）。
pub struct RuntimeRoots;

impl RootProvider for RuntimeRoots {
    fn for_each_shadow_stack_root(&mut self, ctx: &mut GcContext, visit: &mut dyn FnMut(Handle)) {
        let sp = ctx.shadow_sp();
        let end = ctx.shadow_stack_end();
        // shadow stack 区间：[0, sp)（runtime 把 shadow stack 放在 memory 头部）。
        // tag_needs_root 过滤标量；object_heap_start 之后是对象堆，tag 过滤不会误扫。
        if sp == 0 || end == 0 || sp > end {
            return;
        }
        // 先快照所有 raw 值（with_memory 借用周期短），再解析（解析可能 with_state）。
        let vals: Vec<i64> = ctx.with_memory(|_caller, data| {
            let mut out = Vec::new();
            let mut addr = 0usize;
            while addr + 8 <= sp.min(data.len()) {
                let val = i64::from_le_bytes([
                    data[addr], data[addr + 1], data[addr + 2], data[addr + 3],
                    data[addr + 4], data[addr + 5], data[addr + 6], data[addr + 7],
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
        // 稳定 root：IR function property objects（0..num_ir_functions），永久存活。
        let n = ctx.num_ir_functions();
        for h in 0..(n as Handle) {
            visit(h);
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
        let h = (val as u32) as Handle;
        if (h as usize) < count {
            visit(h);
        }
        return;
    }
    if value::is_closure(val) {
        let closure_idx = value::decode_closure_idx(val) as usize;
        let env_obj = ctx.with_state(|st| {
            st.closures.lock().ok().and_then(|g| g.get(closure_idx).map(|e| e.env_obj))
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
        return;
    }
    // 其他 handle tag（bigint/symbol/regexp/proxy/scope_record/iterator/enumerator/
    // runtime_string/exception/bound）：经对应 side-table fixed-point 路径或不持 obj_table 引用。
}

/// 从 native_callable 表项提取其内部持有的对象引用（移植自 trace_native_callable_record）。
fn collect_native_callable_refs(st: &mut crate::RuntimeState, idx: usize) -> Vec<i64> {
    let record = match st.native_callables.lock().ok().and_then(|g| g.get(idx).cloned()) {
        Some(r) => r,
        None => return vec![],
    };
    match record {
        NativeCallable::PromiseResolvingFunction { promise, .. } => vec![promise],
        NativeCallable::PromiseCombinatorReaction { context, .. } => {
            let (rp, ra) = st
                .combinator_contexts
                .lock()
                .ok()
                .and_then(|g| g.get(context).map(|e| (e.result_promise, e.result_array)))
                .unwrap_or((value::encode_undefined(), value::encode_undefined()));
            vec![rp, ra]
        }
        NativeCallable::AsyncGeneratorMethod { generator, .. }
        | NativeCallable::AsyncGeneratorIdentity { generator } => vec![generator],
        NativeCallable::EvalFunction(function) => function.scope_env.into_iter().collect(),
        // 其余变体不直接持 obj_table 引用（method dispatch 的 handle 是 side-table 索引）。
        _ => vec![],
    }
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
        let microtasks = st.microtask_queue.lock().ok().map(|g| g.clone()).unwrap_or_default();
        for task in microtasks {
            match task {
                Microtask::PromiseReaction { promise, handler, argument, .. } => {
                    out.extend([promise, handler, argument]);
                }
                Microtask::PromiseResolveThenable { promise, thenable, then } => {
                    out.extend([promise, thenable, then]);
                }
                Microtask::MicrotaskCallback { callback } => out.push(callback),
                Microtask::TransformStreamTransform { callback, this_val, chunk, controller, write_promise } => {
                    out.extend([callback, this_val, chunk, controller, write_promise]);
                }
                Microtask::TransformStreamFlush { callback, this_val, controller, close_promise, .. } => {
                    out.extend(callback.into_iter());
                    out.extend([this_val, controller, close_promise]);
                }
                Microtask::AsyncResume { continuation, resume_val, .. } => {
                    out.extend([continuation, resume_val]);
                    // continuation 若是 object handle → 解析 continuation_table.captured_vars
                    if value::is_object(continuation) {
                        let cont_idx = value::decode_object_handle(continuation) as usize;
                        if let Some(entry) = st.continuation_table.lock().ok().and_then(|g| g.get(cont_idx).cloned()) {
                            out.push(entry.outer_promise);
                            out.extend(entry.captured_vars);
                        }
                    }
                }
                Microtask::CleanupFinalizationRegistry { callback, held_value } => {
                    out.extend([callback, held_value]);
                }
                Microtask::ReadableStreamPull { callback, this_val, controller } => {
                    out.extend([callback, this_val, controller]);
                }
            }
        }

        // promise_table：state value + reactions（handler/target_promise）。
        let promises = st.promise_table.lock().ok().map(|g| g.clone()).unwrap_or_default();
        for entry in promises.iter() {
            if !entry.is_promise {
                continue;
            }
            match &entry.state {
                PromiseState::Fulfilled(v) | PromiseState::Rejected(v) => out.push(*v),
                PromiseState::Pending => {}
            }
            for reaction in entry.fulfill_reactions.iter().chain(entry.reject_reactions.iter()) {
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
                            let cont_idx = value::decode_object_handle(reaction.target_promise) as usize;
                            if let Some(ce) = st.continuation_table.lock().ok().and_then(|g| g.get(cont_idx).cloned()) {
                                out.push(ce.outer_promise);
                                out.extend(ce.captured_vars);
                            }
                        }
                    }
                }
            }
        }

        // reader_table：pending_read_promise / pending_byob_view
        let readers = st.reader_table.lock().ok().map(|g| g.clone()).unwrap_or_default();
        for r in readers.iter() {
            if let Some(v) = r.pending_read_promise { out.push(v); }
            if let Some(v) = r.pending_byob_view { out.push(v); }
        }

        // byob_request_table：view / promise
        let byobs = st.byob_request_table.lock().ok().map(|g| g.clone()).unwrap_or_default();
        for e in byobs.iter() {
            out.push(e.view);
            out.push(e.promise);
        }

        // stream_controller_table：underlying_source / pull / cancel
        let ctrls = st.stream_controller_table.lock().ok().map(|g| g.clone()).unwrap_or_default();
        for c in ctrls.iter() {
            out.extend(c.underlying_source.into_iter());
            out.extend(c.pull_callback.into_iter());
            out.extend(c.cancel_callback.into_iter());
        }

        // timers：callback（TimerEntry 非 Clone，在 guard 内直接取字段）
        if let Ok(timers) = st.timers.lock() {
            for t in timers.iter() {
                out.push(t.callback);
            }
        }
    });

    out
}
