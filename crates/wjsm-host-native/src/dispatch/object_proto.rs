//! `%Object.prototype%` 按需求值方法的宿主适配层：算法本体在
//! `wjsm_builtins::object_proto`（后端无关），本模块只实现统一对象协议
//! [`ObjectProtocol`]——把 ToObject 分类、[[GetPrototypeOf]] /
//! [[SetPrototypeOf]]（含 Proxy trap）、单层 [[GetOwnProperty]] 归约
//! （普通对象 / callable / 数组 / Proxy / RegExp / 基元包装的 exotic 自有
//! 属性）与 Call / Get / TypeError 映射到宿主内部结构。
//!
//! 这些函数对象在 `ensure_intrinsic_prototypes` 中一次性安装为
//! `%Object.prototype%` 的真实自有属性；本模块只负责调用期分发。

use wjsm_builtins::object_proto::{self, ObjectProtocol, OwnProperty};
use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::object;
use super::runtime::{
    fail_dispatch, get_property, object_handle, property_key, to_property_key_value, type_error,
};
use crate::{NativeAgentState, PropertyKey};

pub(super) fn dispatch_object_proto(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    let this = nth_arg(args, 0);
    let mut protocol = HostObjectProtocol { ctx, state };
    let result = match builtin {
        Builtin::ObjectProtoIsPrototypeOf => {
            object_proto::is_prototype_of(&mut protocol, this, nth_arg(args, 1))
        }
        Builtin::HasOwnProperty => {
            object_proto::has_own_property(&mut protocol, this, nth_arg(args, 1))
        }
        Builtin::ObjectHasOwn => {
            object_proto::object_has_own(&mut protocol, this, nth_arg(args, 1))
        }
        Builtin::PropertyIsEnumerable => {
            object_proto::property_is_enumerable(&mut protocol, this, nth_arg(args, 1))
        }
        Builtin::ObjectProtoToLocaleString => object_proto::to_locale_string(&mut protocol, this),
        Builtin::ObjectProtoGetProto => object_proto::proto_getter(&mut protocol, this),
        Builtin::ObjectProtoSetProto => {
            object_proto::proto_setter(&mut protocol, this, nth_arg(args, 1))
        }
        Builtin::ObjectProtoDefineGetter => object_proto::define_accessor_member(
            &mut protocol,
            this,
            nth_arg(args, 1),
            nth_arg(args, 2),
            true,
        ),
        Builtin::ObjectProtoDefineSetter => object_proto::define_accessor_member(
            &mut protocol,
            this,
            nth_arg(args, 1),
            nth_arg(args, 2),
            false,
        ),
        Builtin::ObjectProtoLookupGetter => {
            object_proto::lookup_accessor_member(&mut protocol, this, nth_arg(args, 1), true)
        }
        Builtin::ObjectProtoLookupSetter => {
            object_proto::lookup_accessor_member(&mut protocol, this, nth_arg(args, 1), false)
        }
        _ => return None,
    };
    Some(result.unwrap_or_else(|exception| exception))
}

fn nth_arg(args: &[i64], index: usize) -> i64 {
    args.get(index)
        .copied()
        .unwrap_or_else(value::encode_undefined)
}

/// 统一对象协议的宿主实现：全部对象类别（普通堆对象 / callable / 数组 /
/// Proxy / RegExp / 基元包装）走同一组内部方法，Proxy trap 与异常语义
/// 由底层 `object::*` / `proxy::*` 分发保证。
struct HostObjectProtocol<'a, 'b> {
    ctx: &'a mut NativeVmContext,
    state: &'b mut NativeAgentState,
}

impl ObjectProtocol for HostObjectProtocol<'_, '_> {
    fn is_object(&mut self, encoded: i64) -> bool {
        is_ecma_object(encoded)
    }

    fn is_callable(&mut self, encoded: i64) -> bool {
        self.state.value_is_callable(encoded)
    }

    fn same_object(&mut self, left: i64, right: i64) -> bool {
        value::strip_gc_color(left) == value::strip_gc_color(right)
    }

