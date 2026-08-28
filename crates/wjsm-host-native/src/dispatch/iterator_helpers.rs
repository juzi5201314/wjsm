//! Iterator Helpers（ES2025 §27.1）：%Iterator% 抽象构造器、%Iterator.prototype%、
//! %IteratorHelperPrototype% 与 %WrapForValidIteratorPrototype% 的物化与分派。
//!
//! 原型方法安装为堆原型对象的真实自有属性；内部迭代器实例（数组/字符串/集合
//! 迭代器与生成器）经 `instance_method` 惰性合成读取同一批方法值，保证
//! `[].values().map === Iterator.prototype.map` 的函数身份一致。

mod eager;
mod lazy;

use std::collections::HashMap;

use wjsm_gc::PropertyKey;
use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::runtime::{
    create_data_property_impl, create_iterator_result, fail_dispatch, get_property, is_truthy,
    render_value, to_number_coerced, type_error,
};
use crate::{BUILTIN_PROTOTYPE_PROPERTY_FLAGS, NativeAgentState, NativeCallableKind};

pub(crate) use eager::eager_method;
pub(crate) use lazy::{create_helper, helper_next, helper_return};

/// %Iterator.prototype% 上 11 个 helper 方法的分派标识（§27.1.4.2–27.1.4.12）。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum IteratorProtoMethod {
    Map,
    Filter,
    Take,
    Drop,
    FlatMap,
    Reduce,
    ToArray,
    ForEach,
    Some,
    Every,
    Find,
}

impl IteratorProtoMethod {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Map => "map",
            Self::Filter => "filter",
            Self::Take => "take",
            Self::Drop => "drop",
            Self::FlatMap => "flatMap",
            Self::Reduce => "reduce",
            Self::ToArray => "toArray",
            Self::ForEach => "forEach",
            Self::Some => "some",
            Self::Every => "every",
            Self::Find => "find",
        }
    }

    /// 规范 length 属性值（可选参数不计入）。
    pub(crate) fn length(self) -> u32 {
        match self {
            Self::ToArray => 0,
            _ => 1,
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "map" => Self::Map,
            "filter" => Self::Filter,
            "take" => Self::Take,
            "drop" => Self::Drop,
            "flatMap" => Self::FlatMap,
            "reduce" => Self::Reduce,
            "toArray" => Self::ToArray,
            "forEach" => Self::ForEach,
            "some" => Self::Some,
            "every" => Self::Every,
            "find" => Self::Find,
            _ => return None,
        })
    }
}

/// Iterator Record（§7.4.1）的宿主形态：[[Iterator]] 与已缓存的 [[NextMethod]]。
#[derive(Clone, Copy)]
pub(crate) struct IteratorRecord {
    pub(crate) iterator: i64,
    pub(crate) next: i64,
}

/// 惰性 helper 的生成器式运行状态（§27.1.2 Iterator Helper 按生成器规范化）。
#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum HelperRunState {
    SuspendedStart,
    SuspendedYield,
    Executing,
    Completed,
}

/// 惰性 helper 的种类与闭包携带值（回调函数或剩余计数）。
#[derive(Clone, Copy)]
pub(crate) enum HelperKind {
    Map(i64),
    Filter(i64),
    FlatMap(i64),
    /// take 的 remaining（+∞ 表示无限制，§27.1.4.11）。
    Take(f64),
    /// drop 的待跳过计数（+∞ 表示全部消费，§27.1.4.5）。
    Drop(f64),
}

/// 单个 Iterator Helper 对象的内部槽（[[UnderlyingIterator]] 等）。
#[derive(Clone, Copy)]
pub(crate) struct IteratorHelper {
    pub(crate) kind: HelperKind,
    pub(crate) underlying: IteratorRecord,
    pub(crate) counter: u64,
    pub(crate) run: HelperRunState,
    /// flatMap 当前内层迭代器 record（§27.1.4.8 步骤 viii）。
    pub(crate) inner: Option<IteratorRecord>,
}

