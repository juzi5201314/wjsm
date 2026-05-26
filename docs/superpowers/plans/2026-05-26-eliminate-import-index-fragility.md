# 消除宿主导入索引脆弱性 — 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 消除运行时的 `Vec<Extern>` 位置依赖、后端 300 行手写索引映射、`register_all_imports` 的 `drain`/`remove` 交错逻辑。以 `HOST_IMPORT_NAMES` 为单一真相来源，使用 wasmtime Linker 按名字链接。

**Architecture:** 运行时从 `Instance::new(&store, &module, &imports)`（按位置）切换为 `Linker`（按 `("env", name)`）。后端从手写 `builtin_func_indices.insert(Builtin::X, N)` 切换为从 `HOST_IMPORT_NAMES` 自动生成。每个 `host_imports/*.rs` 从返回 `Vec<Extern>` 的裸块/函数改为接收 `&mut Linker` 并按名字注册。

**Tech Stack:** Rust 2024, wasmtime 43.0.0

---

### Task 1: 给 Builtin 添加 `import_name()` 和 `ALL_BUILTINS`

**Files:**
- Modify: `crates/wjsm-ir/src/builtin.rs`

- [ ] **Step 1: 添加 `import_name()` 方法**

在 `impl Builtin` 块中添加（`Display` impl 后面）。每个变体返回 HOST_IMPORT_NAMES 中对应的 snake_case 名字：

