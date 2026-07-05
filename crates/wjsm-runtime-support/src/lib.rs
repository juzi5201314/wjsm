pub mod abi;

pub use abi::{SupportGcFlavor, support_abi_union_hash};

#[cfg(feature = "embedded")]
pub static EMBEDDED_MARK_SWEEP_SUPPORT_CWASM: Option<&[u8]> = Some(include_bytes!(concat!(
    env!("OUT_DIR"),
    "/wjsm_support_mark_sweep.cwasm"
)));

#[cfg(feature = "embedded")]
pub static EMBEDDED_G1_SUPPORT_CWASM: Option<&[u8]> = Some(include_bytes!(concat!(
    env!("OUT_DIR"),
    "/wjsm_support_g1.cwasm"
)));

#[cfg(not(feature = "embedded"))]
pub static EMBEDDED_MARK_SWEEP_SUPPORT_CWASM: Option<&[u8]> = None;
#[cfg(not(feature = "embedded"))]
pub static EMBEDDED_G1_SUPPORT_CWASM: Option<&[u8]> = None;

pub fn embedded_support_cwasm(flavor: SupportGcFlavor) -> Option<&'static [u8]> {
    match flavor {
        SupportGcFlavor::MarkSweep => EMBEDDED_MARK_SWEEP_SUPPORT_CWASM,
        SupportGcFlavor::G1 => EMBEDDED_G1_SUPPORT_CWASM,
        SupportGcFlavor::Zgc => None,
    }
}
