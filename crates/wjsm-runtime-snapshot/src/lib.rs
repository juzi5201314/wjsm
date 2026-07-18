//! Build-time embedded startup snapshot bytes.
#[cfg(feature = "embedded")]
pub static EMBEDDED_STARTUP_SNAPSHOT: Option<&[u8]> = Some(include_bytes!(concat!(
    env!("OUT_DIR"),
    "/wjsm_startup_snapshot.bin"
)));

#[cfg(not(feature = "embedded"))]
pub static EMBEDDED_STARTUP_SNAPSHOT: Option<&[u8]> = None;

#[cfg(all(feature = "embedded", feature = "managed-heap-v2"))]
pub static EMBEDDED_MANAGED_HEAP_V2_ARTIFACT_ABI: Option<&[u8]> = Some(include_bytes!(concat!(
    env!("OUT_DIR"),
    "/wjsm_managed_heap_v2_artifact_abi.bin"
)));

#[cfg(not(all(feature = "embedded", feature = "managed-heap-v2")))]
pub static EMBEDDED_MANAGED_HEAP_V2_ARTIFACT_ABI: Option<&[u8]> = None;
