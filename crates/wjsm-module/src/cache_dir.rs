//! 磁盘缓存目录的唯一解析策略（issue #376）。
//!
//! 所有磁盘缓存（native image `.wnat`、builtin IR 段、输入寻址 artifact 缓存）
//! 共用同一根目录，解析顺序：
//!
//! 1. `WJSM_CACHE_DIR` 非空 → 使用该目录；
//! 2. `WJSM_CACHE_DIR` 设为空串 → 显式禁用磁盘缓存（只剩进程内缓存）；
//! 3. 未设置 → 回落 `${XDG_CACHE_HOME}/wjsm`（须为绝对路径，遵循 XDG 规范），
//!    否则 `${HOME}/.cache/wjsm`；两者都不可用则禁用磁盘缓存。
//!
//! 缓存内容全部按内容哈希校验（native cache key / builtin ABI 指纹 / artifact
//! 读集回放），目录被污染只会 miss 重建，不会执行脏数据，因此默认落盘安全。

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// 解析当前进程的磁盘缓存根目录；`None` 表示磁盘缓存被禁用。
pub fn resolve_cache_dir() -> Option<PathBuf> {
    resolve_from(
        std::env::var_os("WJSM_CACHE_DIR"),
        std::env::var_os("XDG_CACHE_HOME"),
        std::env::var_os("HOME"),
    )
}

/// 纯函数形式的解析核心，便于测试覆盖全部回落分支。
fn resolve_from(
    explicit: Option<OsString>,
    xdg_cache_home: Option<OsString>,
    home: Option<OsString>,
) -> Option<PathBuf> {
    if let Some(explicit) = explicit {
        if explicit.is_empty() {
            return None;
        }
        return Some(PathBuf::from(explicit));
    }
    if let Some(xdg) = xdg_cache_home
        && !xdg.is_empty()
    {
        let xdg = PathBuf::from(xdg);
        // XDG 规范：相对路径的 XDG_CACHE_HOME 无效，应忽略并继续回落。
        if xdg.is_absolute() {
            return Some(xdg.join("wjsm"));
        }
    }
    if let Some(home) = home
        && !home.is_empty()
    {
        return Some(Path::new(&home).join(".cache").join("wjsm"));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(value: &str) -> Option<OsString> {
        Some(OsString::from(value))
    }

    #[test]
    fn explicit_directory_wins_over_fallbacks() {
        assert_eq!(
            resolve_from(os("/explicit"), os("/xdg"), os("/home/user")),
            Some(PathBuf::from("/explicit"))
        );
    }

    #[test]
    fn empty_explicit_value_disables_disk_cache() {
        assert_eq!(resolve_from(os(""), os("/xdg"), os("/home/user")), None);
    }

    #[test]
    fn unset_falls_back_to_xdg_cache_home() {
        assert_eq!(
            resolve_from(None, os("/xdg"), os("/home/user")),
            Some(PathBuf::from("/xdg/wjsm"))
        );
    }

    #[test]
    fn relative_xdg_cache_home_is_ignored() {
        assert_eq!(
            resolve_from(None, os("relative/cache"), os("/home/user")),
            Some(PathBuf::from("/home/user/.cache/wjsm"))
        );
    }

    #[test]
    fn unset_falls_back_to_home_cache() {
        assert_eq!(
            resolve_from(None, None, os("/home/user")),
            Some(PathBuf::from("/home/user/.cache/wjsm"))
        );
    }

    #[test]
    fn no_environment_disables_disk_cache() {
        assert_eq!(resolve_from(None, None, None), None);
        assert_eq!(resolve_from(None, os(""), os("")), None);
    }
}
