use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use num_traits::ToPrimitive;
use wjsm_ir::{Builtin, value};
use wjsm_native_abi::NativeVmContext;

use super::{
    bigint, collections, fail_dispatch, modules, object_handle, runtime, structured_clone,
};
use crate::{NativeAgentState, NativeCallableKind};

const BASE_HISTOGRAM: i64 = 0;
const RECORDABLE_HISTOGRAM: i64 = 1;
const INTERVAL_HISTOGRAM: i64 = 2;
const EMPTY_MINIMUM: u64 = i64::MAX as u64;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum NodePerfHooksCallable {
    CloneDetail,
    DrainNativeEntry,
    EventLoopDelayCreate,
    EventLoopDelayDisable,
    EventLoopDelayEnable,
    EventLoopUtilization,
    HistogramAdd,
    HistogramCreate,
    HistogramKind,
    HistogramPercentile,
    HistogramPercentiles,
    HistogramRecord,
    HistogramRecordDelta,
    HistogramReset,
    HistogramStats,
    NodeTiming,
    PerformanceNow,
    RegisterHistogramPrototypes,
    SetNativeConverter,
    SetNativeDispatcher,
    SetObserverState,
    TimeOrigin,
}

#[derive(Default)]
pub(crate) struct NodePerfHooksState {
    bridge: Option<i64>,
    performance: Option<i64>,
    origin: Option<Instant>,
    time_origin_ms: f64,
    observer_mask: u32,
    converter: Option<i64>,
    dispatcher: Option<i64>,
    pub(crate) observer_callback: Option<i64>,
    pub(crate) native_entries: VecDeque<i64>,
    histograms: HashMap<u32, HistogramTransfer>,
    histogram_prototypes: [Option<u32>; 3],
    histogram_brand: Option<i64>,
    histogram_kind_key: Option<i64>,
    histogram_map_key: Option<i64>,
}

struct NativeHistogram {
    highest: u64,
    values: Vec<u64>,
    count: u64,
    exceeds: u64,
    delta_origin: Option<Instant>,
    interval_resolution_ms: u64,
    interval_enabled_at_ms: Option<u64>,
    interval_samples: u64,
}

impl NativeHistogram {
    fn record(&mut self, sample: u64) {
        if sample > self.highest {
            self.exceeds = self.exceeds.saturating_add(1);
        } else {
            self.values.push(sample);
            self.count = self.count.saturating_add(1);
        }
    }

    fn sample_interval(&mut self, now_ms: u64) {
        let Some(enabled_at_ms) = self.interval_enabled_at_ms else {
            return;
        };
        let elapsed_ms = now_ms.saturating_sub(enabled_at_ms);
        let samples = elapsed_ms / self.interval_resolution_ms.max(1);
        while self.interval_samples < samples {
            self.record(self.interval_resolution_ms.max(1) * 1_000_000);
            self.interval_samples += 1;
        }
    }
}

#[derive(Clone)]
pub(crate) struct HistogramTransfer {
    kind: i64,
    state: Arc<Mutex<NativeHistogram>>,
}

impl HistogramTransfer {
    fn lock(&self) -> Option<MutexGuard<'_, NativeHistogram>> {
        self.state.lock().ok()
    }

    fn for_clone(&self) -> Self {
        Self {
            kind: if self.kind == INTERVAL_HISTOGRAM {
                BASE_HISTOGRAM
            } else {
                self.kind
            },
            state: Arc::clone(&self.state),
        }
    }
}

