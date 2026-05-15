use wasmtime::*;
use wjsm_ir::value;
use std::time::Duration;
use std::time::Instant;

use crate::types::*;
use crate::runtime::*;

pub(crate) fn create_host_functions(store: &mut Store<RuntimeState>) -> Vec<(usize, Func)> {
    let set_timeout_fn = Func::wrap(
        &mut *store,
        |caller: Caller<'_, RuntimeState>, callback: i64, delay: i64| -> i64 {
            let delay_f64 = if value::is_f64(delay) {
                f64::from_bits(delay as u64)
            } else {
                f64::NAN
            };
            let delay_ms: u64 = if delay_f64.is_nan() || delay_f64.is_sign_negative() {
                0
            } else if delay_f64 > (u32::MAX as f64) {
                u32::MAX as u64
            } else {
                delay_f64 as u64
            };
            let id = {
                let mut next_id = caller
                    .data()
                    .next_timer_id
                    .lock()
                    .expect("next_timer_id mutex");
                let id = *next_id;
                *next_id += 1;
                id
            };
            let deadline = Instant::now() + Duration::from_millis(delay_ms);
            let mut timers = caller.data().timers.lock().expect("timers mutex");
            timers.push(TimerEntry {
                id,
                deadline,
                callback,
                repeating: false,
                interval: Duration::from_millis(delay_ms),
            });
            value::encode_f64(id as f64)
        },
    );

    // ── Import 28: clear_timeout(i64) → () ────────────────────────────────

    let clear_timeout_fn = Func::wrap(
        &mut *store,
        |caller: Caller<'_, RuntimeState>, timer_id: i64| {
            if value::is_f64(timer_id) {
                let id = f64::from_bits(timer_id as u64) as u32;
                caller
                    .data()
                    .cancelled_timers
                    .lock()
                    .expect("cancelled_timers mutex")
                    .insert(id);
            }
            // For simplicity, mark as cancelled rather than removing from the vec
        },
    );

    // ── Import 29: set_interval(i64, i64) → i64 ───────────────────────────

    let set_interval_fn = Func::wrap(
        &mut *store,
        |caller: Caller<'_, RuntimeState>, callback: i64, delay: i64| -> i64 {
            let delay_f64 = if value::is_f64(delay) {
                f64::from_bits(delay as u64)
            } else {
                f64::NAN
            };
            let delay_ms: u64 = if delay_f64.is_nan() || delay_f64.is_sign_negative() {
                0
            } else if delay_f64 > (u32::MAX as f64) {
                u32::MAX as u64
            } else {
                delay_f64 as u64
            };
            let id = {
                let mut next_id = caller
                    .data()
                    .next_timer_id
                    .lock()
                    .expect("next_timer_id mutex");
                let id = *next_id;
                *next_id += 1;
                id
            };
            let deadline = Instant::now() + Duration::from_millis(delay_ms);
            let mut timers = caller.data().timers.lock().expect("timers mutex");
            timers.push(TimerEntry {
                id,
                deadline,
                callback,
                repeating: true,
                interval: Duration::from_millis(delay_ms),
            });
            value::encode_f64(id as f64)
        },
    );

    // ── Import 30: clear_interval(i64) → () ───────────────────────────────

    let clear_interval_fn = Func::wrap(
        &mut *store,
        |caller: Caller<'_, RuntimeState>, timer_id: i64| {
            if value::is_f64(timer_id) {
                let id = f64::from_bits(timer_id as u64) as u32;
                caller
                    .data()
                    .cancelled_timers
                    .lock()
                    .expect("cancelled_timers mutex")
                    .insert(id);
            }
            // simplified no-op
        },
    );

    // ── Import 31: fetch(i64) → i64 ────────────────────────────────────────

    vec![
        (28, set_timeout_fn),
        (29, clear_timeout_fn),
        (30, set_interval_fn),
        (31, clear_interval_fn),
    ]
}