```rust
impl Builtin {
    /// 返回 WASM import section / HOST_IMPORT_NAMES 中对应的名字。
    pub fn import_name(self) -> &'static str {
        match self {
            Self::ConsoleLog => "console_log",
            Self::ConsoleError => "console_error",
            Self::ConsoleWarn => "console_warn",
            Self::ConsoleInfo => "console_info",
            Self::ConsoleDebug => "console_debug",
            Self::ConsoleTrace => "console_trace",
            Self::Debugger => "debugger",
            Self::Throw => "throw",
            Self::AbortShadowStackOverflow => "abort_shadow_stack_overflow",
            Self::F64Mod => "f64_mod",
            Self::F64Exp => "f64_pow",
            Self::IteratorFrom => "iterator_from",
            Self::IteratorNext => "iterator_next",
            Self::IteratorClose => "iterator_close",
            Self::AsyncIteratorFrom => "async_iterator_from",
            Self::IteratorValue => "iterator_value",
            Self::IteratorDone => "iterator_done",
            Self::EnumeratorFrom => "enumerator_from",
            Self::EnumeratorNext => "enumerator_next",
            Self::EnumeratorKey => "enumerator_key",
            Self::EnumeratorDone => "enumerator_done",
            Self::TypeOf => "typeof",
            Self::In => "op_in",
            Self::InstanceOf => "op_instanceof",
            Self::AbstractEq => "abstract_eq",
            Self::AbstractCompare => "abstract_compare",
            Self::DefineProperty => "define_property",
            Self::GetOwnPropDesc => "get_own_prop_desc",
            Self::SetTimeout => "set_timeout",
            Self::ClearTimeout => "clear_timeout",
            Self::SetInterval => "set_interval",
            Self::ClearInterval => "clear_interval",
            Self::Fetch => "fetch",
            Self::Eval => "eval_direct",
            Self::EvalIndirect => "eval_indirect",
            Self::JsonStringify => "json_stringify",
            Self::JsonParse => "json_parse",
            Self::CreateClosure => "closure_create",
            Self::ArrayPush => "arr_push",
            Self::ArrayPop => "arr_pop",
            Self::ArrayIncludes => "arr_includes",
            Self::ArrayIndexOf => "arr_index_of",
            Self::ArrayJoin => "arr_join",
            Self::ArrayConcat => "arr_concat",
            Self::ArraySlice => "arr_slice",
            Self::ArrayFill => "arr_fill",
            Self::ArrayReverse => "arr_reverse",
            Self::ArrayFlat => "arr_flat",
            Self::ArrayInitLength => "arr_init_length",
            Self::ArrayGetLength => "arr_get_length",
            Self::ArrayShift => "arr_proto_shift",
            Self::ArrayUnshiftVa => "arr_proto_unshift",
            Self::ArraySort => "arr_proto_sort",
            Self::ArrayAt => "arr_proto_at",
            Self::ArrayCopyWithin => "arr_proto_copy_within",
            Self::ArrayForEach => "arr_proto_for_each",
            Self::ArrayMap => "arr_proto_map",
            Self::ArrayFilter => "arr_proto_filter",
            Self::ArrayReduce => "arr_proto_reduce",
            Self::ArrayReduceRight => "arr_proto_reduce_right",
            Self::ArrayFind => "arr_proto_find",
            Self::ArrayFindIndex => "arr_proto_find_index",
            Self::ArraySome => "arr_proto_some",
            Self::ArrayEvery => "arr_proto_every",
            Self::ArrayFlatMap => "arr_proto_flat_map",
            Self::ArrayIsArray => "arr_proto_is_array",
            Self::ArrayFrom => "array.from",
            Self::ArraySpliceVa => "arr_proto_splice",
            Self::ArrayConcatVa => "string_concat_va",
            Self::FuncCall => "func_call",
            Self::FuncApply => "func_apply",
            Self::FuncBind => "func_bind",
            Self::ObjectRest => "object_rest",
            Self::HasOwnProperty => "has_own_property",
            Self::PrivateGet => "private_get",
            Self::PrivateSet => "private_set",
            Self::PrivateHas => "private_has",
            Self::ObjectProtoToString => "obj_proto_to_string",
            Self::ObjectProtoValueOf => "obj_proto_value_of",
            Self::ObjectKeys => "obj_keys",
            Self::ObjectValues => "obj_values",
            Self::ObjectEntries => "obj_entries",
            Self::ObjectAssign => "obj_assign",
            Self::ObjectCreate => "obj_create",
            Self::ObjectGetPrototypeOf => "obj_get_proto_of",
            Self::ObjectSetPrototypeOf => "obj_set_proto_of",
            Self::ObjectGetOwnPropertyNames => "obj_get_own_prop_names",
            Self::ObjectIs => "obj_is",
            Self::ObjectGroupBy => "object.group_by",
            Self::MapGroupBy => "map.group_by",
            Self::BigIntFromLiteral => "bigint_from_literal",
            Self::BigIntAdd => "bigint_add",
            Self::BigIntSub => "bigint_sub",
            Self::BigIntMul => "bigint_mul",
            Self::BigIntDiv => "bigint_div",
            Self::BigIntMod => "bigint_mod",
            Self::BigIntPow => "bigint_pow",
            Self::BigIntNeg => "bigint_neg",
            Self::BigIntEq => "bigint_eq",
            Self::BigIntCmp => "bigint_cmp",
            Self::SymbolCreate => "symbol_create",
            Self::SymbolFor => "symbol_for",
            Self::SymbolKeyFor => "symbol_key_for",
            Self::SymbolWellKnown => "symbol_well_known",
            Self::RegExpCreate => "regex_create",
            Self::RegExpTest => "regex_test",
            Self::RegExpExec => "regex_exec",
            Self::StringMatch => "string_match",
            Self::StringReplace => "string_replace",
            Self::StringSearch => "string_search",
            Self::StringSplit => "string_split",
            Self::PromiseCreate => "promise_create",
            Self::PromiseInstanceResolve => "promise_instance_resolve",
            Self::PromiseInstanceReject => "promise_instance_reject",
            Self::PromiseCreateResolveFunction => "promise_create_resolve_function",
            Self::PromiseCreateRejectFunction => "promise_create_reject_function",
            Self::PromiseThen => "promise_then",
            Self::PromiseCatch => "promise_catch",
            Self::PromiseFinally => "promise_finally",
            Self::PromiseAll => "promise_all",
            Self::PromiseRace => "promise_race",
            Self::PromiseAllSettled => "promise_all_settled",
            Self::PromiseAny => "promise_any",
            Self::PromiseResolveStatic => "promise_resolve_static",
            Self::PromiseRejectStatic => "promise_reject_static",
            Self::IsPromise => "is_promise",
            Self::QueueMicrotask => "queue_microtask",
            Self::DrainMicrotasks => "drain_microtasks",
            Self::AsyncFunctionStart => "async_function_start",
            Self::AsyncFunctionResume => "async_function_resume",
            Self::AsyncFunctionSuspend => "async_function_suspend",
            Self::ContinuationCreate => "continuation_create",
            Self::ContinuationSaveVar => "continuation_save_var",
            Self::ContinuationLoadVar => "continuation_load_var",
            Self::AsyncGeneratorStart => "async_generator_start",
            Self::AsyncGeneratorNext => "async_generator_next",
            Self::AsyncGeneratorReturn => "async_generator_return",
            Self::AsyncGeneratorThrow => "async_generator_throw",
            Self::PromiseWithResolvers => "promise_with_resolvers",
            Self::IsCallable => "is_callable",
            Self::DynamicImport => "dynamic_import",
            Self::RegisterModuleNamespace => "register_module_namespace",
            Self::JsxCreateElement => "jsx_create_element",
            Self::ProxyCreate => "proxy_create",
            Self::ProxyRevocable => "proxy_revocable",
            Self::ReflectGet => "reflect_get",
            Self::ReflectSet => "reflect_set",
            Self::ReflectHas => "reflect_has",
            Self::ReflectDeleteProperty => "reflect_delete_property",
            Self::ReflectApply => "reflect_apply",
            Self::ReflectConstruct => "reflect_construct",
            Self::ReflectGetPrototypeOf => "reflect_get_prototype_of",
            Self::ReflectSetPrototypeOf => "reflect_set_prototype_of",
            Self::ReflectIsExtensible => "reflect_is_extensible",
            Self::ReflectPreventExtensions => "reflect_prevent_extensions",
            Self::ReflectGetOwnPropertyDescriptor => "reflect_get_own_property_descriptor",
            Self::ReflectDefineProperty => "reflect_define_property",
            Self::ReflectOwnKeys => "reflect_own_keys",
            Self::StringAt => "string_at",
            Self::StringCharAt => "string_char_at",
            Self::StringCharCodeAt => "string_char_code_at",
            Self::StringCodePointAt => "string_code_point_at",
            Self::StringConcatVa => "string_concat_proto",
            Self::StringEndsWith => "string_ends_with",
            Self::StringIncludes => "string_includes",
            Self::StringIndexOf => "string_index_of",
            Self::StringLastIndexOf => "string_last_index_of",
            Self::StringMatchAll => "string_match_all",
            Self::StringPadEnd => "string_pad_end",
            Self::StringPadStart => "string_pad_start",
            Self::StringRepeat => "string_repeat",
            Self::StringReplaceAll => "string_replace_all",
            Self::StringSlice => "string_slice",
            Self::StringStartsWith => "string_starts_with",
            Self::StringSubstring => "string_substring",
            Self::StringToLowerCase => "string_to_lower_case",
            Self::StringToUpperCase => "string_to_upper_case",
            Self::StringTrim => "string_trim",
            Self::StringTrimEnd => "string_trim_end",
            Self::StringTrimStart => "string_trim_start",
            Self::StringToString => "string_to_string",
            Self::StringValueOf => "string_value_of",
            Self::StringIterator => "string_iterator",
            Self::StringFromCharCode => "string_from_char_code",
            Self::StringFromCodePoint => "string_from_code_point",
            Self::MathAbs => "math_abs",
            Self::MathAcos => "math_acos",
            Self::MathAcosh => "math_acosh",
            Self::MathAsin => "math_asin",
            Self::MathAsinh => "math_asinh",
            Self::MathAtan => "math_atan",
            Self::MathAtanh => "math_atanh",
            Self::MathAtan2 => "math_atan2",
            Self::MathCbrt => "math_cbrt",
            Self::MathCeil => "math_ceil",
            Self::MathClz32 => "math_clz32",
            Self::MathCos => "math_cos",
            Self::MathCosh => "math_cosh",
            Self::MathExp => "math_exp",
            Self::MathExpm1 => "math_expm1",
            Self::MathFloor => "math_floor",
            Self::MathFround => "math_fround",
            Self::MathHypot => "math_hypot",
            Self::MathImul => "math_imul",
            Self::MathLog => "math_log",
            Self::MathLog1p => "math_log1p",
            Self::MathLog10 => "math_log10",
            Self::MathLog2 => "math_log2",
            Self::MathMax => "math_max",
            Self::MathMin => "math_min",
            Self::MathPow => "math_pow",
            Self::MathRandom => "math_random",
            Self::MathRound => "math_round",
            Self::MathSign => "math_sign",
            Self::MathSin => "math_sin",
            Self::MathSinh => "math_sinh",
            Self::MathSqrt => "math_sqrt",
            Self::MathTan => "math_tan",
            Self::MathTanh => "math_tanh",
            Self::MathTrunc => "math_trunc",
            Self::NumberConstructor => "number_constructor",
            Self::NumberIsNaN => "number_is_nan",
            Self::NumberIsFinite => "number_is_finite",
            Self::NumberIsInteger => "number_is_integer",
            Self::NumberIsSafeInteger => "number_is_safe_integer",
            Self::NumberParseInt => "number_parse_int",
            Self::NumberParseFloat => "number_parse_float",
            Self::NumberProtoToString => "number_proto_to_string",
            Self::NumberProtoValueOf => "number_proto_value_of",
            Self::NumberProtoToFixed => "number_proto_to_fixed",
            Self::NumberProtoToExponential => "number_proto_to_exponential",
            Self::NumberProtoToPrecision => "number_proto_to_precision",
            Self::BooleanConstructor => "boolean_constructor",
            Self::BooleanProtoToString => "boolean_proto_to_string",
            Self::BooleanProtoValueOf => "boolean_proto_value_of",
            Self::ErrorConstructor => "error_constructor",
            Self::TypeErrorConstructor => "type_error_constructor",
            Self::RangeErrorConstructor => "range_error_constructor",
            Self::SyntaxErrorConstructor => "syntax_error_constructor",
            Self::ReferenceErrorConstructor => "reference_error_constructor",
            Self::URIErrorConstructor => "uri_error_constructor",
            Self::EvalErrorConstructor => "eval_error_constructor",
            Self::ErrorProtoToString => "error_proto_to_string",
            Self::MapConstructor => "map_constructor",
            Self::MapProtoSet => "map_proto_set",
            Self::MapProtoGet => "map_proto_get",
            Self::SetConstructor => "set_constructor",
            Self::SetProtoAdd => "set_proto_add",
            Self::MapSetHas => "map_set_has",
            Self::MapSetDelete => "map_set_delete",
            Self::MapSetClear => "map_set_clear",
            Self::MapSetGetSize => "map_set_get_size",
            Self::MapSetForEach => "map_set_for_each",
            Self::MapSetKeys => "map_set_keys",
            Self::MapSetValues => "map_set_values",
            Self::MapSetEntries => "map_set_entries",
            Self::DateConstructor => "date_constructor",
            Self::DateNow => "date_now",
            Self::DateParse => "date_parse",
            Self::DateUTC => "date_utc",
            Self::WeakMapConstructor => "weakmap_constructor",
            Self::WeakMapProtoSet => "weakmap_proto_set",
            Self::WeakMapProtoGet => "weakmap_proto_get",
            Self::WeakMapProtoHas => "weakmap_proto_has",
            Self::WeakMapProtoDelete => "weakmap_proto_delete",
            Self::WeakSetConstructor => "weakset_constructor",
            Self::WeakSetProtoAdd => "weakset_proto_add",
            Self::WeakSetProtoHas => "weakset_proto_has",
            Self::WeakSetProtoDelete => "weakset_proto_delete",
            Self::SharedArrayBufferConstructor => "sharedarraybuffer_constructor",
            Self::SharedArrayBufferProtoByteLength => "sharedarraybuffer_proto_byte_length",
            Self::SharedArrayBufferProtoSlice => "sharedarraybuffer_proto_slice",
            Self::SharedArrayBufferSpecies => "sharedarraybuffer_proto_species",
            Self::AtomicsLoad => "atomics_load",
            Self::AtomicsStore => "atomics_store",
            Self::AtomicsAdd => "atomics_add",
            Self::AtomicsSub => "atomics_sub",
            Self::AtomicsAnd => "atomics_and",
            Self::AtomicsOr => "atomics_or",
            Self::AtomicsXor => "atomics_xor",
            Self::AtomicsExchange => "atomics_exchange",
            Self::AtomicsCompareExchange => "atomics_compare_exchange",
            Self::AtomicsIsLockFree => "atomics_is_lock_free",
            Self::AtomicsWait => "atomics_wait",
            Self::AtomicsNotify => "atomics_notify",
            Self::AtomicsWaitAsync => "atomics_wait_async",
            Self::WeakRefConstructor => "weakref_constructor",
            Self::WeakRefProtoDeref => "weakref_proto_deref",
            Self::FinalizationRegistryConstructor => "finalization_registry_constructor",
            Self::FinalizationRegistryProtoRegister => "finalization_registry_proto_register",
            Self::FinalizationRegistryProtoUnregister => "finalization_registry_proto_unregister",
            Self::ArrayBufferConstructor => "arraybuffer_constructor",
            Self::ArrayBufferProtoByteLength => "arraybuffer_proto_byte_length",
            Self::ArrayBufferProtoSlice => "arraybuffer_proto_slice",
            Self::DataViewConstructor => "dataview_constructor",
            Self::DataViewProtoGetFloat64 => "dataview_proto_get_float64",
            Self::DataViewProtoGetFloat32 => "dataview_proto_get_float32",
            Self::DataViewProtoGetInt32 => "dataview_proto_get_int32",
            Self::DataViewProtoGetUint32 => "dataview_proto_get_uint32",
            Self::DataViewProtoGetInt16 => "dataview_proto_get_int16",
            Self::DataViewProtoGetUint16 => "dataview_proto_get_uint16",
            Self::DataViewProtoGetInt8 => "dataview_proto_get_int8",
            Self::DataViewProtoGetUint8 => "dataview_proto_get_uint8",
            Self::DataViewProtoSetFloat64 => "dataview_proto_set_float64",
            Self::DataViewProtoSetFloat32 => "dataview_proto_set_float32",
            Self::DataViewProtoSetInt32 => "dataview_proto_set_int32",
            Self::DataViewProtoSetUint32 => "dataview_proto_set_uint32",
            Self::DataViewProtoSetInt16 => "dataview_proto_set_int16",
            Self::DataViewProtoSetUint16 => "dataview_proto_set_uint16",
            Self::DataViewProtoSetInt8 => "dataview_proto_set_int8",
            Self::DataViewProtoSetUint8 => "dataview_proto_set_uint8",
            Self::Int8ArrayConstructor => "int8array_constructor",
            Self::Uint8ArrayConstructor => "uint8array_constructor",
            Self::Uint8ClampedArrayConstructor => "uint8clampedarray_constructor",
            Self::Int16ArrayConstructor => "int16array_constructor",
            Self::Uint16ArrayConstructor => "uint16array_constructor",
            Self::Int32ArrayConstructor => "int32array_constructor",
            Self::Uint32ArrayConstructor => "uint32array_constructor",
            Self::Float32ArrayConstructor => "float32array_constructor",
            Self::Float64ArrayConstructor => "float64array_constructor",
            Self::BigInt64ArrayConstructor => "bigint64array_constructor",
            Self::BigUint64ArrayConstructor => "biguint64array_constructor",
            Self::TypedArrayProtoLength => "typedarray_proto_length",
            Self::TypedArrayProtoByteLength => "typedarray_proto_byte_length",
            Self::TypedArrayProtoByteOffset => "typedarray_proto_byte_offset",
            Self::TypedArrayProtoSet => "typedarray_proto_set",
            Self::TypedArrayProtoSlice => "typedarray_proto_slice",
            Self::TypedArrayProtoSubarray => "typedarray_proto_subarray",
            Self::TypedArrayProtoFill => "typedarray_proto_fill",
            Self::TypedArrayProtoReverse => "typedarray_proto_reverse",
            Self::TypedArrayProtoIndexOf => "typedarray_proto_index_of",
            Self::TypedArrayProtoLastIndexOf => "typedarray_proto_last_index_of",
            Self::TypedArrayProtoIncludes => "typedarray_proto_includes",
            Self::TypedArrayProtoJoin => "typedarray_proto_join",
            Self::TypedArrayProtoToString => "typedarray_proto_to_string",
            Self::TypedArrayProtoCopyWithin => "typedarray_proto_copy_within",
            Self::TypedArrayProtoAt => "typedarray_proto_at",
            Self::TypedArrayProtoForEach => "typedarray_proto_for_each",
            Self::TypedArrayProtoMap => "typedarray_proto_map",
            Self::TypedArrayProtoFilter => "typedarray_proto_filter",
            Self::TypedArrayProtoReduce => "typedarray_proto_reduce",
            Self::TypedArrayProtoReduceRight => "typedarray_proto_reduce_right",
            Self::TypedArrayProtoFind => "typedarray_proto_find",
            Self::TypedArrayProtoFindIndex => "typedarray_proto_find_index",
            Self::TypedArrayProtoSome => "typedarray_proto_some",
            Self::TypedArrayProtoEvery => "typedarray_proto_every",
            Self::TypedArrayProtoSort => "typedarray_proto_sort",
            Self::TypedArrayProtoEntries => "typedarray_proto_entries",
            Self::TypedArrayProtoKeys => "typedarray_proto_keys",
            Self::TypedArrayProtoValues => "typedarray_proto_values",
            Self::GetBuiltinGlobal => "get_builtin_global",
            Self::CreateGlobalObject => "create_global_object",
            Self::CreateException => "create_exception",
            Self::ExceptionValue => "exception_value",
            Self::IsException => "is_exception",
            Self::NewTarget => "new_target",
            Self::CreateUnmappedArgumentsObject => "create_unmapped_arguments_object",
            Self::CreateMappedArgumentsObject => "create_mapped_arguments_object",
            Self::ScopeRecordCreate => "scope_record_create",
            Self::ScopeRecordAddBinding => "scope_record_add_binding",
            Self::EvalGetBinding => "eval_get_binding",
            Self::EvalSetBinding => "eval_set_binding",
            Self::EvalHasBinding => "eval_has_binding",
            Self::EvalSuperBase => "eval_super_base",
            Self::ScopeRecordSetMeta => "scope_record_set_meta",
            Self::ScopeRecordDestroy => "scope_record_destroy",
        }
    }
}
```