    fn prototype_of(&mut self, object: i64) -> Result<i64, i64> {
        let result = object::get_prototype(self.ctx, self.state, &[object]);
        if value::is_exception(result) {
            return Err(result);
        }
        Ok(result)
    }

    fn primitive_prototype(&mut self, primitive: i64) -> Result<i64, i64> {
        self.state
            .primitive_wrapper_prototype(primitive)
            .ok_or_else(|| fail_dispatch(self.ctx))
    }

    fn set_prototype_of(&mut self, object: i64, prototype: i64) -> Result<bool, i64> {
        // [[SetPrototypeOf]] 的拒绝路径（环 / 不可扩展 / proxy trap falsish）
        // 在 `object::set_prototype` 内已折算为 TypeError 异常值。
        let result = object::set_prototype(self.ctx, self.state, &[object, prototype]);
        if value::is_exception(result) {
            return Err(result);
        }
        Ok(true)
    }

    fn own_property(&mut self, holder: i64, key: i64) -> Result<OwnProperty, i64> {
        own_property(self.ctx, self.state, holder, key)
    }

    fn to_property_key(&mut self, encoded: i64) -> Result<i64, i64> {
        to_property_key_value(self.ctx, self.state, encoded)
    }

    fn define_accessor(
        &mut self,
        object: i64,
        key: i64,
        accessor: i64,
        is_getter: bool,
    ) -> Result<(), i64> {
        let descriptor = accessor_descriptor(self.ctx, self.state, accessor, is_getter)
            .ok_or_else(|| fail_dispatch(self.ctx))?;
        let result = object::define_property(self.ctx, self.state, &[object, key, descriptor]);
        if value::is_exception(result) {
            return Err(result);
        }
        Ok(())
    }

    fn get_named(&mut self, object: i64, name: &str) -> Result<i64, i64> {
        let key = self
            .state
            .intern_property_string(name.into())
            .ok_or_else(|| fail_dispatch(self.ctx))?;
        let result = get_property(self.ctx, self.state, object, key.to_value())
            .map_err(|()| fail_dispatch(self.ctx))?;
        if value::is_exception(result) {
            return Err(result);
        }
        Ok(result)
    }

    fn call(&mut self, callable: i64, this_value: i64, arguments: &[i64]) -> Result<i64, i64> {
        let result = self
            .state
            .invoke_callable(self.ctx, callable, this_value, arguments)
            .ok_or_else(|| fail_dispatch(self.ctx))?;
        if value::is_exception(result) {
            return Err(result);
        }
        Ok(result)
    }

    fn type_error(&mut self, message: &str) -> i64 {
        type_error(self.ctx, self.state, message)
    }

    fn describe_non_callable(&mut self, encoded: i64) -> String {
        non_callable_description(self.state, encoded)
    }
}

/// 规范意义上的 Object 值：本引擎 TAG_REGEXP 独立于 `is_js_object`
/// （后者已含数组 / callable / Proxy）。
pub(super) fn is_ecma_object(encoded: i64) -> bool {
    value::is_js_object(encoded) || value::is_regexp(encoded)
}

/// V8 对 Invoke 命中非 callable 值的措辞（`number 1 is not a function` 等）。
fn non_callable_description(state: &mut NativeAgentState, encoded: i64) -> String {
    if value::is_f64(encoded) {
        return format!(
            "number {}",
            wjsm_builtins::format_number_js(value::decode_f64(encoded))
        );
    }
    if value::is_string(encoded) {
        let text = state
            .string_owned(encoded)
            .and_then(|text| text.to_utf8())
            .unwrap_or_default();
        return format!("string \"{text}\"");
    }
    if value::is_bool(encoded) {
        return format!("boolean {}", value::decode_bool(encoded));
    }
    if value::is_null(encoded) {
        return "object null".to_owned();
    }
    if value::is_undefined(encoded) {
        return "undefined".to_owned();
    }
    if value::is_bigint(encoded) {
        return "bigint".to_owned();
    }
    if value::is_symbol(encoded) {
        return "symbol".to_owned();
    }
    "object".to_owned()
}

