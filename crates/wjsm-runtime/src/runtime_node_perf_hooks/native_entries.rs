use std::sync::atomic::Ordering;

use wasmtime::Caller;

use crate::*;

pub(crate) const OBSERVE_GC: u32 = 1 << 2;
pub(crate) const OBSERVE_NET: u32 = 1 << 5;
const OBSERVE_RESOURCE: u32 = 1 << 6;
const GC_MINOR: u32 = 1;
const GC_MAJOR: u32 = 4;
const GC_INCREMENTAL: u32 = 8;
const GC_FORCED: u32 = 4;

#[derive(Clone, Debug)]
pub(crate) struct NativePerformanceEntry {
    pub(crate) name: String,
    pub(crate) entry_type: &'static str,
    pub(crate) start_time: f64,
    pub(crate) duration: f64,
    pub(crate) detail: NativePerformanceDetail,
}

#[derive(Clone, Debug)]
pub(crate) enum NativePerformanceDetail {
    Gc { kind: u32, flags: u32 },
    Net { host: String, port: u16 },
    Resource(NativeResourceTiming),
}

#[derive(Clone, Debug)]
pub(crate) struct NativeResourceTiming {
    pub(crate) name: String,
    pub(crate) start_time: f64,
    pub(crate) request_start_time: f64,
    pub(crate) response_start_time: f64,
    pub(crate) end_time: f64,
    pub(crate) response_status: u16,
    pub(crate) encoded_body_size: u64,
    pub(crate) decoded_body_size: u64,
}

pub(crate) fn set_observer_state(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let mask = args
        .first()
        .copied()
        .filter(|raw| value::is_f64(*raw))
        .map(|raw| value::decode_f64(raw).max(0.0) as u32)
        .unwrap_or(0);
    let callback = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    caller
        .data()
        .performance_observer_mask
        .store(mask, Ordering::Relaxed);
    if mask == 0 || !value::is_undefined(callback) {
        caller
            .data()
            .performance_native_sink
            .store(callback, Ordering::Relaxed);
    }
    if mask == 0 {
        caller
            .data()
            .performance_native_entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
        caller
            .data()
            .performance_native_delivery_scheduled
            .store(false, Ordering::Relaxed);
    }
    value::encode_undefined()
}

pub(crate) fn set_native_converter(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let converter = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    caller
        .data()
        .performance_native_converter
        .store(converter, Ordering::Relaxed);
    value::encode_undefined()
}

pub(crate) fn queue_gc_entry(
    state: &RuntimeState,
    stats: &crate::runtime_gc::api::GcStats,
    forced: bool,
) {
    if state.performance_observer_mask.load(Ordering::Relaxed) & OBSERVE_GC == 0 {
        return;
    }
    let kind = match stats.cycle_kind {
        crate::runtime_gc::api::CycleKind::Young => GC_MINOR,
        crate::runtime_gc::api::CycleKind::Step => GC_INCREMENTAL,
        crate::runtime_gc::api::CycleKind::Full
        | crate::runtime_gc::api::CycleKind::Mixed
        | crate::runtime_gc::api::CycleKind::ZgcCycle => GC_MAJOR,
    };
    let duration = stats.elapsed.as_secs_f64() * 1000.0;
    let start_time =
        (state.performance_origin.elapsed().as_secs_f64() * 1000.0 - duration).max(0.0);
    state
        .performance_native_entries
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .push_back(NativePerformanceEntry {
            name: "gc".to_string(),
            entry_type: "gc",
            start_time,
            duration,
            detail: NativePerformanceDetail::Gc {
                kind,
                flags: if forced { GC_FORCED } else { 0 },
            },
        });
    schedule_native_delivery(state);
}