- [ ] **Step 2: 添加 `ALL_BUILTINS` 常量**

在同文件底部（`impl fmt::Display for Builtin` 后面，`#[cfg(test)]` 前面）：

```rust
/// 所有 Builtin 变体的数组，用于自动生成 builtin_func_indices。
pub const ALL_BUILTINS: &[Builtin] = &[
    Builtin::ConsoleLog,
    Builtin::ConsoleError,
    Builtin::ConsoleWarn,
    Builtin::ConsoleInfo,
    Builtin::ConsoleDebug,
    Builtin::ConsoleTrace,
    Builtin::Debugger,
    Builtin::Throw,
    Builtin::AbortShadowStackOverflow,
    Builtin::F64Mod,
    Builtin::F64Exp,
    Builtin::IteratorFrom,
    Builtin::IteratorNext,
    Builtin::IteratorClose,
    Builtin::AsyncIteratorFrom,
    Builtin::IteratorValue,
    Builtin::IteratorDone,
    Builtin::EnumeratorFrom,
    Builtin::EnumeratorNext,
    Builtin::EnumeratorKey,
    Builtin::EnumeratorDone,
    Builtin::TypeOf,
    Builtin::In,
    Builtin::InstanceOf,
    Builtin::AbstractEq,
    Builtin::AbstractCompare,
    Builtin::DefineProperty,
    Builtin::GetOwnPropDesc,
    Builtin::SetTimeout,
    Builtin::ClearTimeout,
    Builtin::SetInterval,
    Builtin::ClearInterval,
    Builtin::Fetch,
    Builtin::Eval,
    Builtin::EvalIndirect,
    Builtin::JsonStringify,
    Builtin::JsonParse,
    Builtin::CreateClosure,
    Builtin::ArrayPush,
    Builtin::ArrayPop,
    Builtin::ArrayIncludes,
    Builtin::ArrayIndexOf,
    Builtin::ArrayJoin,
    Builtin::ArrayConcat,
    Builtin::ArraySlice,
    Builtin::ArrayFill,
    Builtin::ArrayReverse,
    Builtin::ArrayFlat,
    Builtin::ArrayInitLength,
    Builtin::ArrayGetLength,
    Builtin::ArrayShift,
    Builtin::ArrayUnshiftVa,
    Builtin::ArraySort,
    Builtin::ArrayAt,
    Builtin::ArrayCopyWithin,
    Builtin::ArrayForEach,
    Builtin::ArrayMap,
    Builtin::ArrayFilter,
    Builtin::ArrayReduce,
    Builtin::ArrayReduceRight,
    Builtin::ArrayFind,
    Builtin::ArrayFindIndex,
    Builtin::ArraySome,
    Builtin::ArrayEvery,
    Builtin::ArrayFlatMap,
    Builtin::ArrayIsArray,
    Builtin::ArrayFrom,
    Builtin::ArraySpliceVa,
    Builtin::ArrayConcatVa,
    Builtin::FuncCall,
    Builtin::FuncApply,
    Builtin::FuncBind,
    Builtin::ObjectRest,
    Builtin::HasOwnProperty,
    Builtin::PrivateGet,
    Builtin::PrivateSet,
    Builtin::PrivateHas,
    Builtin::ObjectProtoToString,
    Builtin::ObjectProtoValueOf,
    Builtin::ObjectKeys,
    Builtin::ObjectValues,
    Builtin::ObjectEntries,
    Builtin::ObjectAssign,
    Builtin::ObjectCreate,
    Builtin::ObjectGetPrototypeOf,
    Builtin::ObjectSetPrototypeOf,
    Builtin::ObjectGetOwnPropertyNames,
    Builtin::ObjectIs,
    Builtin::ObjectGroupBy,
    Builtin::MapGroupBy,
    Builtin::BigIntFromLiteral,
    Builtin::BigIntAdd,
    Builtin::BigIntSub,
    Builtin::BigIntMul,
    Builtin::BigIntDiv,
    Builtin::BigIntMod,
    Builtin::BigIntPow,
    Builtin::BigIntNeg,
    Builtin::BigIntEq,
    Builtin::BigIntCmp,
    Builtin::SymbolCreate,
    Builtin::SymbolFor,
    Builtin::SymbolKeyFor,
    Builtin::SymbolWellKnown,
    Builtin::RegExpCreate,
    Builtin::RegExpTest,
    Builtin::RegExpExec,
    Builtin::StringMatch,
    Builtin::StringReplace,
    Builtin::StringSearch,
    Builtin::StringSplit,
    Builtin::PromiseCreate,
    Builtin::PromiseInstanceResolve,
    Builtin::PromiseInstanceReject,
    Builtin::PromiseCreateResolveFunction,
    Builtin::PromiseCreateRejectFunction,
    Builtin::PromiseThen,
    Builtin::PromiseCatch,
    Builtin::PromiseFinally,
    Builtin::PromiseAll,
    Builtin::PromiseRace,
    Builtin::PromiseAllSettled,
    Builtin::PromiseAny,
    Builtin::PromiseResolveStatic,
    Builtin::PromiseRejectStatic,
    Builtin::IsPromise,
    Builtin::QueueMicrotask,
    Builtin::DrainMicrotasks,
    Builtin::AsyncFunctionStart,
    Builtin::AsyncFunctionResume,
    Builtin::AsyncFunctionSuspend,
    Builtin::ContinuationCreate,
    Builtin::ContinuationSaveVar,
    Builtin::ContinuationLoadVar,
    Builtin::AsyncGeneratorStart,
    Builtin::AsyncGeneratorNext,
    Builtin::AsyncGeneratorReturn,
    Builtin::AsyncGeneratorThrow,
    Builtin::PromiseWithResolvers,
    Builtin::IsCallable,
    Builtin::DynamicImport,
    Builtin::RegisterModuleNamespace,
    Builtin::JsxCreateElement,
    Builtin::ProxyCreate,
    Builtin::ProxyRevocable,
    Builtin::ReflectGet,
    Builtin::ReflectSet,
    Builtin::ReflectHas,
    Builtin::ReflectDeleteProperty,
    Builtin::ReflectApply,
    Builtin::ReflectConstruct,
    Builtin::ReflectGetPrototypeOf,
    Builtin::ReflectSetPrototypeOf,
    Builtin::ReflectIsExtensible,
    Builtin::ReflectPreventExtensions,
    Builtin::ReflectGetOwnPropertyDescriptor,
    Builtin::ReflectDefineProperty,
    Builtin::ReflectOwnKeys,
    Builtin::StringAt,
    Builtin::StringCharAt,
    Builtin::StringCharCodeAt,
    Builtin::StringCodePointAt,
    Builtin::StringConcatVa,
    Builtin::StringEndsWith,
    Builtin::StringIncludes,
    Builtin::StringIndexOf,
    Builtin::StringLastIndexOf,
    Builtin::StringMatchAll,
    Builtin::StringPadEnd,
    Builtin::StringPadStart,
    Builtin::StringRepeat,
    Builtin::StringReplaceAll,
    Builtin::StringSlice,
    Builtin::StringStartsWith,
    Builtin::StringSubstring,
    Builtin::StringToLowerCase,
    Builtin::StringToUpperCase,
    Builtin::StringTrim,
    Builtin::StringTrimEnd,
    Builtin::StringTrimStart,
    Builtin::StringToString,
    Builtin::StringValueOf,
    Builtin::StringIterator,
    Builtin::StringFromCharCode,
    Builtin::StringFromCodePoint,
    Builtin::MathAbs,
    Builtin::MathAcos,
    Builtin::MathAcosh,
    Builtin::MathAsin,
    Builtin::MathAsinh,
    Builtin::MathAtan,
    Builtin::MathAtanh,
    Builtin::MathAtan2,
    Builtin::MathCbrt,
    Builtin::MathCeil,
    Builtin::MathClz32,
    Builtin::MathCos,
    Builtin::MathCosh,
    Builtin::MathExp,
    Builtin::MathExpm1,
    Builtin::MathFloor,
    Builtin::MathFround,
    Builtin::MathHypot,
    Builtin::MathImul,
    Builtin::MathLog,
    Builtin::MathLog1p,
    Builtin::MathLog10,
    Builtin::MathLog2,
    Builtin::MathMax,
    Builtin::MathMin,
    Builtin::MathPow,
    Builtin::MathRandom,
    Builtin::MathRound,
    Builtin::MathSign,
    Builtin::MathSin,
    Builtin::MathSinh,
    Builtin::MathSqrt,
    Builtin::MathTan,
    Builtin::MathTanh,
    Builtin::MathTrunc,
    Builtin::NumberConstructor,
    Builtin::NumberIsNaN,
    Builtin::NumberIsFinite,
    Builtin::NumberIsInteger,
    Builtin::NumberIsSafeInteger,
    Builtin::NumberParseInt,
    Builtin::NumberParseFloat,
    Builtin::NumberProtoToString,
    Builtin::NumberProtoValueOf,
    Builtin::NumberProtoToFixed,
    Builtin::NumberProtoToExponential,
    Builtin::NumberProtoToPrecision,
    Builtin::BooleanConstructor,
    Builtin::BooleanProtoToString,
    Builtin::BooleanProtoValueOf,
    Builtin::ErrorConstructor,
    Builtin::TypeErrorConstructor,
    Builtin::RangeErrorConstructor,
    Builtin::SyntaxErrorConstructor,
    Builtin::ReferenceErrorConstructor,
    Builtin::URIErrorConstructor,
    Builtin::EvalErrorConstructor,
    Builtin::ErrorProtoToString,
    Builtin::MapConstructor,
    Builtin::MapProtoSet,
    Builtin::MapProtoGet,
    Builtin::SetConstructor,
    Builtin::SetProtoAdd,
    Builtin::MapSetHas,
    Builtin::MapSetDelete,
    Builtin::MapSetClear,
    Builtin::MapSetGetSize,
    Builtin::MapSetForEach,
    Builtin::MapSetKeys,
    Builtin::MapSetValues,
    Builtin::MapSetEntries,
    Builtin::DateConstructor,
    Builtin::DateNow,
    Builtin::DateParse,
    Builtin::DateUTC,
    Builtin::WeakMapConstructor,
    Builtin::WeakMapProtoSet,
    Builtin::WeakMapProtoGet,
    Builtin::WeakMapProtoHas,
    Builtin::WeakMapProtoDelete,
    Builtin::WeakSetConstructor,
    Builtin::WeakSetProtoAdd,
    Builtin::WeakSetProtoHas,
    Builtin::WeakSetProtoDelete,
    Builtin::SharedArrayBufferConstructor,
    Builtin::SharedArrayBufferProtoByteLength,
    Builtin::SharedArrayBufferProtoSlice,
    Builtin::SharedArrayBufferSpecies,
    Builtin::AtomicsLoad,
    Builtin::AtomicsStore,
    Builtin::AtomicsAdd,
    Builtin::AtomicsSub,
    Builtin::AtomicsAnd,
    Builtin::AtomicsOr,
    Builtin::AtomicsXor,
    Builtin::AtomicsExchange,
    Builtin::AtomicsCompareExchange,
    Builtin::AtomicsIsLockFree,
    Builtin::AtomicsWait,
    Builtin::AtomicsNotify,
    Builtin::AtomicsWaitAsync,
    Builtin::WeakRefConstructor,
    Builtin::WeakRefProtoDeref,
    Builtin::FinalizationRegistryConstructor,
    Builtin::FinalizationRegistryProtoRegister,
    Builtin::FinalizationRegistryProtoUnregister,
    Builtin::ArrayBufferConstructor,
    Builtin::ArrayBufferProtoByteLength,
    Builtin::ArrayBufferProtoSlice,
    Builtin::DataViewConstructor,
    Builtin::DataViewProtoGetFloat64,
    Builtin::DataViewProtoGetFloat32,
    Builtin::DataViewProtoGetInt32,
    Builtin::DataViewProtoGetUint32,
    Builtin::DataViewProtoGetInt16,
    Builtin::DataViewProtoGetUint16,
    Builtin::DataViewProtoGetInt8,
    Builtin::DataViewProtoGetUint8,
    Builtin::DataViewProtoSetFloat64,
    Builtin::DataViewProtoSetFloat32,
    Builtin::DataViewProtoSetInt32,
    Builtin::DataViewProtoSetUint32,
    Builtin::DataViewProtoSetInt16,
    Builtin::DataViewProtoSetUint16,
    Builtin::DataViewProtoSetInt8,
    Builtin::DataViewProtoSetUint8,
    Builtin::Int8ArrayConstructor,
    Builtin::Uint8ArrayConstructor,
    Builtin::Uint8ClampedArrayConstructor,
    Builtin::Int16ArrayConstructor,
    Builtin::Uint16ArrayConstructor,
    Builtin::Int32ArrayConstructor,
    Builtin::Uint32ArrayConstructor,
    Builtin::Float32ArrayConstructor,
    Builtin::Float64ArrayConstructor,
    Builtin::TypedArrayProtoLength,
    Builtin::TypedArrayProtoByteLength,
    Builtin::TypedArrayProtoByteOffset,
    Builtin::TypedArrayProtoSet,
    Builtin::TypedArrayProtoSlice,
    Builtin::TypedArrayProtoSubarray,
    Builtin::TypedArrayProtoFill,
    Builtin::TypedArrayProtoReverse,
    Builtin::TypedArrayProtoIndexOf,
    Builtin::TypedArrayProtoLastIndexOf,
    Builtin::TypedArrayProtoIncludes,
    Builtin::TypedArrayProtoJoin,
    Builtin::TypedArrayProtoToString,
    Builtin::TypedArrayProtoCopyWithin,
    Builtin::TypedArrayProtoAt,
    Builtin::TypedArrayProtoForEach,
    Builtin::TypedArrayProtoMap,
    Builtin::TypedArrayProtoFilter,
    Builtin::TypedArrayProtoReduce,
    Builtin::TypedArrayProtoReduceRight,
    Builtin::TypedArrayProtoFind,
    Builtin::TypedArrayProtoFindIndex,
    Builtin::TypedArrayProtoSome,
    Builtin::TypedArrayProtoEvery,
    Builtin::TypedArrayProtoSort,
    Builtin::TypedArrayProtoEntries,
    Builtin::TypedArrayProtoKeys,
    Builtin::TypedArrayProtoValues,
    Builtin::GetBuiltinGlobal,
    Builtin::CreateGlobalObject,
    Builtin::CreateException,
    Builtin::ExceptionValue,
    Builtin::NewTarget,
    Builtin::CreateUnmappedArgumentsObject,
    Builtin::CreateMappedArgumentsObject,
    Builtin::ScopeRecordCreate,
    Builtin::ScopeRecordAddBinding,
    Builtin::EvalGetBinding,
    Builtin::EvalSetBinding,
    Builtin::EvalHasBinding,
    Builtin::EvalSuperBase,
    Builtin::ScopeRecordSetMeta,
    Builtin::ScopeRecordDestroy,
];
```

