use std::sync::{Arc, Mutex};

use crate::*;

pub(crate) fn begin_fetch_resource_timing(
    state: &RuntimeState,
    requested_url: String,
    suppressed: bool,
) -> Option<SharedFetchResourceTiming> {
    if suppressed
        || !crate::runtime_node_perf_hooks::resource_entries_enabled(state)
        || !(requested_url.starts_with("http://")
            || requested_url.starts_with("https://")
            || requested_url.starts_with("data:"))
    {
        return None;
    }
    let start_time = performance_now(state);
    Some(Arc::new(Mutex::new(FetchResourceTimingState {
        requested_url,
        start_time,
        request_start_time: start_time,
        response_start_time: 0.0,
        response_status: 0,
        encoded_body_size: 0,
        decoded_body_size: 0,
        completed: false,
    })))
}

pub(crate) fn mark_fetch_request_start(
    state: &RuntimeState,
    timing: &Option<SharedFetchResourceTiming>,
) {
    mutate_timing(timing, |entry| {
        entry.request_start_time = performance_now(state);
    });
}

pub(crate) fn mark_fetch_response_start(
    state: &RuntimeState,
    timing: &Option<SharedFetchResourceTiming>,
    response_status: u16,
) {
    mutate_timing(timing, |entry| {
        entry.response_start_time = performance_now(state);
        entry.response_status = response_status;
    });
}

pub(crate) fn record_fetch_body_bytes(
    timing: &Option<SharedFetchResourceTiming>,
    encoded_size: usize,
    decoded_size: usize,
) {
    mutate_timing(timing, |entry| {
        entry.encoded_body_size = entry.encoded_body_size.saturating_add(encoded_size as u64);
        entry.decoded_body_size = entry.decoded_body_size.saturating_add(decoded_size as u64);
    });
}

pub(crate) fn record_http_response_body_bytes(
    state: &RuntimeState,
    http_handle: u32,
    encoded_size: usize,
    decoded_size: usize,
) {
    let timing = http_resource_timing(state, http_handle);
    record_fetch_body_bytes(&timing, encoded_size, decoded_size);
}

pub(crate) fn complete_http_response_resource_timing(state: &RuntimeState, http_handle: u32) {
    let timing = http_resource_timing(state, http_handle);
    complete_fetch_resource_timing(state, &timing);
}

pub(crate) fn complete_fetch_resource_timing(
    state: &RuntimeState,
    timing: &Option<SharedFetchResourceTiming>,
) {
    let Some(timing) = timing else {
        return;
    };
    let snapshot = {
        let mut timing = timing.lock().unwrap_or_else(|error| error.into_inner());
        if timing.completed {
            return;
        }
        timing.completed = true;
        (
            timing.requested_url.clone(),
            timing.start_time,
            timing.request_start_time,
            timing.response_start_time,
            timing.response_status,
            timing.encoded_body_size,
            timing.decoded_body_size,
        )
    };
    crate::runtime_node_perf_hooks::queue_resource_entry(
        state,
        crate::runtime_node_perf_hooks::NativeResourceTiming {
            name: snapshot.0,
            start_time: snapshot.1,
            request_start_time: snapshot.2,
            response_start_time: snapshot.3,
            end_time: performance_now(state),
            response_status: snapshot.4,
            encoded_body_size: snapshot.5,
            decoded_body_size: snapshot.6,
        },
    );
}

fn http_resource_timing(
    state: &RuntimeState,
    http_handle: u32,
) -> Option<SharedFetchResourceTiming> {
    state
        .http_response_table
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(http_handle as usize)
        .and_then(|entry| entry.resource_timing.clone())
}

fn mutate_timing(
    timing: &Option<SharedFetchResourceTiming>,
    mutate: impl FnOnce(&mut FetchResourceTimingState),
) {
    let Some(timing) = timing else {
        return;
    };
    let mut timing = timing.lock().unwrap_or_else(|error| error.into_inner());
    if !timing.completed {
        mutate(&mut timing);
    }
}

fn performance_now(state: &RuntimeState) -> f64 {
    state.performance_origin.elapsed().as_secs_f64() * 1_000.0
}
