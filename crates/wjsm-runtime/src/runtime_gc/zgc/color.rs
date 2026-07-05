#![allow(dead_code)] // T4.1 建立 T4.2-T4.4 会接入的颜色协议 API。
pub const COLOR_MASK: u32 = 0x3;
pub const PTR_MASK: u32 = !COLOR_MASK;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ZColor {
    Empty = 0b00,
    Marked0 = 0b01,
    Marked1 = 0b10,
    Remapped = 0b11,
}

impl ZColor {
    pub fn from_bits(bits: u32) -> Option<Self> {
        match bits & COLOR_MASK {
            0b00 => Some(Self::Empty),
            0b01 => Some(Self::Marked0),
            0b10 => Some(Self::Marked1),
            0b11 => Some(Self::Remapped),
            _ => None,
        }
    }

    pub fn bits(self) -> u32 {
        self as u32
    }

    pub fn other_mark(self) -> Self {
        match self {
            Self::Marked0 => Self::Marked1,
            Self::Marked1 => Self::Marked0,
            Self::Empty | Self::Remapped => Self::Marked0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZEntry(u32);

impl ZEntry {
    pub fn empty() -> Self {
        Self(0)
    }

    pub fn new(ptr: u32, color: ZColor) -> Self {
        debug_assert_eq!(ptr & COLOR_MASK, 0);
        if ptr == 0 {
            Self::empty()
        } else {
            Self((ptr & PTR_MASK) | color.bits())
        }
    }

    pub fn raw(self) -> u32 {
        self.0
    }

    pub fn ptr(self) -> u32 {
        self.0 & PTR_MASK
    }

    pub fn color(self) -> ZColor {
        ZColor::from_bits(self.0).expect("masked color bits must decode")
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn recolor(self, color: ZColor) -> Self {
        if self.is_empty() {
            Self::empty()
        } else {
            Self::new(self.ptr(), color)
        }
    }

    pub fn is_good(self, good: ZColor) -> bool {
        !self.is_empty() && self.color() == good
    }

    pub fn repair_bad_non_relocating(self, good: ZColor) -> Self {
        if self.is_good(good) || good == ZColor::Remapped {
            self
        } else {
            self.recolor(good)
        }
    }

    pub fn repair_relocate_non_rs(self) -> Self {
        self.recolor(ZColor::Remapped)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZPhase {
    Idle,
    Mark,
    Relocate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZColorState {
    next_mark: ZColor,
    good: ZColor,
    phase: ZPhase,
}

impl Default for ZColorState {
    fn default() -> Self {
        Self {
            next_mark: ZColor::Marked0,
            good: ZColor::Marked0,
            phase: ZPhase::Idle,
        }
    }
}

impl ZColorState {
    pub fn good(self) -> ZColor {
        self.good
    }

    pub fn phase(self) -> ZPhase {
        self.phase
    }

    pub fn start_mark(&mut self) -> ZColor {
        self.good = self.next_mark;
        self.next_mark = self.next_mark.other_mark();
        self.phase = ZPhase::Mark;
        self.good
    }

    pub fn start_relocate(&mut self) -> ZColor {
        self.good = ZColor::Remapped;
        self.phase = ZPhase::Relocate;
        self.good
    }

    pub fn finish_cycle(&mut self) {
        self.phase = ZPhase::Idle;
    }
}

#[cfg(test)]
mod tests {
    use super::{ZColor, ZColorState, ZEntry, ZPhase};

    #[test]
    fn entry_uses_low_two_color_bits() {
        let entry = ZEntry::new(0x1000, ZColor::Marked1);

        assert_eq!(entry.raw(), 0x1002);
        assert_eq!(entry.ptr(), 0x1000);
        assert_eq!(entry.color(), ZColor::Marked1);
        assert!(ZEntry::empty().is_empty());
    }

    #[test]
    fn good_color_switches_mark_mark_relocate() {
        let mut state = ZColorState::default();

        assert_eq!(state.start_mark(), ZColor::Marked0);
        assert_eq!(state.phase(), ZPhase::Mark);
        assert_eq!(state.start_relocate(), ZColor::Remapped);
        state.finish_cycle();
        assert_eq!(state.start_mark(), ZColor::Marked1);
        assert_eq!(state.start_relocate(), ZColor::Remapped);
        state.finish_cycle();
        assert_eq!(state.start_mark(), ZColor::Marked0);
    }

    #[test]
    fn bad_mark_color_repairs_to_current_good() {
        let stale = ZEntry::new(0x2000, ZColor::Remapped);

        assert_eq!(
            stale.repair_bad_non_relocating(ZColor::Marked1),
            ZEntry::new(0x2000, ZColor::Marked1)
        );
    }

    #[test]
    fn relocate_non_rs_repairs_to_remapped() {
        let marked = ZEntry::new(0x3000, ZColor::Marked0);

        assert_eq!(
            marked.repair_relocate_non_rs(),
            ZEntry::new(0x3000, ZColor::Remapped)
        );
    }
}
