//! 内建函数的 JS 可见 `name`/`length` 元数据表。
//!
//! ECMAScript 对每个内建函数都规定了初始 `name`（§10.2.9 SetFunctionName 的
//! 内建形态，见各函数小节）与 `length`（§17 "unless otherwise specified"，
//! 取必选形参个数；可选/rest 形参不计入）。本表按 `wjsm_ir::Builtin` 变体
//! 逐一登记这两项，数值与 Node（V8）实测一致；仅编译器内部使用、无 JS 函数
//! 身份的变体（运算符、lowering 辅助、协程调度等）返回 `None`。
//!
//! 注意个别变体被多个 JS 身份复用（如 `IteratorFrom` 同时是
//! `Array.prototype.values` 与 `%Symbol.iterator%` 方法），登记值取规范中
//! 该共享函数对象的固有 `name`（Node 行为相同：`Set.prototype.keys.name`
//! 即为 `"values"`）。

use wjsm_ir::Builtin;

/// 内建函数的 JS 可见 `(name, length)`；非用户可观察函数返回 `None`。
pub(crate) fn builtin_function_metadata(builtin: Builtin) -> Option<(&'static str, u32)> {
    console_metadata(builtin)
        .or_else(|| global_metadata(builtin))
        .or_else(|| object_metadata(builtin))
        .or_else(|| function_reflect_metadata(builtin))
        .or_else(|| array_metadata(builtin))
        .or_else(|| string_metadata(builtin))
        .or_else(|| math_metadata(builtin))
        .or_else(|| number_boolean_symbol_metadata(builtin))
        .or_else(|| error_metadata(builtin))
        .or_else(|| collection_metadata(builtin))
        .or_else(|| promise_generator_metadata(builtin))
        .or_else(|| date_regexp_metadata(builtin))
        .or_else(|| binary_data_metadata(builtin))
}

/// console 方法（Node console；WHATWG console 规范 length 均为 0）。
fn console_metadata(builtin: Builtin) -> Option<(&'static str, u32)> {
    Some(match builtin {
        Builtin::ConsoleLog => ("log", 0),
        Builtin::ConsoleError => ("error", 0),
        Builtin::ConsoleWarn => ("warn", 0),
        Builtin::ConsoleInfo => ("info", 0),
        Builtin::ConsoleDebug => ("debug", 0),
        Builtin::ConsoleTrace => ("trace", 0),
        _ => return None,
    })
}

/// 全局函数与全局构造器入口（timer 按 Node 传统签名，web 接口按 Node 实现）。
fn global_metadata(builtin: Builtin) -> Option<(&'static str, u32)> {
    Some(match builtin {
        Builtin::SetTimeout => ("setTimeout", 5),
        Builtin::ClearTimeout => ("clearTimeout", 1),
        Builtin::SetInterval => ("setInterval", 5),
        Builtin::ClearInterval => ("clearInterval", 1),
        Builtin::Fetch => ("fetch", 1),
        Builtin::HeadersConstructor => ("Headers", 0),
        Builtin::RequestConstructor => ("Request", 1),
        Builtin::ResponseConstructor => ("Response", 0),
        Builtin::AbortControllerConstructor => ("AbortController", 0),
        Builtin::AbortSignalConstructor => ("AbortSignal", 0),
        Builtin::EventTargetConstructor => ("EventTarget", 0),
        Builtin::EventConstructor => ("Event", 1),
        Builtin::Eval | Builtin::EvalIndirect => ("eval", 1),
        Builtin::QueueMicrotask => ("queueMicrotask", 1),
        Builtin::StructuredClone => ("structuredClone", 2),
        Builtin::GlobalIsNaN => ("isNaN", 1),
        Builtin::GlobalIsFinite => ("isFinite", 1),
        Builtin::ReadableStreamConstructor => ("ReadableStream", 0),
        Builtin::WritableStreamConstructor => ("WritableStream", 0),
        Builtin::TransformStreamConstructor => ("TransformStream", 0),
        Builtin::CountQueuingStrategyConstructor => ("CountQueuingStrategy", 1),
        Builtin::ByteLengthQueuingStrategyConstructor => ("ByteLengthQueuingStrategy", 1),
        Builtin::PerformanceNow => ("now", 0),
        _ => return None,
    })
}

