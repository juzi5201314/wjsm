use super::*;

pub(crate) const BUILTIN_GLOBALS: &[&str] = &[
    "Array",
    "Object",
    "Function",
    "String",
    "Boolean",
    "Number",
    "Symbol",
    "BigInt",
    "RegExp",
    "Error",
    "TypeError",
    "RangeError",
    "SyntaxError",
    "ReferenceError",
    "URIError",
    "EvalError",
    "AggregateError",
    "SuppressedError",
    "Map",
    "Set",
    "WeakMap",
    "WeakSet",
    "Date",
    "Promise",
    "ArrayBuffer",
    "SharedArrayBuffer",
    "DataView",
    "Int8Array",
    "Uint8Array",
    "Uint8ClampedArray",
    "Int16Array",
    "Uint16Array",
    "Int32Array",
    "Uint32Array",
    "Float32Array",
    "Float64Array",
    "Float16Array",
    "BigInt64Array",
    "BigUint64Array",
    "Proxy",
    "Math",
    "JSON",
    "Reflect",
    "globalThis",
    "parseInt",
    "parseFloat",
    "isNaN",
    "isFinite",
    "decodeURI",
    "decodeURIComponent",
    "encodeURI",
    "encodeURIComponent",
    "Atomics",
    "FinalizationRegistry",
    "WeakRef",
    "Headers",
    "Request",
    "Response",
    "ReadableStream",
    "WritableStream",
    "AbortController",
    "Intl",
    "Iterator",
    "AsyncIterator",
    "$262",
];

pub(crate) fn is_builtin_global(name: &str) -> bool {
    BUILTIN_GLOBALS.contains(&name)
}

pub(crate) fn builtin_from_global_ident(name: &str) -> Option<Builtin> {
    match name {
        "setTimeout" => Some(Builtin::SetTimeout),
        "clearTimeout" => Some(Builtin::ClearTimeout),
        "setInterval" => Some(Builtin::SetInterval),
        "clearInterval" => Some(Builtin::ClearInterval),
        "fetch" => Some(Builtin::Fetch),
        "Headers" => Some(Builtin::HeadersConstructor),
        "Request" => Some(Builtin::RequestConstructor),
        "Response" => Some(Builtin::ResponseConstructor),
        "AbortController" => Some(Builtin::AbortControllerConstructor),
        "ReadableStream" => Some(Builtin::ReadableStreamConstructor),
        "eval" => Some(Builtin::Eval),
        "WritableStream" => Some(Builtin::WritableStreamConstructor),
        "Symbol" => Some(Builtin::SymbolCreate),
        "queueMicrotask" => Some(Builtin::QueueMicrotask),
        "Proxy" => Some(Builtin::ProxyCreate),
        "Number" => Some(Builtin::NumberConstructor),
        "Boolean" => Some(Builtin::BooleanConstructor),
        "Error" => Some(Builtin::ErrorConstructor),
        "TypeError" => Some(Builtin::TypeErrorConstructor),
        "RangeError" => Some(Builtin::RangeErrorConstructor),
        "SyntaxError" => Some(Builtin::SyntaxErrorConstructor),
        "ReferenceError" => Some(Builtin::ReferenceErrorConstructor),
        "URIError" => Some(Builtin::URIErrorConstructor),
        "EvalError" => Some(Builtin::EvalErrorConstructor),
        "Map" => Some(Builtin::MapConstructor),
        "Set" => Some(Builtin::SetConstructor),
        "WeakMap" => Some(Builtin::WeakMapConstructor),
        "WeakSet" => Some(Builtin::WeakSetConstructor),
        "WeakRef" => Some(Builtin::WeakRefConstructor),
        "FinalizationRegistry" => Some(Builtin::FinalizationRegistryConstructor),
        "Date" => Some(Builtin::DateConstructor),
        "ArrayBuffer" => Some(Builtin::ArrayBufferConstructor),
        "SharedArrayBuffer" => Some(Builtin::SharedArrayBufferConstructor),
        "DataView" => Some(Builtin::DataViewConstructor),
        "Int8Array" => Some(Builtin::Int8ArrayConstructor),
        "Uint8Array" => Some(Builtin::Uint8ArrayConstructor),
        "Uint8ClampedArray" => Some(Builtin::Uint8ClampedArrayConstructor),
        "Int16Array" => Some(Builtin::Int16ArrayConstructor),
        "Uint16Array" => Some(Builtin::Uint16ArrayConstructor),
        "Int32Array" => Some(Builtin::Int32ArrayConstructor),
        "Uint32Array" => Some(Builtin::Uint32ArrayConstructor),
        "Float32Array" => Some(Builtin::Float32ArrayConstructor),
        "Float64Array" => Some(Builtin::Float64ArrayConstructor),
        "BigInt64Array" => Some(Builtin::BigInt64ArrayConstructor),
        "BigUint64Array" => Some(Builtin::BigUint64ArrayConstructor),
        _ => None,
    }
}

