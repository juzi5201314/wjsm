use wjsm_ir::value::{TAG_ARRAY, decode_handle, encode_handle, encode_object_handle};
use wjsm_runtime::realm::{MicrotaskMode, Realm, RealmId, RealmIntrinsics};
use wjsm_runtime::{
    HandleGeneration, HandleMap, HandleTableV2, ManagedHeapLayout, V2ConditionalRoots,
    remap_realm_handles_v2,
};

const PAGE_BYTES: u64 = 64 * 1024;

fn handle_table() -> (HandleTableV2, ManagedHeapLayout) {
    let layout = ManagedHeapLayout::new(PAGE_BYTES * 4, PAGE_BYTES).unwrap();
    (HandleTableV2::new(layout.clone()).unwrap(), layout)
}

fn publish(table: &HandleTableV2, address: u64) -> wjsm_runtime::HandleId {
    let handle = table.allocate_handle().unwrap();
    table
        .publish(handle, address, HandleGeneration::Young)
        .unwrap();
    handle
}

#[test]
fn realm_clone_v2_remaps_shared_handles_and_preserves_policy() {
    let (handles, layout) = handle_table();
    let source_handle = publish(&handles, layout.object_heap_base());
    let cloned_handle = publish(&handles, layout.object_heap_base() + 8);
    let mut intrinsics = RealmIntrinsics::empty();
    intrinsics.object_proto = encode_object_handle(source_handle.get());
    intrinsics.array_proto = encode_handle(TAG_ARRAY, source_handle.get());
    intrinsics.typedarray_prototypes[0] = encode_object_handle(source_handle.get());
    let mut source = Realm::new(
        RealmId(0),
        encode_object_handle(source_handle.get()),
        intrinsics,
    );
    source.code_generation.strings = false;
    source.microtask_mode = MicrotaskMode::AfterEvaluate;
    let mut handle_map = HandleMap::new();
    handle_map.insert(source_handle.get(), cloned_handle.get());

    let cloned = remap_realm_handles_v2(&source, RealmId(1), &handle_map, &handles).unwrap();

    assert_eq!(decode_handle(cloned.global_object), cloned_handle.get());
    assert_eq!(
        decode_handle(cloned.intrinsics.object_proto),
        cloned_handle.get()
    );
    assert_eq!(
        decode_handle(cloned.intrinsics.array_proto),
        cloned_handle.get()
    );
    assert_eq!(
        decode_handle(cloned.intrinsics.typedarray_prototypes[0]),
        cloned_handle.get()
    );
    assert!(!cloned.code_generation.strings);
    assert_eq!(cloned.microtask_mode, MicrotaskMode::AfterEvaluate);
}

#[test]
fn vm_gc_realm_roots_v2_only_keep_live_conditional_handles() {
    let (handles, layout) = handle_table();
    let live = publish(&handles, layout.object_heap_base());
    let stale = publish(&handles, layout.object_heap_base() + 8);
    let mut intrinsics = RealmIntrinsics::empty();
    intrinsics.object_proto = encode_object_handle(stale.get());
    let realm = Realm::new(RealmId(1), encode_object_handle(live.get()), intrinsics);
    handles.retire(stale).unwrap();
    let mut roots = V2ConditionalRoots::default();
    roots.push_realm(realm);

    let resolved = roots.collect(&handles);

    assert_eq!(resolved, std::collections::BTreeSet::from([live]));
}

#[test]
fn side_table_gc_v2_filters_dangling_promise_stream_proxy_async_handles() {
    let (handles, layout) = handle_table();
    let live = publish(&handles, layout.object_heap_base());
    let stale = publish(&handles, layout.object_heap_base() + 8);
    handles.retire(stale).unwrap();
    let mut roots = V2ConditionalRoots::default();
    roots.extend_promise_values([encode_object_handle(live.get())]);
    roots.extend_stream_values([encode_object_handle(stale.get())]);
    roots.extend_proxy_values([encode_handle(TAG_ARRAY, live.get())]);
    roots.extend_async_values([encode_handle(TAG_ARRAY, stale.get())]);

    let resolved = roots.collect(&handles);

    assert_eq!(resolved, std::collections::BTreeSet::from([live]));
}
