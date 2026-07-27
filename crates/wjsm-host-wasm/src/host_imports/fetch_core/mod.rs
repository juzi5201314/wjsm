use crate::{
    ArrayBufferEntry, RuntimeState, WasmEnv, alloc_host_object,
    define_host_data_property_from_caller, store_runtime_string,
};
use wasmtime::Caller;
use wjsm_ir::value;

mod resource_timing;
pub(crate) use resource_timing::{
    complete_http_response_resource_timing, record_http_response_body_bytes,
};

pub(crate) fn create_arraybuffer_with_bytes(
    caller: &mut Caller<'_, RuntimeState>,
    bytes: &[u8],
) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let object = alloc_host_object(caller, &env, 2);
    let buffer_handle = {
        let mut buffers = caller
            .data()
            .arraybuffer_table
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let handle = buffers.len() as u32;
        buffers.push(ArrayBufferEntry {
            data: bytes.to_vec(),
        });
        handle
    };
    let _ = define_host_data_property_from_caller(
        caller,
        object,
        "__arraybuffer_handle__",
        value::encode_f64(buffer_handle as f64),
    );
    let _ = define_host_data_property_from_caller(
        caller,
        object,
        "byteLength",
        value::encode_f64(bytes.len() as f64),
    );
    object
}

pub(crate) fn alloc_type_error_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    message: &str,
) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let object = alloc_host_object(caller, &env, 2);
    let name = store_runtime_string(caller, "TypeError".to_string());
    let message = store_runtime_string(caller, message.to_string());
    let _ = define_host_data_property_from_caller(caller, object, "name", name);
    let _ = define_host_data_property_from_caller(caller, object, "message", message);
    object
}
