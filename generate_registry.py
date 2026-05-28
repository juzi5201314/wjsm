#!/usr/bin/env python3
"""Generate host_import_registry.rs from the existing compiler_core.rs definitions."""
import re

# Read files
with open("crates/wjsm-backend-wasm/src/compiler_core.rs") as f:
    core = f.read()
with open("crates/wjsm-backend-wasm/src/lib.rs") as f:
    lib = f.read()

# Extract HOST_IMPORT_NAMES order
host_import_match = re.search(r'const HOST_IMPORT_NAMES: \[&str; \d+\] = \[(.*?)\];', lib, re.DOTALL)
assert host_import_match, "Could not find HOST_IMPORT_NAMES"
names_text = host_import_match.group(1)
names = re.findall(r'"([^"]+)"', names_text)
print(f"Found {len(names)} import names")

# Build name->type_idx map - handle both single-line and multi-line imports
# Single line: imports.import("env", "name", EntityType::Function(N))
# Multi-line: imports.import("env", "name", EntityType::Function(N));
#             imports.import("env", "name", EntityType::Function(N))

name_to_type = {}
# Find all import("env", ...) patterns
# Match the name and the function type index regardless of formatting
for m in re.finditer(
    r'imports\.import\(\s*"env"\s*,\s*"([^"]+)"\s*,\s*EntityType::Function\((\d+)\)\s*\)\s*[;)]',
    core
):
    name_to_type[m.group(1)] = int(m.group(2))

print(f"Found {len(name_to_type)} type mappings")

# Check for missing
missing = [n for n in names if n not in name_to_type]
if missing:
    print(f"MISSING {len(missing)} types:")
    for n in missing:
        print(f"  {n}")

# Also check for extra names in name_to_type not in names
extra = [n for n in name_to_type if n not in names]
if extra:
    print(f"EXTRA names in type map (not in HOST_IMPORT_NAMES): {extra}")

# Fix known HOST_IMPORT_NAMES vs actual WASM import name discrepancies
NAME_FIXES = {
    # HOST_IMPORT_NAMES says "string_concat_proto" but the actual WASM import
    # section and runtime use "string_proto_concat". HOST_IMPORT_NAMES is wrong.
    "string_concat_proto": ("string_proto_concat", "StringConcatVa"),
}
for old_name, (new_name, _) in NAME_FIXES.items():
    if old_name in names:
        idx = names.index(old_name)
        names[idx] = new_name
        print(f"Fixed name at index {idx}: '{old_name}' -> '{new_name}'")