pub(crate) fn ensure_bridge(state: &mut NativeAgentState) -> Option<i64> {
    if let Some(bridge) = state.node_perf_hooks.bridge {
        return Some(bridge);
    }
    let methods = [
        ("cloneDetail", NodePerfHooksCallable::CloneDetail),
        ("drainNativeEntry", NodePerfHooksCallable::DrainNativeEntry),
        (
            "eventLoopDelayCreate",
            NodePerfHooksCallable::EventLoopDelayCreate,
        ),
        (
            "eventLoopDelayDisable",
            NodePerfHooksCallable::EventLoopDelayDisable,
        ),
        (
            "eventLoopDelayEnable",
            NodePerfHooksCallable::EventLoopDelayEnable,
        ),
        (
            "eventLoopUtilization",
            NodePerfHooksCallable::EventLoopUtilization,
        ),
        ("histogramAdd", NodePerfHooksCallable::HistogramAdd),
        ("histogramCreate", NodePerfHooksCallable::HistogramCreate),
        ("histogramKind", NodePerfHooksCallable::HistogramKind),
        (
            "histogramPercentile",
            NodePerfHooksCallable::HistogramPercentile,
        ),
        (
            "histogramPercentiles",
            NodePerfHooksCallable::HistogramPercentiles,
        ),
        ("histogramRecord", NodePerfHooksCallable::HistogramRecord),
        (
            "histogramRecordDelta",
            NodePerfHooksCallable::HistogramRecordDelta,
        ),
        ("histogramReset", NodePerfHooksCallable::HistogramReset),
        ("histogramStats", NodePerfHooksCallable::HistogramStats),
        ("nodeTiming", NodePerfHooksCallable::NodeTiming),
        (
            "registerHistogramPrototypes",
            NodePerfHooksCallable::RegisterHistogramPrototypes,
        ),
        (
            "setNativeConverter",
            NodePerfHooksCallable::SetNativeConverter,
        ),
        (
            "setNativeDispatcher",
            NodePerfHooksCallable::SetNativeDispatcher,
        ),
        ("setObserverState", NodePerfHooksCallable::SetObserverState),
        ("timeOrigin", NodePerfHooksCallable::TimeOrigin),
    ];
    let bridge = state.allocate_object(methods.len() as u32, false).ok()?;
    for (name, method) in methods {
        let callable = state.native_callable(NativeCallableKind::NodePerfHooks(method))?;
        modules::set_named_property(state, bridge, name, callable).ok()?;
    }
    ensure_origin(state);
    state.node_perf_hooks.bridge = Some(bridge);
    Some(bridge)
}

pub(crate) fn ensure_performance(state: &mut NativeAgentState) -> Option<i64> {
    if let Some(performance) = state.node_perf_hooks.performance {
        return Some(performance);
    }
    ensure_origin(state);
    let performance = state.allocate_object(1, false).ok()?;
    let now = state.native_callable(NativeCallableKind::NodePerfHooks(
        NodePerfHooksCallable::PerformanceNow,
    ))?;
    modules::set_named_property(state, performance, "now", now).ok()?;
    state.node_perf_hooks.performance = Some(performance);
    Some(performance)
}

fn ensure_origin(state: &mut NativeAgentState) {
    if state.node_perf_hooks.origin.is_some() {
        return;
    }
    state.node_perf_hooks.origin = Some(Instant::now());
    state.node_perf_hooks.time_origin_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |duration| duration.as_secs_f64() * 1000.0);
}

fn now_ms(state: &mut NativeAgentState) -> f64 {
    ensure_origin(state);
    state
        .node_perf_hooks
        .origin
        .map_or(0.0, |origin| origin.elapsed().as_secs_f64() * 1000.0)
}

pub(super) fn dispatch_perf(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    let _ = (ctx, builtin, args);
    Some(performance_now(state))
}

pub(crate) fn performance_now(state: &mut NativeAgentState) -> i64 {
    value::encode_f64(now_ms(state))
}

