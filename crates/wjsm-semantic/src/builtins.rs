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
    "console",
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
    "global",
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
    "process",
    "Buffer",
    "TextEncoder",
    "TextDecoder",
    "structuredClone",
    "queueMicrotask",
    "atob",
    "btoa",
    "performance",
    "Headers",
    "Request",
    "Response",
    "ReadableStream",
    "WritableStream",
    "TransformStream",
    "CountQueuingStrategy",
    "ByteLengthQueuingStrategy",
    "AbortController",
    "Intl",
    "Iterator",
    "AsyncIterator",
    "gc",
    "setImmediate",
    "clearImmediate",
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
        "structuredClone" => Some(Builtin::StructuredClone),
        "WritableStream" => Some(Builtin::WritableStreamConstructor),
        "TransformStream" => Some(Builtin::TransformStreamConstructor),
        "CountQueuingStrategy" => Some(Builtin::CountQueuingStrategyConstructor),
        "ByteLengthQueuingStrategy" => Some(Builtin::ByteLengthQueuingStrategyConstructor),
        "Symbol" => Some(Builtin::SymbolCreate),
        // queueMicrotask is installed as an ordinary global so calls share the Node/Web runtime path.
        "parseInt" => Some(Builtin::NumberParseInt),
        "parseFloat" => Some(Builtin::NumberParseFloat),
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
        "performance" => match property {
            "now" => Some(Builtin::PerformanceNow),
            _ => None,
        },
        "Array" => match property {
            "isArray" => Some(Builtin::ArrayIsArray),
            "from" => Some(Builtin::ArrayFrom),
            "of" => Some(Builtin::ArrayOf),
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
            "getOwnPropertySymbols" => Some(Builtin::ObjectGetOwnPropertySymbols),
            "is" => Some(Builtin::ObjectIs),
            "groupBy" => Some(Builtin::ObjectGroupBy),
            "hasOwn" => Some(Builtin::ObjectHasOwn),
            "freeze" => Some(Builtin::ObjectFreeze),
            "seal" => Some(Builtin::ObjectSeal),
            "isFrozen" => Some(Builtin::ObjectIsFrozen),
            "isSealed" => Some(Builtin::ObjectIsSealed),
            "isExtensible" => Some(Builtin::ObjectIsExtensible),
            "fromEntries" => Some(Builtin::ObjectFromEntries),
            "getOwnPropertyDescriptors" => Some(Builtin::ObjectGetOwnPropertyDescriptors),
            "defineProperties" => Some(Builtin::ObjectDefineProperties),
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
            "pause" => Some(Builtin::AtomicsPause),
            "wait" => Some(Builtin::AtomicsWait),
            "notify" => Some(Builtin::AtomicsNotify),
            "waitAsync" => Some(Builtin::AtomicsWaitAsync),
            _ => None,
        },
        _ => None,
    }
}

/// 将 Array.prototype 方法名映射到 Builtin 变体，用于语义层优化。
/// 仅对静态已知 Array receiver 使用，避免劫持 Map/Set 等同名方法。
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
        "findLast" => Some(ArrayFindLast),
        "findLastIndex" => Some(ArrayFindLastIndex),
        "lastIndexOf" => Some(ArrayLastIndexOf),
        "toSorted" => Some(ArrayToSorted),
        "toReversed" => Some(ArrayToReversed),
        "toSpliced" => Some(ArrayToSplicedVa),
        "with" => Some(ArrayWith),
        _ => None,
    }
}

/// 将 Map.prototype 方法名映射到 Builtin 变体，用于语义层优化。
/// 仅在 receiver 是静态已知 Map 绑定时启用（monkey-patch 语义与 Array 优化一致：
/// 直接内建调用，不读取被改写的方法属性）。
pub(crate) fn builtin_from_map_proto_method(name: &str) -> Option<Builtin> {
    use Builtin::*;
    match name {
        "set" => Some(MapProtoSet),
        "get" => Some(MapProtoGet),
        "has" => Some(MapSetHas),
        "delete" => Some(MapSetDelete),
        "clear" => Some(MapSetClear),
        "forEach" => Some(MapSetForEach),
        "keys" => Some(MapSetKeys),
        "values" => Some(MapSetValues),
        "entries" => Some(MapSetEntries),
        _ => None,
    }
}

/// 将 Set.prototype 方法名映射到 Builtin 变体（has/delete 用专用直连内建，
/// 免去共享 MapSet.has/delete 先查 __map_handle__ 的额外属性读取）。
pub(crate) fn builtin_from_set_proto_method(name: &str) -> Option<Builtin> {
    use Builtin::*;
    match name {
        "add" => Some(SetProtoAdd),
        "has" => Some(SetProtoHas),
        "delete" => Some(SetProtoDelete),
        "clear" => Some(MapSetClear),
        "forEach" => Some(MapSetForEach),
        "keys" => Some(MapSetKeys),
        "values" => Some(MapSetValues),
        "entries" => Some(MapSetEntries),
        _ => None,
    }
}

/// 将 Object.prototype 方法名映射到 Builtin 变体，用于语义层优化。
///
/// 只拦截无需读取同名函数值的 `hasOwnProperty`。`toString` / `valueOf` 必须走运行时属性
/// 查找，否则对象自有方法会被错误地静态改写为 Object.prototype 方法。
pub(crate) fn builtin_from_object_proto_method(name: &str) -> Option<Builtin> {
    match name {
        "hasOwnProperty" => Some(Builtin::HasOwnProperty),
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
        "trim" => Some(StringTrim),
        "trimEnd" => Some(StringTrimEnd),
        "trimStart" => Some(StringTrimStart),

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
        "toString" => Some(NumberProtoToString),
        "valueOf" => Some(NumberProtoValueOf),
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

/// 将 SharedArrayBuffer.prototype 方法名映射到 Builtin 变体。
pub(crate) fn builtin_from_sharedarraybuffer_proto_method(name: &str) -> Option<Builtin> {
    use Builtin::*;
    match name {
        "byteLength" => Some(SharedArrayBufferProtoByteLength),
        "grow" => Some(SharedArrayBufferProtoGrow),
        "growable" => Some(SharedArrayBufferProtoGrowable),
        "maxByteLength" => Some(SharedArrayBufferProtoMaxByteLength),
        "slice" => Some(SharedArrayBufferProtoSlice),
        _ => None,
    }
}

/// 将 DataView.prototype get/set 方法名映射到 Builtin 变体。
pub(crate) fn builtin_from_dataview_proto_method(name: &str) -> Option<Builtin> {
    use Builtin::*;
    match name {
        "getFloat64" => Some(DataViewProtoGetFloat64),
        "getFloat32" => Some(DataViewProtoGetFloat32),
        "getInt32" => Some(DataViewProtoGetInt32),
        "getUint32" => Some(DataViewProtoGetUint32),
        "getInt16" => Some(DataViewProtoGetInt16),
        "getUint16" => Some(DataViewProtoGetUint16),
        "getInt8" => Some(DataViewProtoGetInt8),
        "getUint8" => Some(DataViewProtoGetUint8),
        "setFloat64" => Some(DataViewProtoSetFloat64),
        "setFloat32" => Some(DataViewProtoSetFloat32),
        "setInt32" => Some(DataViewProtoSetInt32),
        "setUint32" => Some(DataViewProtoSetUint32),
        "setInt16" => Some(DataViewProtoSetInt16),
        "setUint16" => Some(DataViewProtoSetUint16),
        "setInt8" => Some(DataViewProtoSetInt8),
        "setUint8" => Some(DataViewProtoSetUint8),
        _ => None,
    }
}