# Builtin -> import name mapping
BUILTIN_TO_NAME = {
    "ConsoleLog": "console_log", "ConsoleError": "console_error",
    "ConsoleWarn": "console_warn", "ConsoleInfo": "console_info",
    "ConsoleDebug": "console_debug", "ConsoleTrace": "console_trace",
    "Throw": "throw", "AbortShadowStackOverflow": "abort_shadow_stack_overflow",
    "F64Mod": "f64_mod", "F64Exp": "f64_pow",
    "IteratorFrom": "iterator_from", "IteratorNext": "iterator_next",
    "IteratorClose": "iterator_close", "AsyncIteratorFrom": "async_iterator_from",
    "IteratorValue": "iterator_value", "IteratorDone": "iterator_done",
    "EnumeratorFrom": "enumerator_from", "EnumeratorNext": "enumerator_next",
    "EnumeratorKey": "enumerator_key", "EnumeratorDone": "enumerator_done",
    "TypeOf": "typeof", "In": "op_in", "InstanceOf": "op_instanceof",
    "AbstractEq": "abstract_eq", "AbstractCompare": "abstract_compare",
    "DefineProperty": "define_property", "GetOwnPropDesc": "get_own_prop_desc",
    "SetTimeout": "set_timeout", "ClearTimeout": "clear_timeout",
    "SetInterval": "set_interval", "ClearInterval": "clear_interval",
    "Fetch": "fetch", "Eval": "eval_direct", "EvalIndirect": "eval_indirect",
    "JsonStringify": "json_stringify", "JsonParse": "json_parse",
    "CreateClosure": "closure_create",
    "ArrayPush": "arr_push", "ArrayPop": "arr_pop",
    "ArrayIncludes": "arr_includes", "ArrayIndexOf": "arr_index_of",
    "ArrayJoin": "arr_join", "ArrayConcat": "arr_concat",
    "ArraySlice": "arr_slice", "ArrayFill": "arr_fill",
    "ArrayReverse": "arr_reverse", "ArrayFlat": "arr_flat",
    "ArrayInitLength": "arr_init_length", "ArrayGetLength": "arr_get_length",
    "ArrayShift": "arr_proto_shift", "ArrayUnshiftVa": "arr_proto_unshift",
    "ArraySort": "arr_proto_sort", "ArrayAt": "arr_proto_at",
    "ArrayCopyWithin": "arr_proto_copy_within",
    "ArrayForEach": "arr_proto_for_each", "ArrayMap": "arr_proto_map",
    "ArrayFilter": "arr_proto_filter", "ArrayReduce": "arr_proto_reduce",
    "ArrayReduceRight": "arr_proto_reduce_right",
    "ArrayFind": "arr_proto_find", "ArrayFindIndex": "arr_proto_find_index",
    "ArraySome": "arr_proto_some", "ArrayEvery": "arr_proto_every",
    "ArrayFlatMap": "arr_proto_flat_map", "ArrayIsArray": "arr_proto_is_array",
    "ArrayFrom": "array.from", "ArraySpliceVa": "arr_proto_splice",
    "ArrayConcatVa": "string_concat_va",
    "FuncCall": "func_call", "FuncApply": "func_apply", "FuncBind": "func_bind",
    "ObjectRest": "object_rest",
    "HasOwnProperty": "has_own_property",
    "PrivateGet": "private_get", "PrivateSet": "private_set", "PrivateHas": "private_has",
    "ObjectProtoToString": "obj_proto_to_string",
    "ObjectProtoValueOf": "obj_proto_value_of",
    "ObjectKeys": "obj_keys", "ObjectValues": "obj_values",
    "ObjectEntries": "obj_entries", "ObjectAssign": "obj_assign",
    "ObjectCreate": "obj_create", "ObjectGetPrototypeOf": "obj_get_proto_of",
    "ObjectSetPrototypeOf": "obj_set_proto_of",
    "ObjectGetOwnPropertyNames": "obj_get_own_prop_names",
    "ObjectIs": "obj_is", "ObjectGroupBy": "object.group_by",
    "MapGroupBy": "map.group_by",
    "BigIntFromLiteral": "bigint_from_literal",
    "BigIntAdd": "bigint_add", "BigIntSub": "bigint_sub",
    "BigIntMul": "bigint_mul", "BigIntDiv": "bigint_div",
    "BigIntMod": "bigint_mod", "BigIntPow": "bigint_pow",
    "BigIntNeg": "bigint_neg", "BigIntEq": "bigint_eq", "BigIntCmp": "bigint_cmp",
    "SymbolCreate": "symbol_create", "SymbolFor": "symbol_for",
    "SymbolKeyFor": "symbol_key_for", "SymbolWellKnown": "symbol_well_known",
    "RegExpCreate": "regex_create", "RegExpTest": "regex_test", "RegExpExec": "regex_exec",
    "StringMatch": "string_match", "StringReplace": "string_replace",
    "StringSearch": "string_search", "StringSplit": "string_split",
    "PromiseCreate": "promise_create",
    "PromiseInstanceResolve": "promise_instance_resolve",
    "PromiseInstanceReject": "promise_instance_reject",
    "PromiseCreateResolveFunction": "promise_create_resolve_function",
    "PromiseCreateRejectFunction": "promise_create_reject_function",
    "PromiseThen": "promise_then", "PromiseCatch": "promise_catch",
    "PromiseFinally": "promise_finally", "PromiseAll": "promise_all",
    "PromiseRace": "promise_race", "PromiseAllSettled": "promise_all_settled",
    "PromiseAny": "promise_any", "PromiseResolveStatic": "promise_resolve_static",
    "PromiseRejectStatic": "promise_reject_static",
    "IsPromise": "is_promise", "QueueMicrotask": "queue_microtask",
    "DrainMicrotasks": "drain_microtasks",
    "AsyncFunctionStart": "async_function_start",
    "AsyncFunctionResume": "async_function_resume",
    "AsyncFunctionSuspend": "async_function_suspend",
    "ContinuationCreate": "continuation_create",
    "ContinuationSaveVar": "continuation_save_var",
    "ContinuationLoadVar": "continuation_load_var",
    "AsyncGeneratorStart": "async_generator_start",
    "AsyncGeneratorNext": "async_generator_next",
    "AsyncGeneratorReturn": "async_generator_return",
    "AsyncGeneratorThrow": "async_generator_throw",
    "PromiseWithResolvers": "promise_with_resolvers",
    "IsCallable": "is_callable", "DynamicImport": "dynamic_import",
    "RegisterModuleNamespace": "register_module_namespace",
    "JsxCreateElement": "jsx_create_element",
    "ProxyCreate": "proxy_create", "ProxyRevocable": "proxy_revocable",
    "ReflectGet": "reflect_get", "ReflectSet": "reflect_set",
    "ReflectHas": "reflect_has",
    "ReflectDeleteProperty": "reflect_delete_property",
    "ReflectApply": "reflect_apply", "ReflectConstruct": "reflect_construct",
    "ReflectGetPrototypeOf": "reflect_get_prototype_of",
    "ReflectSetPrototypeOf": "reflect_set_prototype_of",
    "ReflectIsExtensible": "reflect_is_extensible",
    "ReflectPreventExtensions": "reflect_prevent_extensions",
    "ReflectGetOwnPropertyDescriptor": "reflect_get_own_property_descriptor",
    "ReflectDefineProperty": "reflect_define_property",
    "ReflectOwnKeys": "reflect_own_keys",
    "StringAt": "string_at", "StringCharAt": "string_char_at",
    "StringCharCodeAt": "string_char_code_at",
    "StringCodePointAt": "string_code_point_at",
    "StringConcatVa": "string_concat_proto",
    "StringEndsWith": "string_ends_with",
    "StringIncludes": "string_includes",
    "StringIndexOf": "string_index_of",
    "StringLastIndexOf": "string_last_index_of",
    "StringMatchAll": "string_match_all",
    "StringPadEnd": "string_pad_end", "StringPadStart": "string_pad_start",
    "StringRepeat": "string_repeat", "StringReplaceAll": "string_replace_all",
    "StringSlice": "string_slice", "StringStartsWith": "string_starts_with",
    "StringSubstring": "string_substring",
    "StringToLowerCase": "string_to_lower_case",
    "StringToUpperCase": "string_to_upper_case",
    "StringTrim": "string_trim", "StringTrimEnd": "string_trim_end",
    "StringTrimStart": "string_trim_start",
    "StringToString": "string_to_string", "StringValueOf": "string_value_of",
    "StringIterator": "string_iterator",
    "StringFromCharCode": "string_from_char_code",
    "StringFromCodePoint": "string_from_code_point",
    "MathAbs": "math_abs", "MathAcos": "math_acos", "MathAcosh": "math_acosh",
    "MathAsin": "math_asin", "MathAsinh": "math_asinh",
    "MathAtan": "math_atan", "MathAtanh": "math_atanh",
    "MathAtan2": "math_atan2", "MathCbrt": "math_cbrt",
    "MathCeil": "math_ceil", "MathClz32": "math_clz32",
    "MathCos": "math_cos", "MathCosh": "math_cosh",
    "MathExp": "math_exp", "MathExpm1": "math_expm1",
    "MathFloor": "math_floor", "MathFround": "math_fround",
    "MathHypot": "math_hypot", "MathImul": "math_imul",
    "MathLog": "math_log", "MathLog1p": "math_log1p",
    "MathLog10": "math_log10", "MathLog2": "math_log2",
    "MathMax": "math_max", "MathMin": "math_min",
    "MathPow": "math_pow", "MathRandom": "math_random",
    "MathRound": "math_round", "MathSign": "math_sign",
    "MathSin": "math_sin", "MathSinh": "math_sinh",
    "MathSqrt": "math_sqrt", "MathTan": "math_tan",
    "MathTanh": "math_tanh", "MathTrunc": "math_trunc",
    "NumberConstructor": "number_constructor",
    "NumberIsNaN": "number_is_nan", "NumberIsFinite": "number_is_finite",
    "NumberIsInteger": "number_is_integer",
    "NumberIsSafeInteger": "number_is_safe_integer",
    "NumberParseInt": "number_parse_int",
    "NumberParseFloat": "number_parse_float",
    "NumberProtoToString": "number_proto_to_string",
    "NumberProtoValueOf": "number_proto_value_of",
    "NumberProtoToFixed": "number_proto_to_fixed",
    "NumberProtoToExponential": "number_proto_to_exponential",
    "NumberProtoToPrecision": "number_proto_to_precision",
    "BooleanConstructor": "boolean_constructor",
    "BooleanProtoToString": "boolean_proto_to_string",
    "BooleanProtoValueOf": "boolean_proto_value_of",
    "ErrorConstructor": "error_constructor",
    "TypeErrorConstructor": "type_error_constructor",
    "RangeErrorConstructor": "range_error_constructor",
    "SyntaxErrorConstructor": "syntax_error_constructor",
    "ReferenceErrorConstructor": "reference_error_constructor",
    "UriErrorConstructor": "uri_error_constructor",
    "EvalErrorConstructor": "eval_error_constructor",
    "ErrorProtoToString": "error_proto_to_string",
    "MapConstructor": "map_constructor", "MapProtoSet": "map_proto_set",
    "MapProtoGet": "map_proto_get",
    "SetConstructor": "set_constructor", "SetProtoAdd": "set_proto_add",
    "MapSetHas": "map_set_has", "MapSetDelete": "map_set_delete",
    "MapSetClear": "map_set_clear", "MapSetGetSize": "map_set_get_size",
    "MapSetForEach": "map_set_for_each", "MapSetKeys": "map_set_keys",
    "MapSetValues": "map_set_values", "MapSetEntries": "map_set_entries",
    "DateConstructor": "date_constructor", "DateNow": "date_now",
    "DateParse": "date_parse", "DateUtc": "date_utc",
    "WeakMapConstructor": "weakmap_constructor",
    "WeakMapProtoSet": "weakmap_proto_set",
    "WeakMapProtoGet": "weakmap_proto_get",
    "WeakMapProtoHas": "weakmap_proto_has",
    "WeakMapProtoDelete": "weakmap_proto_delete",
    "WeakSetConstructor": "weakset_constructor",
    "WeakSetProtoAdd": "weakset_proto_add",
    "WeakSetProtoHas": "weakset_proto_has",
    "WeakSetProtoDelete": "weakset_proto_delete",
    "ArrayBufferConstructor": "arraybuffer_constructor",
    "ArrayBufferProtoByteLength": "arraybuffer_proto_byte_length",
    "ArrayBufferProtoSlice": "arraybuffer_proto_slice",
    "DataViewConstructor": "dataview_constructor",
    "DataViewProtoGetFloat64": "dataview_proto_get_float64",
    "DataViewProtoGetFloat32": "dataview_proto_get_float32",
    "DataViewProtoGetInt32": "dataview_proto_get_int32",
    "DataViewProtoGetUint32": "dataview_proto_get_uint32",
    "DataViewProtoGetInt16": "dataview_proto_get_int16",
    "DataViewProtoGetUint16": "dataview_proto_get_uint16",
    "DataViewProtoGetInt8": "dataview_proto_get_int8",
    "DataViewProtoGetUint8": "dataview_proto_get_uint8",
    "DataViewProtoSetFloat64": "dataview_proto_set_float64",
    "DataViewProtoSetFloat32": "dataview_proto_set_float32",
    "DataViewProtoSetInt32": "dataview_proto_set_int32",
    "DataViewProtoSetUint32": "dataview_proto_set_uint32",
    "DataViewProtoSetInt16": "dataview_proto_set_int16",
    "DataViewProtoSetUint16": "dataview_proto_set_uint16",
    "DataViewProtoSetInt8": "dataview_proto_set_int8",
    "DataViewProtoSetUint8": "dataview_proto_set_uint8",
    "Int8ArrayConstructor": "int8array_constructor",
    "Uint8ArrayConstructor": "uint8array_constructor",
    "Uint8ClampedArrayConstructor": "uint8clampedarray_constructor",
    "Int16ArrayConstructor": "int16array_constructor",
    "Uint16ArrayConstructor": "uint16array_constructor",
    "Int32ArrayConstructor": "int32array_constructor",
    "Uint32ArrayConstructor": "uint32array_constructor",
    "Float32ArrayConstructor": "float32array_constructor",
    "Float64ArrayConstructor": "float64array_constructor",
    "TypedArrayProtoLength": "typedarray_proto_length",
    "TypedArrayProtoByteLength": "typedarray_proto_byte_length",
    "TypedArrayProtoByteOffset": "typedarray_proto_byte_offset",
    "TypedArrayProtoSet": "typedarray_proto_set",
    "TypedArrayProtoSlice": "typedarray_proto_slice",
    "TypedArrayProtoSubarray": "typedarray_proto_subarray",
    "BigInt64ArrayConstructor": "bigint64array_constructor",
    "BigUint64ArrayConstructor": "biguint64array_constructor",
    "TypedArrayProtoFill": "typedarray_proto_fill",
    "TypedArrayProtoReverse": "typedarray_proto_reverse",
    "TypedArrayProtoIndexOf": "typedarray_proto_index_of",
    "TypedArrayProtoLastIndexOf": "typedarray_proto_last_index_of",
    "TypedArrayProtoIncludes": "typedarray_proto_includes",
    "TypedArrayProtoJoin": "typedarray_proto_join",
    "TypedArrayProtoToString": "typedarray_proto_to_string",
    "TypedArrayProtoCopyWithin": "typedarray_proto_copy_within",
    "TypedArrayProtoAt": "typedarray_proto_at",
    "TypedArrayProtoForEach": "typedarray_proto_for_each",
    "TypedArrayProtoMap": "typedarray_proto_map",
    "TypedArrayProtoFilter": "typedarray_proto_filter",
    "TypedArrayProtoReduce": "typedarray_proto_reduce",
    "TypedArrayProtoReduceRight": "typedarray_proto_reduce_right",
    "TypedArrayProtoFind": "typedarray_proto_find",
    "TypedArrayProtoFindIndex": "typedarray_proto_find_index",
    "TypedArrayProtoSome": "typedarray_proto_some",
    "TypedArrayProtoEvery": "typedarray_proto_every",
    "TypedArrayProtoSort": "typedarray_proto_sort",
    "TypedArrayProtoEntries": "typedarray_proto_entries",
    "TypedArrayProtoKeys": "typedarray_proto_keys",
    "TypedArrayProtoValues": "typedarray_proto_values",
    "GetBuiltinGlobal": "get_builtin_global",
    "CreateGlobalObject": "create_global_object",
    "CreateException": "create_exception",
    "ExceptionValue": "exception_value",
    "NewTarget": "new_target",
    "CreateUnmappedArgumentsObject": "create_unmapped_arguments_object",
    "CreateMappedArgumentsObject": "create_mapped_arguments_object",
    "ScopeRecordCreate": "scope_record_create",
    "ScopeRecordAddBinding": "scope_record_add_binding",
    "EvalGetBinding": "eval_get_binding",
    "EvalSetBinding": "eval_set_binding",
    "EvalHasBinding": "eval_has_binding",
    "EvalSuperBase": "eval_super_base",
    "ScopeRecordSetMeta": "scope_record_set_meta",
    "ScopeRecordDestroy": "scope_record_destroy",
    "AtomicsLoad": "atomics_load", "AtomicsStore": "atomics_store",
    "AtomicsAdd": "atomics_add", "AtomicsSub": "atomics_sub",
    "AtomicsAnd": "atomics_and", "AtomicsOr": "atomics_or",
    "AtomicsXor": "atomics_xor", "AtomicsExchange": "atomics_exchange",
    "AtomicsCompareExchange": "atomics_compare_exchange",
    "AtomicsIsLockFree": "atomics_is_lock_free",
    "AtomicsWait": "atomics_wait", "AtomicsNotify": "atomics_notify",
    "AtomicsWaitAsync": "atomics_wait_async",
    "WeakRefConstructor": "weakref_constructor",
    "WeakRefProtoDeref": "weakref_proto_deref",
    "FinalizationRegistryConstructor": "finalization_registry_constructor",
    "FinalizationRegistryProtoRegister": "finalization_registry_proto_register",
    "FinalizationRegistryProtoUnregister": "finalization_registry_proto_unregister",
    "SharedArrayBufferConstructor": "sharedarraybuffer_constructor",
    "SharedArrayBufferProtoByteLength": "sharedarraybuffer_proto_byte_length",
    "SharedArrayBufferProtoSlice": "sharedarraybuffer_proto_slice",
    "SharedArrayBufferProtoSpecies": "sharedarraybuffer_proto_species",
    "IsException": "is_exception",
    "GetPrototypeFromConstructor": "get_prototype_from_constructor",
}

