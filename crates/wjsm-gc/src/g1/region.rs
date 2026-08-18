use wjsm_ir::constants;

pub const CARD_SIZE: usize = constants::GC_CARD_SIZE as usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RegionKind {
    Eden = 1,
    Survivor = 2,
    Old = 3,
    HumongousStart = 4,
}
