//! %String.prototype%（ES §22.1.3）固有原型对象：constructor 与全部已实现
//! 原型方法为真实不可枚举自有数据属性（{[[Writable]], [[Configurable]]}），
//! `length` 为 +0 恒定数据属性（§22.1.3 首段），`@@iterator`（§22.1.3.36）
//! 为 symbol 键自有属性；安装顺序取 Node v22 自有属性序中已实现的子集。
//! 基元字符串读取在 `primitive_property` 未命中后经包装原型链
//! （`primitive_wrapper_prototype`）在此对象上命中，取值后可经
//! call / apply / bind 复用。

use wjsm_ir::{Builtin, value, wk_symbol};

use super::intl::IntlCallable;
use crate::{BUILTIN_PROTOTYPE_PROPERTY_FLAGS, NativeAgentState, NativeCallableKind, PropertyKey};

/// 原型方法值家族：多数为带 receiver 的 Builtin；locale 敏感方法
/// （ECMA-402 §1.4.1 覆盖 §22.1.3 对应条目）沿用 Intl owner。
#[derive(Clone, Copy)]
enum ProtoMethod {
    Builtin(Builtin),
    Intl(IntlCallable),
}

/// Node v22 `Object.getOwnPropertyNames(String.prototype)` 序中本引擎已
/// 实现的方法子集（Annex B 的 HTML 方法族与 substr、`trimLeft` /
/// `trimRight` 未实现，不占位）。
const PROTO_METHODS: &[(&str, ProtoMethod)] = &[
    ("at", ProtoMethod::Builtin(Builtin::StringAt)),
    ("charAt", ProtoMethod::Builtin(Builtin::StringCharAt)),
    (
        "charCodeAt",
        ProtoMethod::Builtin(Builtin::StringCharCodeAt),
    ),
    (
        "codePointAt",
        ProtoMethod::Builtin(Builtin::StringCodePointAt),
    ),
    ("concat", ProtoMethod::Builtin(Builtin::StringConcatVa)),
    ("endsWith", ProtoMethod::Builtin(Builtin::StringEndsWith)),
    ("includes", ProtoMethod::Builtin(Builtin::StringIncludes)),
    ("indexOf", ProtoMethod::Builtin(Builtin::StringIndexOf)),
    (
        "isWellFormed",
        ProtoMethod::Builtin(Builtin::StringIsWellFormed),
    ),
    (
        "lastIndexOf",
        ProtoMethod::Builtin(Builtin::StringLastIndexOf),
    ),
    (
        "localeCompare",
        ProtoMethod::Intl(IntlCallable::StringLocaleCompare),
    ),
    ("match", ProtoMethod::Builtin(Builtin::StringMatch)),
    ("matchAll", ProtoMethod::Builtin(Builtin::StringMatchAll)),
    (
        "normalize",
        ProtoMethod::Intl(IntlCallable::StringNormalize),
    ),
    ("padEnd", ProtoMethod::Builtin(Builtin::StringPadEnd)),
    ("padStart", ProtoMethod::Builtin(Builtin::StringPadStart)),
    ("repeat", ProtoMethod::Builtin(Builtin::StringRepeat)),
    ("replace", ProtoMethod::Builtin(Builtin::StringReplace)),
    (
        "replaceAll",
        ProtoMethod::Builtin(Builtin::StringReplaceAll),
    ),
    ("search", ProtoMethod::Builtin(Builtin::StringSearch)),
    ("slice", ProtoMethod::Builtin(Builtin::StringSlice)),
    ("split", ProtoMethod::Builtin(Builtin::StringSplit)),
    ("substring", ProtoMethod::Builtin(Builtin::StringSubstring)),
    (
        "startsWith",
        ProtoMethod::Builtin(Builtin::StringStartsWith),
    ),
    ("toString", ProtoMethod::Builtin(Builtin::StringToString)),
    (
        "toWellFormed",
        ProtoMethod::Builtin(Builtin::StringToWellFormed),
    ),
    ("trim", ProtoMethod::Builtin(Builtin::StringTrim)),
    ("trimStart", ProtoMethod::Builtin(Builtin::StringTrimStart)),
    ("trimEnd", ProtoMethod::Builtin(Builtin::StringTrimEnd)),
    (
        "toLocaleLowerCase",
        ProtoMethod::Intl(IntlCallable::StringToLocaleLowerCase),
    ),
    (
        "toLocaleUpperCase",
        ProtoMethod::Intl(IntlCallable::StringToLocaleUpperCase),
    ),
    (
        "toLowerCase",
        ProtoMethod::Intl(IntlCallable::StringToLowerCase),
    ),
    (
        "toUpperCase",
        ProtoMethod::Intl(IntlCallable::StringToUpperCase),
    ),
    ("valueOf", ProtoMethod::Builtin(Builtin::StringValueOf)),
];

/// %String.prototype% 懒创建：链尾为 %Object.prototype%，携带 [[StringData]]
/// 空串（§22.1.3 "is a String exotic object"，`String.prototype.valueOf()`
/// 返回 ""）；`String.prototype` 反向链接按 §22.1.2.3 为恒定数据属性。
pub(crate) fn ensure_string_prototype(state: &mut NativeAgentState) -> Option<i64> {
    if let Some(prototype) = state.intl.string_prototype {
        return Some(prototype);
    }
    state.ensure_intrinsic_prototypes().ok()?;
    let constructor = state.native_callable(NativeCallableKind::StringConstructor)?;
    let capacity = u32::try_from(PROTO_METHODS.len() + 3).ok()?;
    let prototype = state.allocate_object(capacity, false).ok()?;
    let handle = value::decode_handle(prototype);
    let empty = state.intern_text(String::new(), value::TAG_STRING)?;
    state.boxed_primitives.insert(handle, empty);
    let length_key = state.intern_property_string("length".into())?;
    state
        .gc
        .heap()
        .define_data_property(handle, length_key, value::encode_f64(0.0) as u64, 0)
        .ok()?;
    let constructor_key = state.intern_property_string("constructor".into())?;
    state
        .gc
        .heap()
        .define_data_property(
            handle,
            constructor_key,
            constructor as u64,
            BUILTIN_PROTOTYPE_PROPERTY_FLAGS,
        )
        .ok()?;
    for (name, method) in PROTO_METHODS {
        let callable = match method {
            ProtoMethod::Builtin(builtin) => {
                state.native_callable(NativeCallableKind::Builtin(*builtin, true))?
            }
            ProtoMethod::Intl(kind) => state.native_callable(NativeCallableKind::Intl(*kind))?,
        };
        let key = state.intern_property_string((*name).into())?;
        state
            .gc
            .heap()
            .define_data_property(
                handle,
                key,
                callable as u64,
                BUILTIN_PROTOTYPE_PROPERTY_FLAGS,
            )
            .ok()?;
    }
    let iterator =
        state.native_callable(NativeCallableKind::Builtin(Builtin::StringIterator, true))?;
    state
        .gc
        .heap()
        .define_data_property(
            handle,
            PropertyKey::symbol(wk_symbol::ITERATOR),
            iterator as u64,
            BUILTIN_PROTOTYPE_PROPERTY_FLAGS,
        )
        .ok()?;
    let prototype_key = state.intern_property_string("prototype".into())?;
    state
        .callable_properties
        .insert((constructor, prototype_key), prototype);
    state
        .callable_property_flags
        .insert((constructor, prototype_key), 0);
    state.intl.string_prototype = Some(prototype);
    Some(prototype)
}
