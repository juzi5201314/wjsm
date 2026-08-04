use std::sync::{Arc, Mutex};

use wjsm_host::{ExecContext, FetchResourceTimingState, SharedFetchResourceTiming};

pub fn begin<E: ExecContext>(
    ctx: &mut E,
    requested_url: String,
    suppressed: bool,
) -> Option<SharedFetchResourceTiming> {
    if suppressed
        || !ctx.fetch_resource_timing_enabled()
        || !(requested_url.starts_with("http://")
            || requested_url.starts_with("https://")
            || requested_url.starts_with("data:"))
    {
        return None;
    }
    let start_time = ctx.performance_now();
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

pub fn mark_request_start<E: ExecContext>(ctx: &mut E, timing: &Option<SharedFetchResourceTiming>) {
    mutate(timing, |entry| {
        entry.request_start_time = ctx.performance_now()
    });
}

pub fn mark_response_start<E: ExecContext>(
    ctx: &mut E,
    timing: &Option<SharedFetchResourceTiming>,
    status: u16,
) {
    let now = ctx.performance_now();
    mutate(timing, |entry| {
        entry.response_start_time = now;
        entry.response_status = status;
    });
}

pub fn record_body_bytes(
    timing: &Option<SharedFetchResourceTiming>,
    encoded_size: usize,
    decoded_size: usize,
) {
    mutate(timing, |entry| {
        entry.encoded_body_size = entry.encoded_body_size.saturating_add(encoded_size as u64);
        entry.decoded_body_size = entry.decoded_body_size.saturating_add(decoded_size as u64);
    });
}
pub fn complete<E: ExecContext>(ctx: &mut E, timing: &Option<SharedFetchResourceTiming>) {
    if let Some(snapshot) = finish(timing) {
        ctx.commit_fetch_resource_timing(&snapshot);
    }
}

pub fn finish(timing: &Option<SharedFetchResourceTiming>) -> Option<FetchResourceTimingState> {
    let timing = timing.as_ref()?;
    let mut timing = timing.lock().unwrap_or_else(|error| error.into_inner());
    if timing.completed {
        return None;
    }
    timing.completed = true;
    Some(timing.clone())
}

fn mutate(
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
