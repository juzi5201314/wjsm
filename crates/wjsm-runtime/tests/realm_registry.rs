//! 主 realm 登记为 active_realms[0]；RealmId 可序；intrinsics empty 全为 undefined。
use wjsm_runtime::realm::{Realm, RealmId, RealmIntrinsics};

#[test]
fn realm_id_is_monotonic() {
    assert!(RealmId(1) > RealmId(0));
    assert!(RealmId(0) < RealmId(2));
    assert_eq!(RealmId(0), RealmId(0));
}

#[test]
fn realm_intrinsics_empty_is_all_undefined() {
    let intr = RealmIntrinsics::empty();
    assert!(
        intr.iter_roots()
            .all(|h| h == RealmIntrinsics::UNDEFINED),
        "empty() 每个 intrinsic 根必须是 undefined"
    );
}

#[test]
fn realm_carries_global_and_intrinsics() {
    let r = Realm::new(RealmId(0), 12345_i64, RealmIntrinsics::empty());
    assert_eq!(r.id, RealmId(0));
    assert_eq!(r.global_object, 12345_i64);
    assert!(r.code_generation.strings);
    assert!(r.code_generation.wasm);
}

#[test]
fn with_execution_realm_is_nested_safe() {
    use std::sync::atomic::{AtomicU32, Ordering};

    // 模拟 RuntimeState.execution_realm 的保存/恢复语义
    let execution_realm = AtomicU32::new(0);
    let outer = wjsm_runtime::realm::with_execution_realm_slot(&execution_realm, RealmId(1), || {
        assert_eq!(execution_realm.load(Ordering::Relaxed), 1);
        let inner =
            wjsm_runtime::realm::with_execution_realm_slot(&execution_realm, RealmId(2), || {
                assert_eq!(execution_realm.load(Ordering::Relaxed), 2);
                42
            });
        assert_eq!(inner, 42);
        assert_eq!(execution_realm.load(Ordering::Relaxed), 1);
        7
    });
    assert_eq!(outer, 7);
    assert_eq!(execution_realm.load(Ordering::Relaxed), 0);
}
