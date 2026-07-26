//! is_callable / queue_microtask / dynamic_import / jsx 等杂项。

use wjsm_host::{ExecContext, Value};
use wjsm_ir::value;

pub fn is_callable<E: ExecContext>(ctx: &mut E, val: Value) -> Value {
    value::encode_bool(ctx.is_callable(val))
}

pub fn is_js_object(_ctx: &mut impl ExecContext, val: Value) -> Value {
    value::encode_bool(value::is_js_object(val) || value::is_regexp(val))
}

pub fn queue_microtask<E: ExecContext>(ctx: &mut E, callback: Value) {
    ctx.queue_microtask(callback);
}

pub fn register_module_namespace<E: ExecContext>(
    ctx: &mut E,
    module_id: Value,
    namespace_obj: Value,
) {
    ctx.register_module_namespace(module_id as u32, namespace_obj);
}

pub fn dynamic_import<E: ExecContext>(ctx: &mut E, module_id: Value) -> Value {
    ctx.dynamic_import(module_id as u32)
}

pub fn jsx_create_element<E: ExecContext>(
    ctx: &mut E,
    tag: Value,
    props: Value,
    children: Value,
) -> Value {
    let obj = ctx.alloc_object(4);
    ctx.define_data_property(obj, "type", tag);
    ctx.define_data_property(obj, "props", props);
    ctx.define_data_property(obj, "children", children);
    obj
}
