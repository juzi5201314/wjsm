use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use num_traits::ToPrimitive;
use wjsm_ir::value;
use wjsm_native_abi::NativeVmContext;

use super::runtime::{fail_dispatch, is_truthy, render_value, to_number};
use crate::NativeAgentState;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum NodeFsMethod {
    Access,
    AppendFile,
    Chmod,
    Chown,
    CopyFile,
    Exists,
    Lstat,
    Mkdir,
    ReadFile,
    Readdir,
    Readlink,
    Realpath,
    Rename,
    Rm,
    Stat,
    Symlink,
    Unlink,
    WriteFile,
}

pub(crate) fn ensure_bridge(state: &mut NativeAgentState) -> Option<i64> {
    if let Some(bridge) = state.node_fs_bridge {
        return Some(bridge);
    }
    let methods = [
        ("accessSync", NodeFsMethod::Access),
        ("appendFileSync", NodeFsMethod::AppendFile),
        ("chmodSync", NodeFsMethod::Chmod),
        ("chownSync", NodeFsMethod::Chown),
        ("copyFileSync", NodeFsMethod::CopyFile),
        ("existsSync", NodeFsMethod::Exists),
        ("lstatSync", NodeFsMethod::Lstat),
        ("mkdirSync", NodeFsMethod::Mkdir),
        ("readFileSync", NodeFsMethod::ReadFile),
        ("readdirSync", NodeFsMethod::Readdir),
        ("readlinkSync", NodeFsMethod::Readlink),
        ("realpathSync", NodeFsMethod::Realpath),
        ("renameSync", NodeFsMethod::Rename),
        ("rmSync", NodeFsMethod::Rm),
        ("statSync", NodeFsMethod::Stat),
        ("symlinkSync", NodeFsMethod::Symlink),
        ("unlinkSync", NodeFsMethod::Unlink),
        ("writeFileSync", NodeFsMethod::WriteFile),
    ];
    let bridge = state.allocate_object(methods.len() as u32, false).ok()?;
    for (name, method) in methods {
        let key = state.intern_property_string(name.into())?;
        let callable = state.native_callable(crate::NativeCallableKind::NodeFs(method))?;
        state
            .gc
            .heap()
            .set_property(value::decode_handle(bridge), key, callable as u64)
            .ok()?;
    }
    state.node_fs_bridge = Some(bridge);
    Some(bridge)
}

pub(crate) fn call(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    method: NodeFsMethod,
    args: &[i64],
) -> i64 {
    match method {
        NodeFsMethod::Access => access(ctx, state, args),
        NodeFsMethod::AppendFile => write_file(ctx, state, args, true),
        NodeFsMethod::Chmod => chmod(ctx, state, args),
        NodeFsMethod::Chown => chown(ctx, state, args),
        NodeFsMethod::CopyFile => copy_file(ctx, state, args),
        NodeFsMethod::Exists => exists(state, args),
        NodeFsMethod::Lstat => stat(ctx, state, args, false),
        NodeFsMethod::Mkdir => mkdir(ctx, state, args),
        NodeFsMethod::ReadFile => read_file(ctx, state, args),
        NodeFsMethod::Readdir => readdir(ctx, state, args),
        NodeFsMethod::Readlink => readlink(ctx, state, args),
        NodeFsMethod::Realpath => realpath(ctx, state, args),
        NodeFsMethod::Rename => rename(ctx, state, args),
        NodeFsMethod::Rm => rm(ctx, state, args),
        NodeFsMethod::Stat => stat(ctx, state, args, true),
        NodeFsMethod::Symlink => symlink(ctx, state, args),
        NodeFsMethod::Unlink => unlink(ctx, state, args),
        NodeFsMethod::WriteFile => write_file(ctx, state, args, false),
    }
}

fn read_file(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(path) = argument_path(state, args.first().copied()) else {
        return type_error(ctx, state, "Invalid path");
    };
    if super::node_fs_snapshot::is_virtual(state, &path) {
        let as_text = encoding_requested(state, args.get(1).copied());
        return super::node_fs_snapshot::read_file(ctx, state, &path, as_text);
    }
    match fs::read(&path) {
        Ok(bytes) if encoding_requested(state, args.get(1).copied()) => state
            .intern_text(
                String::from_utf8_lossy(&bytes).into_owned(),
                value::TAG_STRING,
            )
            .unwrap_or_else(|| fail_dispatch(ctx)),
        Ok(bytes) => {
            super::node_buffer::from_bytes(state, bytes).unwrap_or_else(|| fail_dispatch(ctx))
        }
        Err(error) => io_exception(ctx, state, error, "open", &path),
    }
}