pub(crate) fn queue_resource_entry(state: &RuntimeState, timing: NativeResourceTiming) {
    if !resource_entries_enabled(state) {
        return;
    }
    state
        .performance_native_entries
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .push_back(NativePerformanceEntry {
            name: timing.name.clone(),
            entry_type: "resource",
            start_time: timing.start_time,
            duration: timing.end_time - timing.start_time,
            detail: NativePerformanceDetail::Resource(timing),
        });
    schedule_native_delivery(state);
}

pub(crate) fn queue_net_entry(
    state: &RuntimeState,
    start_time: f64,
    duration: f64,
    host: String,
    port: u16,
) {
    if state.performance_observer_mask.load(Ordering::Relaxed) & OBSERVE_NET == 0 {
        return;
    }
    state
        .performance_native_entries
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .push_back(NativePerformanceEntry {
            name: "connect".to_string(),
            entry_type: "net",
            start_time,
            duration,
            detail: NativePerformanceDetail::Net { host, port },
        });
    schedule_native_delivery(state);
}

pub(crate) fn resource_entries_enabled(state: &RuntimeState) -> bool {
    state.performance_observer_mask.load(Ordering::Relaxed) & OBSERVE_RESOURCE != 0
}

pub(crate) fn set_native_dispatcher(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let dispatcher = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    caller
        .data()
        .performance_native_dispatcher
        .store(dispatcher, Ordering::Relaxed);
    value::encode_undefined()
}

fn schedule_native_delivery(state: &RuntimeState) {
    let callback = state.performance_native_sink.load(Ordering::Relaxed);
    let converter = state.performance_native_converter.load(Ordering::Relaxed);
    if value::is_undefined(callback)
        || state
            .performance_native_delivery_scheduled
            .swap(true, Ordering::AcqRel)
    {
        return;
    }
    state
        .immediate_queue
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .push_back(ImmediateEntry {
            id: u32::MAX,
            callback,
            resource: value::encode_undefined(),
            scope: None,
            native_performance_converter: (!value::is_undefined(converter)).then_some(converter),
            native_performance_dispatcher: (!value::is_undefined(
                state.performance_native_dispatcher.load(Ordering::Relaxed),
            ))
            .then(|| state.performance_native_dispatcher.load(Ordering::Relaxed)),
        });
}

pub(crate) fn drain_native_entry(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    caller
        .data()
        .performance_native_delivery_scheduled
        .store(false, Ordering::Release);
    let (entry, has_more) = {
        let mut queue = caller
            .data()
            .performance_native_entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let entry = queue.pop_front();
        (entry, !queue.is_empty())
    };
    if has_more {
        schedule_native_delivery(caller.data());
    }
    entry.map_or_else(value::encode_undefined, |entry| {
        materialize_entry(caller, entry)
    })
}

fn materialize_entry(caller: &mut Caller<'_, RuntimeState>, entry: NativePerformanceEntry) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let object = alloc_host_object(caller, &env, 5);
    let root = caller.data().push_host_temp_roots([object]);
    let name = store_runtime_string(caller, entry.name);
    let _ = caller.data().push_host_temp_roots([name]);
    let entry_type = store_runtime_string(caller, entry.entry_type.to_string());
    let _ = caller.data().push_host_temp_roots([entry_type]);
    let detail = materialize_detail(caller, &env, entry.detail);
    for (property, raw) in [
        ("name", name),
        ("entryType", entry_type),
        ("startTime", value::encode_f64(entry.start_time)),
        ("duration", value::encode_f64(entry.duration)),
        ("detail", detail),
    ] {
        let _ = define_host_data_property_from_caller(caller, object, property, raw);
    }
    caller.data().truncate_host_temp_roots(root);
    object
}

