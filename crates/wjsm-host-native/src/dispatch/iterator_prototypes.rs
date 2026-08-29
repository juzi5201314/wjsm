//! 内建迭代器实例的真实原型链：%ArrayIteratorPrototype%（§23.1.5.2）、
//! %StringIteratorPrototype%（§22.1.5.1）、%MapIteratorPrototype%（§24.1.5.2）、
//! %SetIteratorPrototype%（§24.2.5.2）与 %RegExpStringIteratorPrototype%
//! （§22.2.9.2）的物化与共享 `next` 分派。
//!
//! 每个原型对象的 [[Prototype]] 是 %Iterator.prototype%（§27.1.2），自有
//! `next`（同族实例共享同一函数身份）与 @@toStringTag 数据属性；实例创建时
//! 经 `attach` 接线真实 [[Prototype]]，helper 方法与 @@iterator 沿链继承，
//! 不再依赖旁挂合成。

use wjsm_gc::PropertyKey;
use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::iterator_helpers;
use super::runtime::{fail_dispatch, render_value, type_error};
use crate::{BUILTIN_PROTOTYPE_PROPERTY_FLAGS, NativeAgentState, NativeCallableKind};

/// 内建迭代器原型家族：决定实例挂哪个原型对象与共享 `next` 的 brand 检查。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum NativeIteratorFamily {
    /// 数组 / arguments / TypedArray 迭代器共用 %ArrayIteratorPrototype%
    /// （§23.2.3.38 的 values 也按 CreateArrayIterator 建实例）。
    Array,
    String,
    Map,
    Set,
    RegExpString,
}

impl NativeIteratorFamily {
    /// @@toStringTag 值（§23.1.5.2.2 等）。
    fn to_string_tag(self) -> &'static str {
        match self {
            Self::Array => "Array Iterator",
            Self::String => "String Iterator",
            Self::Map => "Map Iterator",
            Self::Set => "Set Iterator",
            Self::RegExpString => "RegExp String Iterator",
        }
    }

    /// incompatible receiver 错误里的方法渲染名（对齐 V8：RegExp 家族用
    /// intrinsic 记号，其余用 tag 名）。
    fn method_render(self) -> &'static str {
        match self {
            Self::Array => "Array Iterator.prototype.next",
            Self::String => "String Iterator.prototype.next",
            Self::Map => "Map Iterator.prototype.next",
            Self::Set => "Set Iterator.prototype.next",
            Self::RegExpString => "%RegExpStringIterator%.prototype.next",
        }
    }
}

/// 五个家族原型对象的登记表（GC 根，见 weak.rs）。
#[derive(Default)]
pub(crate) struct IteratorPrototypesState {
    array: Option<i64>,
    string: Option<i64>,
    map: Option<i64>,
    set: Option<i64>,
    regexp_string: Option<i64>,
}

impl IteratorPrototypesState {
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    /// GC 根枚举：已物化的原型对象全部保活。
    pub(crate) fn roots(&self) -> impl Iterator<Item = i64> {
        [self.array, self.string, self.map, self.set, self.regexp_string]
            .into_iter()
            .flatten()
    }

    fn slot(&mut self, family: NativeIteratorFamily) -> &mut Option<i64> {
        match family {
            NativeIteratorFamily::Array => &mut self.array,
            NativeIteratorFamily::String => &mut self.string,
            NativeIteratorFamily::Map => &mut self.map,
            NativeIteratorFamily::Set => &mut self.set,
            NativeIteratorFamily::RegExpString => &mut self.regexp_string,
        }
    }
}

/// `array_iterators` 条目源 → 原型家族。Custom 是用户自己的迭代器对象，
/// 不属于任何内建家族（不得改写其 [[Prototype]]）。
pub(crate) fn family_of_source(source: crate::NativeIteratorSource) -> Option<NativeIteratorFamily> {
    match source {
        crate::NativeIteratorSource::Array(_)
        | crate::NativeIteratorSource::ArrayLike(_)
        | crate::NativeIteratorSource::TypedArray(_) => Some(NativeIteratorFamily::Array),
        crate::NativeIteratorSource::String(_) => Some(NativeIteratorFamily::String),
        crate::NativeIteratorSource::Map(_) => Some(NativeIteratorFamily::Map),
        crate::NativeIteratorSource::Set(_) => Some(NativeIteratorFamily::Set),
        crate::NativeIteratorSource::Custom(_) => None,
    }
}

