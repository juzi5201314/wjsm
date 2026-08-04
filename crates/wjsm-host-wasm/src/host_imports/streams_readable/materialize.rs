use crate::{
    ArrayBufferEntry, RuntimeState, TypedArrayEntry, WasmEnv, alloc_host_object,
    define_host_data_property_from_caller,
};
use wasmtime::{AsContextMut, Caller};
use wjsm_ir::value;

pub(crate) fn build_reader_result(
    caller: &mut Caller<'_, RuntimeState>,
    done: bool,
    result_value: Option<i64>,
) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let object = alloc_host_object(caller, &env, 2);
    let _ = define_host_data_property_from_caller(caller, object, "done", value::encode_bool(done));
    let result_value = result_value.unwrap_or_else(value::encode_undefined);
    let _ = define_host_data_property_from_caller(caller, object, "value", result_value);
    object
}

pub(crate) fn build_reader_result_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    done: bool,
    result_value: Option<i64>,
) -> i64 {
    let object = alloc_host_object(ctx, env, 2);
    let _ = crate::runtime_host_helpers::define_host_data_property_with_env(
        ctx,
        env,
        object,
        "done",
        value::encode_bool(done),
    );
    let result_value = result_value.unwrap_or_else(value::encode_undefined);
    let _ = crate::runtime_host_helpers::define_host_data_property_with_env(
        ctx,
        env,
        object,
        "value",
        result_value,
    );
    object
}

pub(crate) fn create_uint8array_with_env<C: AsContextMut<Data = RuntimeState>>(
    ctx: &mut C,
    env: &WasmEnv,
    bytes: &[u8],
) -> i64 {
    let array_buffer_handle = {
        let store = ctx.as_context_mut();
        let mut buffers = store
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
    let typedarray_handle = {
        let store = ctx.as_context_mut();
        let mut arrays = store
            .data()
            .typedarray_table
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let handle = arrays.len() as u32;
        arrays.push(TypedArrayEntry {
            buffer_handle: array_buffer_handle,
            buffer_object: None,
            byte_offset: 0,
            length: bytes.len() as u32,
            element_size: 1,
            element_kind: 1,
            is_shared: false,
        });
        handle
    };
    let object = alloc_host_object(ctx, env, 8);
    for (name, raw) in [
        ("__typedarray_handle__", typedarray_handle),
        ("__arraybuffer_handle__", array_buffer_handle),
    ] {
        let _ = crate::runtime_host_helpers::define_host_data_property_with_env(
            ctx,
            env,
            object,
            name,
            value::encode_f64(raw as f64),
        );
    }
    let length = value::encode_f64(bytes.len() as f64);
    for name in ["length", "byteLength"] {
        let _ = crate::runtime_host_helpers::define_host_data_property_with_env(
            ctx, env, object, name, length,
        );
    }
    let _ = crate::runtime_host_helpers::define_host_data_property_with_env(
        ctx,
        env,
        object,
        "byteOffset",
        value::encode_f64(0.0),
    );
    object
}