/// Iterator Helpers 家族的宿主状态：intrinsic 对象与实例侧表。
#[derive(Default)]
pub(crate) struct IteratorHelpersState {
    pub(crate) constructor: Option<i64>,
    pub(crate) prototype: Option<i64>,
    pub(crate) helper_prototype: Option<i64>,
    pub(crate) wrap_prototype: Option<i64>,
    pub(crate) helpers: HashMap<u32, IteratorHelper>,
    pub(crate) wraps: HashMap<u32, IteratorRecord>,
}

impl IteratorHelpersState {
    pub(crate) fn clear(&mut self) {
        self.constructor = None;
        self.prototype = None;
        self.helper_prototype = None;
        self.wrap_prototype = None;
        self.helpers.clear();
        self.wraps.clear();
    }
}

/// ECMAScript「is an Object」判定（含宿主 exotic 表示）。
pub(crate) fn is_object_value(encoded: i64) -> bool {
    value::is_js_object(encoded)
        || value::is_array(encoded)
        || value::is_callable(encoded)
        || value::is_proxy(encoded)
        || value::is_regexp(encoded)
}

/// %Iterator% 抽象构造器（§27.1.3.1）：own `prototype`（三特性全 false）与
/// `from`（§27.1.3.2.1）懒创建，`Iterator` 全局与 `constructor` getter 共用。
pub(crate) fn ensure_constructor(state: &mut NativeAgentState) -> Option<i64> {
    if let Some(constructor) = state.iterator_helpers.constructor {
        return Some(constructor);
    }
    let prototype = ensure_prototype(state)?;
    let constructor = state.native_callable(NativeCallableKind::IteratorConstructor)?;
    let prototype_key = state.intern_property_string("prototype".into())?;
    state
        .callable_properties
        .insert((constructor, prototype_key), prototype);
    state
        .callable_property_flags
        .insert((constructor, prototype_key), 0);
    let from = state.native_callable(NativeCallableKind::IteratorStaticFrom)?;
    let from_key = state.intern_property_string("from".into())?;
    state.callable_properties.insert((constructor, from_key), from);
    state
        .callable_property_flags
        .insert((constructor, from_key), BUILTIN_PROTOTYPE_PROPERTY_FLAGS);
    state.iterator_helpers.constructor = Some(constructor);
    Some(constructor)
}

/// %Iterator.prototype%（§27.1.4）：[[Prototype]] 为 %Object.prototype%
/// （allocate_object 缺省），安装 `constructor` / @@toStringTag 访问器对
/// （SetterThatIgnoresPrototypeProperties，§27.1.4.1）、11 个 helper 方法与
/// @@iterator（返回 this，§27.1.4.13）；own key 顺序对齐 V8。
pub(crate) fn ensure_prototype(state: &mut NativeAgentState) -> Option<i64> {
    if let Some(prototype) = state.iterator_helpers.prototype {
        return Some(prototype);
    }
    state.ensure_intrinsic_prototypes().ok()?;
    let prototype = state.allocate_object(16, false).ok()?;
    // 先登记再安装成员：登记表是 GC 根，安装期间的 intern/分配不会回收
    // 尚未挂满成员的 prototype 对象。
    state.iterator_helpers.prototype = Some(prototype);
    let handle = value::decode_handle(prototype);
    let configurable = wjsm_ir::constants::FLAG_CONFIGURABLE as u32;
    let ctor_getter = state.native_callable(NativeCallableKind::IteratorConstructorGetter)?;
    let ctor_setter = state.native_callable(NativeCallableKind::IteratorConstructorSetter)?;
    let ctor_key = state.intern_property_string("constructor".into())?;
    state
        .gc
        .heap()
        .define_accessor_property_with_flags(
            handle,
            ctor_key,
            ctor_getter as u64,
            ctor_setter as u64,
            configurable,
        )
        .ok()?;
    for method in [
        IteratorProtoMethod::Reduce,
        IteratorProtoMethod::ToArray,
        IteratorProtoMethod::ForEach,
        IteratorProtoMethod::Some,
        IteratorProtoMethod::Every,
        IteratorProtoMethod::Find,
        IteratorProtoMethod::Map,
        IteratorProtoMethod::Filter,
        IteratorProtoMethod::Take,
        IteratorProtoMethod::Drop,
        IteratorProtoMethod::FlatMap,
    ] {
        let callable = state.native_callable(NativeCallableKind::IteratorProto(method))?;
        let key = state.intern_property_string(method.name().into())?;
        state
            .gc
            .heap()
            .define_data_property(handle, key, callable as u64, BUILTIN_PROTOTYPE_PROPERTY_FLAGS)
            .ok()?;
    }
    let self_fn = state.native_callable(NativeCallableKind::IteratorProtoIterator)?;
    state
        .gc
        .heap()
        .define_data_property(
            handle,
            PropertyKey::symbol(wjsm_ir::wk_symbol::ITERATOR),
            self_fn as u64,
            BUILTIN_PROTOTYPE_PROPERTY_FLAGS,
        )
        .ok()?;
    let tag_getter = state.native_callable(NativeCallableKind::IteratorToStringTagGetter)?;
    let tag_setter = state.native_callable(NativeCallableKind::IteratorToStringTagSetter)?;
    state
        .gc
        .heap()
        .define_accessor_property_with_flags(
            handle,
            PropertyKey::symbol(wjsm_ir::wk_symbol::TO_STRING_TAG),
            tag_getter as u64,
            tag_setter as u64,
            configurable,
        )
        .ok()?;
    Some(prototype)
}