- [ ] **Step 3: 验证编译**

```bash
cargo check -p wjsm-ir
```

预期: 编译通过。`ALL_BUILTINS` 覆盖全部 260+ 个 Builtin 变体。

---

### Task 2: 后端自动生成 builtin_func_indices

**Files:**
- Modify: `crates/wjsm-backend-wasm/src/compiler_core.rs`

- [ ] **Step 1: 在 `Compiler::new()` 中替换手写映射**

找到 `let mut builtin_func_indices = HashMap::new();`（约第 1010 行）及其后全部 `insert` 调用（到约第 1398 行），替换为：

```rust
let mut builtin_func_indices = HashMap::new();
// 从 HOST_IMPORT_NAMES 自动生成 builtin → WASM 函数索引的映射
{
    let name_to_idx: std::collections::HashMap<&str, u32> = HOST_IMPORT_NAMES
        .iter()
        .enumerate()
        .map(|(i, name)| (*name, i as u32))
        .collect();
    for &builtin in wjsm_ir::builtin::ALL_BUILTINS {
        let name = builtin.import_name();
        let idx = name_to_idx
            .get(name)
            .copied()
            .unwrap_or_else(|| panic!("Builtin::{:?} import_name()=\"{name}\" not found in HOST_IMPORT_NAMES", builtin));
        builtin_func_indices.insert(builtin, idx);
    }
}
```

