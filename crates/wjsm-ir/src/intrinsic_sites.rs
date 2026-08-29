//! intrinsic 调用快路径站点的共享名称表：语义层（快路径识别、守卫发射）与
//! 宿主（`IntrinsicPristine` / `IntrinsicResolve` 按 wire_id 反查名字）共用的
//! 单一事实来源。
//!
//! 守卫与慢路径解析都只携带 `(family, wire_id)`，属性名不进制品常量池——
//! install 期发布字符串常量会改变宿主驻留表，纯 pristine 执行必须做到
//! 零新增驻留。因此名字由宿主经本表反查，正反向映射必须一致：
//! 反向表构建时对同家族内重复的 builtin 直接 panic（首次使用即暴露）。

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::Builtin;

/// 裸全局标识符调用（`parseInt(...)`）的站点表：全局名 → 快路径 builtin。
pub const GLOBAL_IDENT_SITES: &[(&str, Builtin)] = &[
    ("setTimeout", Builtin::SetTimeout),
    ("clearTimeout", Builtin::ClearTimeout),
    ("setInterval", Builtin::SetInterval),
    ("clearInterval", Builtin::ClearInterval),
    ("fetch", Builtin::Fetch),
    ("Headers", Builtin::HeadersConstructor),
    ("Request", Builtin::RequestConstructor),
    ("Response", Builtin::ResponseConstructor),
    ("AbortController", Builtin::AbortControllerConstructor),
    ("AbortSignal", Builtin::AbortSignalConstructor),
    ("EventTarget", Builtin::EventTargetConstructor),
    ("Event", Builtin::EventConstructor),
    ("ReadableStream", Builtin::ReadableStreamConstructor),
    ("eval", Builtin::Eval),
    ("structuredClone", Builtin::StructuredClone),
    ("WritableStream", Builtin::WritableStreamConstructor),
    ("TransformStream", Builtin::TransformStreamConstructor),
    (
        "CountQueuingStrategy",
        Builtin::CountQueuingStrategyConstructor,
    ),
    (
        "ByteLengthQueuingStrategy",
        Builtin::ByteLengthQueuingStrategyConstructor,
    ),
    ("Symbol", Builtin::SymbolCreate),
    ("parseInt", Builtin::NumberParseInt),
    ("parseFloat", Builtin::NumberParseFloat),
    ("Proxy", Builtin::ProxyCreate),
    ("Number", Builtin::NumberConstructor),
    ("Boolean", Builtin::BooleanConstructor),
    ("Error", Builtin::ErrorConstructor),
    ("TypeError", Builtin::TypeErrorConstructor),
    ("RangeError", Builtin::RangeErrorConstructor),
    ("SyntaxError", Builtin::SyntaxErrorConstructor),
    ("ReferenceError", Builtin::ReferenceErrorConstructor),
    ("URIError", Builtin::URIErrorConstructor),
    ("EvalError", Builtin::EvalErrorConstructor),
    ("Map", Builtin::MapConstructor),
    ("Set", Builtin::SetConstructor),
    ("WeakMap", Builtin::WeakMapConstructor),
    ("WeakSet", Builtin::WeakSetConstructor),
    ("WeakRef", Builtin::WeakRefConstructor),
    (
        "FinalizationRegistry",
        Builtin::FinalizationRegistryConstructor,
    ),
    ("Date", Builtin::DateConstructor),
    ("ArrayBuffer", Builtin::ArrayBufferConstructor),
    ("SharedArrayBuffer", Builtin::SharedArrayBufferConstructor),
    ("DataView", Builtin::DataViewConstructor),
    ("Int8Array", Builtin::Int8ArrayConstructor),
    ("Uint8Array", Builtin::Uint8ArrayConstructor),
    ("Uint8ClampedArray", Builtin::Uint8ClampedArrayConstructor),
    ("Int16Array", Builtin::Int16ArrayConstructor),
    ("Uint16Array", Builtin::Uint16ArrayConstructor),
    ("Int32Array", Builtin::Int32ArrayConstructor),
    ("Uint32Array", Builtin::Uint32ArrayConstructor),
    ("Float32Array", Builtin::Float32ArrayConstructor),
    ("Float64Array", Builtin::Float64ArrayConstructor),
    ("BigInt64Array", Builtin::BigInt64ArrayConstructor),
    ("BigUint64Array", Builtin::BigUint64ArrayConstructor),
];