fn materialize_detail(
    caller: &mut Caller<'_, RuntimeState>,
    env: &WasmEnv,
    detail: NativePerformanceDetail,
) -> i64 {
    match detail {
        NativePerformanceDetail::Gc { kind, flags } => {
            let detail = alloc_host_object(caller, env, 2);
            let _ = caller.data().push_host_temp_roots([detail]);
            let _ = define_host_data_property_from_caller(
                caller,
                detail,
                "kind",
                value::encode_f64(kind as f64),
            );
            let _ = define_host_data_property_from_caller(
                caller,
                detail,
                "flags",
                value::encode_f64(flags as f64),
            );
            detail
        }
        NativePerformanceDetail::Net { host, port } => {
            let detail = alloc_host_object(caller, env, 2);
            let _ = caller.data().push_host_temp_roots([detail]);
            let host = store_runtime_string(caller, host);
            let _ = caller.data().push_host_temp_roots([host]);
            let _ = define_host_data_property_from_caller(caller, detail, "host", host);
            let _ = define_host_data_property_from_caller(
                caller,
                detail,
                "port",
                value::encode_f64(port as f64),
            );
            detail
        }
        NativePerformanceDetail::Resource(timing) => {
            materialize_resource_detail(caller, env, timing)
        }
    }
}

fn materialize_resource_detail(
    caller: &mut Caller<'_, RuntimeState>,
    env: &WasmEnv,
    timing: NativeResourceTiming,
) -> i64 {
    let detail = alloc_host_object(caller, env, 5);
    let _ = caller.data().push_host_temp_roots([detail]);
    let timing_info = alloc_host_object(caller, env, 10);
    let _ = caller.data().push_host_temp_roots([timing_info]);
    for (property, raw) in [
        ("startTime", timing.start_time),
        ("redirectStartTime", 0.0),
        ("redirectEndTime", 0.0),
        ("postRedirectStartTime", timing.start_time),
        ("finalServiceWorkerStartTime", 0.0),
        ("finalNetworkRequestStartTime", timing.request_start_time),
        ("finalNetworkResponseStartTime", timing.response_start_time),
        ("endTime", timing.end_time),
        ("encodedBodySize", timing.encoded_body_size as f64),
        ("decodedBodySize", timing.decoded_body_size as f64),
    ] {
        let _ = define_host_data_property_from_caller(
            caller,
            timing_info,
            property,
            value::encode_f64(raw),
        );
    }
    let initiator_type = store_runtime_string(caller, "fetch".to_string());
    let _ = caller.data().push_host_temp_roots([initiator_type]);
    let cache_mode = store_runtime_string(caller, String::new());
    let _ = caller.data().push_host_temp_roots([cache_mode]);
    let delivery_type = store_runtime_string(caller, String::new());
    let _ = caller.data().push_host_temp_roots([delivery_type]);
    for (property, raw) in [
        ("initiatorType", initiator_type),
        ("timingInfo", timing_info),
        ("cacheMode", cache_mode),
        (
            "responseStatus",
            value::encode_f64(timing.response_status as f64),
        ),
        ("deliveryType", delivery_type),
    ] {
        let _ = define_host_data_property_from_caller(caller, detail, property, raw);
    }
    detail
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn net_entry_requires_observer_at_completion() {
        let state = RuntimeState::new();
        queue_net_entry(&state, 1.0, 2.0, "127.0.0.1".to_string(), 8080);
        assert!(
            state
                .performance_native_entries
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .is_empty()
        );

        state
            .performance_observer_mask
            .store(OBSERVE_NET, Ordering::Relaxed);
        queue_net_entry(&state, 1.0, 2.0, "127.0.0.1".to_string(), 8080);
        let entry = state
            .performance_native_entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .pop_front()
            .expect("queued net entry");
        assert_eq!(entry.name, "connect");
        assert_eq!(entry.entry_type, "net");
        assert_eq!(entry.start_time, 1.0);
        assert_eq!(entry.duration, 2.0);
        match entry.detail {
            NativePerformanceDetail::Net { host, port } => {
                assert_eq!(host, "127.0.0.1");
                assert_eq!(port, 8080);
            }
            _ => panic!("expected net entry detail"),
        }
    }
}