pub(crate) fn builtin_from_static_member(object: &str, property: &str) -> Option<Builtin> {
    match object {
        "console" => match property {
            "log" => Some(Builtin::ConsoleLog),
            "error" => Some(Builtin::ConsoleError),
            "warn" => Some(Builtin::ConsoleWarn),
            "info" => Some(Builtin::ConsoleInfo),
            "debug" => Some(Builtin::ConsoleDebug),
            "trace" => Some(Builtin::ConsoleTrace),
            _ => None,
        },
        "Array" => match property {
            "isArray" => Some(Builtin::ArrayIsArray),
            "from" => Some(Builtin::ArrayFrom),
            _ => None,
        },
        "Object" => match property {
            "defineProperty" => Some(Builtin::DefineProperty),
            "getOwnPropertyDescriptor" => Some(Builtin::GetOwnPropDesc),
            "keys" => Some(Builtin::ObjectKeys),
            "values" => Some(Builtin::ObjectValues),
            "entries" => Some(Builtin::ObjectEntries),
            "assign" => Some(Builtin::ObjectAssign),
            "create" => Some(Builtin::ObjectCreate),
            "getPrototypeOf" => Some(Builtin::ObjectGetPrototypeOf),
            "setPrototypeOf" => Some(Builtin::ObjectSetPrototypeOf),
            "getOwnPropertyNames" => Some(Builtin::ObjectGetOwnPropertyNames),
            "is" => Some(Builtin::ObjectIs),
            "groupBy" => Some(Builtin::ObjectGroupBy),
            "isExtensible" => Some(Builtin::ObjectIsExtensible),
            "preventExtensions" => Some(Builtin::ObjectPreventExtensions),
            _ => None,
        },
        "Map" => match property {
            "groupBy" => Some(Builtin::MapGroupBy),
            _ => None,
        },
        "JSON" => match property {
            "stringify" => Some(Builtin::JsonStringify),
            "parse" => Some(Builtin::JsonParse),
            _ => None,
        },
        "Symbol" => match property {
            "for" => Some(Builtin::SymbolFor),
            "keyFor" => Some(Builtin::SymbolKeyFor),
            _ => None,
        },
        "Promise" => match property {
            "resolve" => Some(Builtin::PromiseResolveStatic),
            "reject" => Some(Builtin::PromiseRejectStatic),
            "all" => Some(Builtin::PromiseAll),
            "race" => Some(Builtin::PromiseRace),
            "allSettled" => Some(Builtin::PromiseAllSettled),
            "any" => Some(Builtin::PromiseAny),
            "withResolvers" => Some(Builtin::PromiseWithResolvers),
            _ => None,
        },
        "String" => match property {
            "fromCharCode" => Some(Builtin::StringFromCharCode),
            "fromCodePoint" => Some(Builtin::StringFromCodePoint),
            _ => None,
        },
        "Proxy" => match property {
            "revocable" => Some(Builtin::ProxyRevocable),
            _ => None,
        },
        "Reflect" => match property {
            "get" => Some(Builtin::ReflectGet),
            "set" => Some(Builtin::ReflectSet),
            "has" => Some(Builtin::ReflectHas),
            "deleteProperty" => Some(Builtin::ReflectDeleteProperty),
            "apply" => Some(Builtin::ReflectApply),
            "construct" => Some(Builtin::ReflectConstruct),
            "getPrototypeOf" => Some(Builtin::ReflectGetPrototypeOf),
            "setPrototypeOf" => Some(Builtin::ReflectSetPrototypeOf),
            "isExtensible" => Some(Builtin::ReflectIsExtensible),
            "preventExtensions" => Some(Builtin::ReflectPreventExtensions),
            "getOwnPropertyDescriptor" => Some(Builtin::ReflectGetOwnPropertyDescriptor),
            "defineProperty" => Some(Builtin::ReflectDefineProperty),
            "ownKeys" => Some(Builtin::ReflectOwnKeys),
            _ => None,
        },
        "Math" => match property {
            "abs" => Some(Builtin::MathAbs),
            "acos" => Some(Builtin::MathAcos),
            "acosh" => Some(Builtin::MathAcosh),
            "asin" => Some(Builtin::MathAsin),
            "asinh" => Some(Builtin::MathAsinh),
            "atan" => Some(Builtin::MathAtan),
            "atanh" => Some(Builtin::MathAtanh),
            "atan2" => Some(Builtin::MathAtan2),
            "cbrt" => Some(Builtin::MathCbrt),
            "ceil" => Some(Builtin::MathCeil),
            "clz32" => Some(Builtin::MathClz32),
            "cos" => Some(Builtin::MathCos),
            "cosh" => Some(Builtin::MathCosh),
            "exp" => Some(Builtin::MathExp),
            "expm1" => Some(Builtin::MathExpm1),
            "floor" => Some(Builtin::MathFloor),
            "fround" => Some(Builtin::MathFround),
            "hypot" => Some(Builtin::MathHypot),
            "imul" => Some(Builtin::MathImul),
            "log" => Some(Builtin::MathLog),
            "log1p" => Some(Builtin::MathLog1p),
            "log10" => Some(Builtin::MathLog10),
            "log2" => Some(Builtin::MathLog2),
            "max" => Some(Builtin::MathMax),
            "min" => Some(Builtin::MathMin),
            "pow" => Some(Builtin::MathPow),
            "random" => Some(Builtin::MathRandom),
            "round" => Some(Builtin::MathRound),
            "sign" => Some(Builtin::MathSign),
            "sin" => Some(Builtin::MathSin),
            "sinh" => Some(Builtin::MathSinh),
            "sqrt" => Some(Builtin::MathSqrt),
            "tan" => Some(Builtin::MathTan),
            "tanh" => Some(Builtin::MathTanh),
            "trunc" => Some(Builtin::MathTrunc),
            _ => None,
        },
        "Number" => match property {
            "isNaN" => Some(Builtin::NumberIsNaN),
            "isFinite" => Some(Builtin::NumberIsFinite),
            "isInteger" => Some(Builtin::NumberIsInteger),
            "isSafeInteger" => Some(Builtin::NumberIsSafeInteger),
            "parseInt" => Some(Builtin::NumberParseInt),
            "parseFloat" => Some(Builtin::NumberParseFloat),
            _ => None,
        },
        "Date" => match property {
            "now" => Some(Builtin::DateNow),
            "parse" => Some(Builtin::DateParse),
            "UTC" => Some(Builtin::DateUTC),
            _ => None,
        },
        "WeakRef" => match property {
            "deref" => Some(Builtin::WeakRefProtoDeref),
            _ => None,
        },
        "FinalizationRegistry" => match property {
            "register" => Some(Builtin::FinalizationRegistryProtoRegister),
            "unregister" => Some(Builtin::FinalizationRegistryProtoUnregister),
            _ => None,
        },
        "Atomics" => match property {
            "load" => Some(Builtin::AtomicsLoad),
            "store" => Some(Builtin::AtomicsStore),
            "add" => Some(Builtin::AtomicsAdd),
            "sub" => Some(Builtin::AtomicsSub),
            "and" => Some(Builtin::AtomicsAnd),
            "or" => Some(Builtin::AtomicsOr),
            "xor" => Some(Builtin::AtomicsXor),
            "exchange" => Some(Builtin::AtomicsExchange),
            "compareExchange" => Some(Builtin::AtomicsCompareExchange),
            "isLockFree" => Some(Builtin::AtomicsIsLockFree),
            "wait" => Some(Builtin::AtomicsWait),
            "notify" => Some(Builtin::AtomicsNotify),
            "waitAsync" => Some(Builtin::AtomicsWaitAsync),
            _ => None,
        },
        _ => None,
    }
}

