//! 字符串迭代器纯计算（UTF-16 码点步进）。

use wjsm_host::RuntimeString;

/// 将字符串迭代器 `unit_pos` 推进到下一个码点。
pub fn string_iter_advance_unit_pos(string: &RuntimeString, unit_pos: &mut usize) {
    let Some(unit) = string.code_unit_at(*unit_pos) else {
        return;
    };
    let width = if (0xD800..=0xDBFF).contains(&unit)
        && string
            .code_unit_at(*unit_pos + 1)
            .is_some_and(|next| (0xDC00..=0xDFFF).contains(&next))
    {
        2
    } else {
        1
    };
    *unit_pos += width;
}