# Build reverse map: import name -> first Builtin variant name
# Only include builtins whose import names actually exist in the import section.
import_name_to_builtin = {}
for name, import_name in BUILTIN_TO_NAME.items():
    # Skip builtins that don't have a real import (e.g. IsException, GetPrototypeFromConstructor)
    if import_name not in name_to_type:
        print(f"  (skipping builtin {name} -> '{import_name}': no import entry)")
        continue
    if import_name not in import_name_to_builtin:
        import_name_to_builtin[import_name] = name

# SpecialHostImport variants (from plan)
SPECIAL_IMPORTS = {
    "string_concat": "StringConcat",
    "string_concat_va": "StringConcatVa",
    "gc_collect": "GcCollect",
    "native_call": "NativeCall",
    "obj_spread": "ObjSpread",
    "proxy_trap_get": "ProxyTrapGet",
    "proxy_trap_set": "ProxyTrapSet",
    "proxy_trap_delete": "ProxyTrapDelete",
    "symbol_property_key": "SymbolPropertyKey",
    "array.from": "ArrayFrom",
    "obj_get_by_index": "ObjGetByIndex",
    "typedarray_set_by_index": "TypedArraySetByIndex",
}

# Array prototype method group (27 entries from HOST_IMPORT_NAMES indices 50-76)
ARR_PROTO_METHODS = {
    "arr_proto_push", "arr_proto_pop", "arr_proto_includes",
    "arr_proto_index_of", "arr_proto_join", "arr_proto_concat",
    "arr_proto_slice", "arr_proto_fill", "arr_proto_reverse",
    "arr_proto_flat", "arr_proto_shift", "arr_proto_unshift",
    "arr_proto_sort", "arr_proto_at", "arr_proto_copy_within",
    "arr_proto_for_each", "arr_proto_map", "arr_proto_filter",
    "arr_proto_reduce", "arr_proto_reduce_right", "arr_proto_find",
    "arr_proto_find_index", "arr_proto_some", "arr_proto_every",
    "arr_proto_flat_map", "arr_proto_splice", "arr_proto_is_array",
}