/// 将 Array.prototype 方法名映射到 Builtin 变体，用于语义层优化。
/// 当 `a.filter(cb)` 被识别时，跳过运行时属性解析，直接发出 CallBuiltin。
/// 仅包含使用 Type 12 影子栈调用约定的方法（Group 2）。
pub(crate) fn builtin_from_array_proto_method(name: &str) -> Option<Builtin> {
    use Builtin::*;
    match name {
        "shift" => Some(ArrayShift),
        "unshift" => Some(ArrayUnshiftVa),
        "sort" => Some(ArraySort),
        "at" => Some(ArrayAt),
        "copyWithin" => Some(ArrayCopyWithin),
        "forEach" => Some(ArrayForEach),
        "map" => Some(ArrayMap),
        "filter" => Some(ArrayFilter),
        "reduce" => Some(ArrayReduce),
        "reduceRight" => Some(ArrayReduceRight),
        "find" => Some(ArrayFind),
        "findIndex" => Some(ArrayFindIndex),
        "some" => Some(ArraySome),
        "every" => Some(ArrayEvery),
        "flatMap" => Some(ArrayFlatMap),
        "flat" => Some(ArrayFlat),
        "concat" => Some(ArrayConcatVa),
        "splice" => Some(ArraySpliceVa),
        _ => None,
    }
}