/// Web 平台全局（fetch / Fetch 类 / Streams / Abort / Events）在 Node 与
/// 浏览器中都是全局对象上真实的自有数据属性（WebIDL 接口对象为
/// {writable, enumerable: false, configurable}，`fetch` 方法额外
/// enumerable）。宿主在 CreateGlobalObject 时按本表急切物化自有槽；
/// 语义层据此把裸标识符读取路由到全局环境记录语义（删除后
/// ReferenceError），并给 `new` 快路径挂 GLOBAL_IDENT 家族守卫。
/// 元组含义：(全局名, 规范值 builtin, enumerable)。
pub const WEB_GLOBAL_PROPERTIES: &[(&str, Builtin, bool)] = &[
    ("fetch", Builtin::Fetch, true),
    ("Headers", Builtin::HeadersConstructor, false),
    ("Request", Builtin::RequestConstructor, false),
    ("Response", Builtin::ResponseConstructor, false),
    ("ReadableStream", Builtin::ReadableStreamConstructor, false),
    ("WritableStream", Builtin::WritableStreamConstructor, false),
    (
        "TransformStream",
        Builtin::TransformStreamConstructor,
        false,
    ),
    (
        "AbortController",
        Builtin::AbortControllerConstructor,
        false,
    ),
    ("AbortSignal", Builtin::AbortSignalConstructor, false),
    ("EventTarget", Builtin::EventTargetConstructor, false),
    ("Event", Builtin::EventConstructor, false),
];

/// Web 平台全局名 → (规范值 builtin, enumerable)。
pub fn web_global_property(name: &str) -> Option<(Builtin, bool)> {
    WEB_GLOBAL_PROPERTIES
        .iter()
        .find(|(entry, _, _)| *entry == name)
        .map(|(_, builtin, enumerable)| (*builtin, *enumerable))
}

/// ES 侧急切物化的全局自有数据属性：`SharedArrayBuffer` 构造器与 `Atomics`
/// 命名空间在 Node 中都是全局对象上 {writable, configurable, 不可枚举} 的
/// 真实自有属性。`SharedArrayBuffer` 的规范值是构造器 builtin（GLOBAL_IDENT
/// 站点表可反查）；`Atomics` 的规范值是宿主物化的命名空间堆对象（无
/// builtin 形态），槽位值由宿主装配。
pub const ES_EAGER_GLOBAL_PROPERTIES: &[&str] = &["SharedArrayBuffer", "Atomics"];

/// 全局名是否为急切物化的真实自有属性（Web 平台全局 + ES 侧
/// SharedArrayBuffer / Atomics）。语义层据此把裸标识符读取路由到全局环境
/// 记录语义（GlobalEnvGet：删除后 ReferenceError，typeof 容忍读）。
pub fn eager_global_property(name: &str) -> bool {
    web_global_property(name).is_some() || ES_EAGER_GLOBAL_PROPERTIES.contains(&name)
}