print(f"Arr proto methods count: {len(ARR_PROTO_METHODS)}")

# Build the registry file
spec_lines = []
for idx, name in enumerate(names):
    type_idx = name_to_type.get(name, 0)

    # Determine key: Builtin takes priority over Special
    key = "None"
    if name in import_name_to_builtin:
        key = f"Some(HostImportKey::Builtin(Builtin::{import_name_to_builtin[name]}))"
    elif name in SPECIAL_IMPORTS:
        key = f"Some(HostImportKey::Special(SpecialHostImport::{SPECIAL_IMPORTS[name]}))"

    # Determine group
    group = "Some(HostImportGroup::ArrayPrototypeMethod)" if name in ARR_PROTO_METHODS else "None"

    spec_lines.append(f"    HostImportSpec {{ name: \"{name}\", type_idx: {type_idx}, key: {key}, group: {group} }},")

# Check count
spec_count = len(spec_lines)
print(f"First pass: {spec_count} specs")
print(f"Names count: {len(names)}, unique names: {len(set(names))}")
if len(names) != len(set(names)):
    from collections import Counter
    for k, v in Counter(names).items():
        if v > 1:
            print(f"  DUPLICATE name: '{k}' appears {v} times")
assert len(spec_lines) == 387, f"Expected 387 specs, got {len(spec_lines)} (names={len(names)})"

