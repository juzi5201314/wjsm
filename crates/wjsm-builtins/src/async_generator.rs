//! AsyncGenerator 宿主 builtin。

use wjsm_host::{ExecContext, Value};
use wjsm_ir::{constants, value, wk_symbol};

/// `env.async_generator_start(continuation)`。
pub fn async_generator_start<E: ExecContext>(ctx: &mut E, continuation: Value) -> Value {
    let generator = ctx.alloc_object(4);
    let async_gen_proto = ctx.async_generator_prototype();
    if !value::is_undefined(async_gen_proto) {
        ctx.set_object_proto(generator, async_gen_proto);
    }
    if !value::is_object(generator) {
        return value::encode_undefined();
    }
    let next = ctx.create_async_generator_method(generator, 0);
    let ret = ctx.create_async_generator_method(generator, 1);
    let throw = ctx.create_async_generator_method(generator, 2);
    ctx.define_data_property(generator, "next", next);
    ctx.define_data_property(generator, "return", ret);
    ctx.define_data_property(generator, "throw", throw);
    let async_iter = ctx.create_async_generator_identity(generator);
    // Symbol.asyncIterator = well-known id 3（与启动表一致）
    ctx.define_data_property_by_name_id(
        generator,
        wjsm_host::encode_symbol_name_id(wk_symbol::ASYNC_ITERATOR),
        async_iter,
        constants::FLAG_CONFIGURABLE | constants::FLAG_WRITABLE,
    );
    ctx.init_async_generator_entry(generator, continuation);
    generator
}

pub fn async_generator_next<E: ExecContext>(ctx: &mut E, generator: Value, value: Value) -> Value {
    ctx.async_generator_next(generator, value)
}

pub fn async_generator_return<E: ExecContext>(
    ctx: &mut E,
    generator: Value,
    value: Value,
) -> Value {
    ctx.async_generator_return(generator, value)
}

pub fn async_generator_throw<E: ExecContext>(ctx: &mut E, generator: Value, value: Value) -> Value {
    ctx.async_generator_throw(generator, value)
}