pub(crate) fn builtin_from_function_proto_method(name: &str) -> Option<Builtin> {
    match name {
        "call" => Some(Builtin::FuncCall),
        "apply" => Some(Builtin::FuncApply),
        "bind" => Some(Builtin::FuncBind),
        _ => None,
    }
}
/// 将 Object.prototype 方法名映射到 Builtin 变体，用于语义层优化。
/// 当 `obj.hasOwnProperty(key)` 被识别时，跳过运行时属性解析，直接发出 CallBuiltin。
///
/// 注意: toString / valueOf 在此处拦截，用于 Object.prototype.toString/valueOf 调用。
/// 对于 Array.prototype.toString、Date.prototype.valueOf 等特定原型的实现，
/// 仍需通过运行时原型链查找调用（当前未实现）。
pub(crate) fn builtin_from_object_proto_method(name: &str) -> Option<Builtin> {
    match name {
        "hasOwnProperty" => Some(Builtin::HasOwnProperty),
        "toString" => Some(Builtin::ObjectProtoToString),
        "valueOf" => Some(Builtin::ObjectProtoValueOf),
        _ => None,
    }
}

/// 将 String.prototype 方法名映射到 Builtin 变体，用于语义层优化。
/// 当 `str.match(/.../)` 被识别时，跳过运行时属性解析，直接发出 CallBuiltin。
pub(crate) fn builtin_from_string_proto_method(name: &str) -> Option<Builtin> {
    use Builtin::*;
    match name {
        "match" => Some(StringMatch),
        "replace" => Some(StringReplace),
        "search" => Some(StringSearch),
        "split" => Some(StringSplit),
        "at" => Some(StringAt),
        "charAt" => Some(StringCharAt),
        "charCodeAt" => Some(StringCharCodeAt),
        "codePointAt" => Some(StringCodePointAt),
        "concat" => Some(StringConcatVa),
        "endsWith" => Some(StringEndsWith),
        "includes" => Some(StringIncludes),
        "indexOf" => Some(StringIndexOf),
        "lastIndexOf" => Some(StringLastIndexOf),
        "matchAll" => Some(StringMatchAll),
        "padEnd" => Some(StringPadEnd),
        "padStart" => Some(StringPadStart),
        "repeat" => Some(StringRepeat),
        "replaceAll" => Some(StringReplaceAll),
        "slice" => Some(StringSlice),
        "startsWith" => Some(StringStartsWith),
        "substring" => Some(StringSubstring),
        "toLowerCase" => Some(StringToLowerCase),
        "toUpperCase" => Some(StringToUpperCase),
        "trim" => Some(StringTrim),
        "trimEnd" => Some(StringTrimEnd),
        "trimStart" => Some(StringTrimStart),
        "toString" => Some(StringToString),
        "valueOf" => Some(StringValueOf),
        _ => None,
    }
}

/// 将 RegExp.prototype 方法名映射到 Builtin 变体。
/// RegExp 值不是对象属性表中的普通方法，必须直接分派到宿主实现，
/// 否则会走通用 call_indirect 路径并因调用约定不匹配而 trap。
pub(crate) fn builtin_from_regexp_proto_method(name: &str) -> Option<Builtin> {
    use Builtin::*;
    match name {
        "test" => Some(RegExpTest),
        "exec" => Some(RegExpExec),
        _ => None,
    }
}

pub(crate) fn builtin_from_promise_proto_method(name: &str) -> Option<Builtin> {
    use Builtin::*;
    match name {
        "then" => Some(PromiseThen),
        "catch" => Some(PromiseCatch),
        "finally" => Some(PromiseFinally),
        _ => None,
    }
}

pub(crate) fn builtin_from_number_proto_method(name: &str) -> Option<Builtin> {
    use Builtin::*;
    match name {
        "toFixed" => Some(NumberProtoToFixed),
        "toExponential" => Some(NumberProtoToExponential),
        "toPrecision" => Some(NumberProtoToPrecision),
        _ => None,
    }
}

pub(crate) fn builtin_from_boolean_proto_method(name: &str) -> Option<Builtin> {
    // Boolean.prototype methods (toString, valueOf) are dispatched at runtime
    // via property lookup on the Boolean prototype object, not via CallBuiltin.
    let _ = name;
    None
}