- [ ] **Step 2: 验证编译**

```bash
cargo check -p wjsm-backend-wasm
```

预期: 编译通过。如果某个 `Builtin` 的 `import_name()` 与 `HOST_IMPORT_NAMES` 不一致，会在 `Compiler::new()` 时 panic 并给出明确错误信息。

---

### Task 3: 转换 host_imports 裸块文件（core, timers_arrays）

**Files:**
- Modify: `crates/wjsm-runtime/src/host_imports/core.rs`
- Modify: `crates/wjsm-runtime/src/host_imports/timers_arrays.rs`

**模式**：每个文件的转换模式相同 — 将裸块 `{ ... vec![...] }` 改为 `pub(crate) fn define_xxx(linker: &mut Linker<RuntimeState>) -> Result<()> { ... Ok(()) }`。

以 `core.rs` 为例：

```rust
// 旧（裸块开头）:
{
    let console_log = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, args_base: i32, args_count: i32| {
            write_console_values(&mut caller, args_base, args_count, None);
        },
    );

// 新:
use anyhow::Result;
use wasmtime::{Caller, Linker};

use crate::*;

pub(crate) fn define_core(linker: &mut Linker<RuntimeState>) -> Result<()> {
    linker.func_wrap("env", "console_log",
        |mut caller: Caller<'_, RuntimeState>, args_base: i32, args_count: i32| {
            write_console_values(&mut caller, args_base, args_count, None);
        },
    )?;
```

**关键规则**：
- 每个 `Func::wrap(&mut store, |...| { BODY })` → `linker.func_wrap("env", "IMPORT_NAME", |...| { BODY })?;`
- **闭包体完全不变** — 只改外围包装
- 文件末尾 `vec![...]` → `Ok(())`
- 文件不再需要 `&mut store` 参数

- [ ] **Step 1: 转换 core.rs（导入 0-27）**

core.rs 导入列表（名字来自 HOST_IMPORT_NAMES）：
`console_log`, `f64_mod`, `f64_pow`, `throw`, `iterator_from`, `iterator_next`, `iterator_close`, `iterator_value`, `iterator_done`, `enumerator_from`, `enumerator_next`, `enumerator_key`, `enumerator_done`, `typeof`, `op_in`, `op_instanceof`, `string_concat`, `string_concat_va`, `define_property`, `get_own_prop_desc`, `abstract_eq`, `abstract_compare`, `gc_collect`, `console_error`, `console_warn`, `console_info`, `console_debug`, `console_trace`

转换命令：逐个替换 `let VAR = Func::wrap(&mut store, |...| { ... });` → `linker.func_wrap("env", "NAME", |...| { ... })?;`，删除 vec 返回。

- [ ] **Step 2: 转换 timers_arrays.rs（导入 28-49）**

