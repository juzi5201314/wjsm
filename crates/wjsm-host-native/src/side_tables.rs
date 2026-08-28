//! 宿主侧下标表回收。
//!
//! GC 标记期在 [`HostLiveSet`] 里边标边收活下标；`collect_garbage` 完成堆
//! sweep 后由 [`NativeAgentState::sweep_host_index_tables`] 清理宿主下标表。
//! 字符串已经完全位于 ManagedHeap，其存活与退休由 GC 统一负责。

use std::collections::HashSet;

use crate::{NativeAgentState, NativeCallableKind};
use wjsm_ir::value;

/// 标记期收集的宿主侧活下标集合。sweep 只放行出现在这里的槽位。
#[derive(Default)]
pub(crate) struct HostLiveSet {
    pub closures: HashSet<u32>,
    pub bound: HashSet<u32>,
    pub proxies: HashSet<u32>,
    pub regexps: HashSet<u32>,
    pub exceptions: HashSet<u32>,
}

impl NativeAgentState {
    /// GC 后清宿主下标表；字符串对象由 ManagedHeap GC 统一清理。
    pub(crate) fn sweep_host_index_tables(&mut self, retired: &[u32], live: &HostLiveSet) {
        self.sweep_closures(live);
        self.sweep_bound_functions(live);
        self.sweep_proxies(live);
        self.sweep_regexps(live);
        self.sweep_exceptions(live);
        self.sweep_private_side_tables(retired, live);
    }

    /// 删除 callable 侧表里 owner 为指定编码值的所有条目。
    fn retain_callable_owner(&mut self, owner: i64) {
        self.callable_properties
            .retain(|(candidate, _), _| *candidate != owner);
        self.callable_accessors
            .retain(|(candidate, _), _| *candidate != owner);
        self.callable_property_flags
            .retain(|(candidate, _), _| *candidate != owner);
        self.callable_prototypes
            .retain(|candidate, _| *candidate != owner);
        self.non_extensible_callables.remove(&owner);
    }

    fn sweep_closures(&mut self, live: &HostLiveSet) {
        for index in 0..self.closures.len() {
            if self.closures[index].is_none() || live.closures.contains(&(index as u32)) {
                continue;
            }
            self.closures[index] = None;
            self.closure_free.push(index as u32);
            let encoded = value::encode_closure_idx(index as u32);
            // function_closures / latest_function_closures 是 memo 而非权威引用，
            // 闭包死后必须摘除，否则复用槽位后 memo 指向新闭包、语义漂移。
            self.function_closures
                .retain(|_, stored| *stored != encoded);
            self.latest_function_closures
                .retain(|_, stored| *stored != encoded);
            self.retain_callable_owner(encoded);
        }
    }

    fn sweep_bound_functions(&mut self, live: &HostLiveSet) {
        for index in 0..self.bound_functions.len() {
            if self.bound_functions[index].is_none() || live.bound.contains(&(index as u32)) {
                continue;
            }
            self.bound_functions[index] = None;
            self.bound_free.push(index as u32);
            // bound 函数以 NativeCallableKind::Bound(index) 的 native callable 值对外，
            // 按 kind 反查其编码值再清 callable 侧表。
            if let Some(callable) = self
                .native_callable_ids
                .get(&NativeCallableKind::Bound(index as u32))
                .copied()
            {
                self.retain_callable_owner(value::encode_native_callable_idx(callable));
            }
        }
    }

    fn sweep_proxies(&mut self, live: &HostLiveSet) {
        for index in 0..self.proxies.len() {
            if self.proxies[index].is_none() || live.proxies.contains(&(index as u32)) {
                continue;
            }
            self.proxies[index] = None;
            self.proxy_free.push(index as u32);
            self.retain_callable_owner(value::encode_proxy_handle(index as u32));
        }
    }

    fn sweep_regexps(&mut self, live: &HostLiveSet) {
        for index in 0..self.regexps.len() {
            if self.regexps[index].is_none() || live.regexps.contains(&(index as u32)) {
                continue;
            }
            self.regexps[index] = None;
            self.regexp_free.push(index as u32);
        }
    }

    fn sweep_exceptions(&mut self, live: &HostLiveSet) {
        for index in 0..self.exceptions.len() {
            if self.exceptions[index].is_none() || live.exceptions.contains(&(index as u32)) {
                continue;
            }
            self.exceptions[index] = None;
            self.exception_free.push(index as u32);
        }
    }

    /// 清 owner/brand 可能指向闭包/bound/proxy/regexp 的宿主侧表：
    /// `private_slots`（owner 为对象或 callable）、`private_brands`（brand 为
    /// 对象原型或 callable）、`async_iterator_objects`（编码对象值）。
    fn sweep_private_side_tables(&mut self, retired: &[u32], live: &HostLiveSet) {
        self.private_slots
            .retain(|(owner, _), _| host_value_is_live(retired, live, *owner));
        self.private_brands
            .retain(|_, brand| host_value_is_live(retired, live, *brand));
        self.async_iterator_objects
            .retain(|encoded| host_value_is_live(retired, live, *encoded));
    }
}

/// 判定一个宿主编码值是否仍存活。堆对象/数组按 retired handle 判定；
/// 闭包/bound/proxy/regexp 按下标 live set 判定；function/native_callable/
/// null/undefined 等无独立下标表的永活（或另有管理）。
fn host_value_is_live(retired: &[u32], live: &HostLiveSet, encoded: i64) -> bool {
    if value::is_object(encoded) || value::is_array(encoded) {
        return retired
            .binary_search(&value::decode_handle(encoded))
            .is_err();
    }
    if value::is_closure(encoded) {
        return live.closures.contains(&value::decode_closure_idx(encoded));
    }
    if value::is_bound(encoded) {
        return live.bound.contains(&value::decode_bound_idx(encoded));
    }
    if value::is_proxy(encoded) {
        return live.proxies.contains(&value::decode_proxy_handle(encoded));
    }
    if value::is_regexp(encoded) {
        return live.regexps.contains(&value::decode_regexp_handle(encoded));
    }
    true
}
