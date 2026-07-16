//! pristine 可达图克隆隔离测试（Task 1.1 / 1.2）。

use tokio::runtime::Builder;
use wjsm_runtime::probe_clone_pristine_realm;

#[test]
fn clone_array_proto_differs_from_main() {
    let rt = Builder::new_current_thread().enable_all().build().unwrap();
    let probe = rt
        .block_on(probe_clone_pristine_realm())
        .expect("probe clone");

    assert_ne!(
        probe.clone_array_proto_handle, probe.main_array_proto_handle,
        "新 realm array_proto 必须是新 handle"
    );
    assert_ne!(
        probe.clone_object_proto_handle, probe.main_object_proto_handle,
        "新 realm object_proto 必须是新 handle"
    );
    assert!(probe.realm_id >= 1, "克隆 realm id 从 1 起");
    assert!(probe.closure_size >= 2, "闭包至少含 object/array proto");
}

#[test]
fn clone_array_proto_chain_points_to_clone_object_proto() {
    let rt = Builder::new_current_thread().enable_all().build().unwrap();
    let probe = rt
        .block_on(probe_clone_pristine_realm())
        .expect("probe clone");

    assert_eq!(
        probe.clone_array_proto_of, probe.clone_object_proto_handle,
        "新 array_proto.[[Prototype]] 必须指向新 object_proto，而非主 realm"
    );
    assert_ne!(
        probe.clone_array_proto_of, probe.main_object_proto_handle,
        "不得仍挂主 realm object_proto"
    );
}

#[test]
fn primordial_closure_covers_roots_and_is_closed() {
    let rt = Builder::new_current_thread().enable_all().build().unwrap();
    let probe = rt
        .block_on(probe_clone_pristine_realm())
        .expect("probe clone");

    assert!(
        probe.roots_covered,
        "闭包必须包含全部有效 RealmIntrinsics 根"
    );
    assert!(
        probe.closure_closed,
        "闭包内对象的子 object/array handle 必须都在闭包内（无悬挂）"
    );
}
