//! AsyncHooksState：bootstrap ids、new_async_id 单调、push/pop 栈恢复。
use wjsm_runtime::runtime_async_hooks::{AsyncHooksState, CapturedScope, FrameId};

#[test]
fn bootstrap_ids_match_node_v24() {
    let st = AsyncHooksState::bootstrap();
    assert_eq!(st.execution_async_id(), 1);
    assert_eq!(st.trigger_async_id(), 0);
    // Node：先 ++ 再返回；bootstrap 后下一 id 为 2
    assert_eq!(st.peek_next_async_id(), 2);
}

#[test]
fn new_async_id_is_monotonic() {
    let mut st = AsyncHooksState::bootstrap();
    let a = st.new_async_id();
    let b = st.new_async_id();
    let c = st.new_async_id();
    assert_eq!(a, 2);
    assert_eq!(b, 3);
    assert_eq!(c, 4);
    assert!(b > a && c > b);
}

#[test]
fn push_pop_restores_execution_and_trigger() {
    let mut st = AsyncHooksState::bootstrap();
    assert_eq!(st.execution_async_id(), 1);
    assert_eq!(st.trigger_async_id(), 0);

    let id = st.new_async_id();
    st.push_async_context(id, 1, 0);
    assert_eq!(st.execution_async_id(), id);
    assert_eq!(st.trigger_async_id(), 1);

    assert!(st.pop_async_context(id));
    assert_eq!(st.execution_async_id(), 1);
    assert_eq!(st.trigger_async_id(), 0);
}

#[test]
fn context_frame_cow_set_and_get() {
    let mut st = AsyncHooksState::bootstrap();
    let f0 = st.current_frame();
    assert!(f0.is_none());

    let f1 = st.enter_with_store(7, 100);
    assert_eq!(st.get_store(7), Some(100));
    assert_eq!(st.current_frame(), Some(f1));
    st.retain_current_frame();

    let f2 = st.enter_with_store(7, 200);
    assert_eq!(st.get_store(7), Some(200));
    // 父 frame 仍持有旧值（COW）
    assert_eq!(st.frame_get(f1, 7), Some(100));

    st.set_current_frame(Some(f1));
    assert_eq!(st.get_store(7), Some(100));

    st.disable_store(7);
    assert_eq!(st.get_store(7), None);
}

#[test]
fn synchronous_store_replacement_reclaims_old_frames() {
    let mut state = AsyncHooksState::bootstrap();
    let key = state.alloc_als_key(0);
    for value in 0..10_000 {
        state.enter_with_store(key, value);
    }

    assert_eq!(state.get_store(key), Some(9_999));
    assert!(state.gc_roots().len() <= 4);
}

#[test]
fn als_becomes_active_only_while_enabled() {
    let mut state = AsyncHooksState::bootstrap();
    let key = state.alloc_als_key(0);
    assert!(state.capture_for_scheduled_callback(0, false).is_none());

    state.enter_with_store(key, 1);
    assert!(state.capture_for_scheduled_callback(0, false).is_some());

    state.disable_store(key);
    assert!(state.capture_for_scheduled_callback(0, false).is_none());
}

#[test]
fn captured_scope_roundtrip() {
    let scope = CapturedScope {
        async_id: 9,
        trigger_async_id: 1,
        resource: 42,
        frame_id: Some(FrameId(3)),
    };
    assert_eq!(scope.async_id, 9);
    assert_eq!(scope.frame_id, Some(FrameId(3)));
}

#[test]
fn snapshot_gate_accepts_only_pristine_state() {
    let mut state = AsyncHooksState::bootstrap();
    assert!(state.is_empty_for_snapshot());

    state.set_top_level_resource(42);
    assert!(!state.is_empty_for_snapshot());

    let mut state = AsyncHooksState::bootstrap();
    state.new_async_id();
    assert!(!state.is_empty_for_snapshot());
}
