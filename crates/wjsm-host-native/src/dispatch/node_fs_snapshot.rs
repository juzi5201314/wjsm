//! packed 虚拟路径的 `node:fs` 分流：读快照、写 EROFS。

use std::path::{Path, PathBuf};

use wjsm_ir::value;
use wjsm_module::{SNAPSHOT_FILE_URL_PREFIX, SNAPSHOT_VIRTUAL_ROOT, is_snapshot_fs_path};
use wjsm_native_abi::NativeVmContext;

use super::node_fs::{io_exception, io_exception_with_code, path_value, set_property};
use super::runtime::fail_dispatch;
use crate::NativeAgentState;

pub(super) fn resolve_fs_path(state: &NativeAgentState, encoded: Option<i64>) -> Option<PathBuf> {
    let path = super::node_fs::argument_path_raw(state, encoded)?;
    if let Some(text) = path.to_str()
        && let Some(logical) = text.strip_prefix(SNAPSHOT_FILE_URL_PREFIX)
    {
        return Some(PathBuf::from(format!("{SNAPSHOT_VIRTUAL_ROOT}/{logical}")));
    }
    if is_snapshot_fs_path(&path) {
        return Some(path);
    }
    Some(if path.is_absolute() {
        path
    } else {
        state.working_directory.join(path)
    })
}

pub(super) fn is_virtual(state: &NativeAgentState, path: &Path) -> bool {
    state.runtime_modules.store.is_snapshot() && is_snapshot_fs_path(path)
}

pub(super) fn read_file(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    path: &Path,
    as_text: bool,
) -> i64 {
    match state.runtime_modules.store.read_bytes(path) {
        Ok(bytes) if as_text => state
            .intern_text(
                String::from_utf8_lossy(&bytes).into_owned(),
                value::TAG_STRING,
            )
            .unwrap_or_else(|| fail_dispatch(ctx)),
        Ok(bytes) => {
            super::node_buffer::from_bytes(state, bytes).unwrap_or_else(|| fail_dispatch(ctx))
        }
        Err(_) => snapshot_not_found(ctx, state, "open", path),
    }
}

pub(super) fn exists(state: &NativeAgentState, path: &Path) -> i64 {
    value::encode_bool(state.runtime_modules.store.exists(path))
}

pub(super) fn access(ctx: &mut NativeVmContext, state: &mut NativeAgentState, path: &Path) -> i64 {
    if state.runtime_modules.store.exists(path) {
        value::encode_undefined()
    } else {
        snapshot_not_found(ctx, state, "access", path)
    }
}

pub(super) fn stat(ctx: &mut NativeVmContext, state: &mut NativeAgentState, path: &Path) -> i64 {
    let store = &state.runtime_modules.store;
    if store.is_file(path) {
        return match store.read_bytes(path) {
            Ok(bytes) => snapshot_stat_object(ctx, state, bytes.len() as f64, "file"),
            Err(_) => snapshot_not_found(ctx, state, "stat", path),
        };
    }
    if store.is_dir(path) {
        return snapshot_stat_object(ctx, state, 0.0, "directory");
    }
    snapshot_not_found(ctx, state, "stat", path)
}

pub(super) fn realpath(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    path: &Path,
) -> i64 {
    match state.runtime_modules.store.canonicalize(path) {
        Ok(resolved) => path_value(ctx, state, resolved),
        Err(_) => snapshot_not_found(ctx, state, "realpath", path),
    }
}

pub(super) fn readdir(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    path: &Path,
    with_types: bool,
) -> i64 {
    let names = match state.runtime_modules.store.read_dir_names(path) {
        Ok(Some(names)) => names,
        Ok(None) | Err(_) => return snapshot_not_found(ctx, state, "scandir", path),
    };
    let mut values = Vec::with_capacity(names.len());
    for name in names {
        let is_dir = state.runtime_modules.store.is_dir(&path.join(&name));
        let Some(name_value) = state.intern_text(name, value::TAG_STRING) else {
            return fail_dispatch(ctx);
        };
        if !with_types {
            values.push(name_value);
            continue;
        }
        let Ok(object) = state.allocate_object_with_gc_retry(ctx, 2, false) else {
            return fail_dispatch(ctx);
        };
        let kind = if is_dir { "directory" } else { "file" };
        let Some(kind) = state.intern_text(kind.into(), value::TAG_STRING) else {
            return fail_dispatch(ctx);
        };
        if set_property(state, object, "name", name_value).is_none()
            || set_property(state, object, "kind", kind).is_none()
        {
            return fail_dispatch(ctx);
        }
        values.push(object);
    }
    state
        .allocate_array_values_with_gc_retry(ctx, &values)
        .unwrap_or_else(|_| fail_dispatch(ctx))
}

pub(super) fn readonly(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    syscall: &str,
    path: &Path,
) -> i64 {
    io_exception_with_code(
        ctx,
        state,
        std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "snapshot file system is read-only",
        ),
        syscall,
        path,
        "EROFS",
    )
}

fn snapshot_stat_object(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    size: f64,
    kind: &str,
) -> i64 {
    let Ok(object) = state.allocate_object_with_gc_retry(ctx, 7, false) else {
        return fail_dispatch(ctx);
    };
    let kind = state
        .intern_text(kind.into(), value::TAG_STRING)
        .unwrap_or_else(value::encode_undefined);
    for (name, stored) in [
        ("size", value::encode_f64(size)),
        ("mode", value::encode_f64(292.0)),
        ("mtimeMs", value::encode_f64(0.0)),
        ("atimeMs", value::encode_f64(0.0)),
        ("ctimeMs", value::encode_f64(0.0)),
        ("birthtimeMs", value::encode_f64(0.0)),
        ("kind", kind),
    ] {
        if set_property(state, object, name, stored).is_none() {
            return fail_dispatch(ctx);
        }
    }
    object
}

fn snapshot_not_found(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    syscall: &str,
    path: &Path,
) -> i64 {
    io_exception(
        ctx,
        state,
        std::io::Error::new(std::io::ErrorKind::NotFound, "not in snapshot"),
        syscall,
        path,
    )
}