fn write_file(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    append: bool,
) -> i64 {
    let Some(path) = argument_path(state, args.first().copied()) else {
        return type_error(ctx, state, "Invalid path");
    };
    if super::node_fs_snapshot::is_virtual(state, &path) {
        return super::node_fs_snapshot::readonly(ctx, state, "open", &path);
    }
    let data = args.get(1).copied().unwrap_or_else(value::encode_undefined);
    let bytes = super::node_buffer::bytes(state, data).unwrap_or_else(|| {
        state
            .string_owned(data)
            .and_then(|text| text.to_utf8())
            .unwrap_or_else(|| render_value(state, data))
            .into_bytes()
    });
    let result = if append {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut file| file.write_all(&bytes))
    } else {
        fs::write(&path, bytes)
    };
    match result {
        Ok(()) => value::encode_undefined(),
        Err(error) => io_exception(
            ctx,
            state,
            error,
            if append { "write" } else { "open" },
            &path,
        ),
    }
}

fn exists(state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(path) = argument_path(state, args.first().copied()) else {
        return value::encode_bool(false);
    };
    if super::node_fs_snapshot::is_virtual(state, &path) {
        return super::node_fs_snapshot::exists(state, &path);
    }
    value::encode_bool(path.exists())
}

fn stat(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    follow: bool,
) -> i64 {
    let Some(path) = argument_path(state, args.first().copied()) else {
        return type_error(ctx, state, "Invalid path");
    };
    if super::node_fs_snapshot::is_virtual(state, &path) {
        return super::node_fs_snapshot::stat(ctx, state, &path);
    }
    let result = if follow {
        fs::metadata(&path)
    } else {
        fs::symlink_metadata(&path)
    };
    match result {
        Ok(metadata) => metadata_object(ctx, state, &metadata),
        Err(error) => io_exception(ctx, state, error, "stat", &path),
    }
}

fn metadata_object(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    metadata: &fs::Metadata,
) -> i64 {
    let Ok(object) = state.allocate_object(7, false) else {
        return fail_dispatch(ctx);
    };
    let kind = if metadata.file_type().is_symlink() {
        "symlink"
    } else if metadata.is_file() {
        "file"
    } else if metadata.is_dir() {
        "directory"
    } else {
        "other"
    };
    let kind = state
        .intern_text(kind.into(), value::TAG_STRING)
        .unwrap_or_else(value::encode_undefined);
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::MetadataExt;
        f64::from(metadata.mode())
    };
    #[cfg(not(unix))]
    let mode = 0.0;
    let mtime = system_time_ms(metadata.modified().ok());
    let atime = system_time_ms(metadata.accessed().ok());
    let birthtime = system_time_ms(metadata.created().ok());
    for (name, stored) in [
        ("size", value::encode_f64(metadata.len() as f64)),
        ("mode", value::encode_f64(mode)),
        ("mtimeMs", value::encode_f64(mtime)),
        ("atimeMs", value::encode_f64(atime)),
        ("ctimeMs", value::encode_f64(mtime)),
        ("birthtimeMs", value::encode_f64(birthtime)),
        ("kind", kind),
    ] {
        if set_property(state, object, name, stored).is_none() {
            return fail_dispatch(ctx);
        }
    }
    object
}

