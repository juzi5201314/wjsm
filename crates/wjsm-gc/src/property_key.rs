//! 属性键的规范句柄 newtype。
//!
//! 属性表以句柄为键，因此「同内容 → 同句柄」必须由生产者保证：
//! - 字符串键：宿主侧 `NativeAgentState::intern_property_string`（内容去重后包装）。
//! - 符号键：[`PropertyKey::symbol`]（`NAME_ID_SYMBOL_FLAG | symbol_idx`）。
//!
//! 字段私有；除这两个构造入口外，代码只能搬运 / 比较 `PropertyKey`，不能在
//! 局部用原始 `u32` 现造一个键，从而把规范化责任收口到 intern 路径。

use wjsm_ir::constants;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PropertyKey(u32);

impl PropertyKey {
    /// 包装已规范化的 name_id。
    ///
    /// 调用方必须保证该 name_id 已经过规范化：字符串键同内容 → 同句柄，
    /// 符号键携带 `NAME_ID_SYMBOL_FLAG`。否则以句柄为键的属性表会把同名
    /// 属性拆成两条。
    pub const fn from_name_id(name_id: u32) -> Self {
        Self(name_id)
    }

    /// 符号属性键：`NAME_ID_SYMBOL_FLAG | symbol_idx`。
    pub const fn symbol(symbol_idx: u32) -> Self {
        Self(constants::NAME_ID_SYMBOL_FLAG | symbol_idx)
    }

    /// 取出包装的 name_id。
    #[inline]
    pub const fn get(self) -> u32 {
        self.0
    }

    /// 该键是否为符号（而非字符串 / 内存字符串）。
    #[inline]
    pub const fn is_symbol(self) -> bool {
        self.0 & constants::NAME_ID_SYMBOL_FLAG != 0
    }
}