/// Object 构造器静态方法与 Object.prototype 方法。
fn object_metadata(builtin: Builtin) -> Option<(&'static str, u32)> {
    Some(match builtin {
        Builtin::ObjectKeys => ("keys", 1),
        Builtin::ObjectValues => ("values", 1),
        Builtin::ObjectEntries => ("entries", 1),
        Builtin::ObjectAssign => ("assign", 2),
        Builtin::ObjectCreate => ("create", 2),
        Builtin::DefineProperty => ("defineProperty", 3),
        Builtin::ObjectDefineProperties => ("defineProperties", 2),
        Builtin::GetOwnPropDesc => ("getOwnPropertyDescriptor", 2),
        Builtin::ObjectGetOwnPropertyDescriptors => ("getOwnPropertyDescriptors", 1),
        Builtin::ObjectGetOwnPropertyNames => ("getOwnPropertyNames", 1),
        Builtin::ObjectGetOwnPropertySymbols => ("getOwnPropertySymbols", 1),
        Builtin::ObjectGetPrototypeOf => ("getPrototypeOf", 1),
        Builtin::ObjectSetPrototypeOf => ("setPrototypeOf", 2),
        Builtin::ObjectIs => ("is", 2),
        Builtin::ObjectGroupBy => ("groupBy", 2),
        Builtin::ObjectHasOwn => ("hasOwn", 2),
        Builtin::ObjectFreeze => ("freeze", 1),
        Builtin::ObjectSeal => ("seal", 1),
        Builtin::ObjectIsFrozen => ("isFrozen", 1),
        Builtin::ObjectIsSealed => ("isSealed", 1),
        Builtin::ObjectIsExtensible => ("isExtensible", 1),
        Builtin::ObjectPreventExtensions => ("preventExtensions", 1),
        Builtin::ObjectFromEntries => ("fromEntries", 1),
        Builtin::HasOwnProperty => ("hasOwnProperty", 1),
        Builtin::PropertyIsEnumerable => ("propertyIsEnumerable", 1),
        Builtin::ObjectProtoToString => ("toString", 0),
        Builtin::ObjectProtoValueOf => ("valueOf", 0),
        Builtin::ObjectProtoIsPrototypeOf => ("isPrototypeOf", 1),
        Builtin::ObjectProtoToLocaleString => ("toLocaleString", 0),
        Builtin::ObjectProtoGetProto => ("get __proto__", 0),
        Builtin::ObjectProtoSetProto => ("set __proto__", 1),
        Builtin::ObjectProtoDefineGetter => ("__defineGetter__", 2),
        Builtin::ObjectProtoDefineSetter => ("__defineSetter__", 2),
        Builtin::ObjectProtoLookupGetter => ("__lookupGetter__", 1),
        Builtin::ObjectProtoLookupSetter => ("__lookupSetter__", 1),
        Builtin::JsonStringify => ("stringify", 3),
        Builtin::JsonParse => ("parse", 2),
        _ => return None,
    })
}

/// Function.prototype 方法与 Proxy / Reflect。
fn function_reflect_metadata(builtin: Builtin) -> Option<(&'static str, u32)> {
    Some(match builtin {
        Builtin::FuncCall => ("call", 1),
        Builtin::FuncApply => ("apply", 2),
        Builtin::FuncBind => ("bind", 1),
        Builtin::FunctionToString => ("toString", 0),
        Builtin::ProxyCreate => ("Proxy", 2),
        Builtin::ProxyRevocable => ("revocable", 2),
        Builtin::ReflectApply => ("apply", 3),
        Builtin::ReflectConstruct => ("construct", 2),
        Builtin::ReflectDefineProperty => ("defineProperty", 3),
        Builtin::ReflectDeleteProperty => ("deleteProperty", 2),
        Builtin::ReflectGet => ("get", 2),
        Builtin::ReflectGetOwnPropertyDescriptor => ("getOwnPropertyDescriptor", 2),
        Builtin::ReflectGetPrototypeOf => ("getPrototypeOf", 1),
        Builtin::ReflectHas => ("has", 2),
        Builtin::ReflectIsExtensible => ("isExtensible", 1),
        Builtin::ReflectOwnKeys => ("ownKeys", 1),
        Builtin::ReflectPreventExtensions => ("preventExtensions", 1),
        Builtin::ReflectSet => ("set", 3),
        Builtin::ReflectSetPrototypeOf => ("setPrototypeOf", 2),
        _ => return None,
    })
}

