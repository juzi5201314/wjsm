//! Task 4.1/4.2：条件 per-realm GC roots + 死 realm 回收（集成探针）。

use tokio::runtime::Builder;
use wjsm_runtime::probe_clone_pristine_realm;

/// 克隆成功后至少有一个非 0 realm；闭包非空（intrinsic 可被条件 root）。
#[test]
fn clone_registers_non_main_realm_with_closure() {
    let rt = Builder::new_current_thread().enable_all().build().unwrap();
    let probe = rt
        .block_on(probe_clone_pristine_realm())
        .expect("probe clone");
    assert!(probe.realm_id >= 1);
    assert!(probe.closure_size >= 2);
    assert!(probe.closure_closed);
    assert!(probe.roots_covered);
}
