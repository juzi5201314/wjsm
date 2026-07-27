use super::super::fetch_core::complete_http_response_resource_timing;
use crate::{RuntimeState, set_host_data_property_from_caller};
use wasmtime::Caller;
use wjsm_ir::value;

pub(crate) fn mark_response_body_used_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    response_handle: Option<u32>,
    response_object: Option<i64>,
) {
    if let Some(handle) = response_handle {
        let mut responses = caller
            .data()
            .fetch_response_table
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(response) = responses.get_mut(handle as usize) {
            response.body_used = true;
        }
    }
    if let Some(object) = response_object {
        let _ = set_host_data_property_from_caller(
            caller,
            object,
            "bodyUsed",
            value::encode_bool(true),
        );
    }
}

pub(crate) fn cancel_http_response_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    http_handle: u32,
) {
    let mut responses = caller
        .data()
        .http_response_table
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if let Some(response) = responses.get_mut(http_handle as usize) {
        response.response = None;
        response.pending_bytes.clear();
        response.eof = true;
    }
    drop(responses);
    complete_http_response_resource_timing(caller.data(), http_handle);
}
