pub(crate) mod agent;
pub(crate) mod arguments;
mod array;
mod array_callbacks;
mod array_like;
mod array_sort;
pub(crate) mod async_generator;
pub(crate) mod atomics;
mod bigint;
pub(crate) mod buffers;
mod callable_chain;
pub(crate) mod collections;
mod console;
pub(crate) mod date;
mod date_methods;
pub(crate) mod enumerator;
mod errors;
mod eval;
pub(crate) mod events;
pub(crate) mod fetch;
mod function;
pub(crate) mod function_constructor;
pub(crate) mod generator;
pub(crate) mod global_env;
pub(crate) mod idna;
pub(crate) mod intl;
mod intrinsics;
mod iterator;
pub(crate) mod iterator_helpers;
mod json;
mod jsx;
mod math;
pub(crate) mod modules;
pub(crate) mod node_async_hooks;
pub(crate) mod node_buffer;
pub(crate) mod node_child_process;
pub(crate) mod node_crypto;
pub(crate) mod node_dgram;
pub(crate) mod node_fs;
mod node_fs_snapshot;
pub(crate) mod node_net;
pub(crate) mod node_os;
pub(crate) mod node_perf_hooks;
pub(crate) mod node_tls;
pub(crate) mod node_tty;
pub(crate) mod node_vm;
pub(crate) mod node_worker_threads;
pub(crate) mod node_zlib;
mod object;
mod object_proto;
mod operator;
mod primitive;
mod private;
pub(crate) mod promise;
mod property_write;
pub(crate) mod proxy;
pub(crate) mod regexp;
pub(crate) mod runtime;
pub(crate) mod sab;
pub(crate) mod streams;
mod string;
pub(crate) mod string_proto;
pub(crate) mod structured_clone;
mod symbol;
mod timer;
pub(crate) mod typedarray;
mod typedarray_create;
mod typedarray_static;
pub(crate) mod weak;
pub(crate) mod web_encoding;
mod with_env;

pub(crate) use self::array::construct as construct_array;
pub(crate) use self::errors::error_constructor;
pub(super) use self::math::{
    native_math_acos, native_math_acosh, native_math_asin, native_math_asinh, native_math_atan,
    native_math_atan2, native_math_atanh, native_math_cbrt, native_math_cos, native_math_cosh,
    native_math_exp, native_math_expm1, native_math_log, native_math_log1p, native_math_log2,
    native_math_log10, native_math_pow, native_math_sin, native_math_sinh, native_math_tan,
    native_math_tanh,
};
pub(crate) use self::object::construct_object;
use self::runtime::dispatch_runtime;
pub(crate) use self::runtime::encoded_property_key;
use self::runtime::object_handle;
pub(crate) use self::runtime::to_number as number_value;
pub(crate) use self::runtime::{
    array_iterator, array_to_string, error_to_string, fail_dispatch, iterator_next_result,
    render_value, to_string_coerced,
};
pub(crate) use self::symbol::well_known_description;
pub(crate) use self::typedarray_static::{
    abstract_construct as typedarray_abstract_construct, static_from as typedarray_static_from,
    static_of as typedarray_static_of, to_string_tag as typedarray_to_string_tag,
};
use crate::NativeAgentState;
use num_bigint::BigInt;
use wjsm_ir::{Builtin, dispatch_jumptable, value};
use wjsm_native_abi::{NativeRuntimeOp, NativeVmContext, PendingExceptionKind};

pub(crate) fn store_bigint(state: &mut NativeAgentState, input: BigInt) -> Option<i64> {
    bigint::store(state, input)
}

pub(super) unsafe extern "C" fn native_zgc_load_barrier_assist(
    ctx: *mut NativeVmContext,
    handle: u32,
) -> u64 {
    // SAFETY:generated code 传入 pinned vmctx；本叶子 thunk 不保存该引用。
    let Some(ctx) = (unsafe { ctx.as_ref() }) else {
        return 0;
    };
    // SAFETY:heap_state 在 runtime 生命周期内指向 pinned NativeAgentState，屏障 thunk
    // 只在 owner mutator 线程同步调用。
    let Some(state) = (unsafe { ctx.heap_state.cast::<NativeAgentState>().as_ref() }) else {
        return 0;
    };
    state.gc.heap().resolve_handle(handle).unwrap_or(0)
}

pub(super) unsafe extern "C" fn native_zgc_store_barrier(
    ctx: *mut NativeVmContext,
    owner: u32,
    slot: u64,
    value: i64,
) -> u32 {
    // SAFETY:generated code 传入 pinned vmctx；本叶子 thunk 不保存该引用。
    let Some(ctx) = (unsafe { ctx.as_ref() }) else {
        return 1;
    };
    // SAFETY:heap_state 在 runtime 生命周期内指向 pinned NativeAgentState，屏障 thunk
    // 只在 owner mutator 线程同步调用。
    let Some(state) = (unsafe { ctx.heap_state.cast::<NativeAgentState>().as_ref() }) else {
        return 1;
    };
    u32::from(
        state
            .gc
            .heap()
            .store_reference(owner, slot, value as u64)
            .is_err(),
    )
}

