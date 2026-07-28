use crate::exec_context_impl::WasmExecContext;
use crate::*;
use anyhow::Result;
use wasmtime::Store;
use wasmtime::{Caller, Func, Linker};
use wjsm_host::ExecContext;

pub(crate) fn define_math_number_error(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    let math_abs_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::math_abs(&mut ctx, arg)
        },
    );
    linker.define(&mut store, "env", "math_abs", math_abs_fn)?;
    let math_acos_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::math_acos(&mut ctx, arg)
        },
    );
    linker.define(&mut store, "env", "math_acos", math_acos_fn)?;
    let math_acosh_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::math_acosh(&mut ctx, arg)
        },
    );
    linker.define(&mut store, "env", "math_acosh", math_acosh_fn)?;
    let math_asin_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::math_asin(&mut ctx, arg)
        },
    );
    linker.define(&mut store, "env", "math_asin", math_asin_fn)?;
    let math_asinh_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::math_asinh(&mut ctx, arg)
        },
    );
    linker.define(&mut store, "env", "math_asinh", math_asinh_fn)?;
    let math_atan_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::math_atan(&mut ctx, arg)
        },
    );
    linker.define(&mut store, "env", "math_atan", math_atan_fn)?;
    let math_atanh_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::math_atanh(&mut ctx, arg)
        },
    );
    linker.define(&mut store, "env", "math_atanh", math_atanh_fn)?;
    let math_cbrt_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::math_cbrt(&mut ctx, arg)
        },
    );
    linker.define(&mut store, "env", "math_cbrt", math_cbrt_fn)?;
    let math_ceil_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::math_ceil(&mut ctx, arg)
        },
    );
    linker.define(&mut store, "env", "math_ceil", math_ceil_fn)?;
    let math_clz32_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::math_clz32(&mut ctx, arg)
        },
    );
    linker.define(&mut store, "env", "math_clz32", math_clz32_fn)?;
    let math_cos_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::math_cos(&mut ctx, arg)
        },
    );
    linker.define(&mut store, "env", "math_cos", math_cos_fn)?;
    let math_cosh_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::math_cosh(&mut ctx, arg)
        },
    );
    linker.define(&mut store, "env", "math_cosh", math_cosh_fn)?;
    let math_exp_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::math_exp(&mut ctx, arg)
        },
    );
    linker.define(&mut store, "env", "math_exp", math_exp_fn)?;
    let math_expm1_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::math_expm1(&mut ctx, arg)
        },
    );
    linker.define(&mut store, "env", "math_expm1", math_expm1_fn)?;
    let math_floor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::math_floor(&mut ctx, arg)
        },
    );
    linker.define(&mut store, "env", "math_floor", math_floor_fn)?;
    let math_fround_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::math_fround(&mut ctx, arg)
        },
    );
    linker.define(&mut store, "env", "math_fround", math_fround_fn)?;
    let math_log_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::math_log(&mut ctx, arg)
        },
    );
    linker.define(&mut store, "env", "math_log", math_log_fn)?;
    let math_log1p_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::math_log1p(&mut ctx, arg)
        },
    );
    linker.define(&mut store, "env", "math_log1p", math_log1p_fn)?;
    let math_log10_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::math_log10(&mut ctx, arg)
        },
    );
    linker.define(&mut store, "env", "math_log10", math_log10_fn)?;
    let math_log2_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::math_log2(&mut ctx, arg)
        },
    );
    linker.define(&mut store, "env", "math_log2", math_log2_fn)?;
    let math_round_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::math_round(&mut ctx, arg)
        },
    );
    linker.define(&mut store, "env", "math_round", math_round_fn)?;
    let math_sign_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::math_sign(&mut ctx, arg)
        },
    );
    linker.define(&mut store, "env", "math_sign", math_sign_fn)?;
    let math_sin_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::math_sin(&mut ctx, arg)
        },
    );
    linker.define(&mut store, "env", "math_sin", math_sin_fn)?;
    let math_sinh_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::math_sinh(&mut ctx, arg)
        },
    );
    linker.define(&mut store, "env", "math_sinh", math_sinh_fn)?;
    let math_sqrt_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::math_sqrt(&mut ctx, arg)
        },
    );
    linker.define(&mut store, "env", "math_sqrt", math_sqrt_fn)?;
    let math_tan_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::math_tan(&mut ctx, arg)
        },
    );
    linker.define(&mut store, "env", "math_tan", math_tan_fn)?;
    let math_tanh_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::math_tanh(&mut ctx, arg)
        },
    );
    linker.define(&mut store, "env", "math_tanh", math_tanh_fn)?;
    let math_trunc_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::math_trunc(&mut ctx, arg)
        },
    );
    linker.define(&mut store, "env", "math_trunc", math_trunc_fn)?;
    let math_atan2_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::math_atan2(&mut ctx, a, b)
        },
    );
    linker.define(&mut store, "env", "math_atan2", math_atan2_fn)?;
    let math_hypot_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, args_base: i32, args_count: i32| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::math_hypot(&mut ctx, args_base, args_count)
        },
    );
    linker.define(&mut store, "env", "math_hypot", math_hypot_fn)?;
    let math_imul_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::math_imul(&mut ctx, a, b)
        },
    );
    linker.define(&mut store, "env", "math_imul", math_imul_fn)?;
    let math_max_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, args_base: i32, args_count: i32| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::math_max(&mut ctx, args_base, args_count)
        },
    );
    linker.define(&mut store, "env", "math_max", math_max_fn)?;
    let math_max_array_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, args_array: i64| -> i64 {
            let args = {
                let mut ctx = WasmExecContext::new(&mut caller);
                let length = ctx.array_read_length(args_array).unwrap_or(0);
                (0..length)
                    .map(|index| {
                        ctx.array_read_elem(args_array, index)
                            .unwrap_or_else(value::encode_undefined)
                    })
                    .map(|value| i64::from_ne_bytes(value.to_ne_bytes()))
                    .collect::<Vec<_>>()
            };
            let env = WasmEnv::from_caller(&mut caller).expect("WasmEnv");
            let saved_sp =
                push_args_to_shadow_stack(&mut caller, &env, &args).expect("shadow stack capacity");
            let args_count = match i32::try_from(args.len()) {
                Ok(count) => count,
                Err(_) => return value::encode_undefined(),
            };
            let result = {
                let mut ctx = WasmExecContext::new(&mut caller);
                wjsm_builtins::math_number_error::math_max(&mut ctx, saved_sp, args_count)
            };
            restore_shadow_sp(&mut caller, &env, saved_sp);
            result
        },
    );
    linker.define(&mut store, "env", "math_max_array", math_max_array_fn)?;
    let math_min_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, args_base: i32, args_count: i32| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::math_min(&mut ctx, args_base, args_count)
        },
    );
    linker.define(&mut store, "env", "math_min", math_min_fn)?;
    let math_pow_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::math_pow(&mut ctx, a, b)
        },
    );
    linker.define(&mut store, "env", "math_pow", math_pow_fn)?;
    let math_random_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>| -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::math_number_error::math_random(&mut ctx)
    });
    linker.define(&mut store, "env", "math_random", math_random_fn)?;
    let number_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::number_constructor(&mut ctx, arg)
        },
    );
    linker.define(
        &mut store,
        "env",
        "number_constructor",
        number_constructor_fn,
    )?;
    let number_is_nan_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::number_is_nan(&mut ctx, arg)
        },
    );
    linker.define(&mut store, "env", "number_is_nan", number_is_nan_fn)?;
    let number_is_finite_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::number_is_finite(&mut ctx, arg)
        },
    );
    linker.define(&mut store, "env", "number_is_finite", number_is_finite_fn)?;
    let number_is_integer_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::number_is_integer(&mut ctx, arg)
        },
    );
    linker.define(&mut store, "env", "number_is_integer", number_is_integer_fn)?;
    let number_is_safe_integer_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::number_is_safe_integer(&mut ctx, arg)
        },
    );
    linker.define(
        &mut store,
        "env",
        "number_is_safe_integer",
        number_is_safe_integer_fn,
    )?;
    let number_parse_int_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64, radix_val: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::number_parse_int(&mut ctx, arg, radix_val)
        },
    );
    linker.define(&mut store, "env", "number_parse_int", number_parse_int_fn)?;
    let number_parse_float_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::number_parse_float(&mut ctx, arg)
        },
    );
    linker.define(
        &mut store,
        "env",
        "number_parse_float",
        number_parse_float_fn,
    )?;
    let number_proto_to_string_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, radix_val: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::number_proto_to_string(&mut ctx, this_val, radix_val)
        },
    );
    linker.define(
        &mut store,
        "env",
        "number_proto_to_string",
        number_proto_to_string_fn,
    )?;
    let number_proto_value_of_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::number_proto_value_of(&mut ctx, this_val)
        },
    );
    linker.define(
        &mut store,
        "env",
        "number_proto_value_of",
        number_proto_value_of_fn,
    )?;
    let number_proto_to_fixed_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, digits_val: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::number_proto_to_fixed(&mut ctx, this_val, digits_val)
        },
    );
    linker.define(
        &mut store,
        "env",
        "number_proto_to_fixed",
        number_proto_to_fixed_fn,
    )?;
    let number_proto_to_exponential_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, digits_val: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::number_proto_to_exponential(
                &mut ctx, this_val, digits_val,
            )
        },
    );
    linker.define(
        &mut store,
        "env",
        "number_proto_to_exponential",
        number_proto_to_exponential_fn,
    )?;
    let number_proto_to_precision_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64, digits_val: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::number_proto_to_precision(
                &mut ctx, this_val, digits_val,
            )
        },
    );
    linker.define(
        &mut store,
        "env",
        "number_proto_to_precision",
        number_proto_to_precision_fn,
    )?;
    let boolean_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::boolean_constructor(&mut ctx, arg)
        },
    );
    linker.define(
        &mut store,
        "env",
        "boolean_constructor",
        boolean_constructor_fn,
    )?;
    let boolean_proto_to_string_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::boolean_proto_to_string(&mut ctx, this_val)
        },
    );
    linker.define(
        &mut store,
        "env",
        "boolean_proto_to_string",
        boolean_proto_to_string_fn,
    )?;
    let boolean_proto_value_of_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::boolean_proto_value_of(&mut ctx, this_val)
        },
    );
    linker.define(
        &mut store,
        "env",
        "boolean_proto_value_of",
        boolean_proto_value_of_fn,
    )?;
    let error_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64, options: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::error_constructor(&mut ctx, arg, options)
        },
    );
    linker.define(&mut store, "env", "error_constructor", error_constructor_fn)?;
    let type_error_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64, options: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::type_error_constructor(&mut ctx, arg, options)
        },
    );
    linker.define(
        &mut store,
        "env",
        "type_error_constructor",
        type_error_constructor_fn,
    )?;
    let range_error_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64, options: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::range_error_constructor(&mut ctx, arg, options)
        },
    );
    linker.define(
        &mut store,
        "env",
        "range_error_constructor",
        range_error_constructor_fn,
    )?;
    let syntax_error_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64, options: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::syntax_error_constructor(&mut ctx, arg, options)
        },
    );
    linker.define(
        &mut store,
        "env",
        "syntax_error_constructor",
        syntax_error_constructor_fn,
    )?;
    let reference_error_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64, options: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::reference_error_constructor(&mut ctx, arg, options)
        },
    );
    linker.define(
        &mut store,
        "env",
        "reference_error_constructor",
        reference_error_constructor_fn,
    )?;
    let uri_error_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64, options: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::uri_error_constructor(&mut ctx, arg, options)
        },
    );
    linker.define(
        &mut store,
        "env",
        "uri_error_constructor",
        uri_error_constructor_fn,
    )?;
    let eval_error_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, arg: i64, options: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::eval_error_constructor(&mut ctx, arg, options)
        },
    );
    linker.define(
        &mut store,
        "env",
        "eval_error_constructor",
        eval_error_constructor_fn,
    )?;
    let error_proto_to_string_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, this_val: i64| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::error_proto_to_string(&mut ctx, this_val)
        },
    );
    linker.define(
        &mut store,
        "env",
        "error_proto_to_string",
        error_proto_to_string_fn,
    )?;
    let primitive_bigint_get_method_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, boxed: i64, name_id: i32| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::primitive_bigint_get_method(
                &mut ctx,
                boxed,
                name_id as u32,
            )
        },
    );
    linker.define(
        &mut store,
        "env",
        "primitive_bigint_get_method",
        primitive_bigint_get_method_fn,
    )?;
    let primitive_number_get_method_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, boxed: i64, name_id: i32| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::primitive_number_get_method(
                &mut ctx,
                boxed,
                name_id as u32,
            )
        },
    );
    linker.define(
        &mut store,
        "env",
        "primitive_number_get_method",
        primitive_number_get_method_fn,
    )?;
    let primitive_symbol_get_property_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, boxed: i64, name_id: i32| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::primitive_symbol_get_property(
                &mut ctx,
                boxed,
                name_id as u32,
            )
        },
    );
    linker.define(
        &mut store,
        "env",
        "primitive_symbol_get_property",
        primitive_symbol_get_property_fn,
    )?;
    let primitive_regexp_get_property_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, boxed: i64, name_id: i32| -> i64 {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::primitive_regexp_get_property(
                &mut ctx,
                boxed,
                name_id as u32,
            )
        },
    );
    linker.define(
        &mut store,
        "env",
        "primitive_regexp_get_property",
        primitive_regexp_get_property_fn,
    )?;
    let primitive_regexp_set_property_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, boxed: i64, name_id: i32, val: i64| {
            let mut ctx = WasmExecContext::new(&mut caller);
            wjsm_builtins::math_number_error::primitive_regexp_set_property(
                &mut ctx,
                boxed,
                name_id as u32,
                val,
            );
        },
    );
    linker.define(
        &mut store,
        "env",
        "primitive_regexp_set_property",
        primitive_regexp_set_property_fn,
    )?;
    Ok(())
}
