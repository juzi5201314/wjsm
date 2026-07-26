use anyhow::Result;
use wasmtime::Store;
use wasmtime::{Caller, Func, Linker};
use crate::exec_context_impl::WasmExecContext;
use crate::*;
pub(crate) fn define_primitive_core(linker: &mut Linker<RuntimeState>, mut store: &mut Store<RuntimeState>) -> Result<()> {
    let bigint_from_literal_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, ptr: i32, _len: i32| -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::primitive_core::bigint_from_literal(&mut ctx, ptr, _len)
    });
    linker.define(&mut store, "env", "bigint_from_literal", bigint_from_literal_fn)?;
    let bigint_add_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::primitive_core::bigint_add(&mut ctx, a, b)
    });
    linker.define(&mut store, "env", "bigint_add", bigint_add_fn)?;
    let bigint_sub_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::primitive_core::bigint_sub(&mut ctx, a, b)
    });
    linker.define(&mut store, "env", "bigint_sub", bigint_sub_fn)?;
    let bigint_mul_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::primitive_core::bigint_mul(&mut ctx, a, b)
    });
    linker.define(&mut store, "env", "bigint_mul", bigint_mul_fn)?;
    let bigint_div_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::primitive_core::bigint_div(&mut ctx, a, b)
    });
    linker.define(&mut store, "env", "bigint_div", bigint_div_fn)?;
    let bigint_mod_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::primitive_core::bigint_mod(&mut ctx, a, b)
    });
    linker.define(&mut store, "env", "bigint_mod", bigint_mod_fn)?;
    let bigint_pow_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::primitive_core::bigint_pow(&mut ctx, a, b)
    });
    linker.define(&mut store, "env", "bigint_pow", bigint_pow_fn)?;
    let bigint_neg_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, a: i64| -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::primitive_core::bigint_neg(&mut ctx, a)
    });
    linker.define(&mut store, "env", "bigint_neg", bigint_neg_fn)?;
    let bigint_bit_and_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::primitive_core::bigint_bit_and(&mut ctx, a, b)
    });
    linker.define(&mut store, "env", "bigint_bit_and", bigint_bit_and_fn)?;
    let bigint_bit_or_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::primitive_core::bigint_bit_or(&mut ctx, a, b)
    });
    linker.define(&mut store, "env", "bigint_bit_or", bigint_bit_or_fn)?;
    let bigint_bit_xor_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::primitive_core::bigint_bit_xor(&mut ctx, a, b)
    });
    linker.define(&mut store, "env", "bigint_bit_xor", bigint_bit_xor_fn)?;
    let bigint_shl_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::primitive_core::bigint_shl(&mut ctx, a, b)
    });
    linker.define(&mut store, "env", "bigint_shl", bigint_shl_fn)?;
    let bigint_shr_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::primitive_core::bigint_shr(&mut ctx, a, b)
    });
    linker.define(&mut store, "env", "bigint_shr", bigint_shr_fn)?;
    let bigint_bit_not_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, a: i64| -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::primitive_core::bigint_bit_not(&mut ctx, a)
    });
    linker.define(&mut store, "env", "bigint_bit_not", bigint_bit_not_fn)?;
    let bigint_eq_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::primitive_core::bigint_eq(&mut ctx, a, b)
    });
    linker.define(&mut store, "env", "bigint_eq", bigint_eq_fn)?;
    let bigint_cmp_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::primitive_core::bigint_cmp(&mut ctx, a, b)
    });
    linker.define(&mut store, "env", "bigint_cmp", bigint_cmp_fn)?;
    let symbol_create_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, desc: i64| -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::primitive_core::symbol_create(&mut ctx, desc)
    });
    linker.define(&mut store, "env", "symbol_create", symbol_create_fn)?;
    let symbol_for_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, key: i64| -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::primitive_core::symbol_for(&mut ctx, key)
    });
    linker.define(&mut store, "env", "symbol_for", symbol_for_fn)?;
    let symbol_key_for_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, sym: i64| -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::primitive_core::symbol_key_for(&mut ctx, sym)
    });
    linker.define(&mut store, "env", "symbol_key_for", symbol_key_for_fn)?;
    let symbol_well_known_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, id: i32| -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::primitive_core::symbol_well_known(&mut ctx, id)
    });
    linker.define(&mut store, "env", "symbol_well_known", symbol_well_known_fn)?;
    let regex_create_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, pat_ptr: i32, pat_len: i32, flags_ptr: i32, flags_len: i32| -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::primitive_core::regex_create(&mut ctx, pat_ptr, pat_len, flags_ptr, flags_len)
    });
    linker.define(&mut store, "env", "regex_create", regex_create_fn)?;
    let regex_test_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, regex_val: i64, str_val: i64| -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::primitive_core::regex_test(&mut ctx, regex_val, str_val)
    });
    linker.define(&mut store, "env", "regex_test", regex_test_fn)?;
    let regex_exec_fn = Func::wrap(&mut store, |mut caller: Caller<'_, RuntimeState>, regex_val: i64, str_val: i64| -> i64 {
        let mut ctx = WasmExecContext::new(&mut caller);
        wjsm_builtins::primitive_core::regex_exec(&mut ctx, regex_val, str_val)
    });
    linker.define(&mut store, "env", "regex_exec", regex_exec_fn)?;
    Ok(())
}
