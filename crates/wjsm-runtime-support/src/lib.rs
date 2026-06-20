//! Build-time embedded shared support module (precompiled wasmtime artifact).

pub mod abi;

pub use abi::support_module_layout_hash;

#[cfg(feature = "embedded")]
pub static EMBEDDED_SUPPORT_CWASM: Option<&[u8]> = Some(include_bytes!(concat!(
    env!("OUT_DIR"),
    "/wjsm_support.cwasm"
)));

#[cfg(not(feature = "embedded"))]
pub static EMBEDDED_SUPPORT_CWASM: Option<&[u8]> = None;