导入名：`set_timeout`, `clear_timeout`, `set_interval`, `clear_interval`, `fetch`, `json_stringify`, `json_parse`, `closure_create`, `closure_get_func`, `closure_get_env`, `arr_push`, `arr_pop`, `arr_includes`, `arr_index_of`, `arr_join`, `arr_concat`, `arr_slice`, `arr_fill`, `arr_reverse`, `arr_flat`, `arr_init_length`, `arr_get_length`

同模式转换。

- [ ] **Step 3: 验证语法**

```bash
cargo check -p wjsm-runtime 2>&1 | head -30
```

预期: 有编译错误（lib.rs 还在用旧的 `include!` + `Vec<Extern>` 模式）。确认只有 lib.rs 的错，host_imports 文件本身语法正确。

---

### Task 4: 转换 host_imports 裸块文件（array_object, primitive_core）

**Files:**
- Modify: `crates/wjsm-runtime/src/host_imports/array_object.rs`
- Modify: `crates/wjsm-runtime/src/host_imports/primitive_core.rs`

同 Task 3 模式。

- [ ] **Step 1: 转换 array_object.rs**

导入名（约 50-76 范围，参考 HOST_IMPORT_NAMES 索引）：`arr_proto_push`, `arr_proto_pop`, `arr_proto_includes`, `arr_proto_index_of`, `arr_proto_join`, `arr_proto_concat`, `arr_proto_slice`, `arr_proto_fill`, `arr_proto_reverse`, `arr_proto_flat`, `arr_proto_shift`, `arr_proto_unshift`, `arr_proto_sort`, `arr_proto_at`, `arr_proto_copy_within`, `arr_proto_for_each`, `arr_proto_map`, `arr_proto_filter`, `arr_proto_reduce`, `arr_proto_reduce_right`, `arr_proto_find`, `arr_proto_find_index`, `arr_proto_some`, `arr_proto_every`, `arr_proto_flat_map`, `arr_proto_splice`, `arr_proto_is_array`

- [ ] **Step 2: 转换 primitive_core.rs**

导入名：`bigint_from_literal`, `bigint_add`, `bigint_sub`, `bigint_mul`, `bigint_div`, `bigint_mod`, `bigint_pow`, `bigint_neg`, `bigint_eq`, `bigint_cmp`, `symbol_create`, `symbol_for`, `symbol_key_for`, `symbol_well_known`, `regex_create`, `regex_test`, `regex_exec`, `string_match`, `string_replace`, `string_search`, `string_split`

---

### Task 5: 转换 host_imports register_* 函数（promise, combinators, misc, async_fn, async_generator, proxy_reflect）

**Files:**
- Modify: `crates/wjsm-runtime/src/host_imports/promise.rs`
- Modify: `crates/wjsm-runtime/src/host_imports/promise_combinators.rs`
- Modify: `crates/wjsm-runtime/src/host_imports/misc.rs`
- Modify: `crates/wjsm-runtime/src/host_imports/async_fn.rs`
- Modify: `crates/wjsm-runtime/src/host_imports/async_generator.rs`
- Modify: `crates/wjsm-runtime/src/host_imports/proxy_reflect.rs`

这些文件使用 `pub(crate) fn register_xxx_imports(mut store: &mut Store<RuntimeState>) -> Vec<Extern>` 签名，转换为 `pub(crate) fn define_xxx(linker: &mut Linker<RuntimeState>) -> Result<()>`。不再需要 `store` 参数。

**注意**：当前 `register_all_imports`（在 misc.rs 中）使用 drain/remove 交错 p（promise）、c（combinators）、m（misc）、a（async_fn）、g（async_generator）、r（proxy_reflect）的 Vec。转换为 Linker 后，这个交错逻辑**完全删除** — 每个模块独立注册到 Linker，顺序无关。

- [ ] **Step 1: 转换 promise.rs**

导入名：`promise_create`, `promise_instance_resolve`, `promise_instance_reject`, `promise_then`, `promise_catch`, `promise_finally`, `promise_resolve_static`, `promise_reject_static`, `is_promise`, `promise_create_resolve_function`, `promise_create_reject_function`, `promise_with_resolvers`

将 `pub(crate) fn register_promise_imports(mut store: &mut Store<RuntimeState>) -> Vec<Extern>` 改为 `pub(crate) fn define_promise(linker: &mut Linker<RuntimeState>) -> Result<()>`。所有 `Func::wrap(&mut store, |...| { ... })` → `linker.func_wrap("env", "NAME", |...| { ... })?;`。删除末尾 `vec![...]` → `Ok(())`。

- [ ] **Step 2: 转换 promise_combinators.rs**

导入名：`promise_all`, `promise_race`, `promise_all_settled`, `promise_any`

同上模式。

- [ ] **Step 3: 转换 misc.rs**

删除 `register_misc_imports` 和 `register_all_imports` 两个函数。改为一个 `pub(crate) fn define_misc(linker: &mut Linker<RuntimeState>) -> Result<()>`。

导入名：`queue_microtask`, `drain_microtasks`, `native_call`, `is_callable`, `register_module_namespace`, `dynamic_import`, `eval_direct`, `eval_indirect`, `jsx_create_element`

- [ ] **Step 4: 转换 async_fn.rs**

导入名：`async_function_start`, `async_function_resume`, `async_function_suspend`, `continuation_create`, `continuation_save_var`, `continuation_load_var`

- [ ] **Step 5: 转换 async_generator.rs**

导入名：`async_generator_start`, `async_generator_next`, `async_generator_return`, `async_generator_throw`

- [ ] **Step 6: 转换 proxy_reflect.rs**

导入名：`proxy_create`, `proxy_revocable`, `reflect_get`, `reflect_set`, `reflect_has`, `reflect_delete_property`, `reflect_apply`, `reflect_construct`, `reflect_get_prototype_of`, `reflect_set_prototype_of`, `reflect_is_extensible`, `reflect_prevent_extensions`, `reflect_get_own_property_descriptor`, `reflect_define_property`, `reflect_own_keys`

---

### Task 6: 转换 host_imports 裸块文件（string_methods, math_number_error, collections_buffers）

**Files:**
- Modify: `crates/wjsm-runtime/src/host_imports/string_methods.rs`
- Modify: `crates/wjsm-runtime/src/host_imports/math_number_error.rs`
- Modify: `crates/wjsm-runtime/src/host_imports/collections_buffers.rs`

同 Task 3 模式。

- [ ] **Step 1: 转换 string_methods.rs**

导入名（166-192）：`string_at`, `string_char_at`, `string_char_code_at`, `string_code_point_at`, `string_concat_proto`, `string_ends_with`, `string_includes`, `string_index_of`, `string_last_index_of`, `string_match_all`, `string_pad_end`, `string_pad_start`, `string_repeat`, `string_replace_all`, `string_slice`, `string_starts_with`, `string_substring`, `string_to_lower_case`, `string_to_upper_case`, `string_trim`, `string_trim_end`, `string_trim_start`, `string_to_string`, `string_value_of`, `string_iterator`, `string_from_char_code`, `string_from_code_point`

- [ ] **Step 2: 转换 math_number_error.rs**

导入名（193-277 范围）：`math_abs` 到 `math_trunc`（所有 Math 方法），`number_constructor` 到 `number_proto_to_precision`（Number 方法），`boolean_constructor` 到 `boolean_proto_value_of`，`error_constructor` 到 `error_proto_to_string`

- [ ] **Step 3: 转换 collections_buffers.rs**

导入名（251-317 范围）：`map_constructor`, `map_proto_set`, `map_proto_get`, `set_constructor`, `set_proto_add`, `map_set_has`, `map_set_delete`, `map_set_clear`, `map_set_get_size`, `map_set_for_each`, `map_set_keys`, `map_set_values`, `map_set_entries`, `date_constructor`, `date_now`, `date_parse`, `date_utc`, `weakmap_constructor`, `weakmap_proto_set`, `weakmap_proto_get`, `weakmap_proto_has`, `weakmap_proto_delete`, `weakset_constructor`, `weakset_proto_add`, `weakset_proto_has`, `weakset_proto_delete`, `arraybuffer_constructor`, `arraybuffer_proto_byte_length`, `arraybuffer_proto_slice`, `dataview_constructor` 到 `dataview_proto_set_uint8`, typedarray 构造器（`int8array_constructor` 到 `float64array_constructor`），typedarray proto（`typedarray_proto_length` 到 `typedarray_proto_subarray`）

---

### Task 7: 转换剩余 host_imports 裸块文件