pub(crate) fn call(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    callable: NodePerfHooksCallable,
    args: &[i64],
) -> i64 {
    match callable {
        NodePerfHooksCallable::CloneDetail => args
            .first()
            .copied()
            .map_or_else(value::encode_null, |value| {
                structured_clone::clone_value(ctx, state, value)
            }),
        NodePerfHooksCallable::DrainNativeEntry => state
            .node_perf_hooks
            .native_entries
            .pop_front()
            .unwrap_or_else(value::encode_undefined),
        NodePerfHooksCallable::EventLoopDelayCreate => {
            create_histogram(state, args, true).unwrap_or_else(|| fail_dispatch(ctx))
        }
        NodePerfHooksCallable::EventLoopDelayDisable => {
            interval_toggle(state, args, false).unwrap_or_else(|| fail_dispatch(ctx))
        }
        NodePerfHooksCallable::EventLoopDelayEnable => {
            interval_toggle(state, args, true).unwrap_or_else(|| fail_dispatch(ctx))
        }
        NodePerfHooksCallable::EventLoopUtilization => {
            event_loop_utilization(state).unwrap_or_else(|| fail_dispatch(ctx))
        }
        NodePerfHooksCallable::HistogramAdd => {
            histogram_add(state, args).unwrap_or_else(|| fail_dispatch(ctx))
        }
        NodePerfHooksCallable::HistogramCreate => {
            create_histogram(state, args, false).unwrap_or_else(|| fail_dispatch(ctx))
        }
        NodePerfHooksCallable::HistogramKind => histogram_kind(ctx, state, args),
        NodePerfHooksCallable::HistogramPercentile => {
            histogram_percentile(state, args).unwrap_or_else(|| fail_dispatch(ctx))
        }
        NodePerfHooksCallable::HistogramPercentiles => {
            histogram_percentiles(state, args).unwrap_or_else(|| fail_dispatch(ctx))
        }
        NodePerfHooksCallable::HistogramRecord => {
            histogram_record(state, args).unwrap_or_else(|| fail_dispatch(ctx))
        }
        NodePerfHooksCallable::HistogramRecordDelta => {
            histogram_record_delta(state, args).unwrap_or_else(|| fail_dispatch(ctx))
        }
        NodePerfHooksCallable::HistogramReset => {
            histogram_reset(state, args).unwrap_or_else(|| fail_dispatch(ctx))
        }
        NodePerfHooksCallable::HistogramStats => {
            histogram_stats(state, args).unwrap_or_else(|| fail_dispatch(ctx))
        }
        NodePerfHooksCallable::NodeTiming => {
            node_timing(state).unwrap_or_else(|| fail_dispatch(ctx))
        }
        NodePerfHooksCallable::PerformanceNow => value::encode_f64(now_ms(state)),
        NodePerfHooksCallable::RegisterHistogramPrototypes => {
            for (slot, prototype) in state
                .node_perf_hooks
                .histogram_prototypes
                .iter_mut()
                .zip(args.iter().copied())
            {
                *slot = object_handle(prototype);
            }
            state.node_perf_hooks.histogram_brand = args
                .get(3)
                .copied()
                .filter(|symbol| value::is_symbol(*symbol));
            state.node_perf_hooks.histogram_kind_key = args
                .get(4)
                .copied()
                .filter(|symbol| value::is_symbol(*symbol));
            state.node_perf_hooks.histogram_map_key = args
                .get(5)
                .copied()
                .filter(|symbol| value::is_symbol(*symbol));
            value::encode_undefined()
        }
        NodePerfHooksCallable::SetNativeConverter => {
            state.node_perf_hooks.converter = args
                .first()
                .copied()
                .filter(|value| value::is_callable(*value));
            value::encode_undefined()
        }
        NodePerfHooksCallable::SetNativeDispatcher => {
            state.node_perf_hooks.dispatcher = args
                .first()
                .copied()
                .filter(|value| value::is_callable(*value));
            value::encode_undefined()
        }
        NodePerfHooksCallable::SetObserverState => {
            state.node_perf_hooks.observer_mask = args
                .first()
                .filter(|value| value::is_f64(**value))
                .map_or(0, |value| value::decode_f64(*value) as u32);
            if let Some(callback) = args
                .get(1)
                .copied()
                .filter(|value| value::is_callable(*value))
            {
                state.node_perf_hooks.observer_callback = Some(callback);
            }
            value::encode_undefined()
        }
        NodePerfHooksCallable::TimeOrigin => {
            ensure_origin(state);
            value::encode_f64(state.node_perf_hooks.time_origin_ms)
        }
    }
}

fn create_histogram(state: &mut NativeAgentState, args: &[i64], interval: bool) -> Option<i64> {
    let object = state.allocate_object(0, false).ok()?;
    let handle = value::decode_handle(object);
    let kind = if interval {
        INTERVAL_HISTOGRAM
    } else {
        RECORDABLE_HISTOGRAM
    };
    let prototype_index = usize::try_from(kind).ok()?;
    if let Some(prototype) = state.node_perf_hooks.histogram_prototypes[prototype_index] {
        state.heap.set_prototype(handle, prototype).ok()?;
    }
    let highest = if interval {
        u64::MAX
    } else {
        args.get(1)
            .and_then(|value| integer_value(state, *value))
            .unwrap_or(u64::MAX)
    };
    let resolution = if interval {
        args.first()
            .and_then(|value| integer_value(state, *value))
            .unwrap_or(10)
    } else {
        0
    };
    state.node_perf_hooks.histograms.insert(
        handle,
        HistogramTransfer {
            kind,
            state: Arc::new(Mutex::new(NativeHistogram {
                highest,
                values: Vec::new(),
                count: 0,
                exceeds: 0,
                delta_origin: None,
                interval_resolution_ms: resolution.max(1),
                interval_enabled_at_ms: None,
                interval_samples: 0,
            })),
        },
    );
    Some(object)
}

