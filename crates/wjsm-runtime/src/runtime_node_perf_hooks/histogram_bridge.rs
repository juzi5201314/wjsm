use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use num_traits::ToPrimitive;
use wasmtime::{AsContextMut, Caller};

use crate::runtime_node_perf_hooks::EventLoopDelayMonitor;
use crate::runtime_node_perf_hooks_histogram::{HistogramCapability, HistogramWrapperEntry};
use crate::*;

pub(crate) fn create_histogram(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let lowest = unsigned_integer_arg(caller, args.first().copied()).unwrap_or(1);
    let highest = unsigned_integer_arg(caller, args.get(1).copied())
        .unwrap_or(NumberLimit::MAX_SAFE_INTEGER as u64);
    let figures = number_arg(args, 2).unwrap_or(3.0) as u8;
    let Some(shared) = caller.data().shared_state.as_ref() else {
        return make_type_error_exception(caller, "perf_hooks shared state is unavailable");
    };
    match shared.perf_histograms.create(lowest, highest, figures) {
        Ok(capability) => {
            let env = WasmEnv::from_caller(caller).expect("WasmEnv");
            materialize_histogram(caller, &env, capability, 1)
        }
        Err(message) => make_range_error_exception(caller, &message),
    }
}

pub(crate) fn histogram_stats(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let Some(entry) = histogram_entry_arg(caller, args, 0) else {
        return invalid_histogram(caller);
    };
    let Some(shared) = caller.data().shared_state.as_ref() else {
        return invalid_histogram(caller);
    };
    let stats = match shared.perf_histograms.stats(&entry.capability) {
        Ok(stats) => stats,
        Err(message) => return make_type_error_exception(caller, &message),
    };
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let object = alloc_host_object(caller, &env, 10);
    let root_len = caller.data().push_host_temp_roots([object]);
    let count_bigint = encode_bigint(caller, stats.count);
    let min_bigint = encode_bigint(caller, stats.min);
    let max_bigint = encode_bigint(caller, stats.max);
    let exceeds_bigint = encode_bigint(caller, stats.exceeds);
    for (name, raw) in [
        ("count", value::encode_f64(stats.count as f64)),
        ("countBigInt", count_bigint),
        ("min", value::encode_f64(stats.min as f64)),
        ("minBigInt", min_bigint),
        ("max", value::encode_f64(stats.max as f64)),
        ("maxBigInt", max_bigint),
        ("mean", value::encode_f64(stats.mean)),
        ("stddev", value::encode_f64(stats.stddev)),
        ("exceeds", value::encode_f64(stats.exceeds as f64)),
        ("exceedsBigInt", exceeds_bigint),
    ] {
        let _ = define_host_data_property_from_caller(caller, object, name, raw);
    }
    caller.data().truncate_host_temp_roots(root_len);
    object
}

pub(crate) fn histogram_percentile(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let Some(entry) = histogram_entry_arg(caller, args, 0) else {
        return invalid_histogram(caller);
    };
    let percentile = number_arg(args, 1).unwrap_or(f64::NAN);
    let bigint = args
        .get(2)
        .is_some_and(|raw| value::is_bool(*raw) && value::decode_bool(*raw));
    let Some(shared) = caller.data().shared_state.as_ref() else {
        return invalid_histogram(caller);
    };
    match shared
        .perf_histograms
        .percentile(&entry.capability, percentile)
    {
        Ok(result) if bigint => encode_bigint(caller, result),
        Ok(result) => value::encode_f64(result as f64),
        Err(message) => make_range_error_exception(caller, &message),
    }
}