/// %IteratorHelperPrototype%（§27.1.2.1）：父原型 %Iterator.prototype%，
/// own `next` / `return` 与 @@toStringTag 数据属性 "Iterator Helper"
/// （{ writable: false, enumerable: false, configurable: true }）。
pub(crate) fn ensure_helper_prototype(state: &mut NativeAgentState) -> Option<i64> {
    if let Some(prototype) = state.iterator_helpers.helper_prototype {
        return Some(prototype);
    }
    let parent = ensure_prototype(state)?;
    let prototype = state.allocate_object(4, false).ok()?;
    state.iterator_helpers.helper_prototype = Some(prototype);
    let handle = value::decode_handle(prototype);
    state
        .gc
        .heap()
        .set_prototype(handle, value::decode_handle(parent))
        .ok()?;
    for (name, kind) in [
        ("next", NativeCallableKind::IteratorHelperNext),
        ("return", NativeCallableKind::IteratorHelperReturn),
    ] {
        let callable = state.native_callable(kind)?;
        let key = state.intern_property_string(name.into())?;
        state
            .gc
            .heap()
            .define_data_property(handle, key, callable as u64, BUILTIN_PROTOTYPE_PROPERTY_FLAGS)
            .ok()?;
    }
    let tag = state.intern_text("Iterator Helper".into(), value::TAG_STRING)?;
    state
        .gc
        .heap()
        .define_data_property(
            handle,
            PropertyKey::symbol(wjsm_ir::wk_symbol::TO_STRING_TAG),
            tag as u64,
            wjsm_ir::constants::FLAG_CONFIGURABLE as u32,
        )
        .ok()?;
    Some(prototype)
}

/// %WrapForValidIteratorPrototype%（§27.1.3.2.2）：父原型 %Iterator.prototype%，
/// own `next` / `return`（无 @@toStringTag）。
pub(crate) fn ensure_wrap_prototype(state: &mut NativeAgentState) -> Option<i64> {
    if let Some(prototype) = state.iterator_helpers.wrap_prototype {
        return Some(prototype);
    }
    let parent = ensure_prototype(state)?;
    let prototype = state.allocate_object(2, false).ok()?;
    state.iterator_helpers.wrap_prototype = Some(prototype);
    let handle = value::decode_handle(prototype);
    state
        .gc
        .heap()
        .set_prototype(handle, value::decode_handle(parent))
        .ok()?;
    for (name, kind) in [
        ("next", NativeCallableKind::IteratorWrapNext),
        ("return", NativeCallableKind::IteratorWrapReturn),
    ] {
        let callable = state.native_callable(kind)?;
        let key = state.intern_property_string(name.into())?;
        state
            .gc
            .heap()
            .define_data_property(handle, key, callable as u64, BUILTIN_PROTOTYPE_PROPERTY_FLAGS)
            .ok()?;
    }
    Some(prototype)
}