/// 内建容器静态成员调用（`String.raw(...)`）的站点表：
/// (容器全局名, 属性名) → 快路径 builtin。
pub const STATIC_MEMBER_SITES: &[(&str, &str, Builtin)] = &[
    ("console", "log", Builtin::ConsoleLog),
    ("console", "error", Builtin::ConsoleError),
    ("console", "warn", Builtin::ConsoleWarn),
    ("console", "info", Builtin::ConsoleInfo),
    ("console", "debug", Builtin::ConsoleDebug),
    ("console", "trace", Builtin::ConsoleTrace),
    ("performance", "now", Builtin::PerformanceNow),
    ("Array", "isArray", Builtin::ArrayIsArray),
    ("Array", "from", Builtin::ArrayFrom),
    ("Array", "fromAsync", Builtin::ArrayFromAsync),
    ("Array", "of", Builtin::ArrayOf),
    ("Object", "defineProperty", Builtin::DefineProperty),
    (
        "Object",
        "getOwnPropertyDescriptor",
        Builtin::GetOwnPropDesc,
    ),
    ("Object", "keys", Builtin::ObjectKeys),
    ("Object", "values", Builtin::ObjectValues),
    ("Object", "entries", Builtin::ObjectEntries),
    ("Object", "assign", Builtin::ObjectAssign),
    ("Object", "create", Builtin::ObjectCreate),
    ("Object", "getPrototypeOf", Builtin::ObjectGetPrototypeOf),
    ("Object", "setPrototypeOf", Builtin::ObjectSetPrototypeOf),
    (
        "Object",
        "getOwnPropertyNames",
        Builtin::ObjectGetOwnPropertyNames,
    ),
    (
        "Object",
        "getOwnPropertySymbols",
        Builtin::ObjectGetOwnPropertySymbols,
    ),
    ("Object", "is", Builtin::ObjectIs),
    ("Object", "groupBy", Builtin::ObjectGroupBy),
    ("Object", "hasOwn", Builtin::ObjectHasOwn),
    ("Object", "freeze", Builtin::ObjectFreeze),
    ("Object", "seal", Builtin::ObjectSeal),
    ("Object", "isFrozen", Builtin::ObjectIsFrozen),
    ("Object", "isSealed", Builtin::ObjectIsSealed),
    ("Object", "isExtensible", Builtin::ObjectIsExtensible),
    ("Object", "fromEntries", Builtin::ObjectFromEntries),
    (
        "Object",
        "getOwnPropertyDescriptors",
        Builtin::ObjectGetOwnPropertyDescriptors,
    ),
    (
        "Object",
        "defineProperties",
        Builtin::ObjectDefineProperties,
    ),
    (
        "Object",
        "preventExtensions",
        Builtin::ObjectPreventExtensions,
    ),
    ("Map", "groupBy", Builtin::MapGroupBy),
    ("JSON", "stringify", Builtin::JsonStringify),
    ("JSON", "parse", Builtin::JsonParse),
    ("Symbol", "for", Builtin::SymbolFor),
    ("Symbol", "keyFor", Builtin::SymbolKeyFor),
    ("Promise", "resolve", Builtin::PromiseResolveStatic),
    ("Promise", "reject", Builtin::PromiseRejectStatic),
    ("Promise", "all", Builtin::PromiseAll),
    ("Promise", "race", Builtin::PromiseRace),
    ("Promise", "allSettled", Builtin::PromiseAllSettled),
    ("Promise", "any", Builtin::PromiseAny),
    ("Promise", "withResolvers", Builtin::PromiseWithResolvers),
    ("String", "fromCharCode", Builtin::StringFromCharCode),
    ("String", "fromCodePoint", Builtin::StringFromCodePoint),
    ("String", "raw", Builtin::StringRaw),
    ("Proxy", "revocable", Builtin::ProxyRevocable),
    ("Reflect", "get", Builtin::ReflectGet),
    ("Reflect", "set", Builtin::ReflectSet),
    ("Reflect", "has", Builtin::ReflectHas),
    ("Reflect", "deleteProperty", Builtin::ReflectDeleteProperty),
    ("Reflect", "apply", Builtin::ReflectApply),
    ("Reflect", "construct", Builtin::ReflectConstruct),
    ("Reflect", "getPrototypeOf", Builtin::ReflectGetPrototypeOf),
    ("Reflect", "setPrototypeOf", Builtin::ReflectSetPrototypeOf),
    ("Reflect", "isExtensible", Builtin::ReflectIsExtensible),
    (
        "Reflect",
        "preventExtensions",
        Builtin::ReflectPreventExtensions,
    ),
    (
        "Reflect",
        "getOwnPropertyDescriptor",
        Builtin::ReflectGetOwnPropertyDescriptor,
    ),
    ("Reflect", "defineProperty", Builtin::ReflectDefineProperty),
    ("Reflect", "ownKeys", Builtin::ReflectOwnKeys),
    ("Math", "abs", Builtin::MathAbs),
    ("Math", "acos", Builtin::MathAcos),
    ("Math", "acosh", Builtin::MathAcosh),
    ("Math", "asin", Builtin::MathAsin),
    ("Math", "asinh", Builtin::MathAsinh),
    ("Math", "atan", Builtin::MathAtan),
    ("Math", "atanh", Builtin::MathAtanh),
    ("Math", "atan2", Builtin::MathAtan2),
    ("Math", "cbrt", Builtin::MathCbrt),
    ("Math", "ceil", Builtin::MathCeil),
    ("Math", "clz32", Builtin::MathClz32),
    ("Math", "cos", Builtin::MathCos),
    ("Math", "cosh", Builtin::MathCosh),
    ("Math", "exp", Builtin::MathExp),
    ("Math", "expm1", Builtin::MathExpm1),
    ("Math", "floor", Builtin::MathFloor),
    ("Math", "fround", Builtin::MathFround),
    ("Math", "hypot", Builtin::MathHypot),
    ("Math", "imul", Builtin::MathImul),
    ("Math", "log", Builtin::MathLog),
    ("Math", "log1p", Builtin::MathLog1p),
    ("Math", "log10", Builtin::MathLog10),
    ("Math", "log2", Builtin::MathLog2),
    ("Math", "max", Builtin::MathMax),
    ("Math", "min", Builtin::MathMin),
    ("Math", "pow", Builtin::MathPow),
    ("Math", "random", Builtin::MathRandom),
    ("Math", "round", Builtin::MathRound),
    ("Math", "sign", Builtin::MathSign),
    ("Math", "sin", Builtin::MathSin),
    ("Math", "sinh", Builtin::MathSinh),
    ("Math", "sqrt", Builtin::MathSqrt),
    ("Math", "tan", Builtin::MathTan),
    ("Math", "tanh", Builtin::MathTanh),
    ("Math", "trunc", Builtin::MathTrunc),
    ("Number", "isNaN", Builtin::NumberIsNaN),
    ("Number", "isFinite", Builtin::NumberIsFinite),
    ("Number", "isInteger", Builtin::NumberIsInteger),
    ("Number", "isSafeInteger", Builtin::NumberIsSafeInteger),
    ("Number", "parseInt", Builtin::NumberParseInt),
    ("Number", "parseFloat", Builtin::NumberParseFloat),
    ("Date", "now", Builtin::DateNow),
    ("Date", "parse", Builtin::DateParse),
    ("Date", "UTC", Builtin::DateUTC),
    ("WeakRef", "deref", Builtin::WeakRefProtoDeref),
    (
        "FinalizationRegistry",
        "register",
        Builtin::FinalizationRegistryProtoRegister,
    ),
    (
        "FinalizationRegistry",
        "unregister",
        Builtin::FinalizationRegistryProtoUnregister,
    ),
    ("Atomics", "load", Builtin::AtomicsLoad),
    ("Atomics", "store", Builtin::AtomicsStore),
    ("Atomics", "add", Builtin::AtomicsAdd),
    ("Atomics", "sub", Builtin::AtomicsSub),
    ("Atomics", "and", Builtin::AtomicsAnd),
    ("Atomics", "or", Builtin::AtomicsOr),
    ("Atomics", "xor", Builtin::AtomicsXor),
    ("Atomics", "exchange", Builtin::AtomicsExchange),
    (
        "Atomics",
        "compareExchange",
        Builtin::AtomicsCompareExchange,
    ),
    ("Atomics", "isLockFree", Builtin::AtomicsIsLockFree),
    ("Atomics", "pause", Builtin::AtomicsPause),
    ("Atomics", "wait", Builtin::AtomicsWait),
    ("Atomics", "notify", Builtin::AtomicsNotify),
    ("Atomics", "waitAsync", Builtin::AtomicsWaitAsync),
];