/// 执行已由生成代码守卫为字符串参与的动态加法。
///
/// # Safety
/// `ctx` 必须指向当前 owner 线程上存活且 pinned 的 [`NativeVmContext`]；其
/// `heap_state` 必须指向同一 runtime 的 [`NativeAgentState`]，且调用期间不可并发访问。
pub(super) unsafe extern "C" fn native_string_add(
    ctx: *mut NativeVmContext,
    left: i64,
    right: i64,
) -> i64 {
    // SAFETY: generated code 传入 pinned vmctx；本同步 thunk 不保留引用。
    let Some(ctx) = (unsafe { ctx.as_mut() }) else {
        return value::encode_handle(value::TAG_EXCEPTION, 0);
    };
    // SAFETY: heap_state 在 runtime 生命周期内指向 pinned NativeAgentState，且仅 owner
    // mutator 线程进入本 thunk。
    let Some(state) = (unsafe { ctx.heap_state.cast::<NativeAgentState>().as_mut() }) else {
        return fail_dispatch(ctx);
    };
    let result = runtime::binary_add(ctx, state, left, right);
    ctx.proto_generation = state.gc.heap().shapes().proto_generation();
    result
}

/// 执行编译器证明为局部不逃逸累加器的字符串追加。
///
/// # Safety
/// `ctx` 必须指向 owner 线程上存活且 pinned 的 [`NativeVmContext`]。
pub(super) unsafe extern "C" fn native_string_builder_append(
    ctx: *mut NativeVmContext,
    current: i64,
    first: i64,
    second: i64,
) -> i64 {
    // SAFETY: generated code 传入 pinned vmctx；本同步 thunk 不保留引用。
    let Some(ctx) = (unsafe { ctx.as_mut() }) else {
        return value::encode_handle(value::TAG_EXCEPTION, 0);
    };
    // SAFETY: heap_state 在 runtime 生命周期内指向 pinned NativeAgentState，且仅 owner
    // mutator 线程进入本 thunk。
    let Some(state) = (unsafe { ctx.heap_state.cast::<NativeAgentState>().as_mut() }) else {
        return fail_dispatch(ctx);
    };
    string::string_builder_append_direct(ctx, state, current, first, second)
}

/// 追加 boxed 原始值与已证明为 f64 的数字，避免热循环重复装箱和标签分派。
///
/// # Safety
/// `ctx` 必须指向 owner 线程上存活且 pinned 的 [`NativeVmContext`]。
pub(super) unsafe extern "C" fn native_string_builder_append_number(
    ctx: *mut NativeVmContext,
    current: i64,
    first: i64,
    second: f64,
) -> i64 {
    // SAFETY: generated code 传入 pinned vmctx；本同步 thunk 不保留引用。
    let Some(ctx) = (unsafe { ctx.as_mut() }) else {
        return value::encode_handle(value::TAG_EXCEPTION, 0);
    };
    // SAFETY: heap_state 在 runtime 生命周期内指向 pinned NativeAgentState，且仅 owner
    // mutator 线程进入本 thunk。
    let Some(state) = (unsafe { ctx.heap_state.cast::<NativeAgentState>().as_mut() }) else {
        return fail_dispatch(ctx);
    };
    string::string_builder_append_number_direct(ctx, state, current, first, second)
}

/// 冻结编译器局部字符串累加器。
///
/// # Safety
/// `ctx` 必须指向 owner 线程上存活且 pinned 的 [`NativeVmContext`]。
pub(super) unsafe extern "C" fn native_string_builder_finish(
    ctx: *mut NativeVmContext,
    builder: i64,
) -> i64 {
    // SAFETY: generated code 传入 pinned vmctx；本同步 thunk 不保留引用。
    let Some(ctx) = (unsafe { ctx.as_mut() }) else {
        return value::encode_handle(value::TAG_EXCEPTION, 0);
    };
    // SAFETY: heap_state 在 runtime 生命周期内指向 pinned NativeAgentState，且仅 owner
    // mutator 线程进入本 thunk。
    let Some(state) = (unsafe { ctx.heap_state.cast::<NativeAgentState>().as_mut() }) else {
        return fail_dispatch(ctx);
    };
    string::string_builder_finish(ctx, state, &[builder])
}