pub(crate) fn histogram_percentiles(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let Some(entry) = histogram_entry_arg(caller, args, 0) else {
        return invalid_histogram(caller);
    };
    let bigint = args
        .get(1)
        .is_some_and(|raw| value::is_bool(*raw) && value::decode_bool(*raw));
    let Some(shared) = caller.data().shared_state.as_ref() else {
        return invalid_histogram(caller);
    };
    let pairs = match shared.perf_histograms.percentiles(&entry.capability) {
        Ok(pairs) => pairs,
        Err(message) => return make_type_error_exception(caller, &message),
    };
    let array = alloc_array(caller, (pairs.len() * 2) as u32);
    let root_len = caller.data().push_host_temp_roots([array]);
    for (index, (percentile, result)) in pairs.into_iter().enumerate() {
        set_array_elem(
            caller,
            array,
            (index * 2) as i32,
            value::encode_f64(percentile),
        );
        let raw = if bigint {
            encode_bigint(caller, result)
        } else {
            value::encode_f64(result as f64)
        };
        set_array_elem(caller, array, (index * 2 + 1) as i32, raw);
    }
    caller.data().truncate_host_temp_roots(root_len);
    array
}

pub(crate) fn histogram_record(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let Some(entry) = histogram_entry_arg(caller, args, 0).filter(|entry| entry.kind == 1) else {
        return invalid_histogram(caller);
    };
    let Some(recorded) = integer_value(caller, args.get(1).copied()) else {
        return make_range_error_exception(caller, "histogram value must be a positive integer");
    };
    mutate_histogram(caller, |shared| {
        shared.perf_histograms.record(&entry.capability, recorded)
    })
}

pub(crate) fn histogram_record_delta(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let Some(entry) = histogram_entry_arg(caller, args, 0).filter(|entry| entry.kind == 1) else {
        return invalid_histogram(caller);
    };
    mutate_histogram(caller, |shared| {
        shared
            .perf_histograms
            .record_delta(&entry.capability, Instant::now())
    })
}

pub(crate) fn histogram_add(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let (Some(target), Some(source)) = (
        histogram_entry_arg(caller, args, 0).filter(|entry| entry.kind == 1),
        histogram_entry_arg(caller, args, 1).filter(|entry| entry.kind == 1),
    ) else {
        return invalid_histogram(caller);
    };
    mutate_histogram(caller, |shared| {
        shared
            .perf_histograms
            .add(&target.capability, &source.capability)
    })
}

pub(crate) fn histogram_reset(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let Some(entry) = histogram_entry_arg(caller, args, 0) else {
        return invalid_histogram(caller);
    };
    mutate_histogram(caller, |shared| {
        shared.perf_histograms.reset(&entry.capability)
    })
}

pub(crate) fn histogram_kind(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    histogram_entry_arg(caller, args, 0)
        .map(|entry| value::encode_f64(entry.kind as f64))
        .unwrap_or_else(|| value::encode_f64(-1.0))
}

pub(crate) fn event_loop_delay_create(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let resolution = number_arg(args, 0).unwrap_or(10.0) as u64;
    let capability = {
        let Some(shared) = caller.data().shared_state.as_ref() else {
            return make_type_error_exception(caller, "perf_hooks shared state is unavailable");
        };
        match shared
            .perf_histograms
            .create(1_000, NumberLimit::MAX_SAFE_INTEGER as u64, 3)
        {
            Ok(capability) => capability,
            Err(message) => return make_range_error_exception(caller, &message),
        }
    };
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let wrapper = materialize_histogram_with_kind(caller, &env, capability.clone(), 2);
    let Some(side_handle) = histogram_side_handle_arg(caller, &[wrapper], 0) else {
        return make_type_error_exception(caller, "failed to materialize event loop histogram");
    };
    caller
        .data()
        .performance_event_loop_monitors
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(
            side_handle,
            EventLoopDelayMonitor {
                capability,
                resolution: Duration::from_millis(resolution.max(1)),
                next_deadline: None,
                enabled: false,
            },
        );
    wrapper
}

