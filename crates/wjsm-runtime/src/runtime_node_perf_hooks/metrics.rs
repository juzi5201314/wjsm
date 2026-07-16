use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use wasmtime::Caller;

use crate::runtime_node_perf_hooks_histogram::HistogramCapability;
use crate::*;

#[derive(Clone, Debug)]
pub(crate) struct EventLoopDelayMonitor {
    pub(crate) capability: HistogramCapability,
    pub(crate) resolution: Duration,
    pub(crate) next_deadline: Option<Instant>,
    pub(crate) enabled: bool,
}

pub(crate) struct LoopExitGuard {
    origin: std::sync::Arc<Instant>,
    exit_millis: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl Drop for LoopExitGuard {
    fn drop(&mut self) {
        store_millis(
            &self.exit_millis,
            self.origin.elapsed().as_secs_f64() * 1000.0,
        );
    }
}

pub(crate) fn time_origin(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    value::encode_f64(caller.data().performance_time_origin_ms)
}

pub(crate) fn node_timing(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let object = alloc_host_object(caller, &env, 12);
    let root = caller.data().push_host_temp_roots([object]);
    let state = caller.data();
    let fields = [
        ("nodeStart", 0.0),
        ("v8Start", state.performance_v8_start_ms),
        ("environment", state.performance_environment_ms),
        (
            "bootstrapComplete",
            load_millis(&state.performance_bootstrap_complete_ms),
        ),
        ("loopStart", load_millis(&state.performance_loop_start_ms)),
        ("loopExit", load_millis(&state.performance_loop_exit_ms)),
        (
            "idleTime",
            state.performance_idle_ns.load(Ordering::Relaxed) as f64 / 1_000_000.0,
        ),
        (
            "loopCount",
            state.performance_loop_count.load(Ordering::Relaxed) as f64,
        ),
        (
            "events",
            state.performance_events.load(Ordering::Relaxed) as f64,
        ),
        (
            "eventsWaiting",
            state.performance_events_waiting.load(Ordering::Relaxed) as f64,
        ),
    ];
    for (name, number) in fields {
        let _ =
            define_host_data_property_from_caller(caller, object, name, value::encode_f64(number));
    }
    caller.data().truncate_host_temp_roots(root);
    object
}

pub(crate) fn event_loop_utilization(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    let state = caller.data();
    let loop_start = load_millis(&state.performance_loop_start_ms);
    let (idle, active, utilization) = if loop_start < 0.0 {
        (0.0, 0.0, 0.0)
    } else {
        let idle = state.performance_idle_ns.load(Ordering::Relaxed) as f64 / 1_000_000.0;
        let elapsed = state.performance_origin.elapsed().as_secs_f64() * 1000.0 - loop_start;
        let active = (elapsed - idle).max(0.0);
        (idle, active, active / (idle + active))
    };
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let object = alloc_host_object(caller, &env, 3);
    let root = caller.data().push_host_temp_roots([object]);
    for (name, number) in [
        ("idle", idle),
        ("active", active),
        ("utilization", utilization),
    ] {
        let _ =
            define_host_data_property_from_caller(caller, object, name, value::encode_f64(number));
    }
    caller.data().truncate_host_temp_roots(root);
    object
}

pub(crate) fn mark_bootstrap_complete(state: &RuntimeState) {
    store_millis(
        &state.performance_bootstrap_complete_ms,
        state.performance_origin.elapsed().as_secs_f64() * 1000.0,
    );
}

pub(crate) fn mark_loop_start(state: &RuntimeState) {
    if load_millis(&state.performance_loop_start_ms) < 0.0 {
        store_millis(
            &state.performance_loop_start_ms,
            state.performance_origin.elapsed().as_secs_f64() * 1000.0,
        );
    }
}

pub(crate) fn loop_exit_guard(state: &RuntimeState) -> LoopExitGuard {
    LoopExitGuard {
        origin: state.performance_origin.clone(),
        exit_millis: state.performance_loop_exit_ms.clone(),
    }
}

pub(crate) fn mark_loop_iteration(state: &RuntimeState) {
    state.performance_loop_count.fetch_add(1, Ordering::Relaxed);
    sample_event_loop_delay(state);
}

pub(crate) fn increment_event_count(state: &RuntimeState) {
    state.performance_events.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn finish_idle_wait(state: &RuntimeState, started: Instant, events_waiting: usize) {
    let elapsed = started.elapsed();
    let nanos = elapsed.as_nanos().min(u64::MAX as u128) as u64;
    state
        .performance_idle_ns
        .fetch_add(nanos, Ordering::Relaxed);
    state
        .performance_events_waiting
        .store(events_waiting as u64, Ordering::Relaxed);
}

pub(crate) fn next_event_loop_delay_deadline(state: &RuntimeState) -> Option<Instant> {
    state
        .performance_event_loop_monitors
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .values()
        .filter(|monitor| monitor.enabled)
        .filter_map(|monitor| monitor.next_deadline)
        .min()
}

fn sample_event_loop_delay(state: &RuntimeState) {
    let now = Instant::now();
    let Some(shared) = state.shared_state.as_ref() else {
        return;
    };
    let mut monitors = state
        .performance_event_loop_monitors
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    for monitor in monitors.values_mut().filter(|monitor| monitor.enabled) {
        let Some(deadline) = monitor.next_deadline else {
            continue;
        };
        if now < deadline {
            continue;
        }
        let _ = shared
            .perf_histograms
            .record_delta(&monitor.capability, now);
        // libuv 重复 timer 以当前 loop 时间重新挂载；长回调后也不会按
        // blocked_time / resolution 逐次追赶过期 tick。
        monitor.next_deadline = Some(now + monitor.resolution);
    }
}

pub(crate) fn load_millis(value: &std::sync::atomic::AtomicU64) -> f64 {
    f64::from_bits(value.load(Ordering::Relaxed))
}

pub(crate) fn store_millis(value: &std::sync::atomic::AtomicU64, millis: f64) {
    value.store(millis.to_bits(), Ordering::Relaxed);
}