pub(super) unsafe extern "C" fn native_host_operation(
    ctx: *mut NativeVmContext,
    operation: u32,
    args: *const i64,
    args_count: u32,
    feedback_slot: *mut u8,
) -> i64 {
    // SAFETY: generated code passes its pinned vmctx pointer; dispatcher checks null before use and
    // does not retain the reference beyond this synchronous call.
    let Some(ctx) = (unsafe { ctx.as_mut() }) else {
        return value::encode_handle(value::TAG_EXCEPTION, 0);
    };
    let count = match usize::try_from(args_count) {
        Ok(count) => count,
        Err(_) => return fail_dispatch(ctx),
    };
    let args = if count == 0 {
        &[]
    } else {
        if args.is_null() {
            return fail_dispatch(ctx);
        }
        // SAFETY: compiler creates a stack slot containing exactly `args_count` initialized i64
        // values and the dispatcher borrows it only for this synchronous call.
        unsafe { std::slice::from_raw_parts(args, count) }
    };
    // SAFETY: heap_state is initialized from the boxed owner state and remains valid/pinned for the
    // runtime lifetime; host thunks run only synchronously on the owner thread.
    let Some(state) = (unsafe { ctx.heap_state.cast::<NativeAgentState>().as_mut() }) else {
        return fail_dispatch(ctx);
    };
    let runtime_operation = NativeRuntimeOp::from_id(operation);
    let tlab_visibility = state
        .gc
        .operation_requires_native_tlab_flush(ctx, runtime_operation, args)
        .and_then(|requires_flush| {
            if requires_flush {
                state.gc.flush_native_tlab(ctx)
            } else {
                Ok(())
            }
        });
    if let Err(error) = tlab_visibility {
        ctx.pending_exception_kind = PendingExceptionKind::InternalInvariant;
        state
            .stderr
            .borrow_mut()
            .extend_from_slice(error.to_string().as_bytes());
        return fail_dispatch(ctx);
    }
    let allocation_operation = matches!(
        runtime_operation,
        Some(NativeRuntimeOp::NewObject | NativeRuntimeOp::NewArray)
    );
    if allocation_operation {
        if let Err(error) = state.gc.adopt_native_tlab_cursor(ctx) {
            ctx.pending_exception_kind = PendingExceptionKind::InternalInvariant;
            state
                .stderr
                .borrow_mut()
                .extend_from_slice(error.to_string().as_bytes());
            return fail_dispatch(ctx);
        }
        if state.gc.native_tlab_needs_refill(ctx)
            && state.gc.should_collect_before_native_tlab_refill()
            && let Err(error) = state.collect_garbage(ctx)
        {
            ctx.pending_exception_kind = PendingExceptionKind::InternalInvariant;
            state
                .stderr
                .borrow_mut()
                .extend_from_slice(error.to_string().as_bytes());
            return fail_dispatch(ctx);
        }
        if let Err(error) = state.gc.refill_native_tlab_if_exhausted(ctx) {
            ctx.pending_exception_kind = PendingExceptionKind::InternalInvariant;
            state
                .stderr
                .borrow_mut()
                .extend_from_slice(error.to_string().as_bytes());
            return fail_dispatch(ctx);
        }
    }

    // owner 边界：先收敛后台特化编译结果（发布/丢弃都不阻塞 JS），再校验反馈槽。
    let feedback = if state.runtime_config.specialization_enabled {
        state.drain_specialization_results();
        state.validated_feedback_slot(feedback_slot)
    } else {
        None
    };
    let record_value_feedback = feedback.is_some()
        && !matches!(
            NativeRuntimeOp::from_id(operation),
            Some(
                NativeRuntimeOp::PrepareCall
                    | NativeRuntimeOp::PrepareConstruct
                    | NativeRuntimeOp::PrepareSuperCall
                    | NativeRuntimeOp::PrepareSuperCallForward
            )
        );
    if record_value_feedback && let Some(feedback) = feedback {
        state.record_value_feedback(feedback, operation, args);
    }

    let result = if operation <= u32::from(Builtin::last_wire_id()) {
        let builtin_id = match u16::try_from(operation) {
            Ok(id) => id,
            Err(_) => return fail_dispatch(ctx),
        };
        let Some(builtin) = Builtin::from_wire_id(builtin_id) else {
            return fail_dispatch(ctx);
        };
        dispatch_builtin(ctx, state, builtin, args)
    } else {
        let Some(operation) = runtime_operation else {
            return fail_dispatch(ctx);
        };
        dispatch_runtime(ctx, state, operation, args, feedback)
    };
    if runtime_operation.is_some_and(|operation| {
        matches!(
            operation,
            NativeRuntimeOp::SetProp
                | NativeRuntimeOp::SetPropStrict
                | NativeRuntimeOp::CreateDataProperty
                | NativeRuntimeOp::DeleteProp
                | NativeRuntimeOp::DeletePropStrict
                | NativeRuntimeOp::SetProto
                | NativeRuntimeOp::SetPropIc
                | NativeRuntimeOp::SetPropIcStrict
        )
    }) {
        state.bump_ic_epoch(ctx.current_image_id);
    }
    // 任何宿主操作都可能直接或间接改变 shape（builtin 会直接改 heap）；
    // 这里统一同步 proto 世代，让生成代码中的 ProtoData/Accessor IC 在下次命中
    // 前看到最新值。读锁极短，仅在所有宿主操作返回后执行一次。
    ctx.proto_generation = state.gc.heap().shapes().proto_generation();
    result
}