/// 将 TypedArray.prototype 方法名映射到 Builtin 变体，用于语义层优化。
/// 当 `ta.forEach(cb)` 被识别时，跳过运行时属性解析，直接发出 CallBuiltin。
pub(crate) fn builtin_from_typedarray_proto_method(name: &str) -> Option<Builtin> {
    use Builtin::*;
    match name {
        "set" => Some(TypedArrayProtoSet),
        "subarray" => Some(TypedArrayProtoSubarray),
        "slice" => Some(TypedArrayProtoSlice),
        "fill" => Some(TypedArrayProtoFill),
        "reverse" => Some(TypedArrayProtoReverse),
        "indexOf" => Some(TypedArrayProtoIndexOf),
        "lastIndexOf" => Some(TypedArrayProtoLastIndexOf),
        "includes" => Some(TypedArrayProtoIncludes),
        "join" => Some(TypedArrayProtoJoin),
        "toString" => Some(TypedArrayProtoToString),
        "copyWithin" => Some(TypedArrayProtoCopyWithin),
        "at" => Some(TypedArrayProtoAt),
        "forEach" => Some(TypedArrayProtoForEach),
        "map" => Some(TypedArrayProtoMap),
        "filter" => Some(TypedArrayProtoFilter),
        "reduce" => Some(TypedArrayProtoReduce),
        "reduceRight" => Some(TypedArrayProtoReduceRight),
        "find" => Some(TypedArrayProtoFind),
        "findIndex" => Some(TypedArrayProtoFindIndex),
        "some" => Some(TypedArrayProtoSome),
        "every" => Some(TypedArrayProtoEvery),
        "sort" => Some(TypedArrayProtoSort),
        "entries" => Some(TypedArrayProtoEntries),
        "keys" => Some(TypedArrayProtoKeys),
        "values" => Some(TypedArrayProtoValues),
        _ => None,
    }
}

pub(crate) fn builtin_from_error_proto_method(name: &str) -> Option<Builtin> {
    // Error.prototype methods (toString) are dispatched at runtime
    // via property lookup on the Error prototype object, not via CallBuiltin.
    let _ = name;
    None
}