pub(crate) fn event_loop_delay_enable(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let Some(_) = histogram_entry_arg(caller, args, 0).filter(|entry| entry.kind == 2) else {
        return invalid_histogram(caller);
    };
    let Some(side_handle) = histogram_side_handle_arg(caller, args, 0) else {
        return invalid_histogram(caller);
    };
    let mut monitors = caller
        .data()
        .performance_event_loop_monitors
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let Some(monitor) = monitors.get_mut(&side_handle) else {
        drop(monitors);
        return invalid_histogram(caller);
    };
    if monitor.enabled {
        return value::encode_bool(false);
    }
    let baseline_error = caller
        .data()
        .shared_state
        .as_ref()
        .ok_or_else(|| "perf_hooks shared state is unavailable".to_string())
        .and_then(|shared| {
            shared
                .perf_histograms
                .clear_delta_baseline(&monitor.capability)
        });
    if let Err(message) = baseline_error {
        drop(monitors);
        return make_type_error_exception(caller, &message);
    }
    monitor.enabled = true;
    monitor.next_deadline = Some(Instant::now() + monitor.resolution);
    value::encode_bool(true)
}

pub(crate) fn event_loop_delay_disable(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let Some(_) = histogram_entry_arg(caller, args, 0).filter(|entry| entry.kind == 2) else {
        return invalid_histogram(caller);
    };
    let Some(side_handle) = histogram_side_handle_arg(caller, args, 0) else {
        return invalid_histogram(caller);
    };
    let mut monitors = caller
        .data()
        .performance_event_loop_monitors
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let Some(monitor) = monitors.get_mut(&side_handle) else {
        drop(monitors);
        return invalid_histogram(caller);
    };
    if !monitor.enabled {
        return value::encode_bool(false);
    }
    monitor.enabled = false;
    monitor.next_deadline = None;
    value::encode_bool(true)
}

pub(crate) fn register_histogram_prototypes(
    caller: &mut Caller<'_, RuntimeState>,
    args: &[i64],
) -> i64 {
    let base = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let recordable = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    let interval = args.get(2).copied().unwrap_or_else(value::encode_undefined);
    caller
        .data()
        .performance_histogram_base_prototype
        .store(base, Ordering::Relaxed);
    caller
        .data()
        .performance_histogram_recordable_prototype
        .store(recordable, Ordering::Relaxed);
    caller
        .data()
        .performance_histogram_interval_prototype
        .store(interval, Ordering::Relaxed);
    restore_registered_histogram_prototypes(caller);
    value::encode_undefined()
}

pub(crate) fn histogram_identity_from_object(
    caller: &mut Caller<'_, RuntimeState>,
    object: i64,
) -> Option<(HistogramCapability, u8)> {
    let (_, entry) = exact_histogram_entry(caller, object)?;
    Some((entry.capability, entry.kind))
}

pub(crate) fn materialize_histogram<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    capability: HistogramCapability,
    kind: u8,
) -> i64 {
    let cloned_kind = if kind == 2 { 0 } else { kind };
    materialize_histogram_with_kind(ctx, env, capability, cloned_kind)
}

fn materialize_histogram_with_kind<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    capability: HistogramCapability,
    kind: u8,
) -> i64 {
    if kind > 2 {
        return value::encode_undefined();
    }
    let wrappers = ctx
        .as_context()
        .data()
        .performance_histogram_wrappers
        .clone();
    let object = alloc_host_object(ctx, env, 1);
    let root_len = ctx.as_context().data().push_host_temp_roots([object]);
    let side_handle = wrappers.alloc(HistogramWrapperEntry { capability, kind });
    wrappers.bind_obj_handle(value::decode_object_handle(object), side_handle);
    let prototype = histogram_prototype(ctx.as_context().data(), kind);
    if value::is_object(prototype) {
        crate::runtime_heap::set_object_proto_header(ctx, env, object, prototype);
    }
    ctx.as_context().data().truncate_host_temp_roots(root_len);
    object
}