# Find the correct type for each missing import from the source text
def find_type_in_source(source, name):
    """Find EntityType::Function(N) for a given import name in source."""
    # Try multiline pattern first
    pattern = rf'imports\.import\(\s*\n\s*"env"\s*,\s*\n\s*"{re.escape(name)}"\s*,\s*\n\s*EntityType::Function\((\d+)\)'
    m = re.search(pattern, source)
    if m:
        return int(m.group(1))
    # Try single line but with possible whitespace
    pattern = rf'imports\.import\(\s*"env"\s*,\s*"{re.escape(name)}"\s*,\s*EntityType::Function\((\d+)\)'
    m = re.search(pattern, source)
    if m:
        return int(m.group(1))
    return None

# Fill in missing types by searching the source directly
for n in missing:
    type_idx = find_type_in_source(core, n)
    if type_idx:
        name_to_type[n] = type_idx
        print(f"Found missing type for '{n}': {type_idx}")
    else:
        print(f"Could NOT find type for '{n}'")

# Rebuild spec lines with correct types
spec_lines = []
for idx, name in enumerate(names):
    type_idx = name_to_type.get(name, 0)
    if type_idx == 0 and name not in name_to_type:
        print(f"ERROR: type_idx=0 for '{name}' at index {idx} (not in name_to_type)")
    elif type_idx == 0 and name in name_to_type:
        pass  # real type 0 is valid

    key = "None"
    if name in import_name_to_builtin:
        key = f"Some(HostImportKey::Builtin(Builtin::{import_name_to_builtin[name]}))"
    elif name in SPECIAL_IMPORTS:
        key = f"Some(HostImportKey::Special(SpecialHostImport::{SPECIAL_IMPORTS[name]}))"

    group = "Some(HostImportGroup::ArrayPrototypeMethod)" if name in ARR_PROTO_METHODS else "None"

    spec_lines.append(f"    HostImportSpec {{ name: \"{name}\", type_idx: {type_idx}, key: {key}, group: {group} }},")