pub(crate) fn transfer_histogram(
    state: &NativeAgentState,
    encoded: i64,
) -> Option<HistogramTransfer> {
    value::is_js_object(encoded)
        .then(|| value::decode_handle(encoded))
        .and_then(|handle| state.node_perf_hooks.histograms.get(&handle))
        .cloned()
        .map(|histogram| histogram.for_clone())
}

pub(crate) fn materialize_histogram(
    state: &mut NativeAgentState,
    histogram: HistogramTransfer,
) -> Result<i64, String> {
    let kind = histogram.kind;
    let prototype = usize::try_from(kind)
        .ok()
        .and_then(|index| {
            state
                .node_perf_hooks
                .histogram_prototypes
                .get(index)
                .copied()
                .flatten()
        })
        .ok_or_else(|| "DataCloneError: histogram prototype is unavailable".to_string())?;
    let (brand, kind_key, map_key) = match (
        state.node_perf_hooks.histogram_brand,
        state.node_perf_hooks.histogram_kind_key,
        state.node_perf_hooks.histogram_map_key,
    ) {
        (Some(brand), Some(kind_key), Some(map_key)) => (brand, kind_key, map_key),
        _ => return Err("DataCloneError: histogram branding is unavailable".to_string()),
    };
    let map = collections::create_map(state)
        .ok_or_else(|| "DataCloneError: histogram percentile map allocation failed".to_string())?;
    let object = state
        .allocate_object(3, false)
        .map_err(|error| error.to_string())?;
    let handle = value::decode_handle(object);
    state
        .heap
        .set_prototype(handle, prototype)
        .map_err(|error| error.to_string())?;
    for (key, stored) in [
        (brand, value::encode_bool(true)),
        (kind_key, value::encode_f64(kind as f64)),
        (map_key, map),
    ] {
        let key = runtime::property_key(state, key)
            .ok_or_else(|| "DataCloneError: histogram symbol key is unavailable".to_string())?;
        state
            .heap
            .set_property(handle, key, stored as u64)
            .map_err(|error| error.to_string())?;
    }
    state.node_perf_hooks.histograms.insert(handle, histogram);
    Ok(object)
}

fn histogram_handle(state: &NativeAgentState, args: &[i64]) -> Option<u32> {
    let mut receiver = args.first().copied()?;
    for _ in 0..64 {
        if !value::is_proxy(receiver) {
            break;
        }
        let proxy = state
            .proxies
            .get(usize::try_from(value::decode_proxy_handle(receiver)).ok()?)
            .and_then(|proxy| proxy.as_ref())?;
        if proxy.revoked {
            return None;
        }
        receiver = proxy.target;
    }
    if value::is_proxy(receiver) {
        return None;
    }
    let mut handle = object_handle(receiver)?;
    loop {
        if state.node_perf_hooks.histograms.contains_key(&handle) {
            return Some(handle);
        }
        handle = state.heap.prototype(handle).ok()?;
        if handle == u32::MAX {
            return None;
        }
    }
}

fn histogram_kind(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(receiver) = args.first().copied() else {
        return value::encode_f64(-1.0);
    };
    let Some(brand) = state.node_perf_hooks.histogram_brand else {
        return value::encode_f64(-1.0);
    };
    let branded = match runtime::get_property(ctx, state, receiver, brand) {
        Ok(value) => value,
        Err(()) => return fail_dispatch(ctx),
    };
    if value::is_exception(branded) {
        return branded;
    }
    if !value::is_bool(branded) || !value::decode_bool(branded) {
        return value::encode_f64(-1.0);
    }
    value::encode_f64(
        histogram_handle(state, args)
            .and_then(|handle| state.node_perf_hooks.histograms.get(&handle))
            .map_or(-1.0, |histogram| histogram.kind as f64),
    )
}

fn integer_value(state: &NativeAgentState, encoded: i64) -> Option<u64> {
    if value::is_f64(encoded) {
        let number = value::decode_f64(encoded);
        (number.is_finite() && number >= 0.0).then(|| number as u64)
    } else {
        bigint::read(state, encoded)?.to_u64()
    }
}