pub(crate) fn rejected_call_error(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    callee: i64,
    construct: bool,
) -> i64 {
    if ctx.pending_exception_kind == PendingExceptionKind::StackOverflow {
        ctx.pending_exception_kind = PendingExceptionKind::None;
        let message = "Maximum call stack size exceeded".to_owned();
        let Some(error) = modules::named_error_object(state, "RangeError", message.clone()) else {
            return fail_dispatch(ctx);
        };
        if let Some(trace) = state.pending_stack_trace.take() {
            let stack = format!("RangeError: {message}{trace}");
            if modules::set_error_stack(state, error, stack).is_none() {
                return fail_dispatch(ctx);
            }
        }
        return state
            .create_exception(error)
            .unwrap_or_else(|| fail_dispatch(ctx));
    }
    let message = if value::is_proxy(callee) {
        if construct {
            "Proxy target must be a constructor".to_owned()
        } else {
            "Proxy target must be callable".to_owned()
        }
    } else if construct {
        format!(
            "{} is not a constructor",
            runtime::render_value(state, callee)
        )
    } else {
        format!("{} is not a function", runtime::render_value(state, callee))
    };
    modules::named_error_object(state, "TypeError", message)
        .and_then(|error| state.create_exception(error))
        .unwrap_or_else(|| fail_dispatch(ctx))
}

pub(super) unsafe extern "C" fn native_rejected_call(
    ctx: *mut NativeVmContext,
    callee: i64,
    _this_value: i64,
    _args_base: u32,
    _args_count: u32,
) -> i64 {
    let Some(ctx) = (unsafe { ctx.as_mut() }) else {
        return value::encode_handle(value::TAG_EXCEPTION, 0);
    };
    let Some(state) = (unsafe { ctx.heap_state.cast::<NativeAgentState>().as_mut() }) else {
        return fail_dispatch(ctx);
    };
    rejected_call_error(ctx, state, callee, false)
}

pub(super) unsafe extern "C" fn native_rejected_construct(
    ctx: *mut NativeVmContext,
    callee: i64,
    _this_value: i64,
    _args_base: u32,
    _args_count: u32,
) -> i64 {
    let Some(ctx) = (unsafe { ctx.as_mut() }) else {
        return value::encode_handle(value::TAG_EXCEPTION, 0);
    };
    let Some(state) = (unsafe { ctx.heap_state.cast::<NativeAgentState>().as_mut() }) else {
        return fail_dispatch(ctx);
    };
    rejected_call_error(ctx, state, callee, true)
}

/// 类构造器 [[Call]] 的拒绝 handler（ES §10.2.1 步骤 2）：文案对齐 V8/Node，
/// 命名类含类名，匿名类用复数句式。
pub(super) unsafe extern "C" fn native_class_ctor_rejected(
    ctx: *mut NativeVmContext,
    callee: i64,
    _this_value: i64,
    _args_base: u32,
    _args_count: u32,
) -> i64 {
    let Some(ctx) = (unsafe { ctx.as_mut() }) else {
        return value::encode_handle(value::TAG_EXCEPTION, 0);
    };
    let Some(state) = (unsafe { ctx.heap_state.cast::<NativeAgentState>().as_mut() }) else {
        return fail_dispatch(ctx);
    };
    let name = state.pending_class_ctor_name.take();
    if ctx.pending_exception_kind == PendingExceptionKind::StackOverflow {
        return rejected_call_error(ctx, state, callee, false);
    }
    let message = match name {
        Some(name) if !name.is_empty() => {
            format!("Class constructor {name} cannot be invoked without 'new'")
        }
        _ => "Class constructors cannot be invoked without 'new'".to_owned(),
    };
    modules::named_error_object(state, "TypeError", message)
        .and_then(|error| state.create_exception(error))
        .unwrap_or_else(|| fail_dispatch(ctx))
}