pub(crate) fn builtin_call_signature(builtin: Builtin) -> (&'static str, usize) {
    match builtin {
        Builtin::ConsoleLog => ("console.log", 1),
        Builtin::ConsoleError => ("console.error", 1),
        Builtin::ConsoleWarn => ("console.warn", 1),
        Builtin::ConsoleInfo => ("console.info", 1),
        Builtin::ConsoleDebug => ("console.debug", 1),
        Builtin::ConsoleTrace => ("console.trace", 1),
        Builtin::DefineProperty => ("Object.defineProperty", 3),
        Builtin::GetOwnPropDesc => ("Object.getOwnPropertyDescriptor", 2),
        Builtin::SetTimeout => ("setTimeout", 2),
        Builtin::ClearTimeout => ("clearTimeout", 1),
        Builtin::SetInterval => ("setInterval", 2),
        Builtin::ClearInterval => ("clearInterval", 1),
        Builtin::Fetch => ("fetch", 1),
        Builtin::Eval => ("eval", 2),
        Builtin::EvalIndirect => ("eval.indirect", 1),
        Builtin::EvalResult => ("eval.result", 1),
        Builtin::IsException => ("is_exception", 1),
        Builtin::NewTarget => ("new_target", 1),
        Builtin::JsonStringify => ("JSON.stringify", 1),
        Builtin::JsonParse => ("JSON.parse", 1),
        Builtin::AbstractEq => ("abstract-eq", 2),
        Builtin::AbstractCompare => ("abstract-compare", 2),
        Builtin::HasOwnProperty => ("Object.prototype.hasOwnProperty", 2),
        Builtin::ObjectProtoToString => ("Object.prototype.toString", 1),
        Builtin::ObjectProtoValueOf => ("Object.prototype.valueOf", 1),
        Builtin::ObjectKeys => ("Object.keys", 1),
        Builtin::ObjectValues => ("Object.values", 1),
        Builtin::ObjectEntries => ("Object.entries", 1),
        Builtin::ObjectAssign => ("Object.assign", 1),
        Builtin::ObjectCreate => ("Object.create", 1),
        Builtin::ObjectGetPrototypeOf => ("Object.getPrototypeOf", 1),
        Builtin::ObjectSetPrototypeOf => ("Object.setPrototypeOf", 2),
        Builtin::ObjectGetOwnPropertyNames => ("Object.getOwnPropertyNames", 1),
        Builtin::ObjectIs => ("Object.is", 2),
        // ── BigInt builtins ──
        Builtin::BigIntFromLiteral => ("BigInt.fromLiteral", 1),
        Builtin::BigIntAdd => ("BigInt.add", 2),
        Builtin::BigIntSub => ("BigInt.sub", 2),
        Builtin::BigIntMul => ("BigInt.mul", 2),
        Builtin::BigIntDiv => ("BigInt.div", 2),
        Builtin::BigIntMod => ("BigInt.mod", 2),
        Builtin::BigIntPow => ("BigInt.pow", 2),
        Builtin::BigIntNeg => ("BigInt.neg", 1),
        Builtin::BigIntEq => ("BigInt.eq", 2),
        Builtin::BigIntCmp => ("BigInt.cmp", 2),
        // ── Symbol builtins ──
        Builtin::SymbolCreate => ("Symbol", 0),
        Builtin::SymbolFor => ("Symbol.for", 1),
        Builtin::SymbolKeyFor => ("Symbol.keyFor", 1),
        Builtin::SymbolWellKnown => ("Symbol.wellKnown", 1),
        // ── RegExp builtins ──
        Builtin::RegExpCreate => ("RegExp.create", 2),
        Builtin::RegExpTest => ("RegExp.test", 2),
        Builtin::RegExpExec => ("RegExp.exec", 2),
        // ── String prototype builtins ──
        Builtin::StringMatch => ("String.prototype.match", 2),
        Builtin::StringReplace => ("String.prototype.replace", 3),
        Builtin::StringSearch => ("String.prototype.search", 2),
        Builtin::StringSplit => ("String.prototype.split", 3),
        Builtin::StringAt => ("String.prototype.at", 2),
        Builtin::StringCharAt => ("String.prototype.charAt", 2),
        Builtin::StringCharCodeAt => ("String.prototype.charCodeAt", 2),
        Builtin::StringCodePointAt => ("String.prototype.codePointAt", 2),
        Builtin::StringConcatVa => ("String.prototype.concat", 1),
        Builtin::StringEndsWith => ("String.prototype.endsWith", 3),
        Builtin::StringIncludes => ("String.prototype.includes", 3),
        Builtin::StringIndexOf => ("String.prototype.indexOf", 3),
        Builtin::StringLastIndexOf => ("String.prototype.lastIndexOf", 3),
        Builtin::StringMatchAll => ("String.prototype.matchAll", 2),
        Builtin::StringPadEnd => ("String.prototype.padEnd", 3),
        Builtin::StringPadStart => ("String.prototype.padStart", 3),
        Builtin::StringRepeat => ("String.prototype.repeat", 2),
        Builtin::StringReplaceAll => ("String.prototype.replaceAll", 3),
        Builtin::StringSlice => ("String.prototype.slice", 3),
        Builtin::StringStartsWith => ("String.prototype.startsWith", 3),
        Builtin::StringSubstring => ("String.prototype.substring", 3),
        Builtin::StringToLowerCase => ("String.prototype.toLowerCase", 1),
        Builtin::StringToUpperCase => ("String.prototype.toUpperCase", 1),
        Builtin::StringTrim => ("String.prototype.trim", 1),
        Builtin::StringTrimEnd => ("String.prototype.trimEnd", 1),
        Builtin::StringTrimStart => ("String.prototype.trimStart", 1),
        Builtin::StringToString => ("String.prototype.toString", 1),
        Builtin::StringValueOf => ("String.prototype.valueOf", 1),
        Builtin::StringIterator => ("String.prototype[@@iterator]", 1),
        Builtin::StringFromCharCode => ("String.fromCharCode", 1),
        Builtin::StringFromCodePoint => ("String.fromCodePoint", 1),
        // ── Number builtins ──
        Builtin::NumberConstructor => ("Number", 1),
        Builtin::NumberIsNaN => ("Number.isNaN", 1),
        Builtin::NumberIsFinite => ("Number.isFinite", 1),
        Builtin::NumberIsInteger => ("Number.isInteger", 1),
        Builtin::NumberIsSafeInteger => ("Number.isSafeInteger", 1),
        Builtin::NumberParseInt => ("Number.parseInt", 1),
        Builtin::NumberParseFloat => ("Number.parseFloat", 1),
        Builtin::NumberProtoToString => ("Number.prototype.toString", 1),
        Builtin::NumberProtoValueOf => ("Number.prototype.valueOf", 1),
        Builtin::NumberProtoToFixed => ("Number.prototype.toFixed", 1),
        Builtin::NumberProtoToExponential => ("Number.prototype.toExponential", 1),
        Builtin::NumberProtoToPrecision => ("Number.prototype.toPrecision", 1),
        // ── Boolean builtins ──
        Builtin::BooleanConstructor => ("Boolean", 1),
        Builtin::BooleanProtoToString => ("Boolean.prototype.toString", 1),
        Builtin::BooleanProtoValueOf => ("Boolean.prototype.valueOf", 1),
        // ── Error builtins ──
        Builtin::ErrorConstructor => ("Error", 1),
        Builtin::TypeErrorConstructor => ("TypeError", 1),
        Builtin::RangeErrorConstructor => ("RangeError", 1),
        Builtin::SyntaxErrorConstructor => ("SyntaxError", 1),
        Builtin::ReferenceErrorConstructor => ("ReferenceError", 1),
        Builtin::URIErrorConstructor => ("URIError", 1),
        Builtin::EvalErrorConstructor => ("EvalError", 1),
        Builtin::ErrorProtoToString => ("Error.prototype.toString", 1),
        // ── Map builtins ──
        Builtin::MapConstructor => ("Map", 0),
        // ── Set builtins ──
        Builtin::SetConstructor => ("Set", 0),
        // ── WeakMap builtins ──
        Builtin::WeakMapConstructor => ("WeakMap", 0),
        Builtin::WeakMapProtoSet => ("WeakMap.prototype.set", 3),
        Builtin::WeakMapProtoGet => ("WeakMap.prototype.get", 2),
        Builtin::WeakMapProtoHas => ("WeakMap.prototype.has", 2),
        Builtin::WeakMapProtoDelete => ("WeakMap.prototype.delete", 2),
        // ── WeakSet builtins ──
        Builtin::WeakSetConstructor => ("WeakSet", 0),
        Builtin::WeakSetProtoAdd => ("WeakSet.prototype.add", 2),
        Builtin::WeakSetProtoHas => ("WeakSet.prototype.has", 2),
        Builtin::WeakSetProtoDelete => ("WeakSet.prototype.delete", 2),
        // ── WeakRef builtins ──
        Builtin::WeakRefConstructor => ("WeakRef", 1),
        Builtin::WeakRefProtoDeref => ("WeakRef.prototype.deref", 1),
        // ── FinalizationRegistry builtins ──
        Builtin::FinalizationRegistryConstructor => ("FinalizationRegistry", 1),
        Builtin::FinalizationRegistryProtoRegister => {
            ("FinalizationRegistry.prototype.register", 4)
        }
        Builtin::FinalizationRegistryProtoUnregister => {
            ("FinalizationRegistry.prototype.unregister", 2)
        }
        // ── ArrayBuffer builtins ──
        Builtin::ArrayBufferConstructor => ("ArrayBuffer", 1),
        Builtin::ArrayBufferProtoByteLength => ("ArrayBuffer.prototype.byteLength", 1),
        Builtin::ArrayBufferProtoSlice => ("ArrayBuffer.prototype.slice", 3),
        // ── DataView builtins ──
        Builtin::DataViewConstructor => ("DataView", 3),
        Builtin::DataViewProtoGetFloat64 => ("DataView.prototype.getFloat64", 2),
        Builtin::DataViewProtoGetFloat32 => ("DataView.prototype.getFloat32", 2),
        Builtin::DataViewProtoGetInt32 => ("DataView.prototype.getInt32", 2),
        Builtin::DataViewProtoGetUint32 => ("DataView.prototype.getUint32", 2),
        Builtin::DataViewProtoGetInt16 => ("DataView.prototype.getInt16", 2),
        Builtin::DataViewProtoGetUint16 => ("DataView.prototype.getUint16", 2),
        Builtin::DataViewProtoGetInt8 => ("DataView.prototype.getInt8", 2),
        Builtin::DataViewProtoGetUint8 => ("DataView.prototype.getUint8", 2),
        Builtin::DataViewProtoSetFloat64 => ("DataView.prototype.setFloat64", 3),
        Builtin::DataViewProtoSetFloat32 => ("DataView.prototype.setFloat32", 3),
        Builtin::DataViewProtoSetInt32 => ("DataView.prototype.setInt32", 3),
        Builtin::DataViewProtoSetUint32 => ("DataView.prototype.setUint32", 3),
        Builtin::DataViewProtoSetInt16 => ("DataView.prototype.setInt16", 3),
        Builtin::DataViewProtoSetUint16 => ("DataView.prototype.setUint16", 3),
        Builtin::DataViewProtoSetInt8 => ("DataView.prototype.setInt8", 3),
        Builtin::DataViewProtoSetUint8 => ("DataView.prototype.setUint8", 3),
        // ── TypedArray constructors ──
        Builtin::Int8ArrayConstructor => ("Int8Array", 3),
        Builtin::Uint8ArrayConstructor => ("Uint8Array", 3),
        Builtin::Uint8ClampedArrayConstructor => ("Uint8ClampedArray", 3),
        Builtin::Int16ArrayConstructor => ("Int16Array", 3),
        Builtin::Uint16ArrayConstructor => ("Uint16Array", 3),
        Builtin::Int32ArrayConstructor => ("Int32Array", 3),
        Builtin::Uint32ArrayConstructor => ("Uint32Array", 3),
        Builtin::Float32ArrayConstructor => ("Float32Array", 3),
        Builtin::Float64ArrayConstructor => ("Float64Array", 3),
        // ── TypedArray constructors (new) ──
        Builtin::BigInt64ArrayConstructor => ("BigInt64Array", 3),
        Builtin::BigUint64ArrayConstructor => ("BigUint64Array", 3),
        // ── TypedArray prototype methods ──
        Builtin::TypedArrayProtoLength => ("TypedArray.prototype.length", 1),
        Builtin::TypedArrayProtoByteLength => ("TypedArray.prototype.byteLength", 1),
        Builtin::TypedArrayProtoByteOffset => ("TypedArray.prototype.byteOffset", 1),
        Builtin::TypedArrayProtoSet => ("TypedArray.prototype.set", 3),
        Builtin::TypedArrayProtoSlice => ("TypedArray.prototype.slice", 3),
        Builtin::TypedArrayProtoSubarray => ("TypedArray.prototype.subarray", 3),
        // ── TypedArray new prototype methods ──
        Builtin::TypedArrayProtoFill => ("TypedArray.prototype.fill", 3),
        Builtin::TypedArrayProtoReverse => ("TypedArray.prototype.reverse", 1),
        Builtin::TypedArrayProtoIndexOf => ("TypedArray.prototype.indexOf", 3),
        Builtin::TypedArrayProtoLastIndexOf => ("TypedArray.prototype.lastIndexOf", 3),
        Builtin::TypedArrayProtoIncludes => ("TypedArray.prototype.includes", 3),
        Builtin::TypedArrayProtoJoin => ("TypedArray.prototype.join", 2),
        Builtin::TypedArrayProtoToString => ("TypedArray.prototype.toString", 1),
        Builtin::TypedArrayProtoCopyWithin => ("TypedArray.prototype.copyWithin", 4),
        Builtin::TypedArrayProtoAt => ("TypedArray.prototype.at", 2),
        Builtin::TypedArrayProtoForEach => ("TypedArray.prototype.forEach", 3),
        Builtin::TypedArrayProtoMap => ("TypedArray.prototype.map", 3),
        Builtin::TypedArrayProtoFilter => ("TypedArray.prototype.filter", 3),
        Builtin::TypedArrayProtoReduce => ("TypedArray.prototype.reduce", 3),
        Builtin::TypedArrayProtoReduceRight => ("TypedArray.prototype.reduceRight", 3),
        Builtin::TypedArrayProtoFind => ("TypedArray.prototype.find", 3),
        Builtin::TypedArrayProtoFindIndex => ("TypedArray.prototype.findIndex", 3),
        Builtin::TypedArrayProtoSome => ("TypedArray.prototype.some", 3),
        Builtin::TypedArrayProtoEvery => ("TypedArray.prototype.every", 3),
        Builtin::TypedArrayProtoSort => ("TypedArray.prototype.sort", 2),
        Builtin::TypedArrayProtoEntries => ("TypedArray.prototype.entries", 1),
        Builtin::TypedArrayProtoKeys => ("TypedArray.prototype.keys", 1),
        Builtin::TypedArrayProtoValues => ("TypedArray.prototype.values", 1),
        // ── Date builtins ──
        Builtin::DateConstructor => ("Date", 0),
        Builtin::DateNow => ("Date.now", 0),
        Builtin::DateParse => ("Date.parse", 1),
        Builtin::DateUTC => ("Date.UTC", 2),
        // ── Arguments Exotic Object ──
        Builtin::CreateUnmappedArgumentsObject => ("create_unmapped_arguments_object", 2),
        Builtin::CreateMappedArgumentsObject => ("create_mapped_arguments_object", 3),
        _ => ("builtin", 0),
    }
}
