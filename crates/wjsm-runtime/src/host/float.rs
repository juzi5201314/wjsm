use wasmtime::*;
use wjsm_ir::value;

use crate::types::*;
use crate::runtime::*;

pub(crate) fn create_host_functions(store: &mut Store<RuntimeState>) -> Vec<(usize, Func)> {
    let f64_mod = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            let af = f64::from_bits(a as u64);
            let bf = f64::from_bits(b as u64);
            let result = af - bf * (af / bf).trunc();
            result.to_bits() as i64
        },
    );

    // ── Import 2: f64_pow(i64, i64) → i64 ───────────────────────────────

    let f64_pow = Func::wrap(
        &mut *store,
        |_caller: Caller<'_, RuntimeState>, a: i64, b: i64| -> i64 {
            let af = f64::from_bits(a as u64);
            let bf = f64::from_bits(b as u64);
            let result = af.powf(bf);
            result.to_bits() as i64
        },
    );

    vec![
        (1, f64_mod),
        (2, f64_pow),
    ]
}
