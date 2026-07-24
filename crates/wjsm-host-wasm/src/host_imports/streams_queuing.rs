// QueuingStrategy 实现（WHATWG Streams Phase 2）

use super::fetch_core::{object_property, push_native_callable};
use crate::*;

fn high_water_mark_from_init(caller: &mut Caller<'_, RuntimeState>, init: i64) -> i64 {
    if value::is_object(init)
        && let Some(raw) = object_property(caller, init, "highWaterMark")
        && value::is_f64(raw)
    {
        return raw;
    }
    value::encode_f64(0.0)
}

fn create_queuing_strategy_object(
    caller: &mut Caller<'_, RuntimeState>,
    high_water_mark: i64,
    kind: QueuingStrategySizeKind,
) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 2);
    let _ = define_host_data_property_from_caller(caller, obj, "highWaterMark", high_water_mark);
    let size_callable = NativeCallable::QueuingStrategySize { kind };
    let size_idx = push_native_callable(caller, size_callable);
    let size_val = value::encode_native_callable_idx(size_idx);
    let _ = define_host_data_property_from_caller(caller, obj, "size", size_val);
    obj
}

pub(crate) fn construct_count_queuing_strategy(
    caller: &mut Caller<'_, RuntimeState>,
    _this_val: i64,
    args: &[i64],
) -> Option<i64> {
    let init = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let high_water_mark = high_water_mark_from_init(caller, init);
    Some(create_queuing_strategy_object(
        caller,
        high_water_mark,
        QueuingStrategySizeKind::Count,
    ))
}

pub(crate) fn construct_byte_length_queuing_strategy(
    caller: &mut Caller<'_, RuntimeState>,
    _this_val: i64,
    args: &[i64],
) -> Option<i64> {
    let init = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let high_water_mark = high_water_mark_from_init(caller, init);
    Some(create_queuing_strategy_object(
        caller,
        high_water_mark,
        QueuingStrategySizeKind::ByteLength,
    ))
}

pub(crate) fn call_queuing_strategy_size_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    kind: QueuingStrategySizeKind,
    args: &[i64],
) -> Option<i64> {
    match kind {
        QueuingStrategySizeKind::Count => Some(value::encode_f64(1.0)),
        QueuingStrategySizeKind::ByteLength => {
            let chunk = args
                .first()
                .copied()
                .unwrap_or_else(value::encode_undefined);
            if value::is_object(chunk)
                && let Some(byte_length) = object_property(caller, chunk, "byteLength")
            {
                return Some(byte_length);
            }
            Some(value::encode_undefined())
        }
    }
}