/// 惰性物化家族原型对象：[[Prototype]] 为 %Iterator.prototype%，own `next`
/// （{ writable: true, enumerable: false, configurable: true }）与
/// @@toStringTag（{ writable: false, enumerable: false, configurable: true }）。
pub(crate) fn ensure_prototype(
    state: &mut NativeAgentState,
    family: NativeIteratorFamily,
) -> Option<i64> {
    if let Some(prototype) = *state.iterator_prototypes.slot(family) {
        return Some(prototype);
    }
    let parent = iterator_helpers::ensure_prototype(state)?;
    let prototype = state.allocate_object(2, false).ok()?;
    // 先登记再安装成员：登记表是 GC 根，安装期间的 intern/分配不会回收
    // 尚未挂满成员的 prototype 对象。
    *state.iterator_prototypes.slot(family) = Some(prototype);
    let handle = value::decode_handle(prototype);
    state
        .gc
        .heap()
        .set_prototype(handle, value::decode_handle(parent))
        .ok()?;
    let next = state.native_callable(NativeCallableKind::IteratorFamilyNext(family))?;
    let next_key = state.intern_property_string("next".into())?;
    state
        .gc
        .heap()
        .define_data_property(handle, next_key, next as u64, BUILTIN_PROTOTYPE_PROPERTY_FLAGS)
        .ok()?;
    let tag = state.intern_text(family.to_string_tag().into(), value::TAG_STRING)?;
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

/// 给新建的内建迭代器实例接线家族原型。调用方须在实例分配前完成家族原型
/// 物化无关的准备；本函数内部的 `ensure_prototype` 只走 allocate_object
/// （无 GC 重试），不会移动未根化的新实例。
pub(crate) fn attach(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    iterator: i64,
    family: NativeIteratorFamily,
) -> Result<(), i64> {
    let Some(prototype) = ensure_prototype(state, family) else {
        return Err(fail_dispatch(ctx));
    };
    state
        .gc
        .heap()
        .set_prototype(
            value::decode_handle(iterator),
            value::decode_handle(prototype),
        )
        .map_err(|_| fail_dispatch(ctx))
}

/// 家族共享 `next`（%ArrayIteratorPrototype%.next 等）：按 receiver 的内部
/// 槽做 brand 检查后推进对应实例状态；不匹配抛 V8 口径的
/// incompatible receiver TypeError。
pub(crate) fn family_next(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    family: NativeIteratorFamily,
    receiver: i64,
) -> i64 {
    if value::is_js_object(receiver) {
        let handle = value::decode_handle(receiver);
        if family == NativeIteratorFamily::RegExpString {
            if let Some(iterator_id) = state.regexp_iterator_ids.get(&handle).copied() {
                return super::regexp::next_match_all(ctx, state, iterator_id);
            }
        } else if state
            .array_iterators
            .get(&handle)
            .is_some_and(|entry| family_of_source(entry.source) == Some(family))
        {
            return super::runtime::iterator_next_result(ctx, state, handle);
        }
    }
    let message = format!(
        "Method {} called on incompatible receiver {}",
        family.method_render(),
        render_incompatible_receiver(state, receiver)
    );
    type_error(ctx, state, &message)
}

/// incompatible receiver 的 V8 NoSideEffectsToString 子集：宿主品牌实例
///（Map / Set / WeakMap / WeakSet / Promise，toString 未覆盖且 constructor
/// 为具名函数）先按 `#<Ctor>` 渲染；其余对象沿真实原型链找 @@toStringTag
/// 数据属性（不触发访问器），命中字符串渲染 `[object {tag}]`；数组缺省
/// `[object Array]`；普通对象 `#<Object>`；原始值按值渲染。
pub(crate) fn render_incompatible_receiver(state: &NativeAgentState, receiver: i64) -> String {
    if let Some(name) = host_brand_constructor_name(state, receiver) {
        return format!("#<{name}>");
    }
    if value::is_js_object(receiver) || value::is_array(receiver) {
        if let Some(tag) = no_side_effects_to_string_tag(state, receiver) {
            return format!("[object {tag}]");
        }
        if value::is_array(receiver) {
            return "[object Array]".into();
        }
        "#<Object>".into()
    } else if value::is_proxy(receiver) {
        "#<Object>".into()
    } else {
        render_value(state, receiver)
    }
}

/// V8 `#<Ctor>` 分支的宿主品牌子集：品牌数据在侧表、原型链上无
/// @@toStringTag，构造器名即品牌名。
fn host_brand_constructor_name(state: &NativeAgentState, receiver: i64) -> Option<&'static str> {
    if !value::is_object(receiver) {
        return None;
    }
    let handle = value::decode_handle(receiver);
    if state.maps.contains_key(&handle) {
        return Some("Map");
    }
    if state.sets.contains_key(&handle) {
        return Some("Set");
    }
    if state.promises.contains_key(&handle) {
        return Some("Promise");
    }
    if state.array_buffers.contains_key(&handle) {
        return Some("ArrayBuffer");
    }
    if state.shared_array_buffers.contains_key(&handle) {
        return Some("SharedArrayBuffer");
    }
    if state.data_views.contains_key(&handle) {
        return Some("DataView");
    }
    state.weak.brand_name(handle)
}

/// 沿堆原型链查 @@toStringTag：只认数据属性字符串值，访问器与非字符串
/// 一律放弃（无副作用约束）。
fn no_side_effects_to_string_tag(state: &NativeAgentState, receiver: i64) -> Option<String> {
    let mut handle = value::decode_handle(receiver);
    let key = PropertyKey::symbol(wjsm_ir::wk_symbol::TO_STRING_TAG);
    for _ in 0..1024 {
        if let Some(slot) = state.gc.heap().get_property_slot(handle, key).ok()? {
            if slot.flags & wjsm_ir::constants::FLAG_IS_ACCESSOR as u32 != 0 {
                return None;
            }
            let stored = slot.value as i64;
            if !value::is_string(stored) {
                return None;
            }
            return state.string_owned(stored)?.to_utf8();
        }
        let parent = state.gc.heap().prototype(handle).ok()?;
        let parent = super::runtime::decode_proto_slot(state, parent)?;
        if !(value::is_js_object(parent) || value::is_array(parent)) {
            return None;
        }
        handle = value::decode_handle(parent);
    }
    None
}
