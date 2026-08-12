/// Native call arena 中一段连续实参槽。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(C)]
pub struct CallArgs {
    pub base: u32,
    pub len: u32,
}

impl CallArgs {
    pub const fn new(base: u32, len: u32) -> Self {
        Self { base, len }
    }

    pub fn end(self) -> Option<u32> {
        self.base.checked_add(self.len)
    }

    pub const fn contains(self, index: u32) -> bool {
        index < self.len
    }
}
