//! 后端无关的 host builtins 实现。
//!
//! 以 `<E: wjsm_host::ExecContext>` 泛型单态化，零 vtable 开销。

pub mod array_object;
pub mod async_fn;
pub mod async_generator;
pub mod collections;
pub mod core_async;
pub mod date_parse;
pub mod generator;
pub mod get_builtin_global;
pub mod get_method;
pub mod inspector_host;
pub mod iterable_collect;
pub mod math_number_error;
pub mod misc;
pub mod number_format;
pub mod object_builtins;
pub mod object_builtins_async;
pub mod primitive_core;
pub mod private_fields;
pub mod promise;
pub mod proxy_reflect;
pub mod proxy_reflect_async;
pub mod proxy_traps;
pub mod reentrant;
pub mod streams_queuing;
pub mod string_iter;
pub mod string_methods;
pub mod string_to_number;
pub mod timers_arrays;
pub mod typedarray_methods;
pub mod weakref_finalization;

pub use date_parse::{ms_to_datetime_local, ms_to_datetime_utc, parse_date_string};
pub use number_format::{
    format_number_js, format_number_to_exponential_js, format_number_to_fixed_js,
    format_number_to_precision_js, normalize_exponent, number_proto_to_string_radix,
};
pub use string_iter::string_iter_advance_unit_pos;
pub use string_to_number::{js_string_content_to_f64, string_to_f64, trim_js_whitespace};