/// {get|set, enumerable: true, configurable: true} 描述符对象。
fn accessor_descriptor(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    accessor: i64,
    is_getter: bool,
) -> Option<i64> {
    let descriptor = state.allocate_object_with_gc_retry(ctx, 3, false).ok()?;
    let handle = value::decode_handle(descriptor);
    let slot = if is_getter { "get" } else { "set" };
    for (name, stored) in [
        (slot, accessor),
        ("enumerable", value::encode_bool(true)),
        ("configurable", value::encode_bool(true)),
    ] {
        let key = state.intern_property_string(name.into())?;
        state
            .gc
            .heap()
            .set_property(handle, key, stored as u64)
            .ok()?;
    }
    Some(descriptor)
}

/// 单层 [[GetOwnProperty]] 的统一归约：按对象类别分派——Proxy 走
/// getOwnPropertyDescriptor trap，callable / 数组走宿主旁挂表，RegExp 的
/// exotic 自有层只有 lastIndex（flags / source 等是 %RegExp.prototype% 的
/// 继承成员），字符串基元与其包装对象归约索引 / length exotic 自有属性，
/// 普通堆对象读属性槽。
pub(super) fn own_property(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    holder: i64,
    encoded_key: i64,
) -> Result<OwnProperty, i64> {
    if value::is_proxy(holder) {
        return proxy_own_property(ctx, state, holder, encoded_key);
    }
    if value::is_regexp(holder) {
        if state.text_matches(encoded_key, "lastIndex") {
            return Ok(OwnProperty::Data { enumerable: false });
        }
        return Ok(OwnProperty::Missing);
    }
    if value::is_string(holder) {
        return Ok(string_exotic_own_property(state, holder, encoded_key));
    }
    let Some(key) = property_key(state, encoded_key) else {
        return Err(fail_dispatch(ctx));
    };
    if value::is_callable(holder) {
        let callable = value::strip_gc_color(holder);
        // 先触发 name / length / prototype 的惰性物化，再查旁挂表。
        let _ = state.callable_property(callable, key);
        let enumerable = state
            .callable_property_flags
            .get(&(callable, key))
            .is_some_and(|flags| flags & wjsm_ir::constants::FLAG_ENUMERABLE as u32 != 0);
        if let Some((getter, setter)) = state.callable_accessors.get(&(callable, key)).copied() {
            return Ok(OwnProperty::Accessor {
                getter,
                setter,
                enumerable,
            });
        }
        if state.callable_properties.contains_key(&(callable, key)) {
            return Ok(OwnProperty::Data { enumerable });
        }
        return Ok(OwnProperty::Missing);
    }
    if value::is_array(holder) {
        return array_own_property(state, holder, key, encoded_key);
    }
    let Some(handle) = object_handle(holder) else {
        return Ok(OwnProperty::Missing);
    };
    // 基元包装对象（boxed primitive）的 exotic 自有层：字符串索引 / length。
    if let Some(primitive) = state
        .boxed_primitives
        .get(&value::decode_handle(holder))
        .copied()
        && value::is_string(primitive)
    {
        match string_exotic_own_property(state, primitive, encoded_key) {
            OwnProperty::Missing => {}
            own => return Ok(own),
        }
    }
    match state.gc.heap().get_property_slot(handle, key) {
        Ok(Some(property)) => {
            let enumerable = property.flags & wjsm_ir::constants::FLAG_ENUMERABLE as u32 != 0;
            if property.flags & wjsm_ir::constants::FLAG_IS_ACCESSOR as u32 != 0 {
                Ok(OwnProperty::Accessor {
                    getter: property.getter as i64,
                    setter: property.setter as i64,
                    enumerable,
                })
            } else {
                Ok(OwnProperty::Data { enumerable })
            }
        }
        Ok(None) => Ok(OwnProperty::Missing),
        Err(_) => Err(fail_dispatch(ctx)),
    }
}