fn readdir(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(path) = argument_path(state, args.first().copied()) else {
        return type_error(ctx, state, "Invalid path");
    };
    let with_types = args
        .get(1)
        .copied()
        .is_some_and(|argument| is_truthy(state, argument));
    if super::node_fs_snapshot::is_virtual(state, &path) {
        return super::node_fs_snapshot::readdir(ctx, state, &path, with_types);
    }
    let entries = match fs::read_dir(&path) {
        Ok(entries) => entries,
        Err(error) => return io_exception(ctx, state, error, "scandir", &path),
    };
    let mut values = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => return io_exception(ctx, state, error, "scandir", &path),
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(name_value) = state.intern_text(name, value::TAG_STRING) else {
            return fail_dispatch(ctx);
        };
        if !with_types {
            values.push(name_value);
            continue;
        }
        let Ok(object) = state.allocate_object(2, false) else {
            return fail_dispatch(ctx);
        };
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => return io_exception(ctx, state, error, "scandir", &path),
        };
        let kind = if file_type.is_symlink() {
            "symlink"
        } else if file_type.is_file() {
            "file"
        } else if file_type.is_dir() {
            "directory"
        } else {
            "other"
        };
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
        .allocate_array_values(&values)
        .unwrap_or_else(|_| fail_dispatch(ctx))
}

fn mkdir(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(path) = argument_path(state, args.first().copied()) else {
        return type_error(ctx, state, "Invalid path");
    };
    if super::node_fs_snapshot::is_virtual(state, &path) {
        return super::node_fs_snapshot::readonly(ctx, state, "mkdir", &path);
    }
    let recursive = option_bool(state, args.get(1).copied(), "recursive");
    let result = if recursive {
        fs::create_dir_all(&path)
    } else {
        fs::create_dir(&path)
    };
    match result {
        Ok(()) => value::encode_undefined(),
        Err(error) => io_exception(ctx, state, error, "mkdir", &path),
    }
}

fn rm(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(path) = argument_path(state, args.first().copied()) else {
        return type_error(ctx, state, "Invalid path");
    };
    if super::node_fs_snapshot::is_virtual(state, &path) {
        return super::node_fs_snapshot::readonly(ctx, state, "rm", &path);
    }
    let recursive = option_bool(state, args.get(1).copied(), "recursive");
    let force = option_bool(state, args.get(1).copied(), "force");
    let result = match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_dir() && recursive => fs::remove_dir_all(&path),
        Ok(metadata) if metadata.is_dir() => fs::remove_dir(&path),
        Ok(_) => fs::remove_file(&path),
        Err(error) if force && error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    };
    match result {
        Ok(()) => value::encode_undefined(),
        Err(error) => io_exception(ctx, state, error, "rm", &path),
    }
}

fn unlink(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    unary_path_operation(ctx, state, args, "unlink", |path| fs::remove_file(path))
}

fn rename(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    binary_path_operation(ctx, state, args, "rename", |source, target| {
        fs::rename(source, target)
    })
}

fn copy_file(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(source) = argument_path(state, args.first().copied()) else {
        return type_error(ctx, state, "Invalid source path");
    };
    let Some(target) = argument_path(state, args.get(1).copied()) else {
        return type_error(ctx, state, "Invalid target path");
    };
    if super::node_fs_snapshot::is_virtual(state, &target) {
        return super::node_fs_snapshot::readonly(ctx, state, "copyfile", &target);
    }
    if super::node_fs_snapshot::is_virtual(state, &source) {
        return match state.runtime_modules.store.read_bytes(&source) {
            Ok(bytes) => match fs::write(&target, bytes) {
                Ok(()) => value::encode_undefined(),
                Err(error) => io_exception(ctx, state, error, "copyfile", &target),
            },
            Err(error) => io_exception(
                ctx,
                state,
                std::io::Error::new(std::io::ErrorKind::NotFound, error.to_string()),
                "copyfile",
                &source,
            ),
        };
    }
    match fs::copy(&source, &target) {
        Ok(_) => value::encode_undefined(),
        Err(error) => io_exception(ctx, state, error, "copyfile", &source),
    }
}

fn access(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(path) = argument_path(state, args.first().copied()) else {
        return type_error(ctx, state, "Invalid path");
    };
    if super::node_fs_snapshot::is_virtual(state, &path) {
        return super::node_fs_snapshot::access(ctx, state, &path);
    }
    match fs::metadata(&path) {
        Ok(_) => value::encode_undefined(),
        Err(error) => io_exception(ctx, state, error, "access", &path),
    }
}

fn realpath(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(path) = argument_path(state, args.first().copied()) else {
        return type_error(ctx, state, "Invalid path");
    };
    if super::node_fs_snapshot::is_virtual(state, &path) {
        return super::node_fs_snapshot::realpath(ctx, state, &path);
    }
    match fs::canonicalize(&path) {
        Ok(path) => path_value(ctx, state, path),
        Err(error) => io_exception(ctx, state, error, "realpath", &path),
    }
}