fn restore_registered_histogram_prototypes(caller: &mut Caller<'_, RuntimeState>) {
    let bindings = caller
        .data()
        .performance_histogram_wrappers
        .object_bindings();
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    for (object_handle, side_handle) in bindings {
        let entry = {
            let inner = caller
                .data()
                .performance_histogram_wrappers
                .inner
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            inner.get(side_handle as usize).cloned()
        };
        let Some(entry) = entry else {
            continue;
        };
        let prototype = histogram_prototype(caller.data(), entry.kind);
        if value::is_object(prototype) {
            crate::runtime_heap::set_object_proto_header(
                caller,
                &env,
                value::encode_object_handle(object_handle),
                prototype,
            );
        }
    }
}

fn histogram_prototype(state: &RuntimeState, kind: u8) -> i64 {
    match kind {
        1 => state
            .performance_histogram_recordable_prototype
            .load(Ordering::Relaxed),
        2 => state
            .performance_histogram_interval_prototype
            .load(Ordering::Relaxed),
        _ => state
            .performance_histogram_base_prototype
            .load(Ordering::Relaxed),
    }
}

fn histogram_entry_arg(
    caller: &mut Caller<'_, RuntimeState>,
    args: &[i64],
    index: usize,
) -> Option<HistogramWrapperEntry> {
    histogram_entry_for_receiver(caller, args.get(index).copied()?).map(|(_, entry)| entry)
}

fn histogram_side_handle_arg(
    caller: &mut Caller<'_, RuntimeState>,
    args: &[i64],
    index: usize,
) -> Option<u32> {
    histogram_entry_for_receiver(caller, args.get(index).copied()?)
        .map(|(side_handle, _)| side_handle)
}

fn exact_histogram_entry(
    caller: &mut Caller<'_, RuntimeState>,
    object: i64,
) -> Option<(u32, HistogramWrapperEntry)> {
    if !value::is_object(object) {
        return None;
    }
    let object_handle = value::decode_object_handle(object);
    let side_handle = caller
        .data()
        .performance_histogram_wrappers
        .side_handle_for_obj(object_handle)?;
    let inner = caller
        .data()
        .performance_histogram_wrappers
        .inner
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    inner
        .get(side_handle as usize)
        .cloned()
        .map(|entry| (side_handle, entry))
}

fn histogram_entry_for_receiver(
    caller: &mut Caller<'_, RuntimeState>,
    receiver: i64,
) -> Option<(u32, HistogramWrapperEntry)> {
    let mut current = receiver;
    let mut visited = HashSet::new();
    loop {
        if !visited.insert(current) {
            return None;
        }
        if value::is_proxy(current) {
            current = transparent_proxy_target(caller, current)?;
            continue;
        }
        if let Some(entry) = exact_histogram_entry(caller, current) {
            return Some(entry);
        }
        current = object_prototype(caller, current)?;
    }
}

fn transparent_proxy_target(caller: &mut Caller<'_, RuntimeState>, proxy: i64) -> Option<i64> {
    let entry = {
        let table = caller
            .data()
            .proxy_table
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        table
            .get(value::decode_proxy_handle(proxy) as usize)
            .cloned()
    }?;
    if entry.revoked || proxy_handler_has_get_trap(caller, entry.handler) {
        return None;
    }
    Some(entry.target)
}

fn proxy_handler_has_get_trap(caller: &mut Caller<'_, RuntimeState>, handler: i64) -> bool {
    let Some(pointer) = resolve_handle(caller, handler) else {
        return true;
    };
    let mut visited = HashSet::new();
    read_object_property_by_name_proto_walk(caller, pointer, "get", &mut visited)
        .is_some_and(|trap| !value::is_undefined(trap))
}

