pub mod abi;

pub use abi::{SupportGcFlavor, support_abi_union_hash};

// 非 _v2 文件名与 _v2 同为 V2 artifact（build.rs 双写同一字节）。
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

#[cfg(feature = "embedded")]
pub static EMBEDDED_ZGC_SUPPORT_CWASM: Option<&[u8]> = Some(include_bytes!(concat!(
    env!("OUT_DIR"),
    "/wjsm_support_zgc.cwasm"
)));

#[cfg(feature = "embedded")]
pub static EMBEDDED_MARK_SWEEP_SUPPORT_CWASM_V2: Option<&[u8]> = Some(include_bytes!(concat!(
    env!("OUT_DIR"),
    "/wjsm_support_mark_sweep_v2.cwasm"
)));

#[cfg(feature = "embedded")]
pub static EMBEDDED_G1_SUPPORT_CWASM_V2: Option<&[u8]> = Some(include_bytes!(concat!(
    env!("OUT_DIR"),
    "/wjsm_support_g1_v2.cwasm"
)));

#[cfg(feature = "embedded")]
pub static EMBEDDED_ZGC_SUPPORT_CWASM_V2: Option<&[u8]> = Some(include_bytes!(concat!(
    env!("OUT_DIR"),
    "/wjsm_support_zgc_v2.cwasm"
)));

#[cfg(not(feature = "embedded"))]
pub static EMBEDDED_MARK_SWEEP_SUPPORT_CWASM: Option<&[u8]> = None;
#[cfg(not(feature = "embedded"))]
pub static EMBEDDED_G1_SUPPORT_CWASM: Option<&[u8]> = None;
#[cfg(not(feature = "embedded"))]
pub static EMBEDDED_ZGC_SUPPORT_CWASM: Option<&[u8]> = None;
#[cfg(not(feature = "embedded"))]
pub static EMBEDDED_MARK_SWEEP_SUPPORT_CWASM_V2: Option<&[u8]> = None;
#[cfg(not(feature = "embedded"))]
pub static EMBEDDED_G1_SUPPORT_CWASM_V2: Option<&[u8]> = None;
#[cfg(not(feature = "embedded"))]
pub static EMBEDDED_ZGC_SUPPORT_CWASM_V2: Option<&[u8]> = None;

pub fn embedded_support_cwasm(flavor: SupportGcFlavor) -> Option<&'static [u8]> {
    match flavor {
        SupportGcFlavor::MarkSweep => EMBEDDED_MARK_SWEEP_SUPPORT_CWASM,
        SupportGcFlavor::G1 => EMBEDDED_G1_SUPPORT_CWASM,
        SupportGcFlavor::Zgc => EMBEDDED_ZGC_SUPPORT_CWASM,
    }
}

pub fn embedded_support_cwasm_v2(flavor: SupportGcFlavor) -> Option<&'static [u8]> {
    match flavor {
        SupportGcFlavor::MarkSweep => EMBEDDED_MARK_SWEEP_SUPPORT_CWASM_V2,
        SupportGcFlavor::G1 => EMBEDDED_G1_SUPPORT_CWASM_V2,
        SupportGcFlavor::Zgc => EMBEDDED_ZGC_SUPPORT_CWASM_V2,
    }
}