fn readlink(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(path) = argument_path(state, args.first().copied()) else {
        return type_error(ctx, state, "Invalid path");
    };
    if super::node_fs_snapshot::is_virtual(state, &path) {
        return io_exception(
            ctx,
            state,
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a symlink"),
            "readlink",
            &path,
        );
    }
    match fs::read_link(&path) {
        Ok(path) => path_value(ctx, state, path),
        Err(error) => io_exception(ctx, state, error, "readlink", &path),
    }
}

fn symlink(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(target) = argument_path_raw(state, args.first().copied()) else {
        return type_error(ctx, state, "Invalid target path");
    };
    let Some(path) = argument_path(state, args.get(1).copied()) else {
        return type_error(ctx, state, "Invalid path");
    };
    if super::node_fs_snapshot::is_virtual(state, &path) {
        return super::node_fs_snapshot::readonly(ctx, state, "symlink", &path);
    }
    #[cfg(unix)]
    let result = std::os::unix::fs::symlink(&target, &path);
    #[cfg(windows)]
    let result = std::os::windows::fs::symlink_file(&target, &path);
    match result {
        Ok(()) => value::encode_undefined(),
        Err(error) => io_exception(ctx, state, error, "symlink", &path),
    }
}

fn chmod(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(path) = argument_path(state, args.first().copied()) else {
        return type_error(ctx, state, "Invalid path");
    };
    if super::node_fs_snapshot::is_virtual(state, &path) {
        return super::node_fs_snapshot::readonly(ctx, state, "chmod", &path);
    }
    let mode = args
        .get(1)
        .and_then(|mode| to_number(state, *mode))
        .and_then(|mode| mode.to_u32())
        .unwrap_or(0o666);
    #[cfg(unix)]
    let result = {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(mode))
    };
    #[cfg(not(unix))]
    let result = {
        let mut permissions = match fs::metadata(&path) {
            Ok(metadata) => metadata.permissions(),
            Err(error) => return io_exception(ctx, state, error, "chmod", &path),
        };
        permissions.set_readonly(mode & 0o200 == 0);
        fs::set_permissions(&path, permissions)
    };
    match result {
        Ok(()) => value::encode_undefined(),
        Err(error) => io_exception(ctx, state, error, "chmod", &path),
    }
}

fn chown(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(path) = argument_path(state, args.first().copied()) else {
        return type_error(ctx, state, "Invalid path");
    };
    if super::node_fs_snapshot::is_virtual(state, &path) {
        return super::node_fs_snapshot::readonly(ctx, state, "chown", &path);
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let uid = args
            .get(1)
            .and_then(|value| to_number(state, *value))
            .and_then(|value| value.to_u32())
            .unwrap_or(u32::MAX);
        let gid = args
            .get(2)
            .and_then(|value| to_number(state, *value))
            .and_then(|value| value.to_u32())
            .unwrap_or(u32::MAX);
        let Ok(path_bytes) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
            return type_error(ctx, state, "Path contains NUL");
        };
        // SAFETY: CString guarantees a NUL-terminated path and chown retains no pointer.
        if unsafe { libc::chown(path_bytes.as_ptr(), uid, gid) } == 0 {
            value::encode_undefined()
        } else {
            io_exception(ctx, state, std::io::Error::last_os_error(), "chown", &path)
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        type_error(ctx, state, "chown is unavailable on this platform")
    }
}

fn unary_path_operation(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    syscall: &str,
    operation: impl FnOnce(&Path) -> std::io::Result<()>,
) -> i64 {
    let Some(path) = argument_path(state, args.first().copied()) else {
        return type_error(ctx, state, "Invalid path");
    };
    if super::node_fs_snapshot::is_virtual(state, &path) {
        return super::node_fs_snapshot::readonly(ctx, state, syscall, &path);
    }
    match operation(&path) {
        Ok(()) => value::encode_undefined(),
        Err(error) => io_exception(ctx, state, error, syscall, &path),
    }
}