/// %String.prototype% 方法调用（`"x".slice(...)`）的站点表。
pub const STRING_PROTO_SITES: &[(&str, Builtin)] = &[
    ("match", Builtin::StringMatch),
    ("replace", Builtin::StringReplace),
    ("search", Builtin::StringSearch),
    ("split", Builtin::StringSplit),
    ("at", Builtin::StringAt),
    ("charAt", Builtin::StringCharAt),
    ("charCodeAt", Builtin::StringCharCodeAt),
    ("codePointAt", Builtin::StringCodePointAt),
    ("concat", Builtin::StringConcatVa),
    ("endsWith", Builtin::StringEndsWith),
    ("includes", Builtin::StringIncludes),
    ("indexOf", Builtin::StringIndexOf),
    ("isWellFormed", Builtin::StringIsWellFormed),
    ("lastIndexOf", Builtin::StringLastIndexOf),
    ("matchAll", Builtin::StringMatchAll),
    ("padEnd", Builtin::StringPadEnd),
    ("padStart", Builtin::StringPadStart),
    ("repeat", Builtin::StringRepeat),
    ("replaceAll", Builtin::StringReplaceAll),
    ("slice", Builtin::StringSlice),
    ("startsWith", Builtin::StringStartsWith),
    ("substring", Builtin::StringSubstring),
    ("toWellFormed", Builtin::StringToWellFormed),
    ("trim", Builtin::StringTrim),
    ("trimEnd", Builtin::StringTrimEnd),
    ("trimStart", Builtin::StringTrimStart),
];

