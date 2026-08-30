//! 闭包 `$env` 布局 install 期烘焙：按 `Function::env_layout_keys` 顺序过渡 shape。

use wjsm_gc::{PropertyKey, ShapeTable};
use wjsm_ir::{Constant, Program, value};
use wjsm_ir::constants::{FLAG_CONFIGURABLE, FLAG_ENUMERABLE, FLAG_WRITABLE};

/// 每条函数 env 布局占 2 个 u32：`[shape_id, slot_count]`。
pub const ENV_LAYOUT_META_WORDS: usize = 2;

/// install 期为每个函数烘焙 `$env` 期望 shape；无布局的函数填 `(0, 0)`。
pub fn bake_env_layout_meta_table(
    shapes: &ShapeTable,
    program: &Program,
    string_constants: &[i64],
) -> Vec<u32> {
    let flags = (FLAG_CONFIGURABLE | FLAG_ENUMERABLE | FLAG_WRITABLE) as u32;
    let constants = program.constants();
    let mut meta = Vec::with_capacity(program.functions().len() * ENV_LAYOUT_META_WORDS);
    for function in program.functions() {
        let keys = function.env_layout_keys();
        if keys.is_empty() {
            meta.extend([0, 0]);
            continue;
        }
        let mut shape_id = ShapeTable::empty_shape();
        for key_name in keys {
            let key = property_key_for_env_name(constants, string_constants, key_name);
            let transition = shapes.transition_add(shape_id, key, flags);
            shape_id = transition.shape_id;
        }
        meta.push(shape_id);
        meta.push(u32::try_from(keys.len()).unwrap_or(0));
    }
    meta
}

fn property_key_for_env_name(
    constants: &[Constant],
    string_constants: &[i64],
    name: &str,
) -> PropertyKey {
    for (index, constant) in constants.iter().enumerate() {
        if let Constant::String(text) = constant
            && text == name
        {
            let encoded = string_constants.get(index).copied().unwrap_or(0);
            if value::is_inline_string(encoded) {
                return PropertyKey::inline_string(encoded).expect("install 期 SSO 字符串常量");
            }
            return PropertyKey::from_name_id(value::decode_runtime_string_handle(encoded));
        }
    }
    PropertyKey::from_baked_raw(
        wjsm_ir::string_hash::content_hash_latin1(name.as_bytes()) as u64,
    )
}