fn histogram_record(state: &mut NativeAgentState, args: &[i64]) -> Option<i64> {
    let handle = histogram_handle(state, args)?;
    let sample = integer_value(state, *args.get(1)?)?;
    let histogram = state.node_perf_hooks.histograms.get(&handle)?.clone();
    histogram.lock()?.record(sample);
    Some(value::encode_undefined())
}

fn histogram_record_delta(state: &mut NativeAgentState, args: &[i64]) -> Option<i64> {
    let handle = histogram_handle(state, args)?;
    let histogram = state.node_perf_hooks.histograms.get(&handle)?.clone();
    let mut histogram = histogram.lock()?;
    let now = Instant::now();
    if let Some(origin) = histogram.delta_origin.replace(now) {
        histogram.record(
            now.duration_since(origin)
                .as_nanos()
                .min(u128::from(u64::MAX)) as u64,
        );
    }
    Some(value::encode_undefined())
}

fn histogram_add(state: &mut NativeAgentState, args: &[i64]) -> Option<i64> {
    let now_ms = state.timer_now_ms;
    let target = histogram_handle(state, args)?;
    let source = histogram_handle(state, args.get(1..)?)?;
    let source = state.node_perf_hooks.histograms.get(&source)?.clone();
    let (values, count, exceeds, delta_origin) = {
        let mut source = source.lock()?;
        source.sample_interval(now_ms);
        (
            source.values.clone(),
            source.count,
            source.exceeds,
            source.delta_origin,
        )
    };
    let target = state.node_perf_hooks.histograms.get(&target)?.clone();
    let mut target = target.lock()?;
    target.count = target.count.saturating_add(count);
    target.exceeds = target.exceeds.saturating_add(exceeds);
    target.delta_origin = target.delta_origin.max(delta_origin);
    let highest = target.highest;
    target
        .values
        .extend(values.into_iter().filter(|sample| *sample <= highest));
    Some(value::encode_undefined())
}

fn histogram_reset(state: &mut NativeAgentState, args: &[i64]) -> Option<i64> {
    let now_ms = state.timer_now_ms;
    let histogram = state
        .node_perf_hooks
        .histograms
        .get(&histogram_handle(state, args)?)?
        .clone();
    let mut histogram = histogram.lock()?;
    histogram.values.clear();
    histogram.count = 0;
    histogram.exceeds = 0;
    histogram.delta_origin = None;
    histogram.interval_samples = 0;
    histogram.interval_enabled_at_ms = histogram.interval_enabled_at_ms.map(|_| now_ms);
    Some(value::encode_undefined())
}

fn interval_toggle(state: &mut NativeAgentState, args: &[i64], enabled: bool) -> Option<i64> {
    let now_ms = state.timer_now_ms;
    let histogram = state
        .node_perf_hooks
        .histograms
        .get(&histogram_handle(state, args)?)?
        .clone();
    if histogram.kind != INTERVAL_HISTOGRAM {
        return None;
    }
    let mut histogram = histogram.lock()?;

    if enabled {
        if histogram.interval_enabled_at_ms.is_some() {
            return Some(value::encode_bool(false));
        }
        histogram.interval_enabled_at_ms = Some(now_ms);
        histogram.interval_samples = 0;
    } else {
        if histogram.interval_enabled_at_ms.is_none() {
            return Some(value::encode_bool(false));
        }
        histogram.sample_interval(now_ms);
        histogram.interval_enabled_at_ms = None;
    }
    Some(value::encode_bool(true))
}

fn histogram_stats(state: &mut NativeAgentState, args: &[i64]) -> Option<i64> {
    let now_ms = state.timer_now_ms;
    let handle = histogram_handle(state, args)?;
    let histogram = state.node_perf_hooks.histograms.get(&handle)?.clone();
    let (count, minimum, maximum, mean, stddev, exceeds) = {
        let mut histogram = histogram.lock()?;
        histogram.sample_interval(now_ms);
        let count = histogram.count;
        let sample_count = histogram.values.len() as u64;
        let minimum = histogram
            .values
            .iter()
            .copied()
            .min()
            .unwrap_or(EMPTY_MINIMUM);
        let maximum = histogram.values.iter().copied().max().unwrap_or(0);
        let mean = if sample_count == 0 {
            f64::NAN
        } else {
            histogram
                .values
                .iter()
                .map(|value| *value as f64)
                .sum::<f64>()
                / sample_count as f64
        };
        let stddev = if sample_count == 0 {
            f64::NAN
        } else {
            let variance = histogram
                .values
                .iter()
                .map(|value| (*value as f64 - mean).powi(2))
                .sum::<f64>()
                / sample_count as f64;
            variance.sqrt()
        };
        (count, minimum, maximum, mean, stddev, histogram.exceeds)
    };
    let count_big = bigint::store(state, count.into())?;
    let minimum_big = bigint::store(state, minimum.into())?;
    let maximum_big = bigint::store(state, maximum.into())?;
    let exceeds_big = bigint::store(state, exceeds.into())?;
    create_object(
        state,
        [
            ("count", value::encode_f64(count as f64)),
            ("countBigInt", count_big),
            ("min", value::encode_f64(minimum as f64)),
            ("minBigInt", minimum_big),
            ("max", value::encode_f64(maximum as f64)),
            ("maxBigInt", maximum_big),
            ("mean", value::encode_f64(mean)),
            ("stddev", value::encode_f64(stddev)),
            ("exceeds", value::encode_f64(exceeds as f64)),
            ("exceedsBigInt", exceeds_big),
        ],
    )
}

