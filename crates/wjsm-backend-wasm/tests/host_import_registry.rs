use std::collections::HashSet;
use wjsm_backend_wasm::host_import_registry::{
    HostImportGroup, HostImportKey, array_proto_method_specs, array_proto_property_name,
    array_proto_table_hash, array_proto_table_len, host_import_specs,
};
use wjsm_ir::Builtin;

#[test]
fn registry_has_unique_names_and_keys() {
    let specs = host_import_specs();
    let mut names = HashSet::new();
    let mut builtin_keys = HashSet::new();
    let mut special_keys = HashSet::new();

    for spec in specs {
        assert!(
            names.insert(spec.name),
            "duplicate host import name: {}",
            spec.name
        );
        match spec.key {
            Some(HostImportKey::Builtin(builtin)) => {
                assert!(
                    builtin_keys.insert(builtin),
                    "duplicate builtin key: {builtin:?}"
                );
            }
            Some(HostImportKey::Special(special)) => {
                assert!(
                    special_keys.insert(special),
                    "duplicate special key: {special:?}"
                );
            }
            None => {}
        }
    }
}

#[test]
fn no_host_imports_are_unkeyed() {
    let unkeyed: Vec<_> = host_import_specs()
        .iter()
        .filter(|spec| spec.key.is_none())
        .map(|spec| spec.name)
        .collect();
    assert!(
        unkeyed.is_empty(),
        "every host import must have a HostImportKey (builtin or special); unkeyed: {unkeyed:?}"
    );
}

#[test]
fn array_prototype_group_is_explicit_not_range_based() {
    let specs = host_import_specs();
    let grouped: Vec<_> = array_proto_method_specs().map(|(_, spec)| spec).collect();
    let names: Vec<_> = grouped.iter().map(|spec| spec.name).collect();
    let properties: Vec<_> = grouped
        .iter()
        .map(|spec| array_proto_property_name(spec.name).expect("array prototype property name"))
        .collect();

    assert!(names.starts_with(&["arr_proto_push", "arr_proto_pop"]));
    assert!(names.ends_with(&["arr_proto_splice"]));
    assert_eq!(array_proto_table_len() as usize, names.len());
    assert_eq!(properties.first().map(String::as_str), Some("push"));
    assert_eq!(properties.last().map(String::as_str), Some("splice"));
    assert_ne!(array_proto_table_hash(), 0);

    let filter_count = specs
        .iter()
        .filter(|spec| spec.group == Some(HostImportGroup::ArrayPrototypeMethod))
        .count();
    assert_eq!(filter_count, names.len());
}

#[test]
fn all_specs_have_valid_type_indices() {
    let specs = host_import_specs();
    for spec in specs {
        assert!(
            spec.type_idx < 128,
            "spec '{}' has suspiciously large type_idx {}",
            spec.name,
            spec.type_idx
        );
    }
}

