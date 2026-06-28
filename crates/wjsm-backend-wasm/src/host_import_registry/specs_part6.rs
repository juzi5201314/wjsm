//! 宿主导入注册表数据（第 6/6 部分）。
//!
//! 按位置拆分，条目顺序不可更改（WASM 函数索引依赖位置）。

use super::{HostImportKey, HostImportSpec};
use super::SpecialHostImport;
use wjsm_ir::Builtin;

/// 第 6 部分：`typedarray_proto_find` ~ `gc_take_freed_handle`
pub(crate) static SPECS_PART6: &[HostImportSpec] = &[
    HostImportSpec {
        name: "typedarray_proto_find",
        type_idx: 12,
        key: Some(HostImportKey::Builtin(Builtin::TypedArrayProtoFind)),
        group: None,
    },
    HostImportSpec {
        name: "typedarray_proto_find_index",
        type_idx: 12,
        key: Some(HostImportKey::Builtin(Builtin::TypedArrayProtoFindIndex)),
        group: None,
    },
    HostImportSpec {
        name: "typedarray_proto_some",
        type_idx: 12,
        key: Some(HostImportKey::Builtin(Builtin::TypedArrayProtoSome)),
        group: None,
    },
    HostImportSpec {
        name: "typedarray_proto_every",
        type_idx: 12,
        key: Some(HostImportKey::Builtin(Builtin::TypedArrayProtoEvery)),
        group: None,
    },
    HostImportSpec {
        name: "typedarray_proto_sort",
        type_idx: 12,
        key: Some(HostImportKey::Builtin(Builtin::TypedArrayProtoSort)),
        group: None,
    },
    HostImportSpec {
        name: "typedarray_proto_entries",
        type_idx: 3,
        key: Some(HostImportKey::Builtin(Builtin::TypedArrayProtoEntries)),
        group: None,
    },
    HostImportSpec {
        name: "typedarray_proto_keys",
        type_idx: 3,
        key: Some(HostImportKey::Builtin(Builtin::TypedArrayProtoKeys)),
        group: None,
    },
    HostImportSpec {
        name: "typedarray_proto_values",
        type_idx: 3,
        key: Some(HostImportKey::Builtin(Builtin::TypedArrayProtoValues)),
        group: None,
    },
    HostImportSpec {
        name: "scope_record_create",
        type_idx: 3,
        key: Some(HostImportKey::Builtin(Builtin::ScopeRecordCreate)),
        group: None,
    },
    HostImportSpec {
        name: "scope_record_add_binding",
        type_idx: 34,
        key: Some(HostImportKey::Builtin(Builtin::ScopeRecordAddBinding)),
        group: None,
    },
    HostImportSpec {
        name: "eval_get_binding",
        type_idx: 2,
        key: Some(HostImportKey::Builtin(Builtin::EvalGetBinding)),
        group: None,
    },
    HostImportSpec {
        name: "eval_set_binding",
        type_idx: 16,
        key: Some(HostImportKey::Builtin(Builtin::EvalSetBinding)),
        group: None,
    },
    HostImportSpec {
        name: "eval_has_binding",
        type_idx: 2,
        key: Some(HostImportKey::Builtin(Builtin::EvalHasBinding)),
        group: None,
    },
    HostImportSpec {
        name: "eval_super_base",
        type_idx: 3,
        key: Some(HostImportKey::Builtin(Builtin::EvalSuperBase)),
        group: None,
    },
    HostImportSpec {
        name: "scope_record_set_meta",
        type_idx: 16,
        key: Some(HostImportKey::Builtin(Builtin::ScopeRecordSetMeta)),
        group: None,
    },
    HostImportSpec {
        name: "scope_record_destroy",
        type_idx: 0,
        key: Some(HostImportKey::Builtin(Builtin::ScopeRecordDestroy)),
        group: None,
    },
    HostImportSpec {
        name: "weakref_constructor",
        type_idx: 12,
        key: Some(HostImportKey::Builtin(Builtin::WeakRefConstructor)),
        group: None,
    },
    HostImportSpec {
        name: "headers_constructor",
        type_idx: 12,
        key: Some(HostImportKey::Builtin(Builtin::HeadersConstructor)),
        group: None,
    },
    HostImportSpec {
        name: "request_constructor",
        type_idx: 12,
        key: Some(HostImportKey::Builtin(Builtin::RequestConstructor)),
        group: None,
    },
    HostImportSpec {
        name: "response_constructor",
        type_idx: 12,
        key: Some(HostImportKey::Builtin(Builtin::ResponseConstructor)),
        group: None,
    },
    HostImportSpec {
        name: "abort_controller_constructor",
        type_idx: 12,
        key: Some(HostImportKey::Builtin(Builtin::AbortControllerConstructor)),
        group: None,
    },
    HostImportSpec {
        name: "readable_stream_constructor",
        type_idx: 12,
        key: Some(HostImportKey::Builtin(Builtin::ReadableStreamConstructor)),
        group: None,
    },
    HostImportSpec {
        name: "weakref_proto_deref",
        type_idx: 3,
        key: Some(HostImportKey::Builtin(Builtin::WeakRefProtoDeref)),
        group: None,
    },
    HostImportSpec {
        name: "finalization_registry_constructor",
        type_idx: 12,
        key: Some(HostImportKey::Builtin(
            Builtin::FinalizationRegistryConstructor,
        )),
        group: None,
    },
    HostImportSpec {
        name: "finalization_registry_proto_register",
        type_idx: 12,
        key: Some(HostImportKey::Builtin(
            Builtin::FinalizationRegistryProtoRegister,
        )),
        group: None,
    },
    HostImportSpec {
        name: "finalization_registry_proto_unregister",
        type_idx: 2,
        key: Some(HostImportKey::Builtin(
            Builtin::FinalizationRegistryProtoUnregister,
        )),
        group: None,
    },
    HostImportSpec {
        name: "sharedarraybuffer_constructor",
        type_idx: 16,
        key: Some(HostImportKey::Builtin(
            Builtin::SharedArrayBufferConstructor,
        )),
        group: None,
    },
    HostImportSpec {
        name: "sharedarraybuffer_proto_byte_length",
        type_idx: 3,
        key: Some(HostImportKey::Builtin(
            Builtin::SharedArrayBufferProtoByteLength,
        )),
        group: None,
    },
    HostImportSpec {
        name: "sharedarraybuffer_proto_grow",
        type_idx: 2,
        key: Some(HostImportKey::Builtin(Builtin::SharedArrayBufferProtoGrow)),
        group: None,
    },
    HostImportSpec {
        name: "sharedarraybuffer_proto_growable",
        type_idx: 3,
        key: Some(HostImportKey::Builtin(
            Builtin::SharedArrayBufferProtoGrowable,
        )),
        group: None,
    },
    HostImportSpec {
        name: "sharedarraybuffer_proto_max_byte_length",
        type_idx: 3,
        key: Some(HostImportKey::Builtin(
            Builtin::SharedArrayBufferProtoMaxByteLength,
        )),
        group: None,
    },
    HostImportSpec {
        name: "sharedarraybuffer_proto_slice",
        type_idx: 16,
        key: Some(HostImportKey::Builtin(Builtin::SharedArrayBufferProtoSlice)),
        group: None,
    },
    HostImportSpec {
        name: "sharedarraybuffer_proto_species",
        type_idx: 3,
        key: Some(HostImportKey::Builtin(Builtin::SharedArrayBufferSpecies)),
        group: None,
    },
    HostImportSpec {
        name: "atomics_load",
        type_idx: 16,
        key: Some(HostImportKey::Builtin(Builtin::AtomicsLoad)),
        group: None,
    },
    HostImportSpec {
        name: "atomics_store",
        type_idx: 16,
        key: Some(HostImportKey::Builtin(Builtin::AtomicsStore)),
        group: None,
    },
    HostImportSpec {
        name: "atomics_add",
        type_idx: 16,
        key: Some(HostImportKey::Builtin(Builtin::AtomicsAdd)),
        group: None,
    },
    HostImportSpec {
        name: "atomics_sub",
        type_idx: 16,
        key: Some(HostImportKey::Builtin(Builtin::AtomicsSub)),
        group: None,
    },
    HostImportSpec {
        name: "atomics_and",
        type_idx: 16,
        key: Some(HostImportKey::Builtin(Builtin::AtomicsAnd)),
        group: None,
    },
    HostImportSpec {
        name: "atomics_or",
        type_idx: 16,
        key: Some(HostImportKey::Builtin(Builtin::AtomicsOr)),
        group: None,
    },
    HostImportSpec {
        name: "atomics_xor",
        type_idx: 16,
        key: Some(HostImportKey::Builtin(Builtin::AtomicsXor)),
        group: None,
    },
    HostImportSpec {
        name: "atomics_exchange",
        type_idx: 16,
        key: Some(HostImportKey::Builtin(Builtin::AtomicsExchange)),
        group: None,
    },
    HostImportSpec {
        name: "atomics_compare_exchange",
        type_idx: 17,
        key: Some(HostImportKey::Builtin(Builtin::AtomicsCompareExchange)),
        group: None,
    },
    HostImportSpec {
        name: "atomics_is_lock_free",
        type_idx: 3,
        key: Some(HostImportKey::Builtin(Builtin::AtomicsIsLockFree)),
        group: None,
    },
    HostImportSpec {
        name: "atomics_pause",
        type_idx: 4,
        key: Some(HostImportKey::Builtin(Builtin::AtomicsPause)),
        group: None,
    },
    HostImportSpec {
        name: "atomics_wait",
        type_idx: 17,
        key: Some(HostImportKey::Builtin(Builtin::AtomicsWait)),
        group: None,
    },
    HostImportSpec {
        name: "atomics_notify",
        type_idx: 16,
        key: Some(HostImportKey::Builtin(Builtin::AtomicsNotify)),
        group: None,
    },
    HostImportSpec {
        name: "atomics_wait_async",
        type_idx: 17,
        key: Some(HostImportKey::Builtin(Builtin::AtomicsWaitAsync)),
        group: None,
    },
    HostImportSpec {
        name: "async_iterator_from",
        type_idx: 3,
        key: Some(HostImportKey::Builtin(Builtin::AsyncIteratorFrom)),
        group: None,
    },
    HostImportSpec {
        name: "object.group_by",
        type_idx: 2,
        key: Some(HostImportKey::Builtin(Builtin::ObjectGroupBy)),
        group: None,
    },
    HostImportSpec {
        name: "map.group_by",
        type_idx: 2,
        key: Some(HostImportKey::Builtin(Builtin::MapGroupBy)),
        group: None,
    },
    HostImportSpec {
        name: "native_callable_get_property",
        type_idx: 8,
        key: Some(HostImportKey::Special(
            SpecialHostImport::NativeCallableGetProperty,
        )),
        group: None,
    },
    HostImportSpec {
        name: "primitive_bigint_get_method",
        type_idx: 8,
        key: Some(HostImportKey::Special(
            SpecialHostImport::PrimitiveBigIntGetMethod,
        )),
        group: None,
    },

    HostImportSpec {
        name: "primitive_number_get_method",
        type_idx: 8,
        key: Some(HostImportKey::Special(
            SpecialHostImport::PrimitiveNumberGetMethod,
        )),
        group: None,
    },
    HostImportSpec {
        name: "primitive_symbol_get_property",
        type_idx: 8,
        key: Some(HostImportKey::Special(
            SpecialHostImport::PrimitiveSymbolGetProperty,
        )),
        group: None,
    },
    HostImportSpec {
        name: "primitive_regexp_get_property",
        type_idx: 8,
        key: Some(HostImportKey::Special(
            SpecialHostImport::PrimitiveRegExpGetProperty,
        )),
        group: None,
    },
    HostImportSpec {
        name: "primitive_regexp_set_property",
        type_idx: 9,
        key: Some(HostImportKey::Special(
            SpecialHostImport::PrimitiveRegExpSetProperty,
        )),
        group: None,
    },
    HostImportSpec {
        name: "symbol_property_key",
        type_idx: 10,
        key: Some(HostImportKey::Special(SpecialHostImport::SymbolPropertyKey)),
        group: None,
    },
    HostImportSpec {
        name: "string_to_array_index",
        type_idx: 10,
        key: Some(HostImportKey::Special(
            SpecialHostImport::StringToArrayIndex,
        )),
        group: None,
    },
    HostImportSpec {
        name: "array.from",
        type_idx: 12,
        key: Some(HostImportKey::Builtin(Builtin::ArrayFrom)),
        group: None,
    },
    HostImportSpec {
        name: "obj_get_by_index",
        type_idx: 8,
        key: Some(HostImportKey::Special(SpecialHostImport::ObjGetByIndex)),
        group: None,
    },
    HostImportSpec {
        name: "typedarray_set_by_index",
        type_idx: 32,
        key: Some(HostImportKey::Special(
            SpecialHostImport::TypedArraySetByIndex,
        )),
        group: None,
    },
    HostImportSpec {
        name: "object.is_extensible",
        type_idx: 3,
        key: Some(HostImportKey::Builtin(Builtin::ObjectIsExtensible)),
        group: None,
    },
    HostImportSpec {
        name: "object.prevent_extensions",
        type_idx: 3,
        key: Some(HostImportKey::Builtin(Builtin::ObjectPreventExtensions)),
        group: None,
    },
    HostImportSpec {
        name: "proxy.apply",
        type_idx: 12,
        key: Some(HostImportKey::Special(SpecialHostImport::ProxyApply)),
        group: None,
    },
    HostImportSpec {
        name: "proxy.construct",
        type_idx: 12,
        key: Some(HostImportKey::Special(SpecialHostImport::ProxyConstruct)),
        group: None,
    },
    HostImportSpec {
        name: "writable_stream_constructor",
        type_idx: 12,
        key: Some(HostImportKey::Builtin(Builtin::WritableStreamConstructor)),
        group: None,
    },
    HostImportSpec {
        name: "transform_stream_constructor",
        type_idx: 12,
        key: Some(HostImportKey::Builtin(Builtin::TransformStreamConstructor)),
        group: None,
    },
    HostImportSpec {
        name: "count_queuing_strategy_constructor",
        type_idx: 12,
        key: Some(HostImportKey::Builtin(
            Builtin::CountQueuingStrategyConstructor,
        )),
        group: None,
    },
    HostImportSpec {
        name: "byte_length_queuing_strategy_constructor",
        type_idx: 12,
        key: Some(HostImportKey::Builtin(
            Builtin::ByteLengthQueuingStrategyConstructor,
        )),
        group: None,
    },
    HostImportSpec {
        name: "gc_alloc_slow",
        type_idx: 35,
        key: Some(HostImportKey::Special(SpecialHostImport::GcAllocSlow)),
        group: None,
    },
    HostImportSpec {
        name: "gc_maybe_collect",
        type_idx: 1,
        key: Some(HostImportKey::Special(SpecialHostImport::GcMaybeCollect)),
        group: None,
    },
    HostImportSpec {
        name: "gc_take_freed_handle",
        type_idx: 36,
        key: Some(HostImportKey::Special(SpecialHostImport::GcTakeFreedHandle)),
        group: None,
    },
    HostImportSpec {
        name: "to_number",
        type_idx: 3,
        key: Some(HostImportKey::Special(SpecialHostImport::ToNumber)),
        group: None,
    },
    HostImportSpec {
        name: "to_bool",
        type_idx: 10,
        key: Some(HostImportKey::Special(SpecialHostImport::ToBool)),
        group: None,
    },
    HostImportSpec {
        name: "iterator_step_value",
        type_idx: 3,
        key: Some(HostImportKey::Builtin(Builtin::IteratorStepValue)),
        group: None,
    },
    HostImportSpec {
        name: "object.has_own",
        type_idx: 2,
        key: Some(HostImportKey::Builtin(Builtin::ObjectHasOwn)),
        group: None,
    },
    HostImportSpec {
        name: "object.freeze",
        type_idx: 3,
        key: Some(HostImportKey::Builtin(Builtin::ObjectFreeze)),
        group: None,
    },
    HostImportSpec {
        name: "object.seal",
        type_idx: 3,
        key: Some(HostImportKey::Builtin(Builtin::ObjectSeal)),
        group: None,
    },
    HostImportSpec {
        name: "object.is_frozen",
        type_idx: 3,
        key: Some(HostImportKey::Builtin(Builtin::ObjectIsFrozen)),
        group: None,
    },
    HostImportSpec {
        name: "array_set_length",
        type_idx: 2,
        key: Some(HostImportKey::Special(SpecialHostImport::ArraySetLength)),
        group: None,
    },
    HostImportSpec {
        name: "array_named_get",
        type_idx: 8,
        key: Some(HostImportKey::Special(SpecialHostImport::ArrayNamedGet)),
        group: None,
    },
    HostImportSpec {
        name: "array_named_set",
        type_idx: 9,
        key: Some(HostImportKey::Special(SpecialHostImport::ArrayNamedSet)),
        group: None,
    },
    HostImportSpec {
        name: "object.is_sealed",
        type_idx: 3,
        key: Some(HostImportKey::Builtin(Builtin::ObjectIsSealed)),
        group: None,
    },
];
