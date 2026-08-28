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
    "URL",
    "URLSearchParams",
    "TextEncoder",
    "TextDecoder",
    "structuredClone",
    "queueMicrotask",
    "atob",
    "btoa",
    "performance",
    "fetch",
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

impl Lowerer {
    /// 全局 intrinsic 名在当前解析点是否被静态绑定遮蔽（ResolveBinding 先于全局解析，
    /// §9.4.2）。任一命中都必须禁用 CallBuiltin 等编译期 intrinsic 快路径，改走通用
    /// 标识符解析（含模块导入 live binding 与 TDZ 诊断）。
    ///
    /// 遮蔽来源覆盖三类：
    /// - 作用域树中的声明（含 TDZ 中的 let/const/class——遮蔽取决于声明存在与否，
    ///   TDZ 报错由通用路径负责，不得回退为 intrinsic 劫持）；
    /// - 模块命名/默认导入别名（`import { setTimeout } from 'node:timers/promises'`
    ///   只登记 import_aliases，不进作用域树）；
    /// - `import * as ns` 命名空间局部（按导入方模块隔离）。
    pub(crate) fn global_intrinsic_shadowed(&self, name: &str) -> bool {
        if self.scopes.resolve_scope_id(name).is_ok() {
            return true;
        }
        self.current_module_id.is_some_and(|module_id| {
            self.import_aliases
                .contains_key(&(module_id, name.to_string()))
                || self
                    .static_namespace_import_objects
                    .contains_key(&(module_id, name.to_string()))
        })
    }

    /// `new C(...)` 形状证明（receiver 类型推断）里的构造器名被静态绑定遮蔽时，
    /// 证明不成立：构造结果是用户构造器的实例，不得按内建 receiver 直连原型方法。
    /// 非 `new <ident>` 形状（数组字面量等）恒不被遮蔽。
    pub(crate) fn ctor_shape_shadowed(&self, expr: &swc_ast::Expr) -> bool {
        if let swc_ast::Expr::New(new_expr) = expr
            && let swc_ast::Expr::Ident(ident) = new_expr.callee.as_ref()
        {
            return self.global_intrinsic_shadowed(&ident.sym);
        }
        false
    }
}

/// 全局名 → 快路径 builtin。数据在 `wjsm_ir::intrinsic_sites`（与宿主守卫 /
/// 慢路径反查共用同一张表）。queueMicrotask 作为普通全局安装以复用
/// Node/Web 运行时路径，故不在表内。
pub(crate) fn builtin_from_global_ident(name: &str) -> Option<Builtin> {
    wjsm_ir::intrinsic_sites::global_ident_builtin(name)
}

/// (容器全局名, 属性名) → 快路径 builtin。数据在 `wjsm_ir::intrinsic_sites`。
pub(crate) fn builtin_from_static_member(object: &str, property: &str) -> Option<Builtin> {
    wjsm_ir::intrinsic_sites::static_member_builtin(object, property)
}

/// 将 Array.prototype 方法名映射到 Builtin 变体，用于语义层优化。
/// 仅对静态已知 Array receiver 使用，避免劫持 Map/Set 等同名方法。
pub(crate) fn builtin_from_array_proto_method(name: &str) -> Option<Builtin> {
    wjsm_ir::intrinsic_sites::array_proto_builtin(name)
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
    wjsm_ir::intrinsic_sites::string_proto_builtin(name)
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
        "getBigInt64" => Some(DataViewProtoGetBigInt64),
        "getBigUint64" => Some(DataViewProtoGetBigUint64),
        "setFloat64" => Some(DataViewProtoSetFloat64),
        "setFloat32" => Some(DataViewProtoSetFloat32),
        "setInt32" => Some(DataViewProtoSetInt32),
        "setUint32" => Some(DataViewProtoSetUint32),
        "setInt16" => Some(DataViewProtoSetInt16),
        "setUint16" => Some(DataViewProtoSetUint16),
        "setInt8" => Some(DataViewProtoSetInt8),
        "setUint8" => Some(DataViewProtoSetUint8),
        "setBigInt64" => Some(DataViewProtoSetBigInt64),
        "setBigUint64" => Some(DataViewProtoSetBigUint64),
        _ => None,
    }
}