#[test]
fn emitted_builtin_imports_have_registry_keys() {
    let expected = [
        (Builtin::ArrayConcatVa, "arr_proto_concat"),
        (
            Builtin::PromiseCreateResolveFunction,
            "promise_create_resolve_function",
        ),
        (
            Builtin::PromiseCreateRejectFunction,
            "promise_create_reject_function",
        ),
        (
            Builtin::ReflectPreventExtensions,
            "reflect_prevent_extensions",
        ),
        (
            Builtin::ReflectGetOwnPropertyDescriptor,
            "reflect_get_own_property_descriptor",
        ),
        (Builtin::StringConcatVa, "string_proto_concat"),
        (
            Builtin::NumberProtoToExponential,
            "number_proto_to_exponential",
        ),
        (
            Builtin::ReferenceErrorConstructor,
            "reference_error_constructor",
        ),
        (
            Builtin::ArrayBufferProtoByteLength,
            "arraybuffer_proto_byte_length",
        ),
        (
            Builtin::DataViewProtoSetFloat64,
            "dataview_proto_set_float64",
        ),
        (
            Builtin::DataViewProtoSetFloat32,
            "dataview_proto_set_float32",
        ),
        (
            Builtin::Uint8ClampedArrayConstructor,
            "uint8clampedarray_constructor",
        ),
        (
            Builtin::BigUint64ArrayConstructor,
            "biguint64array_constructor",
        ),
        (
            Builtin::TypedArrayProtoByteLength,
            "typedarray_proto_byte_length",
        ),
        (
            Builtin::TypedArrayProtoByteOffset,
            "typedarray_proto_byte_offset",
        ),
        (
            Builtin::TypedArrayProtoLastIndexOf,
            "typedarray_proto_last_index_of",
        ),
        (
            Builtin::TypedArrayProtoCopyWithin,
            "typedarray_proto_copy_within",
        ),
        (
            Builtin::TypedArrayProtoReduceRight,
            "typedarray_proto_reduce_right",
        ),
        (
            Builtin::TypedArrayProtoFindIndex,
            "typedarray_proto_find_index",
        ),
        (
            Builtin::CreateUnmappedArgumentsObject,
            "create_unmapped_arguments_object",
        ),
        (
            Builtin::CreateMappedArgumentsObject,
            "create_mapped_arguments_object",
        ),
        (
            Builtin::FinalizationRegistryConstructor,
            "finalization_registry_constructor",
        ),
        (
            Builtin::FinalizationRegistryProtoRegister,
            "finalization_registry_proto_register",
        ),
        (
            Builtin::FinalizationRegistryProtoUnregister,
            "finalization_registry_proto_unregister",
        ),
        (
            Builtin::SharedArrayBufferConstructor,
            "sharedarraybuffer_constructor",
        ),
        (
            Builtin::SharedArrayBufferProtoByteLength,
            "sharedarraybuffer_proto_byte_length",
        ),
        (
            Builtin::SharedArrayBufferProtoGrow,
            "sharedarraybuffer_proto_grow",
        ),
        (
            Builtin::SharedArrayBufferProtoGrowable,
            "sharedarraybuffer_proto_growable",
        ),
        (
            Builtin::SharedArrayBufferProtoMaxByteLength,
            "sharedarraybuffer_proto_max_byte_length",
        ),
        (
            Builtin::SharedArrayBufferProtoSlice,
            "sharedarraybuffer_proto_slice",
        ),
        (
            Builtin::SharedArrayBufferSpecies,
            "sharedarraybuffer_proto_species",
        ),
    ];

    for (builtin, expected_name) in expected {
        let actual_name = host_import_specs().iter().find_map(|spec| match spec.key {
            Some(HostImportKey::Builtin(found)) if found == builtin => Some(spec.name),
            _ => None,
        });

        assert_eq!(
            actual_name,
            Some(expected_name),
            "missing or incorrect host import registry key for {builtin:?}"
        );
    }
}

#[test]
fn compiler_registry_matches_expected_import_count() {
    let module = wjsm_parser::parse_module(r#"console.log('hello');"#).expect("parse");
    let program = wjsm_semantic::lower_module(module, false).expect("lower");
    let wasm = wjsm_backend_wasm::compile(&program).expect("compile");

    let import_count = wasmparser::Parser::new(0)
        .parse_all(&wasm)
        .filter_map(|payload| match payload.expect("payload") {
            wasmparser::Payload::ImportSection(s) => Some(s.count()),
            _ => None,
        })
        .next()
        .expect("import section");

    const SHARED_ENV_IMPORTS: usize = 1 + 1 + 19; // memory + table + 共享 globals
    const SUPPORT_HELPER_IMPORTS: usize = 10; // obj_*/arr_*/elem_*/string_eq/to_int32/get_proto_from_ctor
    assert_eq!(
        import_count as usize,
        host_import_specs().len() + SHARED_ENV_IMPORTS + SUPPORT_HELPER_IMPORTS
    );
}