/// 字符串 exotic 自有属性（§10.4.3.5）：有效索引与 length。
fn string_exotic_own_property(
    state: &mut NativeAgentState,
    text: i64,
    encoded_key: i64,
) -> OwnProperty {
    if state.text_matches(encoded_key, "length") {
        return OwnProperty::Data { enumerable: false };
    }
    if let Some(index) = super::runtime::array_index(state, encoded_key)
        && state
            .string_len(text)
            .is_some_and(|length| (index as usize) < length)
    {
        return OwnProperty::Data { enumerable: true };
    }
    OwnProperty::Missing
}

/// 数组自有层：length、旁挂命名属性 / 访问器、元素槽。
fn array_own_property(
    state: &mut NativeAgentState,
    holder: i64,
    key: PropertyKey,
    encoded_key: i64,
) -> Result<OwnProperty, i64> {
    let handle = value::decode_handle(holder);
    if let Some((getter, setter, flags)) = state.array_accessors.get(&(handle, key)).copied() {
        return Ok(OwnProperty::Accessor {
            getter,
            setter,
            enumerable: flags & wjsm_ir::constants::FLAG_ENUMERABLE as u32 != 0,
        });
    }
    if state.text_matches(encoded_key, "length") {
        return Ok(OwnProperty::Data { enumerable: false });
    }
    if state.array_properties.contains_key(&(handle, key)) {
        let enumerable = state
            .array_property_flags
            .get(&(handle, key))
            .copied()
            .unwrap_or(
                wjsm_ir::constants::FLAG_WRITABLE as u32
                    | wjsm_ir::constants::FLAG_ENUMERABLE as u32
                    | wjsm_ir::constants::FLAG_CONFIGURABLE as u32,
            )
            & wjsm_ir::constants::FLAG_ENUMERABLE as u32
            != 0;
        return Ok(OwnProperty::Data { enumerable });
    }
    if let Some(index) = super::runtime::array_index(state, encoded_key)
        && state
            .gc
            .heap()
            .get_element(handle, index)
            .ok()
            .flatten()
            .is_some_and(|element| !value::is_array_hole(element as i64))
    {
        return Ok(OwnProperty::Data { enumerable: true });
    }
    Ok(OwnProperty::Missing)
}

/// Proxy 层：经 [[GetOwnProperty]] trap 产出的描述符对象读取 get / set /
/// enumerable（trap 异常原样传播）。
fn proxy_own_property(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    holder: i64,
    encoded_key: i64,
) -> Result<OwnProperty, i64> {
    let descriptor = object::get_own_property_descriptor(ctx, state, &[holder, encoded_key]);
    if value::is_exception(descriptor) {
        return Err(descriptor);
    }
    if value::is_undefined(descriptor) {
        return Ok(OwnProperty::Missing);
    }
    let handle = value::decode_handle(descriptor);
    let mut sides = [value::encode_undefined(), value::encode_undefined()];
    let mut is_accessor = false;
    for (index, name) in ["get", "set"].into_iter().enumerate() {
        let Some(key) = state.intern_property_string(name.into()) else {
            return Err(fail_dispatch(ctx));
        };
        if let Ok(Some(property)) = state.gc.heap().get_property_slot(handle, key) {
            sides[index] = property.value as i64;
            is_accessor = true;
        }
    }
    let Some(enumerable_key) = state.intern_property_string("enumerable".into()) else {
        return Err(fail_dispatch(ctx));
    };
    let enumerable = state
        .gc
        .heap()
        .get_property(handle, enumerable_key)
        .ok()
        .flatten()
        .is_some_and(|stored| super::runtime::is_truthy(state, stored as i64));
    if is_accessor {
        return Ok(OwnProperty::Accessor {
            getter: sides[0],
            setter: sides[1],
            enumerable,
        });
    }
    Ok(OwnProperty::Data { enumerable })
}