/// Array 构造器静态方法与 Array.prototype 方法。
fn array_metadata(builtin: Builtin) -> Option<(&'static str, u32)> {
    Some(match builtin {
        Builtin::ArrayIsArray => ("isArray", 1),
        Builtin::ArrayFrom => ("from", 1),
        Builtin::ArrayFromAsync => ("fromAsync", 1),
        Builtin::ArrayOf => ("of", 0),
        Builtin::ArrayPush => ("push", 1),
        Builtin::ArrayPop => ("pop", 0),
        Builtin::ArrayIncludes => ("includes", 1),
        Builtin::ArrayIndexOf => ("indexOf", 1),
        Builtin::ArrayLastIndexOf => ("lastIndexOf", 1),
        Builtin::ArrayJoin => ("join", 1),
        Builtin::ArrayConcat | Builtin::ArrayConcatVa => ("concat", 1),
        Builtin::ArraySlice => ("slice", 2),
        Builtin::ArraySpliceVa => ("splice", 2),
        Builtin::ArrayFill => ("fill", 1),
        Builtin::ArrayReverse => ("reverse", 0),
        Builtin::ArrayFlat => ("flat", 0),
        Builtin::ArrayFlatMap => ("flatMap", 1),
        Builtin::ArrayShift => ("shift", 0),
        Builtin::ArrayUnshiftVa => ("unshift", 1),
        Builtin::ArraySort => ("sort", 1),
        Builtin::ArrayAt => ("at", 1),
        Builtin::ArrayCopyWithin => ("copyWithin", 2),
        Builtin::ArrayForEach => ("forEach", 1),
        Builtin::ArrayMap => ("map", 1),
        Builtin::ArrayFilter => ("filter", 1),
        Builtin::ArrayReduce => ("reduce", 1),
        Builtin::ArrayReduceRight => ("reduceRight", 1),
        Builtin::ArrayFind => ("find", 1),
        Builtin::ArrayFindIndex => ("findIndex", 1),
        Builtin::ArrayFindLast => ("findLast", 1),
        Builtin::ArrayFindLastIndex => ("findLastIndex", 1),
        Builtin::ArraySome => ("some", 1),
        Builtin::ArrayEvery => ("every", 1),
        Builtin::ArrayToSorted => ("toSorted", 1),
        Builtin::ArrayToReversed => ("toReversed", 0),
        Builtin::ArrayToSplicedVa => ("toSpliced", 2),
        Builtin::ArrayWith => ("with", 2),
        // Array.prototype.values 与 %Symbol.iterator% 共享同一函数对象
        // （§23.1.3.41），固有 name 为 "values"。
        Builtin::IteratorFrom => ("values", 0),
        _ => return None,
    })
}

/// String 构造器静态方法与 String.prototype 方法。
fn string_metadata(builtin: Builtin) -> Option<(&'static str, u32)> {
    Some(match builtin {
        Builtin::StringFromCharCode => ("fromCharCode", 1),
        Builtin::StringFromCodePoint => ("fromCodePoint", 1),
        Builtin::StringRaw => ("raw", 1),
        Builtin::StringAt => ("at", 1),
        Builtin::StringCharAt => ("charAt", 1),
        Builtin::StringCharCodeAt => ("charCodeAt", 1),
        Builtin::StringCodePointAt => ("codePointAt", 1),
        Builtin::StringConcatVa => ("concat", 1),
        Builtin::StringEndsWith => ("endsWith", 1),
        Builtin::StringIncludes => ("includes", 1),
        Builtin::StringIndexOf => ("indexOf", 1),
        Builtin::StringIsWellFormed => ("isWellFormed", 0),
        Builtin::StringLastIndexOf => ("lastIndexOf", 1),
        Builtin::StringMatch => ("match", 1),
        Builtin::StringMatchAll => ("matchAll", 1),
        Builtin::StringNormalize => ("normalize", 0),
        Builtin::StringPadEnd => ("padEnd", 1),
        Builtin::StringPadStart => ("padStart", 1),
        Builtin::StringRepeat => ("repeat", 1),
        Builtin::StringReplace => ("replace", 2),
        Builtin::StringReplaceAll => ("replaceAll", 2),
        Builtin::StringSearch => ("search", 1),
        Builtin::StringSlice => ("slice", 2),
        Builtin::StringSplit => ("split", 2),
        Builtin::StringStartsWith => ("startsWith", 1),
        Builtin::StringSubstring => ("substring", 2),
        Builtin::StringToLowerCase => ("toLowerCase", 0),
        Builtin::StringToUpperCase => ("toUpperCase", 0),
        Builtin::StringToWellFormed => ("toWellFormed", 0),
        Builtin::StringTrim => ("trim", 0),
        Builtin::StringTrimEnd => ("trimEnd", 0),
        Builtin::StringTrimStart => ("trimStart", 0),
        Builtin::StringToString => ("toString", 0),
        Builtin::StringValueOf => ("valueOf", 0),
        Builtin::StringIterator => ("[Symbol.iterator]", 0),
        _ => return None,
    })
}

