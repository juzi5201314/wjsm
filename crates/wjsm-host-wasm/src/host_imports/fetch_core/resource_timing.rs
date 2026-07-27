use crate::{RuntimeState, SharedFetchResourceTiming};

pub(crate) fn record_http_response_body_bytes(
    state: &RuntimeState,
    http_handle: u32,
    encoded_size: usize,
    decoded_size: usize,
) {
    let timing = http_resource_timing(state, http_handle);
    wjsm_builtins::fetch::resource_timing::record_body_bytes(
        &timing,
        encoded_size,
        decoded_size,
    );
}

pub(crate) fn complete_http_response_resource_timing(
    state: &RuntimeState,
    http_handle: u32,
) {
    let timing = http_resource_timing(state, http_handle);
    let Some(timing) = wjsm_builtins::fetch::resource_timing::finish(&timing) else {
        return;
    };
    crate::runtime_node_perf_hooks::queue_resource_entry(
        state,
        crate::runtime_node_perf_hooks::NativeResourceTiming {
            name: timing.requested_url,
            start_time: timing.start_time,
            request_start_time: timing.request_start_time,
            response_start_time: timing.response_start_time,
            end_time: state.performance_origin.elapsed().as_secs_f64() * 1_000.0,
            response_status: timing.response_status,
            encoded_body_size: timing.encoded_body_size,
            decoded_body_size: timing.decoded_body_size,
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