/// 内部迭代器实例（数组/字符串/参数对象/集合迭代器、生成器、helper 与
/// wrapper）对 helper 方法名的惰性合成：语义上这些实例的原型链穿过
/// %Iterator.prototype%（§27.1.2），返回原型对象当前的同名自有属性值，
/// 用户对 `Iterator.prototype.map` 的覆盖 / 删除对实例读取立即可见。
pub(crate) fn instance_method(
    state: &mut NativeAgentState,
    receiver: i64,
    key: &str,
) -> Option<i64> {
    let method = IteratorProtoMethod::from_name(key)?;
    if !is_iterator_instance(state, receiver) {
        return None;
    }
    let prototype = ensure_prototype(state)?;
    let property_key = state.intern_property_string(method.name().into())?;
    let slot = state
        .gc
        .heap()
        .get_property_slot(value::decode_handle(prototype), property_key)
        .ok()??;
    // 访问器覆盖（Object.defineProperty(Iterator.prototype, 'map', {get})）
    // 走不到合成路径的 getter 语义，交还通用链行走：实例真实原型链不含
    // %Iterator.prototype%，此处按数据属性直读即可（覆盖值仍可见）。
    if slot.flags & wjsm_ir::constants::FLAG_IS_ACCESSOR as u32 != 0 {
        return None;
    }
    Some(slot.value as i64)
}

/// 宿主侧「原型链穿过 %Iterator.prototype%」的家族判定（OrdinaryHasInstance
/// 对 %Iterator% 的实例语义，§27.1.3.1 Iterator.from 步骤 2）。
pub(crate) fn is_iterator_instance(state: &NativeAgentState, encoded: i64) -> bool {
    if !is_object_value(encoded) {
        return false;
    }
    if value::is_js_object(encoded) {
        let handle = value::decode_handle(encoded);
        if state.array_iterators.contains_key(&handle)
            || state.iterator_next.contains_key(&handle)
            || state.generators.contains_key(&handle)
            || state.iterator_helpers.helpers.contains_key(&handle)
            || state.iterator_helpers.wraps.contains_key(&handle)
        {
            return true;
        }
    }
    let Some(prototype) = state.iterator_helpers.prototype else {
        return false;
    };
    // 真实堆原型链（Object.create(Iterator.prototype) / class extends Iterator
    // 的实例）：从 [[Prototype]] 起步匹配 %Iterator.prototype%。
    let mut current = encoded;
    let mut depth = 0;
    while depth < 1024 {
        let Some(handle) = (value::is_js_object(current) || value::is_array(current))
            .then(|| value::decode_handle(current))
        else {
            return false;
        };
        let Ok(parent) = state.gc.heap().prototype(handle) else {
            return false;
        };
        let Some(parent) = super::runtime::decode_proto_slot(state, parent) else {
            return false;
        };
        if parent == prototype {
            return true;
        }
        current = parent;
        depth += 1;
    }
    false
}

/// GetIteratorDirect(obj)（§7.4.10）：Get(obj, "next") 组 Iterator Record，
/// next 是否可调用推迟到首次调用时校验（与规范一致）。
pub(crate) fn get_iterator_direct(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
) -> Result<IteratorRecord, i64> {
    let Some(next_key) = state.intern_text("next".into(), value::TAG_STRING) else {
        return Err(fail_dispatch(ctx));
    };
    let next = get_property(ctx, state, object, next_key).map_err(|()| fail_dispatch(ctx))?;
    if value::is_exception(next) {
        return Err(next);
    }
    Ok(IteratorRecord {
        iterator: object,
        next,
    })
}

