use crate::{
    AbortSignalEntry, RuntimeState, WasmEnv, alloc_host_object,
    define_host_data_property_from_caller, read_object_property_by_name, resolve_handle,
    set_host_data_property_from_caller,
};
use wasmtime::Caller;
use wjsm_ir::value;

pub(crate) fn create_writable_abort_signal_object(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let signal_handle = {
        let mut table = caller
            .data()
            .abort_signal_table
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let handle = table.len() as u32;
        table.push(AbortSignalEntry {
            aborted: false,
            reason: None,
        });
        handle
    };
    let signal = alloc_host_object(caller, &env, 2);
    let _ = define_host_data_property_from_caller(
        caller,
        signal,
        "__abort_signal_handle__",
        value::encode_f64(signal_handle as f64),
    );
    let _ =
        define_host_data_property_from_caller(caller, signal, "aborted", value::encode_bool(false));
    signal
}

fn mark_abort_signal(caller: &mut Caller<'_, RuntimeState>, signal: i64, reason: i64) {
    let handle = resolve_handle(caller, signal)
        .and_then(|pointer| {
            read_object_property_by_name(caller, pointer, "__abort_signal_handle__")
        })
        .filter(|raw| value::is_f64(*raw))
        .map(|raw| value::decode_f64(raw) as usize);
    if let Some(handle) = handle {
        let mut table = caller
            .data()
            .abort_signal_table
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(entry) = table.get_mut(handle) {
            entry.aborted = true;
            entry.reason = Some(reason);
        }
    }
    let _ = set_host_data_property_from_caller(caller, signal, "aborted", value::encode_bool(true));
}

pub(crate) fn mark_writable_stream_signal_aborted(
    caller: &mut Caller<'_, RuntimeState>,
    stream_handle: u32,
    reason: i64,
) {
    let signal = {
        let table = caller
            .data()
            .writable_stream_table
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        table
            .get(stream_handle as usize)
            .and_then(|entry| entry.abort_signal)
    };
    if let Some(signal) = signal {
        mark_abort_signal(caller, signal, reason);
    }
}
