// register_xxx → define_xxx 模块
mod async_fn;
mod async_generator;
mod generator;
mod inspector_host;
mod misc;
mod modules;
mod promise;
mod promise_combinators;
mod proxy_reflect;
mod proxy_reflect_async;
mod reentrant_async;

pub(crate) use async_fn::define_async_fn;
pub(crate) use async_generator::define_async_generator;
pub(crate) use generator::define_generator;
pub(crate) use inspector_host::define_inspector_host;
pub(crate) use misc::define_misc;
pub(crate) use modules::{create_require_cache_proxy, define_modules};
pub(crate) use promise::define_promise;
pub(crate) use promise_combinators::define_promise_combinators;
pub(crate) use proxy_reflect::define_proxy_reflect;
// 以下 async 函数已迁移到 wjsm_builtins::proxy_reflect_async，
// 通过 exec_context_impl 委托调用。
pub(crate) use proxy_reflect_async::define_proxy_reflect_async;
pub(crate) use reentrant_async::define_array_object_async;
pub(crate) use reentrant_async::define_misc_async;
pub(crate) use reentrant_async::define_primitive_core_async;
pub(crate) use reentrant_async::define_proxy_traps_async;
pub(crate) use reentrant_async::define_timers_arrays_async;
pub(crate) use reentrant_async::define_typedarray_new_methods_async;
pub(crate) use reentrant_async::string_replace_default_async_body;
mod object_builtins;
pub(crate) use object_builtins::{
    define_object_builtins, proto_handle_from_value, read_property_by_string_key_raw,
};
mod object_builtins_async;
pub(crate) use object_builtins_async::define_object_builtins_async;
mod core_async;
mod get_method;
pub(crate) use core_async::define_core_async;
pub(crate) use get_method::{get_by_name_id_sync, get_method_by_name_id};

// 原 include! 裸块文件 → 模块声明
mod array_object;
mod atomics;
mod collections_buffers;
mod core;
mod fetch;
mod fetch_core;
mod fetch_http;
mod gc;
mod get_builtin_global_entry;
mod math_number_error;
mod primitive_core;
mod private_fields;
mod streams_fetch_body;
mod streams_queuing;
mod streams_readable;
mod streams_writable;
mod string_methods;
mod timers_arrays;
pub(crate) mod typedarray_new_methods;
mod weakref_finalization;

pub(crate) use array_object::define_array_object;
pub(crate) use atomics::define_atomics;
pub(crate) use collections_buffers::define_collections_buffers;

pub(crate) use core::define_core;
pub(crate) use gc::{allocate_v2_array_handle, define_v2};

pub(crate) use fetch::define_fetch;
pub(crate) use fetch_http::perform_http_fetch;
pub(crate) use fetch_core::create_arraybuffer_with_bytes;
pub(crate) use get_builtin_global_entry::define_get_builtin_global;
pub(crate) use math_number_error::define_math_number_error;
pub(crate) use primitive_core::define_primitive_core;
pub(crate) use streams_queuing::call_queuing_strategy_size_from_caller;
pub(crate) use streams_queuing::construct_byte_length_queuing_strategy;
pub(crate) use streams_queuing::construct_count_queuing_strategy;
pub(crate) use streams_readable::build_reader_result_with_env;
pub(crate) use streams_readable::{
    create_uint8array_with_env, mark_response_body_used_from_caller,
    transfer_byob_view_with_env, typedarray_u8_bytes, write_u8_bytes_to_view,
};
pub(crate) use streams_readable::cancel_http_response_from_caller;
pub(crate) use streams_fetch_body::call_fetch_body_reader_read;
pub(crate) use streams_fetch_body::consume_fetch_body_to_bytes;
pub(crate) use streams_writable::{
    create_writable_abort_signal_object, mark_writable_stream_signal_aborted,
};
pub(crate) use string_methods::define_string_methods;
pub(crate) use timers_arrays::define_timers_arrays;
pub(crate) use typedarray_new_methods::define_typedarray_new_methods;
pub(crate) use weakref_finalization::define_weakref_finalization;