/// CalledNonCallable 的 V8 渲染（typeof 词 + 值渲染，字符串带引号）。
pub(crate) fn called_non_callable(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    callee: i64,
) -> i64 {
    let rendered = if value::is_undefined(callee) {
        "undefined".to_owned()
    } else if value::is_null(callee) {
        "object null".to_owned()
    } else if value::is_f64(callee) {
        format!("number {}", render_value(state, callee))
    } else if value::is_string(callee) {
        format!("string \"{}\"", render_value(state, callee))
    } else if value::is_bool(callee) {
        format!("boolean {}", render_value(state, callee))
    } else if value::is_symbol(callee) {
        "symbol".to_owned()
    } else if value::is_bigint(callee) {
        "bigint".to_owned()
    } else {
        "object".to_owned()
    };
    let message = format!("{rendered} is not a function");
    type_error(ctx, state, &message)
}

/// IteratorStepValue(iteratorRecord)（§7.4.8）：调用 next、校验结果对象、
/// 读取 done / value。`Ok(None)` 表示迭代完成；异常经 `Err` 传播。
pub(crate) fn step_value(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    record: &IteratorRecord,
) -> Result<Option<i64>, i64> {
    if !value::is_callable(record.next) {
        return Err(called_non_callable(ctx, state, record.next));
    }
    let Some(result) = state.invoke_callable(ctx, record.next, record.iterator, &[]) else {
        return Err(fail_dispatch(ctx));
    };
    if value::is_exception(result) {
        return Err(result);
    }
    if !is_object_value(result) {
        let message = format!(
            "Iterator result {} is not an object",
            render_value(state, result)
        );
        return Err(type_error(ctx, state, &message));
    }
    let Some(done_key) = state.intern_text("done".into(), value::TAG_STRING) else {
        return Err(fail_dispatch(ctx));
    };
    let done = get_property(ctx, state, result, done_key).map_err(|()| fail_dispatch(ctx))?;
    if value::is_exception(done) {
        return Err(done);
    }
    if is_truthy(state, done) {
        return Ok(None);
    }
    let Some(value_key) = state.intern_text("value".into(), value::TAG_STRING) else {
        return Err(fail_dispatch(ctx));
    };
    let stepped = get_property(ctx, state, result, value_key).map_err(|()| fail_dispatch(ctx))?;
    if value::is_exception(stepped) {
        return Err(stepped);
    }
    Ok(Some(stepped))
}

/// IteratorClose(iterator, completion)（§7.4.11）的对象版：throw 完成吞掉
/// close 期一切 JS 异常，normal 完成传播 return 方法的异常与结果校验失败。
pub(crate) fn close_iterator(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    iterator: i64,
    completion: i64,
    completion_is_throw: bool,
) -> i64 {
    if !is_object_value(iterator) {
        return completion;
    }
    let Some(return_key) = state.intern_text("return".into(), value::TAG_STRING) else {
        return fail_dispatch(ctx);
    };
    let Ok(method) = get_property(ctx, state, iterator, return_key) else {
        return fail_dispatch(ctx);
    };
    if value::is_exception(method) {
        return if completion_is_throw { completion } else { method };
    }
    if value::is_undefined(method) || value::is_null(method) {
        return completion;
    }
    if !value::is_callable(method) {
        if completion_is_throw {
            return completion;
        }
        return called_non_callable(ctx, state, method);
    }
    let Some(result) = state.invoke_callable(ctx, method, iterator, &[]) else {
        return fail_dispatch(ctx);
    };
    if value::is_exception(result) {
        return if completion_is_throw { completion } else { result };
    }
    if !is_object_value(result) {
        if completion_is_throw {
            return completion;
        }
        let message = format!(
            "Iterator result {} is not an object",
            render_value(state, result)
        );
        return type_error(ctx, state, &message);
    }
    completion
}

/// GetIteratorFlattenable 的原始值处理策略（§7.4.12）。
#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum PrimitiveHandling {
    /// Iterator.from：字符串原语允许迭代（iterate-string-primitives）。
    IterateStringPrimitives,
    /// flatMap 内层：一切原始值拒绝（reject-primitives）。
    RejectPrimitives,
}

