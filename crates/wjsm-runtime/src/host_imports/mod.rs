// register_xxx → define_xxx 模块
mod promise;
mod promise_combinators;
mod async_fn;
mod async_generator;
mod proxy_reflect;
mod misc;

pub(crate) use promise::define_promise;
pub(crate) use promise_combinators::define_promise_combinators;
pub(crate) use misc::define_misc;
pub(crate) use async_fn::define_async_fn;
pub(crate) use async_generator::define_async_generator;
pub(crate) use proxy_reflect::define_proxy_reflect;
mod object_builtins;
pub(crate) use object_builtins::define_object_builtins;

// 原 include! 裸块文件 → 模块声明
mod core;
mod timers_arrays;
mod array_object;
mod primitive_core;
mod string_methods;
mod math_number_error;
mod collections_buffers;
mod proxy_traps;
mod typedarray_new_methods;
mod weakref_finalization;
mod atomics;
mod get_builtin_global_entry;

pub(crate) use core::define_core;
pub(crate) use timers_arrays::define_timers_arrays;
pub(crate) use array_object::define_array_object;
pub(crate) use primitive_core::define_primitive_core;
pub(crate) use string_methods::define_string_methods;
pub(crate) use math_number_error::define_math_number_error;
pub(crate) use collections_buffers::define_collections_buffers;
pub(crate) use proxy_traps::define_proxy_traps;
pub(crate) use typedarray_new_methods::define_typedarray_new_methods;
pub(crate) use weakref_finalization::define_weakref_finalization;
pub(crate) use atomics::define_atomics;
pub(crate) use get_builtin_global_entry::define_get_builtin_global;
