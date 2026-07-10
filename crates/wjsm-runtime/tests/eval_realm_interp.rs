//! Task 2.1/2.2：执行帧内数组分配使用 realm array_proto
//!（interp `eval_array_lit` / compiled `arr_new` / ArrayConstructor 同源）。

use tokio::runtime::Builder;
use wjsm_runtime::probe_eval_array_literal_in_realm;

#[test]
fn array_alloc_in_realm_frame_uses_realm_array_proto() {
    let rt = Builder::new_current_thread().enable_all().build().unwrap();
    let p = rt
        .block_on(probe_eval_array_literal_in_realm())
        .expect("probe");

    assert_ne!(
        p.realm_array_proto, p.main_array_proto,
        "cloned realm array_proto 必须不同于主 realm"
    );
    assert_eq!(
        p.result_proto, p.realm_array_proto,
        "执行帧内分配的数组 [[Prototype]] 必须是该 realm 的 array_proto"
    );
}
