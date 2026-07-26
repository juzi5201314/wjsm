//! Generator 宿主 builtin。

use wjsm_host::{ExecContext, Value};
use wjsm_ir::{constants, value, wk_symbol};

/// `env.generator_start(continuation)`。
pub fn generator_start<E: ExecContext>(ctx: &mut E, continuation: Value) -> Value {
    let generator = ctx.alloc_object(4);
    let generator_proto = ctx.generator_prototype();
    if !value::is_undefined(generator_proto) {
        ctx.set_object_proto(generator, generator_proto);
    }
    if !value::is_object(generator) {
        return value::encode_undefined();
    }
    let next = ctx.create_generator_method(generator, 0);
    let ret = ctx.create_generator_method(generator, 1);
    let throw = ctx.create_generator_method(generator, 2);
    let iterator_identity = ctx.create_generator_identity(generator);
    ctx.define_data_property(generator, "next", next);
    ctx.define_data_property(generator, "return", ret);
    ctx.define_data_property(generator, "throw", throw);
    ctx.define_data_property_by_name_id(
        generator,
        wjsm_host::encode_symbol_name_id(wk_symbol::ITERATOR),
        iterator_identity,
        constants::FLAG_CONFIGURABLE | constants::FLAG_WRITABLE,
    );
    ctx.init_generator_entry(generator, continuation)
}

/// `env.generator_next`。
pub fn generator_next<E: ExecContext>(ctx: &mut E, generator: Value, value: Value) -> Value {
    ctx.generator_next(generator, value)
}

/// `env.generator_return`。
pub fn generator_return<E: ExecContext>(ctx: &mut E, generator: Value, value: Value) -> Value {
    ctx.generator_return(generator, value)
}

/// `env.generator_throw`。
pub fn generator_throw<E: ExecContext>(ctx: &mut E, generator: Value, value: Value) -> Value {
    ctx.generator_throw(generator, value)
}
