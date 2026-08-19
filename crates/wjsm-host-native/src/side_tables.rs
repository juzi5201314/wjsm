//! 宿主侧下标表回收。
//!
//! GC 标记期在 [`HostLiveSet`] 里边标边收活下标；`collect_garbage` 完成堆
//! sweep 后由 [`NativeAgentState::sweep_host_index_tables`] 把不可达槽位
//! tombstone、归还空闲表，并清掉指向它们的 callable / string intern 侧表。
//! 这样 `closures` / `bound_functions` / `proxies` / `regexps` / `exceptions` /
//! `strings` 不再只增不缩，高频闭包与 RegExp 负载下固定时间窗 RSS 不再无界上涨。

use std::collections::HashSet;

use wjsm_host::RuntimeString;
use wjsm_ir::value;

use crate::dispatch::SYMBOL_PROPERTY_KEY_BIT;
use crate::{NativeAgentState, NativeCallableKind};

/// 标记期收集的宿主侧活下标集合。sweep 只放行出现在这里的槽位。
#[derive(Default)]
pub(crate) struct HostLiveSet {
    pub closures: HashSet<u32>,
    pub bound: HashSet<u32>,
    pub proxies: HashSet<u32>,
    pub regexps: HashSet<u32>,
    pub exceptions: HashSet<u32>,
    pub strings: HashSet<u32>,
}

impl NativeAgentState {
    /// GC 后清宿主下标表：tombstone 不可达槽位、归还空闲槽、清对应 callable
    /// 侧表与 string intern 表。`retired` 是堆 handle 的 retired 集合（已排序）。
    pub(crate) fn sweep_host_index_tables(&mut self, retired: &[u32], live: &HostLiveSet) {
        self.sweep_closures(live);
        self.sweep_bound_functions(live);
        self.sweep_proxies(live);
        self.sweep_regexps(live);
        self.sweep_exceptions(live);
        // 闭包/bound/proxy 的 callable_* 已按 owner 清掉；此后再取 callable_* 的
        // name_id 只剩活 callable，可直接作为存活字符串的钉扎来源。
        self.sweep_private_side_tables(retired, live);
        self.sweep_strings(live);
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

    /// young ZGC 后只处理年轻字符串：新值连续存活两轮后晋升，避免每轮重复扫描。
    pub(crate) fn sweep_young_strings(&mut self, live: &HostLiveSet) {
        let live_strings = self.live_string_handles(live);
        for index in 0..self.strings.len() {
            if !self.string_occupied[index] || self.string_ages[index] == u8::MAX {
                continue;
            }
            if live_strings.contains(&(index as u32)) {
                self.string_ages[index] += 1;
                if self.string_ages[index] >= 2 {
                    self.string_ages[index] = u8::MAX;
                }
                continue;
            }
            self.retire_string(index);
        }
    }

    /// full/old GC 已建立完整 live set，可同时回收年轻和晋升字符串。
    fn sweep_strings(&mut self, live: &HostLiveSet) {
        let live_strings = self.live_string_handles(live);
        for index in 0..self.strings.len() {
            if !self.string_occupied[index] {
                continue;
            }
            if live_strings.contains(&(index as u32)) {
                self.string_ages[index] = u8::MAX;
                continue;
            }
            self.retire_string(index);
        }
    }

    /// managed young/old bridge与宿主边都已在 ZGC mark 中展开，report 的 host live set
    /// 对字符串完整；属性名及专用侧表键另行钉扎。
    fn live_string_handles(&self, live: &HostLiveSet) -> HashSet<u32> {
        let mut live_strings = live.strings.clone();
        live_strings.extend(self.gc.heap().property_name_ids());
        live_strings.extend(self.array_properties.keys().map(|(_, key)| *key));
        live_strings.extend(self.array_accessors.keys().map(|(_, key)| *key));
        live_strings.extend(self.array_property_flags.keys().map(|(_, key)| *key));
        for keys in self.array_property_order.values() {
            live_strings.extend(keys.iter().copied());
        }
        // 符号 bit 置位的 key 是符号而非字符串，不计入字符串存活集。
        live_strings.extend(
            self.callable_properties
                .keys()
                .map(|(_, key)| *key)
                .filter(|key| key & SYMBOL_PROPERTY_KEY_BIT == 0),
        );
        live_strings.extend(
            self.callable_accessors
                .keys()
                .map(|(_, key)| *key)
                .filter(|key| key & SYMBOL_PROPERTY_KEY_BIT == 0),
        );
        live_strings.extend(
            self.callable_property_flags
                .keys()
                .map(|(_, key)| *key)
                .filter(|key| key & SYMBOL_PROPERTY_KEY_BIT == 0),
        );
        live_strings
    }

    fn retire_string(&mut self, index: usize) {
        if self.strings[index].is_flat()
            && self.strings[index].utf16_len() <= 64
            && self.string_ids.get(&self.strings[index]).copied() == Some(index as u32)
        {
            self.string_ids.remove(&self.strings[index]);
        }
        self.strings[index] = RuntimeString::empty();
        self.string_occupied[index] = false;
        self.string_ages[index] = 0;
        self.string_free.push(index as u32);
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