/// Math 命名空间静态方法。
fn math_metadata(builtin: Builtin) -> Option<(&'static str, u32)> {
    Some(match builtin {
        Builtin::MathAbs => ("abs", 1),
        Builtin::MathAcos => ("acos", 1),
        Builtin::MathAcosh => ("acosh", 1),
        Builtin::MathAsin => ("asin", 1),
        Builtin::MathAsinh => ("asinh", 1),
        Builtin::MathAtan => ("atan", 1),
        Builtin::MathAtanh => ("atanh", 1),
        Builtin::MathAtan2 => ("atan2", 2),
        Builtin::MathCbrt => ("cbrt", 1),
        Builtin::MathCeil => ("ceil", 1),
        Builtin::MathClz32 => ("clz32", 1),
        Builtin::MathCos => ("cos", 1),
        Builtin::MathCosh => ("cosh", 1),
        Builtin::MathExp => ("exp", 1),
        Builtin::MathExpm1 => ("expm1", 1),
        Builtin::MathFloor => ("floor", 1),
        Builtin::MathFround => ("fround", 1),
        Builtin::MathHypot => ("hypot", 2),
        Builtin::MathImul => ("imul", 2),
        Builtin::MathLog => ("log", 1),
        Builtin::MathLog1p => ("log1p", 1),
        Builtin::MathLog10 => ("log10", 1),
        Builtin::MathLog2 => ("log2", 1),
        Builtin::MathMax => ("max", 2),
        Builtin::MathMin => ("min", 2),
        Builtin::MathPow => ("pow", 2),
        Builtin::MathRandom => ("random", 0),
        Builtin::MathRound => ("round", 1),
        Builtin::MathSign => ("sign", 1),
        Builtin::MathSin => ("sin", 1),
        Builtin::MathSinh => ("sinh", 1),
        Builtin::MathSqrt => ("sqrt", 1),
        Builtin::MathTan => ("tan", 1),
        Builtin::MathTanh => ("tanh", 1),
        Builtin::MathTrunc => ("trunc", 1),
        _ => return None,
    })
}

/// Number / Boolean / Symbol / BigInt 构造器、静态与原型方法。
fn number_boolean_symbol_metadata(builtin: Builtin) -> Option<(&'static str, u32)> {
    Some(match builtin {
        Builtin::NumberConstructor => ("Number", 1),
        Builtin::NumberIsNaN => ("isNaN", 1),
        Builtin::NumberIsFinite => ("isFinite", 1),
        Builtin::NumberIsInteger => ("isInteger", 1),
        Builtin::NumberIsSafeInteger => ("isSafeInteger", 1),
        Builtin::NumberParseInt => ("parseInt", 2),
        Builtin::NumberParseFloat => ("parseFloat", 1),
        Builtin::NumberProtoToString => ("toString", 1),
        Builtin::NumberProtoValueOf => ("valueOf", 0),
        Builtin::NumberProtoToFixed => ("toFixed", 1),
        Builtin::NumberProtoToExponential => ("toExponential", 1),
        Builtin::NumberProtoToPrecision => ("toPrecision", 1),
        Builtin::BooleanConstructor => ("Boolean", 1),
        Builtin::BooleanProtoToString => ("toString", 0),
        Builtin::BooleanProtoValueOf => ("valueOf", 0),
        Builtin::SymbolCreate => ("Symbol", 0),
        Builtin::SymbolFor => ("for", 1),
        Builtin::SymbolKeyFor => ("keyFor", 1),
        Builtin::SymbolProtoToString => ("toString", 0),
        Builtin::SymbolProtoValueOf => ("valueOf", 0),
        Builtin::BigIntFromLiteral => ("BigInt", 1),
        Builtin::BigIntProtoToString => ("toString", 0),
        Builtin::BigIntProtoValueOf => ("valueOf", 0),
        _ => return None,
    })
}