fn histogram_percentile(state: &mut NativeAgentState, args: &[i64]) -> Option<i64> {
    let now_ms = state.timer_now_ms;
    let handle = histogram_handle(state, args)?;
    let percentile = args
        .get(1)
        .filter(|value| value::is_f64(**value))
        .map(|value| value::decode_f64(*value))?;
    let bigint_result = args
        .get(2)
        .is_some_and(|value| value::is_bool(*value) && value::decode_bool(*value));
    let histogram = state.node_perf_hooks.histograms.get(&handle)?.clone();
    let sample = {
        let mut histogram = histogram.lock()?;
        histogram.sample_interval(now_ms);
        percentile_value(&histogram.values, percentile)
    };
    if bigint_result {
        bigint::store(state, sample.into())
    } else {
        Some(value::encode_f64(sample as f64))
    }
}

fn histogram_percentiles(state: &mut NativeAgentState, args: &[i64]) -> Option<i64> {
    let now_ms = state.timer_now_ms;
    let handle = histogram_handle(state, args)?;
    let bigint_result = args
        .get(1)
        .is_some_and(|value| value::is_bool(*value) && value::decode_bool(*value));
    let histogram = state.node_perf_hooks.histograms.get(&handle)?.clone();
    let values = {
        let mut histogram = histogram.lock()?;
        histogram.sample_interval(now_ms);
        [0.0, 25.0, 50.0, 75.0, 90.0, 95.0, 99.0, 100.0]
            .into_iter()
            .map(|percentile| (percentile, percentile_value(&histogram.values, percentile)))
            .collect::<Vec<_>>()
    };
    let mut flattened = Vec::with_capacity(values.len() * 2);
    for (percentile, sample) in values {
        flattened.push(value::encode_f64(percentile));
        flattened.push(if bigint_result {
            bigint::store(state, sample.into())?
        } else {
            value::encode_f64(sample as f64)
        });
    }
    state.allocate_array_values(&flattened).ok()
}