/// %Array.prototype% 方法调用（`[1].map(...)`）的站点表。
pub const ARRAY_PROTO_SITES: &[(&str, Builtin)] = &[
    ("shift", Builtin::ArrayShift),
    ("unshift", Builtin::ArrayUnshiftVa),
    ("sort", Builtin::ArraySort),
    ("at", Builtin::ArrayAt),
    ("copyWithin", Builtin::ArrayCopyWithin),
    ("forEach", Builtin::ArrayForEach),
    ("map", Builtin::ArrayMap),
    ("filter", Builtin::ArrayFilter),
    ("reduce", Builtin::ArrayReduce),
    ("reduceRight", Builtin::ArrayReduceRight),
    ("find", Builtin::ArrayFind),
    ("findIndex", Builtin::ArrayFindIndex),
    ("some", Builtin::ArraySome),
    ("every", Builtin::ArrayEvery),
    ("flatMap", Builtin::ArrayFlatMap),
    ("flat", Builtin::ArrayFlat),
    ("concat", Builtin::ArrayConcatVa),
    ("splice", Builtin::ArraySpliceVa),
    ("findLast", Builtin::ArrayFindLast),
    ("findLastIndex", Builtin::ArrayFindLastIndex),
    ("lastIndexOf", Builtin::ArrayLastIndexOf),
    ("toSorted", Builtin::ArrayToSorted),
    ("toReversed", Builtin::ArrayToReversed),
    ("toSpliced", Builtin::ArrayToSplicedVa),
    ("with", Builtin::ArrayWith),
];

fn global_ident_forward() -> &'static HashMap<&'static str, Builtin> {
    static MAP: OnceLock<HashMap<&'static str, Builtin>> = OnceLock::new();
    MAP.get_or_init(|| GLOBAL_IDENT_SITES.iter().copied().collect())
}

fn static_member_forward() -> &'static HashMap<(&'static str, &'static str), Builtin> {
    static MAP: OnceLock<HashMap<(&'static str, &'static str), Builtin>> = OnceLock::new();
    MAP.get_or_init(|| {
        STATIC_MEMBER_SITES
            .iter()
            .map(|(object, property, builtin)| ((*object, *property), *builtin))
            .collect()
    })
}