pub(super) fn dispatch_builtin(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> i64 {
    // 实参求值异常在语义层按 ES ArgumentListEvaluation 的 `? GetValue` 语义
    // 就地分叉传播（含 async / async generator 状态机体，见语义层
    // `lower_call_operand_then_continue`），普通值实参不会携带 TAG_EXCEPTION 哨兵。
    // 少数机器角色 builtin（ExceptionValue / IteratorClose / ContinuationSaveVar /
    // AsyncGenerator* / Promise 结算指令等）以 completion 载荷为业务实参，其
    // 哨兵处理在各自 handler 内实现。
    // 跳表：`wire_id()` 是连续判别值，编译器把下面的 match 编译为跳表 / 二分。
    // 每个 builtin 变体在此统一注册到领域 handler；handler 未认领（`None`）
    // 时落入 `fail_dispatch`（历史上不可达的 IR builtin）。
    dispatch_jumptable! {
        builtin, (ctx, state, args) {
            fail_dispatch(ctx)
        } => {
            modules::dispatch_module => Builtin::CjsCreateRequire | Builtin::CjsRegisterModule | Builtin::DynamicImport | Builtin::DynamicImportRuntime | Builtin::ImportMetaResolve | Builtin::RegisterModuleNamespace | Builtin::FinalizeModuleNamespace,
            bigint::dispatch_bigint => Builtin::BigIntAdd | Builtin::BigIntBitAnd | Builtin::BigIntBitNot | Builtin::BigIntBitOr | Builtin::BigIntBitXor | Builtin::BigIntCmp | Builtin::BigIntDiv | Builtin::BigIntEq | Builtin::BigIntFromLiteral | Builtin::BigIntMod | Builtin::BigIntMul | Builtin::BigIntNeg | Builtin::BigIntPow | Builtin::BigIntProtoToString | Builtin::BigIntProtoValueOf | Builtin::BigIntShl | Builtin::BigIntShr | Builtin::BigIntSub,
            typedarray::dispatch_typed_array => Builtin::BigInt64ArrayConstructor | Builtin::BigUint64ArrayConstructor | Builtin::Float32ArrayConstructor | Builtin::Float64ArrayConstructor | Builtin::Int16ArrayConstructor | Builtin::Int32ArrayConstructor | Builtin::Int8ArrayConstructor | Builtin::TypedArrayProtoAt | Builtin::TypedArrayProtoByteLength | Builtin::TypedArrayProtoByteOffset | Builtin::TypedArrayProtoCopyWithin | Builtin::TypedArrayProtoEntries | Builtin::TypedArrayProtoEvery | Builtin::TypedArrayProtoFill | Builtin::TypedArrayProtoFilter | Builtin::TypedArrayProtoFind | Builtin::TypedArrayProtoFindIndex | Builtin::TypedArrayProtoForEach | Builtin::TypedArrayProtoIncludes | Builtin::TypedArrayProtoIndexOf | Builtin::TypedArrayProtoJoin | Builtin::TypedArrayProtoKeys | Builtin::TypedArrayProtoLastIndexOf | Builtin::TypedArrayProtoLength | Builtin::TypedArrayProtoMap | Builtin::TypedArrayProtoReduce | Builtin::TypedArrayProtoReduceRight | Builtin::TypedArrayProtoReverse | Builtin::TypedArrayProtoSet | Builtin::TypedArrayProtoSlice | Builtin::TypedArrayProtoSome | Builtin::TypedArrayProtoSort | Builtin::TypedArrayProtoSubarray | Builtin::TypedArrayProtoToString | Builtin::TypedArrayProtoValues | Builtin::Uint16ArrayConstructor | Builtin::Uint32ArrayConstructor | Builtin::Uint8ArrayConstructor | Builtin::Uint8ClampedArrayConstructor,
            promise::dispatch_promise => Builtin::AsyncFunctionResume | Builtin::AsyncFunctionStart | Builtin::AsyncFunctionSuspend | Builtin::ContinuationCreate | Builtin::ContinuationLoadVar | Builtin::ContinuationSaveVar | Builtin::DrainMicrotasks | Builtin::IsPromise | Builtin::PromiseAll | Builtin::PromiseAllSettled | Builtin::PromiseAny | Builtin::PromiseCatch | Builtin::PromiseCreate | Builtin::PromiseCreateRejectFunction | Builtin::PromiseCreateResolveFunction | Builtin::PromiseFinally | Builtin::PromiseInstanceReject | Builtin::PromiseInstanceResolve | Builtin::PromiseRace | Builtin::PromiseRejectStatic | Builtin::PromiseResolveStatic | Builtin::PromiseThen | Builtin::PromiseWithResolvers | Builtin::QueueMicrotask,
            async_generator::dispatch_async_generator => Builtin::AsyncGeneratorNext | Builtin::AsyncGeneratorReturn | Builtin::AsyncGeneratorStart | Builtin::AsyncGeneratorThrow | Builtin::AsyncIteratorFrom,
            generator::dispatch_generator => Builtin::GeneratorNext | Builtin::GeneratorReturn | Builtin::GeneratorStart | Builtin::GeneratorThrow,
            streams::dispatch_streams => Builtin::ByteLengthQueuingStrategyConstructor | Builtin::CountQueuingStrategyConstructor | Builtin::ReadableStreamConstructor | Builtin::TransformStreamConstructor | Builtin::WritableStreamConstructor,
            fetch::dispatch_fetch => Builtin::AbortControllerConstructor | Builtin::Fetch | Builtin::HeadersConstructor | Builtin::RequestConstructor | Builtin::ResponseConstructor,
            events::dispatch_events => Builtin::AbortSignalConstructor | Builtin::EventConstructor | Builtin::EventTargetConstructor,
            buffers::dispatch_buffer => Builtin::ArrayBufferConstructor | Builtin::ArrayBufferProtoByteLength | Builtin::ArrayBufferProtoSlice | Builtin::DataViewConstructor | Builtin::DataViewProtoGetFloat32 | Builtin::DataViewProtoGetFloat64 | Builtin::DataViewProtoGetInt16 | Builtin::DataViewProtoGetInt32 | Builtin::DataViewProtoGetInt8 | Builtin::DataViewProtoGetUint16 | Builtin::DataViewProtoGetUint32 | Builtin::DataViewProtoGetUint8 | Builtin::DataViewProtoSetFloat32 | Builtin::DataViewProtoSetFloat64 | Builtin::DataViewProtoSetInt16 | Builtin::DataViewProtoSetInt32 | Builtin::DataViewProtoSetInt8 | Builtin::DataViewProtoSetUint16 | Builtin::DataViewProtoSetUint32 | Builtin::DataViewProtoSetUint8 | Builtin::DataViewProtoGetBigInt64 | Builtin::DataViewProtoGetBigUint64 | Builtin::DataViewProtoSetBigInt64 | Builtin::DataViewProtoSetBigUint64,
            sab::dispatch_sab => Builtin::SharedArrayBufferConstructor | Builtin::SharedArrayBufferProtoByteLength | Builtin::SharedArrayBufferProtoGrow | Builtin::SharedArrayBufferProtoGrowable | Builtin::SharedArrayBufferProtoMaxByteLength | Builtin::SharedArrayBufferProtoSlice | Builtin::SharedArrayBufferSpecies,
            atomics::dispatch_atomics => Builtin::AtomicsAdd | Builtin::AtomicsAnd | Builtin::AtomicsCompareExchange | Builtin::AtomicsExchange | Builtin::AtomicsIsLockFree | Builtin::AtomicsLoad | Builtin::AtomicsNotify | Builtin::AtomicsOr | Builtin::AtomicsPause | Builtin::AtomicsStore | Builtin::AtomicsSub | Builtin::AtomicsWait | Builtin::AtomicsWaitAsync | Builtin::AtomicsXor,
            enumerator::dispatch_enumerator => Builtin::EnumeratorDone | Builtin::EnumeratorFrom | Builtin::EnumeratorKey | Builtin::EnumeratorNext,
            collections::dispatch_collection => Builtin::MapConstructor | Builtin::MapGroupBy | Builtin::MapProtoGet | Builtin::MapProtoSet | Builtin::MapSetClear | Builtin::MapSetDelete | Builtin::MapSetEntries | Builtin::MapSetFirstKey | Builtin::MapSetForEach | Builtin::MapSetGetSize | Builtin::MapSetHas | Builtin::MapSetKeys | Builtin::MapSetValues | Builtin::SetConstructor | Builtin::SetProtoAdd | Builtin::SetProtoDelete | Builtin::SetProtoHas,
            array::dispatch_array => Builtin::ArrayAllocate | Builtin::ArrayAt | Builtin::ArrayConcat | Builtin::ArrayConcatVa | Builtin::ArrayCopyWithin | Builtin::ArrayFill | Builtin::ArrayFlat | Builtin::ArrayFrom | Builtin::ArrayGetLength | Builtin::ArrayHasElement | Builtin::ArrayIncludes | Builtin::ArrayIndexOf | Builtin::ArrayInitLength | Builtin::ArrayIsArray | Builtin::ArrayIsPlain | Builtin::ArraySpeciesDefault | Builtin::ArrayJoin | Builtin::ArrayLastIndexOf | Builtin::ArrayOf | Builtin::ArrayPop | Builtin::ArrayPush | Builtin::ArrayPushHole | Builtin::ArrayPushSpread | Builtin::ArrayReverse | Builtin::ArrayShift | Builtin::ArraySlice | Builtin::ArraySpliceVa | Builtin::ArrayToReversed | Builtin::ArrayToSplicedVa | Builtin::ArrayUnshiftVa | Builtin::ArrayWith,
            function::dispatch_function => Builtin::FuncApply | Builtin::FuncBind | Builtin::FuncCall | Builtin::SuperApply | Builtin::CreateClosure | Builtin::FunctionSetName | Builtin::FunctionToString,
            array_callbacks::dispatch_array_callback => Builtin::ArrayEvery | Builtin::ArrayFilter | Builtin::ArrayFind | Builtin::ArrayFindIndex | Builtin::ArrayFindLast | Builtin::ArrayFindLastIndex | Builtin::ArrayFlatMap | Builtin::ArrayForEach | Builtin::ArrayMap | Builtin::ArrayReduce | Builtin::ArrayReduceRight | Builtin::ArraySome | Builtin::ArraySort | Builtin::ArrayToSorted,
            json::dispatch_json => Builtin::JsonParse | Builtin::JsonStringify,
            date::dispatch_date => Builtin::DateConstructor | Builtin::DateConstructorNew | Builtin::DateNow | Builtin::DateParse | Builtin::DateUTC,
            math::dispatch_math => Builtin::MathAbs | Builtin::MathAcos | Builtin::MathAcosh | Builtin::MathAsin | Builtin::MathAsinh | Builtin::MathAtan | Builtin::MathAtan2 | Builtin::MathAtanh | Builtin::MathCbrt | Builtin::MathCeil | Builtin::MathClz32 | Builtin::MathCos | Builtin::MathCosh | Builtin::MathExp | Builtin::MathExpm1 | Builtin::MathFloor | Builtin::MathFround | Builtin::MathHypot | Builtin::MathImul | Builtin::MathLog | Builtin::MathLog10 | Builtin::MathLog1p | Builtin::MathLog2 | Builtin::MathMax | Builtin::MathMaxArray | Builtin::MathMin | Builtin::MathPow | Builtin::MathRandom | Builtin::MathRound | Builtin::MathSign | Builtin::MathSin | Builtin::MathSinh | Builtin::MathSqrt | Builtin::MathTan | Builtin::MathTanh | Builtin::MathTrunc,
            object::dispatch_object => Builtin::DefineProperty | Builtin::GetOwnPropDesc | Builtin::ObjectAssign | Builtin::ObjectCreate | Builtin::ObjectDefineProperties | Builtin::ObjectEntries | Builtin::ObjectFreeze | Builtin::ObjectFromEntries | Builtin::ObjectGetOwnPropertyDescriptors | Builtin::ObjectGetOwnPropertyNames | Builtin::ObjectGetOwnPropertySymbols | Builtin::ObjectGetPrototypeOf | Builtin::ObjectGroupBy | Builtin::ObjectIs | Builtin::ObjectIsExtensible | Builtin::ObjectIsFrozen | Builtin::ObjectIsSealed | Builtin::ObjectKeys | Builtin::ObjectPreventExtensions | Builtin::ObjectRest | Builtin::ObjectSeal | Builtin::ObjectSetPrototypeOf | Builtin::ObjectValues | Builtin::ObjectProtoToString | Builtin::ObjectProtoValueOf | Builtin::CreateGlobalObject,
            object_proto::dispatch_object_proto => Builtin::ObjectProtoIsPrototypeOf | Builtin::ObjectProtoToLocaleString | Builtin::ObjectProtoGetProto | Builtin::ObjectProtoSetProto | Builtin::ObjectProtoDefineGetter | Builtin::ObjectProtoDefineSetter | Builtin::ObjectProtoLookupGetter | Builtin::ObjectProtoLookupSetter | Builtin::ObjectHasOwn | Builtin::HasOwnProperty | Builtin::PropertyIsEnumerable,
            private::dispatch_private => Builtin::PrivateAccessorBind | Builtin::PrivateGet | Builtin::PrivateHas | Builtin::PrivateSet,
            regexp::dispatch_regexp => Builtin::RegExpCreate | Builtin::RegExpExec | Builtin::RegExpProtoMatch | Builtin::RegExpProtoReplace | Builtin::RegExpProtoSearch | Builtin::RegExpProtoSplit | Builtin::RegExpTest,
            proxy::dispatch_proxy => Builtin::ProxyCreate | Builtin::ProxyRevocable | Builtin::ReflectApply | Builtin::ReflectConstruct | Builtin::ReflectDefineProperty | Builtin::ReflectDeleteProperty | Builtin::ReflectGet | Builtin::ReflectGetOwnPropertyDescriptor | Builtin::ReflectGetPrototypeOf | Builtin::ReflectHas | Builtin::ReflectIsExtensible | Builtin::ReflectOwnKeys | Builtin::ReflectPreventExtensions | Builtin::ReflectSet | Builtin::ReflectSetPrototypeOf,
            primitive::dispatch_primitive => Builtin::BooleanConstructor | Builtin::BooleanProtoToString | Builtin::BooleanProtoValueOf | Builtin::GlobalIsFinite | Builtin::GlobalIsNaN | Builtin::NumberConstructor | Builtin::NumberIsFinite | Builtin::NumberIsInteger | Builtin::NumberIsNaN | Builtin::NumberIsSafeInteger | Builtin::NumberParseFloat | Builtin::NumberParseInt | Builtin::NumberProtoToExponential | Builtin::NumberProtoToFixed | Builtin::NumberProtoToPrecision | Builtin::NumberProtoToString | Builtin::NumberProtoValueOf | Builtin::ToBoolean,
            symbol::dispatch_symbol => Builtin::SymbolCreate | Builtin::SymbolFor | Builtin::SymbolKeyFor | Builtin::SymbolProtoToString | Builtin::SymbolProtoValueOf | Builtin::SymbolWellKnown,
            string::dispatch_string => Builtin::StringAt | Builtin::StringBuilderAppend | Builtin::StringBuilderFinish | Builtin::StringCharAt | Builtin::StringCharCodeAt | Builtin::StringCodePointAt | Builtin::StringConcatVa | Builtin::StringEndsWith | Builtin::StringFromCharCode | Builtin::StringFromCodePoint | Builtin::StringIncludes | Builtin::StringIndexOf | Builtin::StringLastIndexOf | Builtin::StringMatch | Builtin::StringMatchAll | Builtin::StringNormalize | Builtin::StringPadEnd | Builtin::StringPadStart | Builtin::StringRaw | Builtin::StringRepeat | Builtin::StringReplace | Builtin::StringReplaceAll | Builtin::StringSearch | Builtin::StringSlice | Builtin::StringSplit | Builtin::StringStartsWith | Builtin::StringSubstring | Builtin::StringToLowerCase | Builtin::StringToString | Builtin::StringToUpperCase | Builtin::StringTrim | Builtin::StringTrimEnd | Builtin::StringTrimStart | Builtin::StringValueOf,
            weak::dispatch_weak => Builtin::FinalizationRegistryConstructor | Builtin::FinalizationRegistryProtoRegister | Builtin::FinalizationRegistryProtoUnregister | Builtin::WeakMapConstructor | Builtin::WeakMapProtoDelete | Builtin::WeakMapProtoGet | Builtin::WeakMapProtoHas | Builtin::WeakMapProtoSet | Builtin::WeakRefConstructor | Builtin::WeakRefProtoDeref | Builtin::WeakSetConstructor | Builtin::WeakSetProtoAdd | Builtin::WeakSetProtoDelete | Builtin::WeakSetProtoHas,
            // ── 原 dispatch_inline 兜底 match 拆分出的领域 handler ──
            modules::dispatch_scope => Builtin::ScopeRecordCreate | Builtin::ScopeRecordAddBinding | Builtin::ScopeRecordSetMeta | Builtin::ScopeRecordDestroy | Builtin::ScopeRecordAddWithLayer | Builtin::ScopeRecordGetBinding,
            arguments::dispatch_arguments => Builtin::CreateMappedArgumentsObject | Builtin::CreateUnmappedArgumentsObject | Builtin::MappedArgumentsBindingRead | Builtin::MappedArgumentsBindingWrite,
            console::dispatch_console => Builtin::ConsoleLog | Builtin::ConsoleInfo | Builtin::ConsoleDebug | Builtin::ConsoleWarn | Builtin::ConsoleError | Builtin::ConsoleTrace,
            errors::dispatch_error => Builtin::ErrorConstructor | Builtin::EvalErrorConstructor | Builtin::RangeErrorConstructor | Builtin::ReferenceErrorConstructor | Builtin::SyntaxErrorConstructor | Builtin::TypeErrorConstructor | Builtin::URIErrorConstructor,
            eval::dispatch_eval => Builtin::Eval | Builtin::EvalIndirect | Builtin::EvalGetBinding | Builtin::EvalSetBinding | Builtin::EvalHasBinding | Builtin::EvalDeleteBinding | Builtin::EvalSuperBase | Builtin::EvalWithBase,
            global_env::dispatch_global_env => Builtin::GlobalEnvCheck | Builtin::GlobalEnvDeclareVar | Builtin::GlobalEnvDeclareFunc | Builtin::GlobalEnvDeclareLex | Builtin::GlobalEnvInitLex | Builtin::GlobalEnvGet | Builtin::GlobalEnvSet | Builtin::GlobalEnvDelete,
            with_env::dispatch_with => Builtin::WithHasBinding | Builtin::WithToObject,
            iterator::dispatch_iterator => Builtin::IteratorFrom | Builtin::StringIterator | Builtin::IteratorDone | Builtin::IteratorValue | Builtin::IteratorStepValue | Builtin::IteratorNext | Builtin::IteratorClose | Builtin::IteratorCloseThrowCompletion,
            node_perf_hooks::dispatch_perf => Builtin::PerformanceNow,
            operator::dispatch_operator => Builtin::AbstractCompare | Builtin::AbstractEq | Builtin::StrictEq | Builtin::TypeOf | Builtin::InstanceOf | Builtin::In | Builtin::Throw | Builtin::ExceptionValue | Builtin::NewTarget | Builtin::Debugger | Builtin::IsCallable | Builtin::IsJsObject | Builtin::GetPrototypeFromConstructor | Builtin::IsString | Builtin::TdzCheck | Builtin::ToPropertyKey | Builtin::ThisTdzCheck | Builtin::SuperCallOnceCheck,
            structured_clone::dispatch_structured_clone => Builtin::StructuredClone,
            intrinsics::dispatch_intrinsics => Builtin::IntrinsicPristine | Builtin::IntrinsicResolve,
            timer::dispatch_timer => Builtin::SetTimeout | Builtin::SetInterval | Builtin::ClearTimeout | Builtin::ClearInterval,
            jsx::dispatch_jsx => Builtin::JsxCreateElement,
        }
    }
}