fn binary_path_operation(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
    syscall: &str,
    operation: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> i64 {
    let Some(source) = argument_path(state, args.first().copied()) else {
        return type_error(ctx, state, "Invalid source path");
    };
    let Some(target) = argument_path(state, args.get(1).copied()) else {
        return type_error(ctx, state, "Invalid target path");
    };
    if super::node_fs_snapshot::is_virtual(state, &source)
        || super::node_fs_snapshot::is_virtual(state, &target)
    {
        return super::node_fs_snapshot::readonly(ctx, state, syscall, &source);
    }
    match operation(&source, &target) {
        Ok(()) => value::encode_undefined(),
        Err(error) => io_exception(ctx, state, error, syscall, &source),
    }
}

fn argument_path(state: &NativeAgentState, encoded: Option<i64>) -> Option<PathBuf> {
    super::node_fs_snapshot::resolve_fs_path(state, encoded)
}

pub(super) fn argument_path_raw(state: &NativeAgentState, encoded: Option<i64>) -> Option<PathBuf> {
    state
        .string_owned(encoded?)
        .and_then(|path| path.to_utf8())
        .map(PathBuf::from)
}

fn encoding_requested(state: &mut NativeAgentState, encoded: Option<i64>) -> bool {
    let Some(encoded) = encoded.filter(|encoded| !value::is_undefined(*encoded)) else {
        return false;
    };
    if value::is_string(encoded) {
        return true;
    }
    super::modules::named_property(state, encoded, "encoding")
        .is_some_and(|encoding| !value::is_undefined(encoding))
}

fn option_bool(state: &mut NativeAgentState, options: Option<i64>, name: &str) -> bool {
    options
        .filter(|options| !value::is_undefined(*options))
        .and_then(|options| super::modules::named_property(state, options, name))
        .is_some_and(|option| is_truthy(state, option))
}

pub(super) fn path_value(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    path: PathBuf,
) -> i64 {
    state
        .intern_text(path.to_string_lossy().into_owned(), value::TAG_STRING)
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn system_time_ms(time: Option<SystemTime>) -> f64 {
    time.and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0.0, |duration| duration.as_secs_f64() * 1_000.0)
}

pub(super) fn set_property(
    state: &mut NativeAgentState,
    object: i64,
    name: &str,
    stored: i64,
) -> Option<()> {
    let key = state.intern_property_string(name.into())?;
    state
        .gc
        .heap()
        .set_property(value::decode_handle(object), key, stored as u64)
        .ok()
}

pub(super) fn io_exception(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    error: std::io::Error,
    syscall: &str,
    path: &Path,
) -> i64 {
    let code = match error.kind() {
        std::io::ErrorKind::NotFound => "ENOENT",
        std::io::ErrorKind::PermissionDenied => "EACCES",
        std::io::ErrorKind::AlreadyExists => "EEXIST",
        std::io::ErrorKind::InvalidInput => "EINVAL",
        std::io::ErrorKind::NotADirectory => "ENOTDIR",
        std::io::ErrorKind::IsADirectory => "EISDIR",
        _ => "EIO",
    };
    io_exception_with_code(ctx, state, error, syscall, path, code)
}

pub(super) fn io_exception_with_code(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    error: std::io::Error,
    syscall: &str,
    path: &Path,
    code: &str,
) -> i64 {
    let message = format!("{code}: {}, {syscall} '{}'", error, path.to_string_lossy());
    let Some(object) = super::modules::named_error_object(state, "Error", message) else {
        return fail_dispatch(ctx);
    };
    let Some(code) = state.intern_text(code.into(), value::TAG_STRING) else {
        return fail_dispatch(ctx);
    };
    let Some(syscall) = state.intern_text(syscall.into(), value::TAG_STRING) else {
        return fail_dispatch(ctx);
    };
    let Some(path) = state.intern_text(path.to_string_lossy().into_owned(), value::TAG_STRING)
    else {
        return fail_dispatch(ctx);
    };
    if set_property(state, object, "code", code).is_none()
        || set_property(state, object, "syscall", syscall).is_none()
        || set_property(state, object, "path", path).is_none()
    {
        return fail_dispatch(ctx);
    }
    state
        .create_exception(object)
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn type_error(ctx: &mut NativeVmContext, state: &mut NativeAgentState, message: &str) -> i64 {
    super::modules::named_error_object(state, "TypeError", message.into())
        .and_then(|error| state.create_exception(error))
        .unwrap_or_else(|| fail_dispatch(ctx))
}
