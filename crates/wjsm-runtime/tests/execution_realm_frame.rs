//! Task 2.0：execution_realm 帧 + array/object proto global swap。

use tokio::runtime::Builder;
use wjsm_runtime::probe_execution_realm_frame;

#[test]
fn enter_cloned_realm_swaps_proto_globals_and_restores() {
    let rt = Builder::new_current_thread().enable_all().build().unwrap();
    let p = rt
        .block_on(probe_execution_realm_frame())
        .expect("probe frame");

    assert!(p.main_array >= 0 && p.main_object >= 0, "main protos ready");
    assert_ne!(
        p.inside_array, p.main_array,
        "enter 后 __array_proto_handle 必须切到克隆 realm"
    );
    assert_ne!(
        p.inside_object, p.main_object,
        "enter 后 __object_proto_handle 必须切到克隆 realm"
    );
    assert!(
        p.inside_execution_realm >= 1,
        "execution_realm 应为克隆 id"
    );
    assert_eq!(p.after_array, p.main_array, "exit 恢复 array proto global");
    assert_eq!(p.after_object, p.main_object, "exit 恢复 object proto global");
    assert_eq!(p.after_execution_realm, 0, "exit 恢复 execution_realm=0");
}