/// Error 构造器族与 Error.prototype.toString。
fn error_metadata(builtin: Builtin) -> Option<(&'static str, u32)> {
    Some(match builtin {
        Builtin::ErrorConstructor => ("Error", 1),
        Builtin::TypeErrorConstructor => ("TypeError", 1),
        Builtin::RangeErrorConstructor => ("RangeError", 1),
        Builtin::SyntaxErrorConstructor => ("SyntaxError", 1),
        Builtin::ReferenceErrorConstructor => ("ReferenceError", 1),
        Builtin::URIErrorConstructor => ("URIError", 1),
        Builtin::EvalErrorConstructor => ("EvalError", 1),
        Builtin::ErrorProtoToString => ("toString", 0),
        _ => return None,
    })
}

/// Map / Set / WeakMap / WeakSet / WeakRef / FinalizationRegistry。
fn collection_metadata(builtin: Builtin) -> Option<(&'static str, u32)> {
    Some(match builtin {
        Builtin::MapConstructor => ("Map", 0),
        Builtin::MapGroupBy => ("groupBy", 2),
        Builtin::MapProtoSet => ("set", 2),
        Builtin::MapProtoGet => ("get", 1),
        Builtin::SetConstructor => ("Set", 0),
        Builtin::SetProtoAdd => ("add", 1),
        Builtin::SetProtoHas => ("has", 1),
        Builtin::SetProtoDelete => ("delete", 1),
        Builtin::SetProtoUnion => ("union", 1),
        Builtin::SetProtoIntersection => ("intersection", 1),
        Builtin::SetProtoDifference => ("difference", 1),
        Builtin::SetProtoSymmetricDifference => ("symmetricDifference", 1),
        Builtin::SetProtoIsSubsetOf => ("isSubsetOf", 1),
        Builtin::SetProtoIsSupersetOf => ("isSupersetOf", 1),
        Builtin::SetProtoIsDisjointFrom => ("isDisjointFrom", 1),
        Builtin::MapSetHas => ("has", 1),
        Builtin::MapSetDelete => ("delete", 1),
        Builtin::MapSetClear => ("clear", 0),
        Builtin::MapSetForEach => ("forEach", 1),
        Builtin::MapSetKeys => ("keys", 0),
        // Set.prototype.keys 与 values 共享同一函数对象（§24.2.4.16），
        // 固有 name 为 "values"。
        Builtin::MapSetValues => ("values", 0),
        Builtin::MapSetEntries => ("entries", 0),
        Builtin::WeakMapConstructor => ("WeakMap", 0),
        Builtin::WeakMapProtoSet => ("set", 2),
        Builtin::WeakMapProtoGet => ("get", 1),
        Builtin::WeakMapProtoHas => ("has", 1),
        Builtin::WeakMapProtoDelete => ("delete", 1),
        Builtin::WeakSetConstructor => ("WeakSet", 0),
        Builtin::WeakSetProtoAdd => ("add", 1),
        Builtin::WeakSetProtoHas => ("has", 1),
        Builtin::WeakSetProtoDelete => ("delete", 1),
        Builtin::WeakRefConstructor => ("WeakRef", 1),
        Builtin::WeakRefProtoDeref => ("deref", 0),
        Builtin::FinalizationRegistryConstructor => ("FinalizationRegistry", 1),
        Builtin::FinalizationRegistryProtoRegister => ("register", 2),
        Builtin::FinalizationRegistryProtoUnregister => ("unregister", 1),
        _ => return None,
    })
}

