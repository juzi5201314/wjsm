//! 堆内字符串扁平载荷的读取视图（Latin-1 / UTF-16 二态）。
//!
//! 视图借用 GC 堆内存：**借用期内不得在 JS 堆上分配**（任何分配都可能触发 GC
//! 搬迁使视图悬垂）。宿主经 [`crate::heap_access::HeapAccessV2::with_string_bytes`]
//! 在闭包内消费视图，闭包体只做纯计算或 Rust 侧分配；debug 构建下分配入口有
//! 断言拦截违规。

/// 字符串扁平载荷视图。
///
/// ECMAScript 语义永远是 UTF-16 码元序列；Latin-1 只是单字节存储表示（每个字节
/// 即一个码元，0x00..=0xFF），读取必须经 [`StrView::unit`] / [`StrView::to_utf16`]
/// 展开，不可直接把字节当 UTF-8。
#[derive(Clone, Copy)]
pub enum StrView<'a> {
    /// 单字节载荷。

    Latin1(&'a [u8]),
    /// 双字节小端载荷。
    Utf16(&'a [u16]),
}

impl<'a> StrView<'a> {
    /// 码元数。
    pub fn len(&self) -> usize {
        match self {
            Self::Latin1(bytes) => bytes.len(),
            Self::Utf16(units) => units.len(),
        }
    }

    /// 视图是否不含任何 UTF-16 码元。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }


    /// 读第 `index` 个码元；越界返回 `None`。
    pub fn unit(&self, index: usize) -> Option<u16> {
        match self {
            Self::Latin1(bytes) => bytes.get(index).copied().map(u16::from),
            Self::Utf16(units) => units.get(index).copied(),
        }
    }

    /// UTF-16 表示的零拷贝视图；Latin-1 返回 `None`（需 [`StrView::to_utf16`] 展开）。
    pub fn as_utf16(&self) -> Option<&'a [u16]> {
        match self {
            Self::Latin1(_) => None,
            Self::Utf16(units) => Some(units),
        }
    }

    /// Latin-1 表示的零拷贝视图；UTF-16 返回 `None`。
    pub fn as_latin1(&self) -> Option<&'a [u8]> {
        match self {
            Self::Latin1(bytes) => Some(bytes),
            Self::Utf16(_) => None,
        }
    }

    /// 展开为 owned UTF-16 码元序列（Latin-1 逐字节、UTF-16 直接克隆）。
    pub fn to_utf16(&self) -> Vec<u16> {
        match self {
            Self::Latin1(bytes) => bytes.iter().map(|&byte| u16::from(byte)).collect(),
            Self::Utf16(units) => units.to_vec(),
        }
    }

    /// 无损 UTF-8（与 `String::from_utf16` 同语义：未配对代理对返回 `None`）。
    pub fn to_utf8(&self) -> Option<String> {
        String::from_utf16(&self.to_utf16()).ok()
    }

    /// 有损 UTF-8（未配对代理对替换为 U+FFFD）。
    pub fn to_utf8_lossy(&self) -> String {
        String::from_utf16_lossy(&self.to_utf16())
    }
}