fn string_proto_forward() -> &'static HashMap<&'static str, Builtin> {
    static MAP: OnceLock<HashMap<&'static str, Builtin>> = OnceLock::new();
    MAP.get_or_init(|| STRING_PROTO_SITES.iter().copied().collect())
}

fn array_proto_forward() -> &'static HashMap<&'static str, Builtin> {
    static MAP: OnceLock<HashMap<&'static str, Builtin>> = OnceLock::new();
    MAP.get_or_init(|| ARRAY_PROTO_SITES.iter().copied().collect())
}

/// 反向表构建：同家族内同一 builtin 出现两次即为映射歧义（宿主无法从
/// wire_id 反推唯一名字），直接 panic 暴露表错误。
fn reverse_unique<T: Copy>(
    entries: impl Iterator<Item = (Builtin, T)>,
    family: &str,
) -> HashMap<u16, T> {
    let mut map = HashMap::new();
    for (builtin, names) in entries {
        assert!(
            map.insert(builtin.wire_id(), names).is_none(),
            "intrinsic 站点表 {family} 中 builtin {builtin} 重复，反向映射歧义"
        );
    }
    map
}

fn global_ident_reverse() -> &'static HashMap<u16, &'static str> {
    static MAP: OnceLock<HashMap<u16, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| {
        reverse_unique(
            GLOBAL_IDENT_SITES
                .iter()
                .map(|(name, builtin)| (*builtin, *name)),
            "GLOBAL_IDENT",
        )
    })
}

fn static_member_reverse() -> &'static HashMap<u16, (&'static str, &'static str)> {
    static MAP: OnceLock<HashMap<u16, (&'static str, &'static str)>> = OnceLock::new();
    MAP.get_or_init(|| {
        reverse_unique(
            STATIC_MEMBER_SITES
                .iter()
                .map(|(object, property, builtin)| (*builtin, (*object, *property))),
            "STATIC_MEMBER",
        )
    })
}

fn string_proto_reverse() -> &'static HashMap<u16, &'static str> {
    static MAP: OnceLock<HashMap<u16, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| {
        reverse_unique(
            STRING_PROTO_SITES
                .iter()
                .map(|(name, builtin)| (*builtin, *name)),
            "STRING_PROTO",
        )
    })
}

fn array_proto_reverse() -> &'static HashMap<u16, &'static str> {
    static MAP: OnceLock<HashMap<u16, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| {
        reverse_unique(
            ARRAY_PROTO_SITES
                .iter()
                .map(|(name, builtin)| (*builtin, *name)),
            "ARRAY_PROTO",
        )
    })
}

/// 全局名 → 快路径 builtin（语义层快路径识别）。
pub fn global_ident_builtin(name: &str) -> Option<Builtin> {
    global_ident_forward().get(name).copied()
}

/// (容器全局名, 属性名) → 快路径 builtin。
pub fn static_member_builtin(object: &str, property: &str) -> Option<Builtin> {
    static_member_forward().get(&(object, property)).copied()
}

/// %String.prototype% 方法名 → 快路径 builtin。
pub fn string_proto_builtin(name: &str) -> Option<Builtin> {
    string_proto_forward().get(name).copied()
}

/// %Array.prototype% 方法名 → 快路径 builtin。
pub fn array_proto_builtin(name: &str) -> Option<Builtin> {
    array_proto_forward().get(name).copied()
}

/// 快路径 builtin → 全局名（宿主守卫 / 慢路径解析反查）。
pub fn global_ident_name(builtin: Builtin) -> Option<&'static str> {
    global_ident_reverse().get(&builtin.wire_id()).copied()
}

/// 快路径 builtin → (容器全局名, 属性名)。
pub fn static_member_names(builtin: Builtin) -> Option<(&'static str, &'static str)> {
    static_member_reverse().get(&builtin.wire_id()).copied()
}