/// Promise 构造器、静态与原型方法，以及（异步）生成器原型方法。
fn promise_generator_metadata(builtin: Builtin) -> Option<(&'static str, u32)> {
    Some(match builtin {
        Builtin::PromiseCreate => ("Promise", 1),
        Builtin::PromiseThen => ("then", 2),
        Builtin::PromiseCatch => ("catch", 1),
        Builtin::PromiseFinally => ("finally", 1),
        Builtin::PromiseAll => ("all", 1),
        Builtin::PromiseRace => ("race", 1),
        Builtin::PromiseAllSettled => ("allSettled", 1),
        Builtin::PromiseAny => ("any", 1),
        Builtin::PromiseResolveStatic => ("resolve", 1),
        Builtin::PromiseRejectStatic => ("reject", 1),
        Builtin::PromiseWithResolvers => ("withResolvers", 0),
        Builtin::GeneratorNext | Builtin::AsyncGeneratorNext => ("next", 1),
        Builtin::GeneratorReturn | Builtin::AsyncGeneratorReturn => ("return", 1),
        Builtin::GeneratorThrow | Builtin::AsyncGeneratorThrow => ("throw", 1),
        _ => return None,
    })
}

/// Date 构造器/静态方法与 RegExp 方法（含 well-known symbol 方法）。
fn date_regexp_metadata(builtin: Builtin) -> Option<(&'static str, u32)> {
    Some(match builtin {
        Builtin::DateConstructor | Builtin::DateConstructorNew => ("Date", 7),
        Builtin::DateNow => ("now", 0),
        Builtin::DateParse => ("parse", 1),
        Builtin::DateUTC => ("UTC", 7),
        Builtin::RegExpCreate => ("RegExp", 2),
        Builtin::RegExpTest => ("test", 1),
        Builtin::RegExpExec => ("exec", 1),
        Builtin::RegExpProtoMatch => ("[Symbol.match]", 1),
        Builtin::RegExpProtoMatchAll => ("[Symbol.matchAll]", 1),
        Builtin::RegExpProtoReplace => ("[Symbol.replace]", 2),
        Builtin::RegExpProtoSearch => ("[Symbol.search]", 1),
        Builtin::RegExpProtoSplit => ("[Symbol.split]", 2),
        _ => return None,
    })
}

