//! `node:perf_hooks` host bridge。
//!
//! JS 层拥有 Performance Timeline、Observer 与公共类形状；本模块只提供
//! 单调时钟、事件循环指标、HDR Histogram 和原生事件的运行时事实。

mod histogram_bridge;
mod metrics;
mod native_entries;

use wasmtime::Caller;

use crate::*;

pub(crate) use histogram_bridge::{histogram_identity_from_object, materialize_histogram};
pub(crate) use metrics::{
    EventLoopDelayMonitor, finish_idle_wait, increment_event_count, loop_exit_guard,
    mark_bootstrap_complete, mark_loop_iteration, mark_loop_start, next_event_loop_delay_deadline,
};
pub(crate) use native_entries::{
    NativePerformanceEntry, NativeResourceTiming, OBSERVE_NET, queue_gc_entry, queue_net_entry,
    queue_resource_entry, resource_entries_enabled,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PerfHooksMethodKind {
    TimeOrigin = 0,
    NodeTiming = 1,
    EventLoopUtilization = 2,
    SetObserverState = 3,
    DrainNativeEntry = 4,
    HistogramCreate = 5,
    HistogramStats = 6,
    HistogramPercentile = 7,
    HistogramPercentiles = 8,
    HistogramRecord = 9,
    HistogramRecordDelta = 10,
    HistogramAdd = 11,
    HistogramReset = 12,
    EventLoopDelayCreate = 13,
    EventLoopDelayEnable = 14,
    EventLoopDelayDisable = 15,
    RegisterHistogramPrototypes = 16,
    HistogramKind = 17,
    SetNativeConverter = 18,
    SetNativeDispatcher = 19,
}

impl PerfHooksMethodKind {
    pub(crate) fn method(self) -> u8 {
        self as u8
    }

    pub(crate) fn from_method(method: u8) -> Option<Self> {
        match method {
            0 => Some(Self::TimeOrigin),
            1 => Some(Self::NodeTiming),
            2 => Some(Self::EventLoopUtilization),
            3 => Some(Self::SetObserverState),
            4 => Some(Self::DrainNativeEntry),
            5 => Some(Self::HistogramCreate),
            6 => Some(Self::HistogramStats),
            7 => Some(Self::HistogramPercentile),
            8 => Some(Self::HistogramPercentiles),
            9 => Some(Self::HistogramRecord),
            10 => Some(Self::HistogramRecordDelta),
            11 => Some(Self::HistogramAdd),
            12 => Some(Self::HistogramReset),
            13 => Some(Self::EventLoopDelayCreate),
            14 => Some(Self::EventLoopDelayEnable),
            15 => Some(Self::EventLoopDelayDisable),
            16 => Some(Self::RegisterHistogramPrototypes),
            17 => Some(Self::HistogramKind),
            18 => Some(Self::SetNativeConverter),
            19 => Some(Self::SetNativeDispatcher),
            _ => None,
        }
    }
}

pub(crate) fn create_perf_hooks_host_object(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let object = alloc_host_object(caller, &env, 21);
    let root = caller.data().push_host_temp_roots([object]);
    for (name, kind) in [
        ("timeOrigin", PerfHooksMethodKind::TimeOrigin),
        ("nodeTiming", PerfHooksMethodKind::NodeTiming),
        (
            "eventLoopUtilization",
            PerfHooksMethodKind::EventLoopUtilization,
        ),
        ("setObserverState", PerfHooksMethodKind::SetObserverState),
        ("drainNativeEntry", PerfHooksMethodKind::DrainNativeEntry),
        (
            "setNativeConverter",
            PerfHooksMethodKind::SetNativeConverter,
        ),
        (
            "setNativeDispatcher",
            PerfHooksMethodKind::SetNativeDispatcher,
        ),
        ("histogramCreate", PerfHooksMethodKind::HistogramCreate),
        ("histogramStats", PerfHooksMethodKind::HistogramStats),
        (
            "histogramPercentile",
            PerfHooksMethodKind::HistogramPercentile,
        ),
        (
            "histogramPercentiles",
            PerfHooksMethodKind::HistogramPercentiles,
        ),
        ("histogramRecord", PerfHooksMethodKind::HistogramRecord),
        (
            "histogramRecordDelta",
            PerfHooksMethodKind::HistogramRecordDelta,
        ),
        ("histogramAdd", PerfHooksMethodKind::HistogramAdd),
        ("histogramReset", PerfHooksMethodKind::HistogramReset),
        ("histogramKind", PerfHooksMethodKind::HistogramKind),
        (
            "eventLoopDelayCreate",
            PerfHooksMethodKind::EventLoopDelayCreate,
        ),
        (
            "eventLoopDelayEnable",
            PerfHooksMethodKind::EventLoopDelayEnable,
        ),
        (
            "eventLoopDelayDisable",
            PerfHooksMethodKind::EventLoopDelayDisable,
        ),
        (
            "registerHistogramPrototypes",
            PerfHooksMethodKind::RegisterHistogramPrototypes,
        ),
    ] {
        let callable =
            create_native_callable(caller.data(), NativeCallable::PerfHooksMethod { kind });
        let _ = define_host_data_property_from_caller(caller, object, name, callable);
    }
    let clone_detail = create_native_callable(caller.data(), NativeCallable::StructuredClone);
    let clone_detail_name = find_memory_c_string_with_env(caller, &env, "cloneDetail")
        .or_else(|| alloc_heap_c_string_with_env(caller, &env, "cloneDetail"));
    if let Some(name_id) = clone_detail_name {
        let _ = define_host_data_property_by_name_id_with_flags(
            caller,
            object,
            encode_string_name_id(name_id),
            clone_detail,
            0,
        );
    }
    caller.data().truncate_host_temp_roots(root);
    object
}

pub(crate) fn call_perf_hooks_method(
    caller: &mut Caller<'_, RuntimeState>,
    kind: PerfHooksMethodKind,
    args: &[i64],
) -> i64 {
    match kind {
        PerfHooksMethodKind::TimeOrigin => metrics::time_origin(caller),
        PerfHooksMethodKind::NodeTiming => metrics::node_timing(caller),
        PerfHooksMethodKind::EventLoopUtilization => metrics::event_loop_utilization(caller),
        PerfHooksMethodKind::SetObserverState => native_entries::set_observer_state(caller, args),
        PerfHooksMethodKind::DrainNativeEntry => native_entries::drain_native_entry(caller),
        PerfHooksMethodKind::SetNativeConverter => {
            native_entries::set_native_converter(caller, args)
        }
        PerfHooksMethodKind::SetNativeDispatcher => {
            native_entries::set_native_dispatcher(caller, args)
        }
        PerfHooksMethodKind::HistogramCreate => histogram_bridge::create_histogram(caller, args),
        PerfHooksMethodKind::HistogramStats => histogram_bridge::histogram_stats(caller, args),
        PerfHooksMethodKind::HistogramPercentile => {
            histogram_bridge::histogram_percentile(caller, args)
        }
        PerfHooksMethodKind::HistogramPercentiles => {
            histogram_bridge::histogram_percentiles(caller, args)
        }
        PerfHooksMethodKind::HistogramRecord => histogram_bridge::histogram_record(caller, args),
        PerfHooksMethodKind::HistogramRecordDelta => {
            histogram_bridge::histogram_record_delta(caller, args)
        }
        PerfHooksMethodKind::HistogramAdd => histogram_bridge::histogram_add(caller, args),
        PerfHooksMethodKind::HistogramReset => histogram_bridge::histogram_reset(caller, args),
        PerfHooksMethodKind::HistogramKind => histogram_bridge::histogram_kind(caller, args),
        PerfHooksMethodKind::EventLoopDelayCreate => {
            histogram_bridge::event_loop_delay_create(caller, args)
        }
        PerfHooksMethodKind::EventLoopDelayEnable => {
            histogram_bridge::event_loop_delay_enable(caller, args)
        }
        PerfHooksMethodKind::EventLoopDelayDisable => {
            histogram_bridge::event_loop_delay_disable(caller, args)
        }
        PerfHooksMethodKind::RegisterHistogramPrototypes => {
            histogram_bridge::register_histogram_prototypes(caller, args)
        }
    }
}