assert len(spec_lines) == 387, f"Expected 387 specs, got {len(spec_lines)}"

# Write the file
output = []
output.append("//! Canonical owner of all host import definitions.")
output.append("//!")
output.append("//! This module is the single source of truth for host import names,")
output.append("//! WASM function type indices, Builtin bindings, special index requirements,")
output.append("//! and grouping. All other modules derive their knowledge from this registry.")
output.append("//!")
output.append("//! Modifying host imports: add/remove/reorder entries here, then")
output.append("//! update the corresponding runtime host function implementations.")
output.append("")
output.append("use wjsm_ir::Builtin;")
output.append("")
output.append("#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]")
output.append("pub(crate) enum SpecialHostImport {")
for var in sorted(set(SPECIAL_IMPORTS.values())):
    output.append(f"    {var},")
output.append("}")
output.append("")
output.append("#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]")
output.append("pub(crate) enum HostImportGroup {")
output.append("    ArrayPrototypeMethod,")
output.append("}")
output.append("")
output.append("#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]")
output.append("pub enum HostImportKey {")
output.append("    Builtin(Builtin),")
output.append("    Special(SpecialHostImport),")
output.append("}")
output.append("")
output.append("#[derive(Debug, Clone, Copy)]")
output.append("pub struct HostImportSpec {")
output.append("    pub name: &'static str,")
output.append("    pub type_idx: u32,")
output.append("    pub key: Option<HostImportKey>,")
output.append("    pub group: Option<HostImportGroup>,")
output.append("}")
output.append("")
output.append("static HOST_IMPORT_SPECS: &[HostImportSpec] = &[")
for line in spec_lines:
    output.append(line)
output.append("];")
output.append("")
output.append("pub fn host_import_specs() -> &'static [HostImportSpec] {")
output.append("    HOST_IMPORT_SPECS")
output.append("}")

result = "\n".join(output) + "\n"
with open("crates/wjsm-backend-wasm/src/host_import_registry.rs", "w") as f:
    f.write(result)

# Verify
spec_count = result.count("HostImportSpec {")
print(f"Generated {spec_count} specs (expected 387)")
all_types_ok = all(n in name_to_type for n in names)
print(f"All types filled: {all_types_ok}")
if not all_types_ok:
    still_missing = [n for n in names if n not in name_to_type]
    print(f"Still missing types for: {still_missing}")