fn percentile_value(values: &[u64], percentile: f64) -> u64 {
    if values.is_empty() {
        return 0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let rank = ((percentile / 100.0) * sorted.len() as f64).ceil().max(1.0) as usize - 1;
    sorted[rank.min(sorted.len() - 1)]
}

fn node_timing(state: &mut NativeAgentState) -> Option<i64> {
    let now = now_ms(state);
    create_object(
        state,
        [
            ("loopCount", value::encode_f64(1.0)),
            ("events", value::encode_f64(0.0)),
            ("eventsWaiting", value::encode_f64(0.0)),
            ("nodeStart", value::encode_f64(0.0)),
            ("v8Start", value::encode_f64(0.0)),
            ("environment", value::encode_f64(0.0)),
            ("loopStart", value::encode_f64(0.0)),
            ("loopExit", value::encode_f64(-1.0)),
            ("bootstrapComplete", value::encode_f64(0.0)),
            ("idleTime", value::encode_f64(0.0)),
            ("duration", value::encode_f64(now)),
        ],
    )
}

fn event_loop_utilization(state: &mut NativeAgentState) -> Option<i64> {
    let active = now_ms(state);
    create_object(
        state,
        [
            ("idle", value::encode_f64(0.0)),
            ("active", value::encode_f64(active)),
            (
                "utilization",
                value::encode_f64(if active == 0.0 { 0.0 } else { 1.0 }),
            ),
        ],
    )
}

fn create_object<const N: usize>(
    state: &mut NativeAgentState,
    properties: [(&str, i64); N],
) -> Option<i64> {
    let object = state.allocate_object(N as u32, false).ok()?;
    for (name, stored) in properties {
        modules::set_named_property(state, object, name, stored).ok()?;
    }
    Some(object)
}

pub(crate) fn emit_gc_entry(ctx: &mut NativeVmContext, state: &mut NativeAgentState) {
    let Some(detail) = create_object(
        state,
        [
            ("kind", value::encode_f64(4.0)),
            ("flags", value::encode_f64(4.0)),
        ],
    ) else {
        return;
    };
    emit_native_entry(ctx, state, "gc", "gc", detail);
}

pub(crate) fn emit_net_entry(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    host: &str,
    port: u16,
) {
    let Some(host) = state.intern_text(host.into(), value::TAG_STRING) else {
        return;
    };
    let Some(detail) = create_object(
        state,
        [("host", host), ("port", value::encode_f64(f64::from(port)))],
    ) else {
        return;
    };
    emit_native_entry(ctx, state, "connect", "net", detail);
}

pub(crate) struct FetchBodySizes {
    pub(crate) encoded: usize,
    pub(crate) decoded: usize,
}

pub(crate) fn emit_fetch_resource_entry(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    url: &str,
    status: u16,
    body_sizes: FetchBodySizes,
) {
    let now = now_ms(state);
    let Some(timing_info) = create_object(
        state,
        [
            ("startTime", value::encode_f64(now)),
            ("redirectStartTime", value::encode_f64(0.0)),
            ("redirectEndTime", value::encode_f64(0.0)),
            ("postRedirectStartTime", value::encode_f64(now)),
            ("finalServiceWorkerStartTime", value::encode_f64(0.0)),
            ("finalNetworkRequestStartTime", value::encode_f64(now)),
            ("finalNetworkResponseStartTime", value::encode_f64(now)),
            ("endTime", value::encode_f64(now)),
            (
                "encodedBodySize",
                value::encode_f64(body_sizes.encoded as f64),
            ),
            (
                "decodedBodySize",
                value::encode_f64(body_sizes.decoded as f64),
            ),
            ("finalConnectionTimingInfo", value::encode_undefined()),
        ],
    ) else {
        return;
    };
    let Some(initiator_type) = state.intern_text("fetch".into(), value::TAG_STRING) else {
        return;
    };
    let Some(empty) = state.intern_text(String::new(), value::TAG_STRING) else {
        return;
    };
    let Some(detail) = create_object(
        state,
        [
            ("timingInfo", timing_info),
            ("initiatorType", initiator_type),
            ("cacheMode", empty),
            ("responseStatus", value::encode_f64(f64::from(status))),
            ("deliveryType", empty),
        ],
    ) else {
        return;
    };
    emit_native_entry(ctx, state, url, "resource", detail);
}

pub(crate) fn emit_native_entry(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    name: &str,
    entry_type: &str,
    detail: i64,
) {
    let mask = match entry_type {
        "dns" => 1,
        "function" => 2,
        "gc" => 4,
        "http" => 8,
        "http2" => 16,
        "net" => 32,
        "resource" => 64,
        _ => 0,
    };
    if state.node_perf_hooks.observer_mask & mask == 0 {
        return;
    }
    let start = now_ms(state);
    let Some(name) = state.intern_text(name.into(), value::TAG_STRING) else {
        return;
    };
    let Some(entry_type) = state.intern_text(entry_type.into(), value::TAG_STRING) else {
        return;
    };
    let Some(raw) = create_object(
        state,
        [
            ("name", name),
            ("entryType", entry_type),
            ("startTime", value::encode_f64(start)),
            ("duration", value::encode_f64(0.0)),
            ("detail", detail),
        ],
    ) else {
        return;
    };
    if let Some(converter) = state.node_perf_hooks.converter {
        let _ = state.invoke_callable(ctx, converter, value::encode_undefined(), &[raw]);
        if let Some(dispatcher) = state.node_perf_hooks.dispatcher {
            let _ = state.invoke_callable(ctx, dispatcher, value::encode_undefined(), &[]);
        }
    } else {
        state.node_perf_hooks.native_entries.push_back(raw);
        if let Some(callback) = state.node_perf_hooks.observer_callback {
            let _ = state.invoke_callable(ctx, callback, value::encode_undefined(), &[]);
        }
    }
}