**Files:**
- Modify: `crates/wjsm-runtime/src/host_imports/proxy_traps.rs`
- Modify: `crates/wjsm-runtime/src/host_imports/typedarray_new_methods.rs`
- Modify: `crates/wjsm-runtime/src/host_imports/weakref_finalization.rs`
- Modify: `crates/wjsm-runtime/src/host_imports/atomics.rs`
- Modify: `crates/wjsm-runtime/src/host_imports/get_builtin_global_entry.rs`

同 Task 3 模式。

- [ ] **Step 1: 转换 proxy_traps.rs**

导入名：`proxy_trap_get`, `proxy_trap_set`, `proxy_trap_delete`（参考当前 `// 316`, `// 317`, `// 318`）

- [ ] **Step 2: 转换 typedarray_new_methods.rs**

导入名（326-347）：`typedarray_proto_fill`, `typedarray_proto_reverse`, `typedarray_proto_index_of`, `typedarray_proto_last_index_of`, `typedarray_proto_includes`, `typedarray_proto_join`, `typedarray_proto_to_string`, `typedarray_proto_copy_within`, `typedarray_proto_at`, `typedarray_proto_for_each`, `typedarray_proto_map`, `typedarray_proto_filter`, `typedarray_proto_reduce`, `typedarray_proto_reduce_right`, `typedarray_proto_find`, `typedarray_proto_find_index`, `typedarray_proto_some`, `typedarray_proto_every`, `typedarray_proto_sort`, `typedarray_proto_entries`, `typedarray_proto_keys`, `typedarray_proto_values`

- [ ] **Step 3: 转换 weakref_finalization.rs**

导入名：`weakref_constructor`, `weakref_proto_deref`, `finalization_registry_constructor`, `finalization_registry_proto_register`, `finalization_registry_proto_unregister`

- [ ] **Step 4: 转换 atomics.rs**

导入名：`sharedarraybuffer_constructor`, `sharedarraybuffer_proto_byte_length`, `sharedarraybuffer_proto_slice`, `sharedarraybuffer_proto_species`, `atomics_load`, `atomics_store`, `atomics_add`, `atomics_sub`, `atomics_and`, `atomics_or`, `atomics_xor`, `atomics_exchange`, `atomics_compare_exchange`, `atomics_is_lock_free`, `atomics_wait`, `atomics_notify`, `atomics_wait_async`

- [ ] **Step 5: 转换 get_builtin_global_entry.rs**

导入名：`get_builtin_global`（当前是 `include!` 返回单个 Extern，通过 `imports.push(include!(...))`。改为 `pub(crate) fn define_get_builtin_global(linker: &mut Linker<RuntimeState>) -> Result<()>`）

---

### Task 8: 重新布线 lib.rs（Linker + 内联函数转换）

**Files:**
- Modify: `crates/wjsm-runtime/src/lib.rs`

- [ ] **Step 1: 替换导入定义段（约 211-794 行）**

将当前从 `let mut imports: Vec<Extern>` 到 `let instance = Instance::new(...)` 的约 580 行替换为：

```rust
    let mut linker = Linker::new(engine);

    // ── 注册所有宿主函数（按名字链接，顺序无关） ──
    define_core(&mut linker)?;
    define_timers_arrays(&mut linker)?;
    define_array_object(&mut linker)?;
    define_primitive_core(&mut linker)?;
    define_promise(&mut linker)?;
    define_promise_combinators(&mut linker)?;
    define_misc(&mut linker)?;
    define_async_fn(&mut linker)?;
    define_async_generator(&mut linker)?;
    define_proxy_reflect(&mut linker)?;
    define_string_methods(&mut linker)?;
    define_math_number_error(&mut linker)?;
    define_collections_buffers(&mut linker)?;
    define_proxy_traps(&mut linker)?;
    define_get_builtin_global(&mut linker)?;

    // ── 内联函数（原先在 lib.rs 中直接 push 的） ──

    // new_target
    linker.func_wrap("env", "new_target",
        |caller: Caller<'_, RuntimeState>, _dummy: i64| -> i64 { caller.data().new_target.get() },
    )?;

    // new_target_set
    linker.func_wrap("env", "new_target_set",
        |caller: Caller<'_, RuntimeState>, new_target: i64| -> i64 {
            let previous = caller.data().new_target.get();
            caller.data().new_target.set(new_target);
            previous
        },
    )?;

    // create_unmapped_arguments_object
    linker.func_wrap("env", "create_unmapped_arguments_object",
        |mut caller: Caller<'_, RuntimeState>, args_array: i64, param_count: i64| -> i64 {
            create_unmapped_arguments_object(&mut caller, args_array, param_count)
        },
    )?;

    // create_mapped_arguments_object
    linker.func_wrap("env", "create_mapped_arguments_object",
        |mut caller: Caller<'_, RuntimeState>, args_array: i64, param_count: i64, func_ref: i64| -> i64 {
            create_mapped_arguments_object(&mut caller, args_array, param_count, func_ref)
        },
    )?;

    // ScopeRecord eval bridge
    linker.func_wrap("env", "scope_record_create",
        |mut caller: Caller<'_, RuntimeState>, capacity: i64| -> i64 {
            scope_record_create(caller, capacity)
        },
    )?;

    linker.func_wrap("env", "scope_record_add_binding",
        |mut caller: Caller<'_, RuntimeState>, record: i64, name: i64, val: i64, is_tdz: i64, is_const: i64| {
            scope_record_add_binding(caller, record, name, val, is_tdz, is_const);
        },
    )?;

    linker.func_wrap("env", "eval_get_binding",
        |mut caller: Caller<'_, RuntimeState>, record: i64, name: i64| -> i64 {
            eval_get_binding_from_caller(&mut caller, record, name)
        },
    )?;

    linker.func_wrap("env", "eval_set_binding",
        |mut caller: Caller<'_, RuntimeState>, record: i64, name: i64, val: i64| -> i64 {
            eval_set_binding_from_caller(&mut caller, record, name, val)
        },
    )?;

    linker.func_wrap("env", "eval_has_binding",
        |mut caller: Caller<'_, RuntimeState>, record: i64, name: i64| -> i64 {
            eval_has_binding_from_caller(&mut caller, record, name)
        },
    )?;

    linker.func_wrap("env", "eval_super_base",
        |mut caller: Caller<'_, RuntimeState>, record: i64| -> i64 {
            eval_get_super_base(&mut caller, record)
        },
    )?;

    linker.func_wrap("env", "scope_record_set_meta",
        |mut caller: Caller<'_, RuntimeState>, record: i64, key: i64, val: i64| {
            scope_record_set_meta(&mut caller, record, key, val);
        },
    )?;

    linker.func_wrap("env", "scope_record_destroy",
        |mut caller: Caller<'_, RuntimeState>, record: i64| {
            scope_record_destroy(caller, record);
        },
    )?;

    // TypedArray extra methods
    define_typedarray_new_methods(&mut linker)?;
    // WeakRef / FinalizationRegistry
    define_weakref_finalization(&mut linker)?;
    // SharedArrayBuffer + Atomics
    define_atomics(&mut linker)?;

    // async_iterator_from
    linker.func_wrap("env", "async_iterator_from",
        |mut caller: Caller<'_, RuntimeState>, iterable: i64| -> i64 {
            if !(value::is_object(iterable)
                || value::is_function(iterable)
                || value::is_undefined(iterable)
                || value::is_null(iterable))
                || !value::is_callable(
                    read_object_property_by_name(&mut caller, iterable, "next")
                        .unwrap_or(value::encode_undefined()),
                )
            {
                create_error_object(&mut caller, "TypeError", value::encode_undefined())
            } else {
                let handle = alloc_object(&mut caller, 0);
                define_host_data_property(
                    &mut caller,
                    handle,
                    "source",
                    iterable,
                );
                handle
            }
        },
    )?;

    // create_error_object 函数已在前面定义，这儿需要引用
    // 注意：该闭包依赖 create_error_object，如果在函数内定义，需要确保可见性

    // object.group_by
    linker.func_wrap("env", "object.group_by",
        |mut caller: Caller<'_, RuntimeState>, items: i64, callbackfn: i64| -> i64 {
            // ... 完整函数体从当前 lib.rs 搬过来，完全不变
            // (函数体太长，执行时根据源文件复制)
        },
    )?;

    // map.group_by
    linker.func_wrap("env", "map.group_by",
        |mut caller: Caller<'_, RuntimeState>, items: i64, callbackfn: i64| -> i64 {
            // ... 完整函数体从当前 lib.rs 搬过来
        },
    )?;

    // symbol_property_key
    linker.func_wrap("env", "symbol_property_key",
        |mut caller: Caller<'_, RuntimeState>, key: i64| -> i32 {
            // ... 完整函数体
        },
    )?;

    // array.from
    linker.func_wrap("env", "array.from",
        |mut caller: Caller<'_, RuntimeState>,
         _env_obj: i64, _this_val: i64, args_base: i32, args_count: i32|
         -> i64 {
            // ... 完整函数体
        },
    )?;

    // obj_get_by_index
    linker.func_wrap("env", "obj_get_by_index",
        |mut caller: Caller<'_, RuntimeState>, boxed: i64, index: i32| -> i64 {
            // ... 完整函数体
        },
    )?;

    // ── 实例化 ──
    let instance = linker.instantiate(&mut store, &module)?;
```

