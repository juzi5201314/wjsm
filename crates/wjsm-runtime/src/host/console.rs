use wasmtime::*;
use wjsm_ir::value;

use crate::types::*;
use crate::runtime::*;

pub(crate) fn create_host_functions(store: &mut Store<RuntimeState>) -> Vec<(usize, Func)> {
    let console_log = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, val: i64| {
            write_console_value(&mut caller, val, None);
        },
    );

    let console_error = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, val: i64| {
            write_console_value(&mut caller, val, Some("error"));
        },
    );

    let console_warn = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, val: i64| {
            write_console_value(&mut caller, val, Some("warn"));
        },
    );

    let console_info = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, val: i64| {
            write_console_value(&mut caller, val, Some("info"));
        },
    );

    let console_debug = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, val: i64| {
            write_console_value(&mut caller, val, Some("debug"));
        },
    );

    let console_trace = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, val: i64| {
            write_console_value(&mut caller, val, Some("trace"));
        },
    );

    // ── Import 1: f64_mod(i64, i64) → i64 ───────────────────────────────

    vec![
        (0, console_log),
        (23, console_error),
        (24, console_warn),
        (25, console_info),
        (26, console_debug),
        (27, console_trace),
    ]
}