/// GetIteratorFlattenable(obj, primitiveHandling)（§7.4.12）。TypeError 文案
/// 按调用位点对齐 V8：from 报 "Iterator.from called on non-object" /
/// "{渲染} is not iterable"，flatMap 报 "Iterator.prototype.flatMap called on
/// non-object" / "{渲染} is not iterable"。
pub(crate) fn get_iterator_flattenable(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    object: i64,
    handling: PrimitiveHandling,
    non_object_message: &str,
) -> Result<IteratorRecord, i64> {
    if !is_object_value(object)
        && (handling == PrimitiveHandling::RejectPrimitives || !value::is_string(object))
    {
        return Err(type_error(ctx, state, non_object_message));
    }
    let symbol = value::encode_handle(value::TAG_SYMBOL, wjsm_ir::wk_symbol::ITERATOR);
    let method = get_property(ctx, state, object, symbol).map_err(|()| fail_dispatch(ctx))?;
    if value::is_exception(method) {
        return Err(method);
    }
    let iterator = if value::is_undefined(method) || value::is_null(method) {
        object
    } else {
        if !value::is_callable(method) {
            return Err(called_non_callable(ctx, state, method));
        }
        let Some(result) = state.invoke_callable(ctx, method, object, &[]) else {
            return Err(fail_dispatch(ctx));
        };
        if value::is_exception(result) {
            return Err(result);
        }
        result
    };
    if !is_object_value(iterator) {
        let message = format!("{} is not iterable", render_receiver(state, object));
        return Err(type_error(ctx, state, &message));
    }
    get_iterator_direct(ctx, state, iterator)
}

/// 「incompatible receiver」/「is not iterable」错误里的 receiver 渲染
/// （V8 形态：普通对象 #<Object>，原语按值）。
pub(crate) fn render_receiver(state: &NativeAgentState, receiver: i64) -> String {
    if value::is_js_object(receiver) || value::is_proxy(receiver) {
        "#<Object>".into()
    } else if value::is_array(receiver) {
        "[object Array]".into()
    } else {
        render_value(state, receiver)
    }
}

/// %Iterator% 本体 Call / Construct（§27.1.3.1）：无 new 调用与直接 new 均
/// TypeError（文案对齐 V8）；`class X extends Iterator` 的 super()（newTarget
/// 为子类）按 OrdinaryCreateFromConstructor 以 newTarget.prototype 建实例。
pub(crate) fn constructor_call(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    callee: i64,
) -> i64 {
    let new_target = state
        .activations
        .last()
        .map_or_else(value::encode_undefined, |activation| activation.new_target);
    if value::is_undefined(new_target) {
        return type_error(ctx, state, "Constructor Iterator requires 'new'");
    }
    if value::strip_gc_color(new_target) == value::strip_gc_color(callee) {
        return type_error(
            ctx,
            state,
            "Abstract class Iterator not directly constructable",
        );
    }
    let Ok(instance) = state.allocate_object_with_gc_retry(ctx, 0, false) else {
        return fail_dispatch(ctx);
    };
    let Some(prototype_key) = state.intern_text("prototype".into(), value::TAG_STRING) else {
        return fail_dispatch(ctx);
    };
    let Ok(prototype) = get_property(ctx, state, new_target, prototype_key) else {
        return fail_dispatch(ctx);
    };
    if value::is_exception(prototype) {
        return prototype;
    }
    let resolved = if is_object_value(prototype) {
        Some(prototype)
    } else {
        ensure_prototype(state)
    };
    let Some(resolved) = resolved else {
        return fail_dispatch(ctx);
    };
    if state
        .gc
        .heap()
        .set_prototype(
            value::decode_handle(instance),
            value::decode_handle(resolved),
        )
        .is_err()
    {
        return fail_dispatch(ctx);
    }
    instance
}