**关键规则**：
- 所有 `Func::wrap(&mut store, |...| { BODY })` → `linker.func_wrap("env", "IMPORT_NAME", |...| { BODY })?;`
- `let VAR = linker.func_wrap(...)?;` 的返回值是 `&mut Linker`，不需要赋值给变量
- 闭包体**完全不变** — 只需搬运
- `Instance::new(&mut store, &module, &imports)` → `linker.instantiate(&mut store, &module)`
- 注意 `create_error_object` 等函数在闭包内的可见性。如果原来通过 `Func::wrap` 内部捕获（而非调用模块级函数），需要确认。对于 `async_iterator_from` 使用了 `create_error_object`，检查它是否是模块级函数。如果是，保持不变；如果不是（在另一个闭包内定义），需要提取为模块级函数或传入引用。

**`create_error_object` 检查**：在 lib.rs 中搜索。它很可能是一个 `fn create_error_object(...)` 模块级函数。如果是，闭包可以直接调用。

- [ ] **Step 2: 更新 host_imports/mod.rs**

```rust
// 旧:
mod promise;
mod promise_combinators;
mod async_fn;
mod async_generator;
mod proxy_reflect;
mod misc;
mod promise_async;
pub(crate) use promise_async::register_all_imports;

// 新（删除 promise_async 重新导出，改为暴露 define_xxx 函数）:
mod promise;
mod promise_combinators;
mod async_fn;
mod async_generator;
mod proxy_reflect;
mod misc;

// 注意：裸块文件（core.rs, timers_arrays.rs 等）通过 include! 在 lib.rs 中使用，
// 不在 host_imports/mod.rs 中声明。改为 define_xxx 函数后，需要在 lib.rs 中直接
// 声明 use host_imports::xxx::define_xxx 或通过 mod.rs 重新导出。

// 方案：在 mod.rs 中重新导出所有 define_xxx 函数，lib.rs 通过 host_imports::* 获取
pub(crate) use promise::define_promise;
pub(crate) use promise_combinators::define_promise_combinators;
pub(crate) use misc::define_misc;
pub(crate) use async_fn::define_async_fn;
pub(crate) use async_generator::define_async_generator;
pub(crate) use proxy_reflect::define_proxy_reflect;
```

**重要**：对于原来通过 `include!` 的裸块文件（core.rs, timers_arrays.rs 等），它们不在 `host_imports/mod.rs` 中声明。转换为 `define_xxx` 函数后，有两个选项：
1. 在 `mod.rs` 中添加模块声明并重新导出
2. 在 `lib.rs` 中通过 `use crate::host_imports::xxx::define_xxx` 直接引用

**选择方案 1**：在 `mod.rs` 中添加所有模块声明。统一管理。

---

### Task 9: 清理死代码

**Files:**
- Modify: `crates/wjsm-runtime/src/host_imports/mod.rs`
- Delete (内容): `crates/wjsm-runtime/src/host_imports/promise_async.rs`

- [ ] **Step 1: 删除 promise_async.rs**

该文件当前仅包含：
```rust
pub(crate) use super::misc::register_all_imports;
```
替换为空文件或删除模块声明。

- [ ] **Step 2: 更新 host_imports/mod.rs**

最终 `mod.rs` 应包含所有模块声明和重新导出：

```rust
// 通过 mod.rs 注册的模块（register_*/define_* 函数）
mod promise;
mod promise_combinators;
mod async_fn;
mod async_generator;
mod proxy_reflect;
mod misc;

pub(crate) use promise::define_promise;
pub(crate) use promise_combinators::define_promise_combinators;
pub(crate) use misc::define_misc;
pub(crate) use async_fn::define_async_fn;
pub(crate) use async_generator::define_async_generator;
pub(crate) use proxy_reflect::define_proxy_reflect;

// include! 裸块文件 → 改为模块声明
mod core;
mod timers_arrays;
mod array_object;
mod primitive_core;
mod string_methods;
mod math_number_error;
mod collections_buffers;
mod proxy_traps;
mod typedarray_new_methods;
mod weakref_finalization;
mod atomics;
mod get_builtin_global_entry;

pub(crate) use core::define_core;
pub(crate) use timers_arrays::define_timers_arrays;
pub(crate) use array_object::define_array_object;
pub(crate) use primitive_core::define_primitive_core;
pub(crate) use string_methods::define_string_methods;
pub(crate) use math_number_error::define_math_number_error;
pub(crate) use collections_buffers::define_collections_buffers;
pub(crate) use proxy_traps::define_proxy_traps;
pub(crate) use typedarray_new_methods::define_typedarray_new_methods;
pub(crate) use weakref_finalization::define_weakref_finalization;
pub(crate) use atomics::define_atomics;
pub(crate) use get_builtin_global_entry::define_get_builtin_global;
```

- [ ] **Step 3: 更新 lib.rs 的 use 声明**

当前 `lib.rs` 第 32-33 行：
```rust
mod host_imports;
pub(crate) use host_imports::register_all_imports;
```

改为：
```rust
mod host_imports;
use host_imports::*;
```

（通过 glob import 导入所有 define_xxx 函数，lib.rs 中直接使用）

---

### Task 10: 编译、测试、验证

- [ ] **Step 1: 编译检查**

```bash
cargo check --workspace
```

预期: 编译通过，无警告（或仅保留原有的警告）。

- [ ] **Step 2: 完整构建**

```bash
cargo build
```

- [ ] **Step 3: 运行 E2E fixture 测试**

```bash
cargo nextest run --workspace -E 'not test(happy__new_prototype_chain) & not test(happy__global_fn_visible_in_nested) & not test(happy__eval_exception_expression_contexts) & not test(happy__weakref) & not test(happy__finalization_registry)'
```

预期: 全部通过（排除已知的超时 fixture）。如果有失败，检查对应 fixture 的 stdout/stderr 输出定位问题。

- [ ] **Step 4: 运行 IR 快照测试**

```bash
cargo nextest run -p wjsm-semantic
cargo nextest run -p wjsm-ir
```

预期: 全部通过（这些测试不涉及运行时层面，不应受影响）。

- [ ] **Step 5: 提交**

```bash
git add -A
git commit -m "refactor: eliminate import index fragility with name-based linking

Replace positional Vec<Extern> linking with wasmtime Linker name-based
registration. Auto-generate builtin_func_indices from HOST_IMPORT_NAMES.
Convert include! bare blocks to proper define_xxx functions. Delete
register_all_imports drain/remove interleaving.

BREAKING CHANGE: None (internal refactor, same WASM module output)"
```

---

## 验证清单

完成所有 Task 后确认：

- [ ] `cargo check --workspace` 通过
- [ ] 所有 E2E fixture 测试通过（排除已知超时）
- [ ] `git grep "register_all_imports"` 无结果
- [ ] `git grep "include!.*host_imports"` 仅在 `mod.rs` 中有 `mod` 声明
- [ ] `git grep "\.drain\(" crates/wjsm-runtime/src/host_imports/` 无结果
- [ ] `git grep "builtin_func_indices.insert"` 仅在 `compiler_core.rs` 的自动生成循环中
- [ ] `git grep "unwrap_or(0)" crates/wjsm-backend-wasm/src/` 无 `builtin_func_indices` 关联项
- [ ] 新增导入仅需：Builtin 枚举加变体 + `import_name()` 加分支 + `ALL_BUILTINS` 加条目 + `HOST_IMPORT_NAMES` 加名字 + 运行时 `linker.func_wrap` 注册
