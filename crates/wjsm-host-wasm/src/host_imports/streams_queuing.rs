// QueuingStrategy 实现（WHATWG Streams Phase 2）— 薄注册层。
use crate::RuntimeState;
use crate::exec_context_impl::WasmExecContext;
use wasmtime::Caller;
use wjsm_host::QueuingStrategySizeKind;

fn convert_kind(kind: crate::QueuingStrategySizeKind) -> QueuingStrategySizeKind {
    match kind {
        crate::QueuingStrategySizeKind::Count => QueuingStrategySizeKind::Count,
        crate::QueuingStrategySizeKind::ByteLength => QueuingStrategySizeKind::ByteLength,
    }
}

pub(crate) fn construct_count_queuing_strategy(
    caller: &mut Caller<'_, RuntimeState>,
    _this_val: i64,
    args: &[i64],
) -> Option<i64> {
    let mut ctx = WasmExecContext::new(caller);
    wjsm_builtins::streams_queuing::construct_count_queuing_strategy(&mut ctx, _this_val, args)
}

pub(crate) fn construct_byte_length_queuing_strategy(
    caller: &mut Caller<'_, RuntimeState>,
    _this_val: i64,
    args: &[i64],
) -> Option<i64> {
    let mut ctx = WasmExecContext::new(caller);
    wjsm_builtins::streams_queuing::construct_byte_length_queuing_strategy(
        &mut ctx, _this_val, args,
    )
}

pub(crate) fn call_queuing_strategy_size_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    kind: crate::QueuingStrategySizeKind,
    args: &[i64],
) -> Option<i64> {
    let mut ctx = WasmExecContext::new(caller);
    wjsm_builtins::streams_queuing::call_queuing_strategy_size(&mut ctx, convert_kind(kind), args)
}