/// Iterator.from(O)（§27.1.3.2.1）：GetIteratorFlattenable 后，已是
/// %Iterator% 实例（OrdinaryHasInstance）的迭代器原样返回，否则包装为
/// %WrapForValidIteratorPrototype% 实例。
pub(crate) fn static_from(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let source = args.first().copied().unwrap_or_else(value::encode_undefined);
    let record = match get_iterator_flattenable(
        ctx,
        state,
        source,
        PrimitiveHandling::IterateStringPrimitives,
        "Iterator.from called on non-object",
    ) {
        Ok(record) => record,
        Err(exception) => return exception,
    };
    if is_iterator_instance(state, record.iterator) {
        return record.iterator;
    }
    let Some(prototype) = ensure_wrap_prototype(state) else {
        return fail_dispatch(ctx);
    };
    let Ok(wrapper) = state.allocate_object_with_gc_retry(ctx, 0, false) else {
        return fail_dispatch(ctx);
    };
    if state
        .gc
        .heap()
        .set_prototype(
            value::decode_handle(wrapper),
            value::decode_handle(prototype),
        )
        .is_err()
    {
        return fail_dispatch(ctx);
    }
    state
        .iterator_helpers
        .wraps
        .insert(value::decode_handle(wrapper), record);
    wrapper
}

fn wrap_record(state: &NativeAgentState, receiver: i64) -> Option<IteratorRecord> {
    if !value::is_js_object(receiver) {
        return None;
    }
    state
        .iterator_helpers
        .wraps
        .get(&value::decode_handle(receiver))
        .copied()
}

/// %WrapForValidIteratorPrototype%.next（§27.1.3.2.2.1）：直转发底层
/// next（不带实参、不校验结果对象）。
pub(crate) fn wrap_next(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
) -> i64 {
    let Some(record) = wrap_record(state, receiver) else {
        let message = format!(
            "Method %WrapForValidIteratorPrototype%.next called on incompatible receiver {}",
            render_receiver(state, receiver)
        );
        return type_error(ctx, state, &message);
    };
    if !value::is_callable(record.next) {
        return called_non_callable(ctx, state, record.next);
    }
    state
        .invoke_callable(ctx, record.next, record.iterator, &[])
        .unwrap_or_else(|| fail_dispatch(ctx))
}

/// %WrapForValidIteratorPrototype%.return（§27.1.3.2.2.2）：GetMethod 后
/// 直转发（不带实参）；无 return 方法时返回 done 结果对象。
pub(crate) fn wrap_return(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
) -> i64 {
    let Some(record) = wrap_record(state, receiver) else {
        let message = format!(
            "Method %WrapForValidIteratorPrototype%.return called on incompatible receiver {}",
            render_receiver(state, receiver)
        );
        return type_error(ctx, state, &message);
    };
    let Some(return_key) = state.intern_text("return".into(), value::TAG_STRING) else {
        return fail_dispatch(ctx);
    };
    let Ok(method) = get_property(ctx, state, record.iterator, return_key) else {
        return fail_dispatch(ctx);
    };
    if value::is_exception(method) {
        return method;
    }
    if value::is_undefined(method) || value::is_null(method) {
        return create_iterator_result(ctx, state, value::encode_undefined(), true);
    }
    if !value::is_callable(method) {
        return called_non_callable(ctx, state, method);
    }
    state
        .invoke_callable(ctx, method, record.iterator, &[])
        .unwrap_or_else(|| fail_dispatch(ctx))
}

/// get Iterator.prototype.constructor（§27.1.4.1.1）：返回 %Iterator%。
pub(crate) fn constructor_getter(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
) -> i64 {
    ensure_constructor(state).unwrap_or_else(|| fail_dispatch(ctx))
}

/// get Iterator.prototype[@@toStringTag]（§27.1.4.14.1）：返回 "Iterator"。
pub(crate) fn to_string_tag_getter(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
) -> i64 {
    state
        .intern_text("Iterator".into(), value::TAG_STRING)
        .unwrap_or_else(|| fail_dispatch(ctx))
}

/// SetterThatIgnoresPrototypeProperties（§27.1.4.1.2 / §27.1.4.14.2）：
/// this 为 home 对象（%Iterator.prototype%）时 TypeError（文案对齐 V8 的
/// 只读属性赋值），否则在 this 上 CreateDataPropertyOrThrow。
pub(crate) fn setter_that_ignores(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    args: &[i64],
    key_render: &str,
    key_value: i64,
) -> i64 {
    if !is_object_value(receiver) {
        let message = format!("Cannot convert {} to an object", render_value(state, receiver));
        return type_error(ctx, state, &message);
    }
    if state.iterator_helpers.prototype == Some(receiver) {
        let message = format!(
            "Cannot assign to read only property '{key_render}' of object '[object Object]'"
        );
        return type_error(ctx, state, &message);
    }
    let stored = args.first().copied().unwrap_or_else(value::encode_undefined);
    let result = create_data_property_impl(ctx, state, receiver, key_value, stored);
    if value::is_exception(result) {
        return result;
    }
    value::encode_undefined()
}

