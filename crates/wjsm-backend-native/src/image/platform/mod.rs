#[cfg(any(target_os = "linux", target_os = "macos"))]
mod unix;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) use unix::{ExecutableMapping, align_to_page, page_size};
#[cfg(target_os = "windows")]
pub(crate) use windows::{ExecutableMapping, align_to_page, page_size};
