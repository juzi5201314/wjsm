//! TAG_BOUND/TAG_PROXY/TAG_ITERATOR/TAG_SCOPE_RECORD 侧表引用提取。
//! 供 GC marker (resolve_value_handles) 和 root 发现 (push_value_roots) 共用。
//! 新增 TAG_*-backed 侧表时只需更新此处。

use crate::types::IteratorState;

/// TAG_BOUND: bound_objects[idx] → target_func, bound_this, bound_args
pub(crate) fn collect_bound_refs(st: &mut crate::RuntimeState, idx: usize) -> Vec<i64> {
    let g = st.bound_objects.lock().unwrap_or_else(|e| e.into_inner());
    let Some(rec) = g.get(idx) else {
        return vec![];
    };
    let mut out = vec![rec.target_func, rec.bound_this];
    out.extend(rec.bound_args.iter().copied());
    out
}

/// TAG_PROXY: proxy_table[idx] → target, handler
pub(crate) fn collect_proxy_refs(st: &mut crate::RuntimeState, idx: usize) -> Vec<i64> {
    let g = st.proxy_table.lock().unwrap_or_else(|e| e.into_inner());
    let Some(entry) = g.get(idx) else {
        return vec![];
    };
    vec![entry.target, entry.handler]
}

/// TAG_ITERATOR: iterators[idx] → 持有的 JS 值
/// - ObjectIter: iterator, next, return_method, throw_method, current_value
/// - IndexValueIter: values
/// - MapKeyIter/MapValueIter/MapEntryIter/SetValueIter: 间接引用 map_table/set_table，
///   由 collect_host_table_values 全量扫描覆盖
/// - StringIter/ArrayIter/HeadersIter/TypedArrayIter/Error: 不持 JS handle
pub(crate) fn collect_iterator_refs(st: &mut crate::RuntimeState, idx: usize) -> Vec<i64> {
    let g = st.iterators.lock().unwrap_or_else(|e| e.into_inner());
    let Some(iter) = g.get(idx) else {
        return vec![];
    };
    match iter {
        IteratorState::ObjectIter {
            iterator,
            next,
            return_method,
            throw_method,
            current_value,
            ..
        } => {
            let mut out = vec![*iterator, *next, *current_value];
            if let Some(v) = return_method {
                out.push(*v);
            }
            if let Some(v) = throw_method {
                out.push(*v);
            }
            out
        }
        IteratorState::IndexValueIter { values, .. } => values.clone(),
        // 其余 variant 不持直接 JS handle，或间接引用已由 collect_host_table_values 覆盖
        _ => vec![],
    }
}

/// TAG_SCOPE_RECORD: scope_records[&handle] → binding values, home_object, new_target
pub(crate) fn collect_scope_record_refs(st: &mut crate::RuntimeState, handle: u32) -> Vec<i64> {
    let Some(rec) = st.scope_records.get(&handle) else {
        return vec![];
    };
    let mut out: Vec<i64> = rec.bindings.iter().map(|(_, v, _, _)| *v).collect();
    if let Some(v) = rec.home_object {
        out.push(v);
    }
    if let Some(v) = rec.new_target {
        out.push(v);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{BoundRecord, ProxyEntry};
    use crate::runtime_eval::ScopeRecord;
    use std::sync::{Arc, Mutex};
    use wjsm_ir::value;

    #[test]
    fn bound_refs_extracted() {
        let mut st = crate::RuntimeState::new();
        let target = value::encode_object_handle(10);
        let this_val = value::encode_object_handle(11);
        let arg0 = value::encode_object_handle(12);
        let arg1 = value::encode_object_handle(13);
        st.bound_objects = Arc::new(Mutex::new(vec![BoundRecord {
            target_func: target,
            bound_this: this_val,
            bound_args: vec![arg0, arg1],
        }]));
        let refs = collect_bound_refs(&mut st, 0);
        assert_eq!(refs, vec![target, this_val, arg0, arg1]);
    }

    #[test]
    fn proxy_refs_extracted() {
        let mut st = crate::RuntimeState::new();
        let target = value::encode_object_handle(20);
        let handler = value::encode_object_handle(21);
        st.proxy_table = Arc::new(Mutex::new(vec![ProxyEntry {
            target,
            handler,
            revoked: false,
        }]));
        let refs = collect_proxy_refs(&mut st, 0);
        assert_eq!(refs, vec![target, handler]);
    }

    #[test]
    fn iterator_object_iter_refs() {
        let mut st = crate::RuntimeState::new();
        let iterator = value::encode_object_handle(30);
        let next = value::encode_object_handle(31);
        let return_method = value::encode_object_handle(32);
        let throw_method = value::encode_object_handle(33);
        let current_value = value::encode_object_handle(34);
        st.iterators = Arc::new(Mutex::new(vec![IteratorState::ObjectIter {
            iterator,
            next,
            return_method: Some(return_method),
            throw_method: Some(throw_method),
            current_value,
            done: false,
            has_current: true,
        }]));
        let refs = collect_iterator_refs(&mut st, 0);
        assert_eq!(refs, vec![iterator, next, current_value, return_method, throw_method]);
    }

    #[test]
    fn scope_record_refs_extracted() {
        let mut st = crate::RuntimeState::new();
        let v0 = value::encode_object_handle(40);
        let v1 = value::encode_object_handle(41);
        let home = value::encode_object_handle(42);
        let new_target = value::encode_object_handle(43);
        st.scope_records.insert(
            0,
            ScopeRecord {
                bindings: vec![
                    ("x".to_string(), v0, false, false),
                    ("y".to_string(), v1, false, false),
                ],
                home_object: Some(home),
                new_target: Some(new_target),
                has_arguments_binding: false,
                is_strict: false,
            },
        );
        let refs = collect_scope_record_refs(&mut st, 0);
        assert_eq!(refs, vec![v0, v1, home, new_target]);
    }
}
