// register_xxx → define_xxx 模块
mod async_fn;
mod async_generator;
mod misc;
mod promise;
mod promise_combinators;
mod proxy_reflect;

pub(crate) use async_fn::define_async_fn;
pub(crate) use async_generator::define_async_generator;
pub(crate) use misc::define_misc;
pub(crate) use promise::define_promise;
pub(crate) use promise_combinators::define_promise_combinators;
pub(crate) use proxy_reflect::define_proxy_reflect;
mod object_builtins;
pub(crate) use object_builtins::define_object_builtins;

// 原 include! 裸块文件 → 模块声明
mod array_object;
mod atomics;
mod collections_buffers;
mod core;
mod get_builtin_global_entry;
mod math_number_error;
mod primitive_core;
mod proxy_traps;
mod string_methods;
mod timers_arrays;
mod fetch;
mod typedarray_new_methods;
mod weakref_finalization;

pub(crate) use array_object::define_array_object;
pub(crate) use atomics::define_atomics;
pub(crate) use collections_buffers::define_collections_buffers;
pub(crate) use core::define_core;
pub(crate) use get_builtin_global_entry::define_get_builtin_global;
pub(crate) use math_number_error::define_math_number_error;
pub(crate) use primitive_core::define_primitive_core;
pub(crate) use proxy_traps::define_proxy_traps;
pub(crate) use string_methods::define_string_methods;
pub(crate) use timers_arrays::define_timers_arrays;
pub(crate) use typedarray_new_methods::define_typedarray_new_methods;
pub(crate) use fetch::define_fetch;
pub(crate) use fetch::call_headers_method_from_caller;
pub(crate) use fetch::call_response_method_from_caller;
pub(crate) use fetch::call_request_method_from_caller;
pub(crate) use weakref_finalization::define_weakref_finalization;