/// set Iterator.prototype.constructor 的 key 参数封装。
pub(crate) fn constructor_setter(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    args: &[i64],
) -> i64 {
    let Some(key) = state.intern_text("constructor".into(), value::TAG_STRING) else {
        return fail_dispatch(ctx);
    };
    setter_that_ignores(ctx, state, receiver, args, "constructor", key)
}

/// set Iterator.prototype[@@toStringTag] 的 key 参数封装。
pub(crate) fn to_string_tag_setter(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    args: &[i64],
) -> i64 {
    let key = value::encode_handle(value::TAG_SYMBOL, wjsm_ir::wk_symbol::TO_STRING_TAG);
    setter_that_ignores(
        ctx,
        state,
        receiver,
        args,
        "Symbol(Symbol.toStringTag)",
        key,
    )
}

/// Iterator.prototype 的方法总分派：`this` 非对象一律 TypeError（§27.1.4
/// 各方法步骤 2），随后按 lazy / eager 分流。
pub(crate) fn proto_method(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    method: IteratorProtoMethod,
    receiver: i64,
    args: &[i64],
) -> i64 {
    if !is_object_value(receiver) {
        let message = format!("Iterator.prototype.{} called on non-object", method.name());
        return type_error(ctx, state, &message);
    }
    match method {
        IteratorProtoMethod::Map
        | IteratorProtoMethod::Filter
        | IteratorProtoMethod::FlatMap => {
            let callback = args.first().copied().unwrap_or_else(value::encode_undefined);
            if !value::is_callable(callback) {
                let message = if method == IteratorProtoMethod::FlatMap {
                    "Iterator.prototype.flatMap is not a function".to_owned()
                } else {
                    format!(
                        "string \"Iterator.prototype.{}\" is not a function",
                        method.name()
                    )
                };
                return type_error(ctx, state, &message);
            }
            let record = match get_iterator_direct(ctx, state, receiver) {
                Ok(record) => record,
                Err(exception) => return exception,
            };
            let kind = match method {
                IteratorProtoMethod::Map => HelperKind::Map(callback),
                IteratorProtoMethod::Filter => HelperKind::Filter(callback),
                _ => HelperKind::FlatMap(callback),
            };
            create_helper(ctx, state, kind, record)
        }
        IteratorProtoMethod::Take | IteratorProtoMethod::Drop => {
            let limit = args.first().copied().unwrap_or_else(value::encode_undefined);
            let number = match to_number_coerced(ctx, state, limit) {
                Ok(number) => number,
                Err(exception) => return exception,
            };
            if number.is_nan() {
                let message = format!("{} must be positive", render_receiver(state, limit));
                return super::runtime::range_error(ctx, state, &message);
            }
            // ToIntegerOrInfinity（§7.1.5）：±∞ 保留，其余向零截断。
            let integer = if number.is_infinite() {
                number
            } else {
                number.trunc()
            };
            if integer < 0.0 {
                let message = format!("{} must be positive", render_receiver(state, limit));
                return super::runtime::range_error(ctx, state, &message);
            }
            let record = match get_iterator_direct(ctx, state, receiver) {
                Ok(record) => record,
                Err(exception) => return exception,
            };
            let kind = if method == IteratorProtoMethod::Take {
                HelperKind::Take(integer)
            } else {
                HelperKind::Drop(integer)
            };
            create_helper(ctx, state, kind, record)
        }
        IteratorProtoMethod::Reduce
        | IteratorProtoMethod::ToArray
        | IteratorProtoMethod::ForEach
        | IteratorProtoMethod::Some
        | IteratorProtoMethod::Every
        | IteratorProtoMethod::Find => eager_method(ctx, state, method, receiver, args),
    }
}