fn object_prototype(caller: &mut Caller<'_, RuntimeState>, object: i64) -> Option<i64> {
    if !value::is_object(object) && !value::is_array(object) {
        return None;
    }
    // V2 heap：handle 解析为 handle id，原型必须走 HeapAccessV2，不能读线性内存 header。
    #[cfg(feature = "managed-heap-v2")]
    {
        let handle = value::decode_handle(object);
        let access = caller.data().heap_access_v2().clone();
        if access.resolve_handle(handle).is_ok() {
            let prototype = access.prototype(handle).ok()?;
            if prototype == 0 || prototype == u32::MAX {
                return None;
            }
            if prototype & 0x8000_0000 != 0 {
                return Some(value::encode_proxy_handle(prototype & 0x7FFF_FFFF));
            }
            return Some(crate::runtime_host_helpers::prototype_handle_to_value(
                caller, prototype,
            ));
        }
    }
    let pointer = resolve_handle(caller, object)?;
    let env = WasmEnv::from_caller(caller)?;
    let prototype_handle = {
        let data = env.memory.data(&*caller);
        if pointer + 4 > data.len() {
            return None;
        }
        u32::from_le_bytes([
            data[pointer],
            data[pointer + 1],
            data[pointer + 2],
            data[pointer + 3],
        ])
    };
    if prototype_handle == 0 || prototype_handle == u32::MAX {
        return None;
    }
    if prototype_handle & 0x8000_0000 != 0 {
        return Some(value::encode_proxy_handle(prototype_handle & 0x7FFF_FFFF));
    }
    Some(crate::runtime_host_helpers::prototype_handle_to_value(
        caller,
        prototype_handle,
    ))
}

fn mutate_histogram(
    caller: &mut Caller<'_, RuntimeState>,
    operation: impl FnOnce(&crate::shared_buffer::SharedRuntimeState) -> Result<(), String>,
) -> i64 {
    let Some(shared) = caller.data().shared_state.as_deref() else {
        return invalid_histogram(caller);
    };
    match operation(shared) {
        Ok(()) => value::encode_undefined(),
        Err(message) => make_type_error_exception(caller, &message),
    }
}

fn number_arg(args: &[i64], index: usize) -> Option<f64> {
    args.get(index)
        .copied()
        .filter(|raw| value::is_f64(*raw))
        .map(value::decode_f64)
}

fn integer_value(caller: &Caller<'_, RuntimeState>, raw: Option<i64>) -> Option<u64> {
    let raw = raw?;
    if value::is_f64(raw) {
        let number = value::decode_f64(raw);
        return (number.is_finite()
            && (1.0..=NumberLimit::MAX_SAFE_INTEGER).contains(&number)
            && number.fract() == 0.0)
            .then_some(number as u64);
    }
    if value::is_bigint(raw) {
        return caller.data().bigint_table.lock().ok().and_then(|table| {
            let value = table
                .get(value::decode_bigint_handle(raw) as usize)?
                .to_u64()?;
            (value <= i64::MAX as u64).then_some(value)
        });
    }
    None
}

fn unsigned_integer_arg(caller: &Caller<'_, RuntimeState>, raw: Option<i64>) -> Option<u64> {
    let raw = raw?;
    if value::is_f64(raw) {
        let number = value::decode_f64(raw);
        return (number.is_finite() && number >= 0.0 && number.fract() == 0.0)
            .then_some(number as u64);
    }
    if value::is_bigint(raw) {
        return caller.data().bigint_table.lock().ok().and_then(|table| {
            table
                .get(value::decode_bigint_handle(raw) as usize)?
                .to_u64()
        });
    }
    None
}

fn encode_bigint(caller: &Caller<'_, RuntimeState>, number: u64) -> i64 {
    let mut table = caller
        .data()
        .bigint_table
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let handle = table.len() as u32;
    table.push(num_bigint::BigInt::from(number));
    value::encode_bigint_handle(handle)
}

fn invalid_histogram(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    make_type_error_exception(caller, "invalid Histogram receiver")
}

struct NumberLimit;

impl NumberLimit {
    const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;
}