/// 快路径 builtin → %String.prototype% 方法名。
pub fn string_proto_name(builtin: Builtin) -> Option<&'static str> {
    string_proto_reverse().get(&builtin.wire_id()).copied()
}

/// 快路径 builtin → %Array.prototype% 方法名。
pub fn array_proto_name(builtin: Builtin) -> Option<&'static str> {
    array_proto_reverse().get(&builtin.wire_id()).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 四张站点表的正反向映射必须一一对应：宿主按 wire_id 反查出的名字
    /// 必须与语义层匹配到该 builtin 的名字完全一致。
    #[test]
    fn intrinsic_site_tables_round_trip() {
        for (name, builtin) in GLOBAL_IDENT_SITES {
            assert_eq!(global_ident_builtin(name), Some(*builtin));
            assert_eq!(global_ident_name(*builtin), Some(*name));
        }
        for (object, property, builtin) in STATIC_MEMBER_SITES {
            assert_eq!(static_member_builtin(object, property), Some(*builtin));
            assert_eq!(static_member_names(*builtin), Some((*object, *property)));
        }
        for (name, builtin) in STRING_PROTO_SITES {
            assert_eq!(string_proto_builtin(name), Some(*builtin));
            assert_eq!(string_proto_name(*builtin), Some(*name));
        }
        for (name, builtin) in ARRAY_PROTO_SITES {
            assert_eq!(array_proto_builtin(name), Some(*builtin));
            assert_eq!(array_proto_name(*builtin), Some(*name));
        }
    }

    /// Web 全局真实属性名单必须与 GLOBAL_IDENT 站点表一一对应：`new` 守卫
    /// 与慢路径解析按 (GLOBAL_IDENT, wire_id) 反查名字，两表不一致会让守卫
    /// 判定与实际物化的属性错位。
    #[test]
    fn web_global_properties_align_with_global_ident_sites() {
        for (name, builtin, enumerable) in WEB_GLOBAL_PROPERTIES {
            assert_eq!(web_global_property(name), Some((*builtin, *enumerable)));
            assert_eq!(global_ident_builtin(name), Some(*builtin));
            assert_eq!(global_ident_name(*builtin), Some(*name));
        }
    }

    /// ES 侧急切物化名与 GLOBAL_IDENT 站点表对齐：`new SharedArrayBuffer`
    /// 守卫按 (GLOBAL_IDENT, wire_id) 反查名字；`Atomics` 只作 STATIC_MEMBER
    /// 容器名（命名空间对象无构造快路径），不得进 GLOBAL_IDENT 表。
    #[test]
    fn es_eager_globals_align_with_site_tables() {
        assert!(eager_global_property("SharedArrayBuffer"));
        assert!(eager_global_property("Atomics"));
        assert!(eager_global_property("fetch"));
        assert!(!eager_global_property("Math"));
        assert_eq!(
            global_ident_builtin("SharedArrayBuffer"),
            Some(Builtin::SharedArrayBufferConstructor)
        );
        assert_eq!(
            global_ident_name(Builtin::SharedArrayBufferConstructor),
            Some("SharedArrayBuffer")
        );
        assert_eq!(global_ident_builtin("Atomics"), None);
        assert!(
            STATIC_MEMBER_SITES
                .iter()
                .any(|(object, _, _)| *object == "Atomics")
        );
    }

    /// 站点名字全部为 ASCII：宿主可直接按字节构造 UTF-16 码元做非驻留探测。
    #[test]
    fn intrinsic_site_names_are_ascii() {
        let names = GLOBAL_IDENT_SITES
            .iter()
            .map(|(name, _)| *name)
            .chain(
                STATIC_MEMBER_SITES
                    .iter()
                    .flat_map(|(object, property, _)| [*object, *property]),
            )
            .chain(STRING_PROTO_SITES.iter().map(|(name, _)| *name))
            .chain(ARRAY_PROTO_SITES.iter().map(|(name, _)| *name));
        for name in names {
            assert!(name.is_ascii(), "站点名 {name:?} 必须为 ASCII");
        }
    }
}