/// ArrayBuffer / SharedArrayBuffer / DataView / TypedArray / Atomics。
fn binary_data_metadata(builtin: Builtin) -> Option<(&'static str, u32)> {
    Some(match builtin {
        Builtin::ArrayBufferConstructor => ("ArrayBuffer", 1),
        Builtin::ArrayBufferProtoSlice => ("slice", 2),
        Builtin::SharedArrayBufferConstructor => ("SharedArrayBuffer", 1),
        Builtin::SharedArrayBufferProtoGrow => ("grow", 1),
        Builtin::SharedArrayBufferProtoSlice => ("slice", 2),
        Builtin::DataViewConstructor => ("DataView", 1),
        Builtin::DataViewProtoGetFloat64 => ("getFloat64", 1),
        Builtin::DataViewProtoGetFloat32 => ("getFloat32", 1),
        Builtin::DataViewProtoGetInt32 => ("getInt32", 1),
        Builtin::DataViewProtoGetUint32 => ("getUint32", 1),
        Builtin::DataViewProtoGetInt16 => ("getInt16", 1),
        Builtin::DataViewProtoGetUint16 => ("getUint16", 1),
        Builtin::DataViewProtoGetInt8 => ("getInt8", 1),
        Builtin::DataViewProtoGetUint8 => ("getUint8", 1),
        Builtin::DataViewProtoSetFloat64 => ("setFloat64", 2),
        Builtin::DataViewProtoSetFloat32 => ("setFloat32", 2),
        Builtin::DataViewProtoSetInt32 => ("setInt32", 2),
        Builtin::DataViewProtoSetUint32 => ("setUint32", 2),
        Builtin::DataViewProtoSetInt16 => ("setInt16", 2),
        Builtin::DataViewProtoSetUint16 => ("setUint16", 2),
        Builtin::DataViewProtoSetInt8 => ("setInt8", 2),
        Builtin::DataViewProtoSetUint8 => ("setUint8", 2),
        Builtin::DataViewProtoGetBigInt64 => ("getBigInt64", 1),
        Builtin::DataViewProtoGetBigUint64 => ("getBigUint64", 1),
        Builtin::DataViewProtoSetBigInt64 => ("setBigInt64", 2),
        Builtin::DataViewProtoSetBigUint64 => ("setBigUint64", 2),
        Builtin::Int8ArrayConstructor => ("Int8Array", 3),
        Builtin::Uint8ArrayConstructor => ("Uint8Array", 3),
        Builtin::Uint8ClampedArrayConstructor => ("Uint8ClampedArray", 3),
        Builtin::Int16ArrayConstructor => ("Int16Array", 3),
        Builtin::Uint16ArrayConstructor => ("Uint16Array", 3),
        Builtin::Int32ArrayConstructor => ("Int32Array", 3),
        Builtin::Uint32ArrayConstructor => ("Uint32Array", 3),
        Builtin::Float32ArrayConstructor => ("Float32Array", 3),
        Builtin::Float64ArrayConstructor => ("Float64Array", 3),
        Builtin::BigInt64ArrayConstructor => ("BigInt64Array", 3),
        Builtin::BigUint64ArrayConstructor => ("BigUint64Array", 3),
        Builtin::TypedArrayProtoLength => ("get length", 0),
        Builtin::TypedArrayProtoByteLength => ("get byteLength", 0),
        Builtin::TypedArrayProtoByteOffset => ("get byteOffset", 0),
        Builtin::TypedArrayProtoSet => ("set", 1),
        Builtin::TypedArrayProtoSlice => ("slice", 2),
        Builtin::TypedArrayProtoSubarray => ("subarray", 2),
        Builtin::TypedArrayProtoFill => ("fill", 1),
        Builtin::TypedArrayProtoReverse => ("reverse", 0),
        Builtin::TypedArrayProtoIndexOf => ("indexOf", 1),
        Builtin::TypedArrayProtoLastIndexOf => ("lastIndexOf", 1),
        Builtin::TypedArrayProtoIncludes => ("includes", 1),
        Builtin::TypedArrayProtoJoin => ("join", 1),
        Builtin::TypedArrayProtoToString => ("toString", 0),
        Builtin::TypedArrayProtoCopyWithin => ("copyWithin", 2),
        Builtin::TypedArrayProtoAt => ("at", 1),
        Builtin::TypedArrayProtoForEach => ("forEach", 1),
        Builtin::TypedArrayProtoMap => ("map", 1),
        Builtin::TypedArrayProtoFilter => ("filter", 1),
        Builtin::TypedArrayProtoReduce => ("reduce", 1),
        Builtin::TypedArrayProtoReduceRight => ("reduceRight", 1),
        Builtin::TypedArrayProtoFind => ("find", 1),
        Builtin::TypedArrayProtoFindIndex => ("findIndex", 1),
        Builtin::TypedArrayProtoSome => ("some", 1),
        Builtin::TypedArrayProtoEvery => ("every", 1),
        Builtin::TypedArrayProtoSort => ("sort", 1),
        Builtin::TypedArrayProtoEntries => ("entries", 0),
        Builtin::TypedArrayProtoKeys => ("keys", 0),
        Builtin::TypedArrayProtoValues => ("values", 0),
        Builtin::AtomicsLoad => ("load", 2),
        Builtin::AtomicsStore => ("store", 3),
        Builtin::AtomicsAdd => ("add", 3),
        Builtin::AtomicsSub => ("sub", 3),
        Builtin::AtomicsAnd => ("and", 3),
        Builtin::AtomicsOr => ("or", 3),
        Builtin::AtomicsXor => ("xor", 3),
        Builtin::AtomicsExchange => ("exchange", 3),
        Builtin::AtomicsCompareExchange => ("compareExchange", 4),
        Builtin::AtomicsIsLockFree => ("isLockFree", 1),
        Builtin::AtomicsPause => ("pause", 0),
        Builtin::AtomicsWait => ("wait", 4),
        Builtin::AtomicsNotify => ("notify", 3),
        Builtin::AtomicsWaitAsync => ("waitAsync", 4),
        _ => return None,
    })
}
