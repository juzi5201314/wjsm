//! Build-time embedded startup snapshot bytes.
#[cfg(feature = "embedded")]
pub static EMBEDDED_STARTUP_SNAPSHOT: Option<&[u8]> = Some(include_bytes!(concat!(
    env!("OUT_DIR"),
    "/wjsm_startup_snapshot.bin"
)));

#[cfg(not(feature = "embedded"))]
pub static EMBEDDED_STARTUP_SNAPSHOT: Option<&[u8]> = None;
