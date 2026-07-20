//! Build-time embedded startup snapshot bytes.
//!
//! When the `embedded` feature is off, both artifacts are `None`.
//! When `embedded` is on, `build.rs` generates `embeds.rs` based on the
//! *runtime* ABI after Cargo feature unification (not only this crate's
//! `managed-heap-v2` feature), so V1 snapshot build never runs against a V2
//! support module.

#[cfg(feature = "embedded")]
include!(concat!(env!("OUT_DIR"), "/embeds.rs"));

#[cfg(not(feature = "embedded"))]
pub static EMBEDDED_STARTUP_SNAPSHOT: Option<&[u8]> = None;

#[cfg(not(feature = "embedded"))]
pub static EMBEDDED_MANAGED_HEAP_V2_ARTIFACT_ABI: Option<&[u8]> = None;
