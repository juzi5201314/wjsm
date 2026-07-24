use std::collections::BTreeSet;

use wjsm_ir::value;

use crate::heap::{HandleId, HandleTableV2};
use crate::realm::Realm;

#[derive(Default)]
pub struct V2ConditionalRoots {
    realms: Vec<Realm>,
    promise_values: Vec<i64>,
    stream_values: Vec<i64>,
    proxy_values: Vec<i64>,
    async_values: Vec<i64>,
}

impl V2ConditionalRoots {
    pub fn push_realm(&mut self, realm: Realm) {
        self.realms.push(realm);
    }

    pub fn extend_promise_values(&mut self, values: impl IntoIterator<Item = i64>) {
        self.promise_values.extend(values);
    }

    pub fn extend_stream_values(&mut self, values: impl IntoIterator<Item = i64>) {
        self.stream_values.extend(values);
    }

    pub fn extend_proxy_values(&mut self, values: impl IntoIterator<Item = i64>) {
        self.proxy_values.extend(values);
    }

    pub fn extend_async_values(&mut self, values: impl IntoIterator<Item = i64>) {
        self.async_values.extend(values);
    }

    pub fn collect(&self, handles: &HandleTableV2) -> BTreeSet<HandleId> {
        let mut roots = BTreeSet::new();
        for realm in &self.realms {
            if let Some(global) = live_handle(realm.global_object, handles) {
                roots.insert(global);
                roots.extend(
                    realm
                        .intrinsics
                        .iter_roots()
                        .filter_map(|value| live_handle(value, handles)),
                );
            }
        }
        for value in self
            .promise_values
            .iter()
            .chain(&self.stream_values)
            .chain(&self.proxy_values)
            .chain(&self.async_values)
        {
            if let Some(handle) = live_handle(*value, handles) {
                roots.insert(handle);
            }
        }
        roots
    }
}

fn live_handle(value: i64, handles: &HandleTableV2) -> Option<HandleId> {
    if !value::is_object(value) && !value::is_array(value) {
        return None;
    }
    let handle = HandleId::new(value::decode_handle(value));
    handles.resolve(handle).is_some().then_some(handle)
}
