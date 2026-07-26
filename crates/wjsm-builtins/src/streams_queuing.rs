//! QueuingStrategy 实现（WHATWG Streams Phase 2）— 后端无关算法。

use wjsm_host::{ExecContext, NativeCallableRef, QueuingStrategySizeKind, Value};
use wjsm_ir::value;

/// 从 init 对象读取 `highWaterMark`（f64）；不存在或非数字返回 0.0。
fn high_water_mark_from_init<E: ExecContext>(ctx: &mut E, init: Value) -> Value {
    if value::is_object(init)
        || value::is_function(init)
        || value::is_array(init)
        || value::is_proxy(init)
    {
        let raw = ctx.read_data_property(init, "highWaterMark");
        if value::is_f64(raw) {
            return raw;
        }
    }
    value::encode_f64(0.0)
}

/// 创建 QueuingStrategy 对象：`{ highWaterMark, size }`。
fn create_queuing_strategy_object<E: ExecContext>(
    ctx: &mut E,
    high_water_mark: Value,
    kind: QueuingStrategySizeKind,
) -> Value {
    let obj = ctx.alloc_object(2);
    ctx.define_data_property(obj, "highWaterMark", high_water_mark);
    let size_val = ctx.create_native_callable(NativeCallableRef::QueuingStrategySize { kind });
    ctx.define_data_property(obj, "size", size_val);
    obj
}

/// `CountQueuingStrategy` 构造器。
pub fn construct_count_queuing_strategy<E: ExecContext>(
    ctx: &mut E,
    _this_val: Value,
    args: &[Value],
) -> Option<Value> {
    let init = args.first().copied().unwrap_or_else(value::encode_undefined);
    let high_water_mark = high_water_mark_from_init(ctx, init);
    Some(create_queuing_strategy_object(ctx, high_water_mark, QueuingStrategySizeKind::Count))
}

/// `ByteLengthQueuingStrategy` 构造器。
pub fn construct_byte_length_queuing_strategy<E: ExecContext>(
    ctx: &mut E,
    _this_val: Value,
    args: &[Value],
) -> Option<Value> {
    let init = args.first().copied().unwrap_or_else(value::encode_undefined);
    let high_water_mark = high_water_mark_from_init(ctx, init);
    Some(create_queuing_strategy_object(
        ctx,
        high_water_mark,
        QueuingStrategySizeKind::ByteLength,
    ))
}

/// QueuingStrategy `size(chunk)` 调用分派。
pub fn call_queuing_strategy_size<E: ExecContext>(
    ctx: &mut E,
    kind: QueuingStrategySizeKind,
    args: &[Value],
) -> Option<Value> {
    match kind {
        QueuingStrategySizeKind::Count => Some(value::encode_f64(1.0)),
        QueuingStrategySizeKind::ByteLength => {
            let chunk = args.first().copied().unwrap_or_else(value::encode_undefined);
            if value::is_object(chunk)
                || value::is_function(chunk)
                || value::is_array(chunk)
                || value::is_proxy(chunk)
            {
                let byte_length = ctx.read_data_property(chunk, "byteLength");
                return Some(byte_length);
            }
            Some(value::encode_undefined())
        }
    }
}
