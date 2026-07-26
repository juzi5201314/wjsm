//! get_builtin_global — 按名解析全局构造器 / 内置对象。

use wjsm_host::{ExecContext, Value};
use wjsm_ir::value;

/// `env.get_builtin_global(name_val)`。
pub fn get_builtin_global<E: ExecContext>(ctx: &mut E, name_val: Value) -> Value {
    let name = ctx.read_string_utf8_lossy(name_val);
    let Some(val) = ctx.create_global_builtin(&name) else {
        return value::encode_undefined();
    };
    if name == "Symbol" {
        ctx.install_well_known_symbols_on_symbol_constructor(val);
    }
    val
}
