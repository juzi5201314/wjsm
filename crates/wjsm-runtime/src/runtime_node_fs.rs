use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::runtime_buffer::{arraybuffer_visible_bytes, create_buffer_from_bytes, visible_bytes};
use crate::runtime_encoding::{
    BufferEncoding, decode_bytes, encode_js_string, encoding_from_value, js_string_lossy,
};
use crate::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FsMethodKind {
    ReadFileSync,
    WriteFileSync,
    ExistsSync,
    StatSync,
    LstatSync,
    ReaddirSync,
    MkdirSync,
    RmSync,
    AppendFileSync,
    UnlinkSync,
    RenameSync,
    CopyFileSync,
    AccessSync,
    RealpathSync,
    ReadlinkSync,
    SymlinkSync,
    ChmodSync,
    ChownSync,
}

impl FsMethodKind {
    pub(crate) fn from_method(method: u8) -> Option<Self> {
        match method {
            0 => Some(Self::ReadFileSync),
            1 => Some(Self::WriteFileSync),
            2 => Some(Self::ExistsSync),
            3 => Some(Self::StatSync),
            4 => Some(Self::LstatSync),
            5 => Some(Self::ReaddirSync),
            6 => Some(Self::MkdirSync),
            7 => Some(Self::RmSync),
            8 => Some(Self::AppendFileSync),
            9 => Some(Self::UnlinkSync),
            10 => Some(Self::RenameSync),
            11 => Some(Self::CopyFileSync),
            12 => Some(Self::AccessSync),
            13 => Some(Self::RealpathSync),
            14 => Some(Self::ReadlinkSync),
            15 => Some(Self::SymlinkSync),
            16 => Some(Self::ChmodSync),
            17 => Some(Self::ChownSync),
            _ => None,
        }
    }

    pub(crate) fn method(self) -> u8 {
        match self {
            Self::ReadFileSync => 0,
            Self::WriteFileSync => 1,
            Self::ExistsSync => 2,
            Self::StatSync => 3,
            Self::LstatSync => 4,
            Self::ReaddirSync => 5,
            Self::MkdirSync => 6,
            Self::RmSync => 7,
            Self::AppendFileSync => 8,
            Self::UnlinkSync => 9,
            Self::RenameSync => 10,
            Self::CopyFileSync => 11,
            Self::AccessSync => 12,
            Self::RealpathSync => 13,
            Self::ReadlinkSync => 14,
            Self::SymlinkSync => 15,
            Self::ChmodSync => 16,
            Self::ChownSync => 17,
        }
    }
}

pub(crate) fn create_fs_host_object(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 18);
    let temp_root_len = caller.data().push_host_temp_roots([obj]);
    install_fs_method(caller, obj, "readFileSync", FsMethodKind::ReadFileSync);
    install_fs_method(caller, obj, "writeFileSync", FsMethodKind::WriteFileSync);
    install_fs_method(caller, obj, "existsSync", FsMethodKind::ExistsSync);
    install_fs_method(caller, obj, "statSync", FsMethodKind::StatSync);
    install_fs_method(caller, obj, "lstatSync", FsMethodKind::LstatSync);
    install_fs_method(caller, obj, "readdirSync", FsMethodKind::ReaddirSync);
    install_fs_method(caller, obj, "mkdirSync", FsMethodKind::MkdirSync);
    install_fs_method(caller, obj, "rmSync", FsMethodKind::RmSync);
    install_fs_method(caller, obj, "appendFileSync", FsMethodKind::AppendFileSync);
    install_fs_method(caller, obj, "unlinkSync", FsMethodKind::UnlinkSync);
    install_fs_method(caller, obj, "renameSync", FsMethodKind::RenameSync);
    install_fs_method(caller, obj, "copyFileSync", FsMethodKind::CopyFileSync);
    install_fs_method(caller, obj, "accessSync", FsMethodKind::AccessSync);
    install_fs_method(caller, obj, "realpathSync", FsMethodKind::RealpathSync);
    install_fs_method(caller, obj, "readlinkSync", FsMethodKind::ReadlinkSync);
    install_fs_method(caller, obj, "symlinkSync", FsMethodKind::SymlinkSync);
    install_fs_method(caller, obj, "chmodSync", FsMethodKind::ChmodSync);
    install_fs_method(caller, obj, "chownSync", FsMethodKind::ChownSync);
    caller.data().truncate_host_temp_roots(temp_root_len);
    obj
}

pub(crate) fn call_fs_method(
    caller: &mut Caller<'_, RuntimeState>,
    kind: FsMethodKind,
    args: &[i64],
) -> i64 {
    match kind {
        FsMethodKind::ReadFileSync => read_file_sync(caller, args),
        FsMethodKind::WriteFileSync => write_file_sync(caller, args, false),
        FsMethodKind::ExistsSync => exists_sync(caller, args),
        FsMethodKind::StatSync => stat_sync(caller, args, false),
        FsMethodKind::LstatSync => stat_sync(caller, args, true),
        FsMethodKind::ReaddirSync => readdir_sync(caller, args),
        FsMethodKind::MkdirSync => mkdir_sync(caller, args),
        FsMethodKind::RmSync => rm_sync(caller, args),
        FsMethodKind::AppendFileSync => write_file_sync(caller, args, true),
        FsMethodKind::UnlinkSync => unlink_sync(caller, args),
        FsMethodKind::RenameSync => rename_sync(caller, args),
        FsMethodKind::CopyFileSync => copy_file_sync(caller, args),
        FsMethodKind::AccessSync => access_sync(caller, args),
        FsMethodKind::RealpathSync => realpath_sync(caller, args),
        FsMethodKind::ReadlinkSync => readlink_sync(caller, args),
        FsMethodKind::SymlinkSync => symlink_sync(caller, args),
        FsMethodKind::ChmodSync => chmod_sync(caller, args),
        FsMethodKind::ChownSync => chown_sync(caller, args),
    }
}

fn install_fs_method(
    caller: &mut Caller<'_, RuntimeState>,
    obj: i64,
    name: &str,
    kind: FsMethodKind,
) {
    let callable = create_native_callable(caller.data(), NativeCallable::FsMethod { kind });
    let _ = define_host_data_property_from_caller(caller, obj, name, callable);
}

fn read_file_sync(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let path = match path_arg(caller, args.first().copied(), "open") {
        Ok(path) => path,
        Err(err) => return err,
    };
    if let Err(err) = check_read_allowed(caller, &path, "open") {
        return err;
    }
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) => return io_error(caller, err, "open", &path),
    };
    match read_encoding_option(caller, args.get(1).copied()) {
        Ok(Some(encoding)) => decode_bytes(caller, &bytes, encoding),
        Ok(None) => create_buffer_from_bytes(caller, bytes),
        Err(err) => err,
    }
}

fn write_file_sync(caller: &mut Caller<'_, RuntimeState>, args: &[i64], append: bool) -> i64 {
    let syscall = "write";
    let path = match path_arg(caller, args.first().copied(), syscall) {
        Ok(path) => path,
        Err(err) => return err,
    };
    if let Err(err) = check_write_allowed(caller, &path, syscall) {
        return err;
    }
    let data = match data_arg(caller, args.get(1).copied(), args.get(2).copied()) {
        Ok(data) => data,
        Err(err) => return err,
    };
    let append = append || write_flag_append(caller, args.get(2).copied());
    let result = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(!append)
        .append(append)
        .open(&path)
        .and_then(|mut file| std::io::Write::write_all(&mut file, &data));
    match result {
        Ok(()) => value::encode_undefined(),
        Err(err) => io_error(caller, err, syscall, &path),
    }
}

fn exists_sync(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let path = match path_arg(caller, args.first().copied(), "access") {
        Ok(path) => path,
        Err(_) => return value::encode_bool(false),
    };
    if !path.exists() {
        return value::encode_bool(false);
    }
    if check_read_allowed(caller, &path, "access").is_err() {
        return value::encode_bool(false);
    }
    value::encode_bool(true)
}

fn stat_sync(caller: &mut Caller<'_, RuntimeState>, args: &[i64], symlink: bool) -> i64 {
    let syscall = if symlink { "lstat" } else { "stat" };
    let path = match path_arg(caller, args.first().copied(), syscall) {
        Ok(path) => path,
        Err(err) => return err,
    };
    if let Err(err) = check_read_allowed(caller, &path, syscall) {
        return err;
    }
    let metadata = if symlink {
        fs::symlink_metadata(&path)
    } else {
        fs::metadata(&path)
    };
    match metadata {
        Ok(metadata) => metadata_object(caller, &metadata),
        Err(err) => io_error(caller, err, syscall, &path),
    }
}

fn readdir_sync(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let path = match path_arg(caller, args.first().copied(), "scandir") {
        Ok(path) => path,
        Err(err) => return err,
    };
    if let Err(err) = check_read_allowed(caller, &path, "scandir") {
        return err;
    }
    let with_file_types = args
        .get(1)
        .copied()
        .is_some_and(|value| to_boolean(caller, value));
    let mut entries = match fs::read_dir(&path) {
        Ok(entries) => entries.filter_map(Result::ok).collect::<Vec<_>>(),
        Err(err) => return io_error(caller, err, "scandir", &path),
    };
    entries.sort_by_key(|entry| entry.file_name());
    let arr = alloc_array(caller, entries.len() as u32);
    for (index, entry) in entries.into_iter().enumerate() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let value = if with_file_types {
            let kind = entry
                .file_type()
                .map(|file_type| file_kind(&file_type))
                .unwrap_or("other");
            dirent_object(caller, name, kind)
        } else {
            store_runtime_string(caller, name)
        };
        set_array_elem(caller, arr, index as i32, value);
    }
    arr
}

fn mkdir_sync(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let path = match path_arg(caller, args.first().copied(), "mkdir") {
        Ok(path) => path,
        Err(err) => return err,
    };
    if let Err(err) = check_write_allowed(caller, &path, "mkdir") {
        return err;
    }
    let recursive = object_bool_option(caller, args.get(1).copied(), "recursive");
    let result = if recursive {
        fs::create_dir_all(&path)
    } else {
        fs::create_dir(&path)
    };
    match result {
        Ok(()) => value::encode_undefined(),
        Err(err) => io_error(caller, err, "mkdir", &path),
    }
}

fn rm_sync(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let path = match path_arg(caller, args.first().copied(), "rm") {
        Ok(path) => path,
        Err(err) => return err,
    };
    let recursive = object_bool_option(caller, args.get(1).copied(), "recursive");
    let force = object_bool_option(caller, args.get(1).copied(), "force");
    if !path.exists() {
        return if force {
            value::encode_undefined()
        } else {
            node_fs_error(caller, "ENOENT", -2, "rm", &path, None)
        };
    }
    if let Err(err) = check_write_allowed(caller, &path, "rm") {
        return err;
    }
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(err) => return io_error(caller, err, "rm", &path),
    };
    let result = if metadata.is_dir() && !metadata.file_type().is_symlink() {
        if recursive {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_dir(&path)
        }
    } else {
        fs::remove_file(&path)
    };
    match result {
        Ok(()) => value::encode_undefined(),
        Err(err) => io_error(caller, err, "rm", &path),
    }
}

fn unlink_sync(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let path = match path_arg(caller, args.first().copied(), "unlink") {
        Ok(path) => path,
        Err(err) => return err,
    };
    if let Err(err) = check_write_allowed(caller, &path, "unlink") {
        return err;
    }
    match fs::remove_file(&path) {
        Ok(()) => value::encode_undefined(),
        Err(err) => io_error(caller, err, "unlink", &path),
    }
}

fn rename_sync(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let old_path = match path_arg(caller, args.first().copied(), "rename") {
        Ok(path) => path,
        Err(err) => return err,
    };
    let new_path = match path_arg(caller, args.get(1).copied(), "rename") {
        Ok(path) => path,
        Err(err) => return err,
    };
    if let Err(err) = check_write_allowed(caller, &old_path, "rename") {
        return err;
    }
    if let Err(err) = check_write_allowed(caller, &new_path, "rename") {
        return err;
    }
    match fs::rename(&old_path, &new_path) {
        Ok(()) => value::encode_undefined(),
        Err(err) => io_error(caller, err, "rename", &old_path),
    }
}

fn copy_file_sync(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let src = match path_arg(caller, args.first().copied(), "copyfile") {
        Ok(path) => path,
        Err(err) => return err,
    };
    let dest = match path_arg(caller, args.get(1).copied(), "copyfile") {
        Ok(path) => path,
        Err(err) => return err,
    };
    if let Err(err) = check_read_allowed(caller, &src, "copyfile") {
        return err;
    }
    if let Err(err) = check_write_allowed(caller, &dest, "copyfile") {
        return err;
    }
    let mode = args
        .get(2)
        .copied()
        .filter(|value| !value::is_undefined(*value))
        .map(|value| value::decode_f64(to_number(caller, value)).trunc() as i32)
        .unwrap_or(0);
    if (mode & 1) != 0 && dest.exists() {
        return node_fs_error(caller, "EEXIST", -17, "copyfile", &dest, None);
    }
    match fs::copy(&src, &dest) {
        Ok(_) => value::encode_undefined(),
        Err(err) => io_error(caller, err, "copyfile", &src),
    }
}

fn access_sync(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let path = match path_arg(caller, args.first().copied(), "access") {
        Ok(path) => path,
        Err(err) => return err,
    };
    if let Err(err) = check_read_allowed(caller, &path, "access") {
        return err;
    }
    let mode = args
        .get(1)
        .copied()
        .filter(|value| !value::is_undefined(*value))
        .map(|value| value::decode_f64(to_number(caller, value)).trunc() as i32)
        .unwrap_or(0);
    if let Err(err) = access_path(&path, mode) {
        return io_error(caller, err, "access", &path);
    }
    value::encode_undefined()
}

fn realpath_sync(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let path = match path_arg(caller, args.first().copied(), "realpath") {
        Ok(path) => path,
        Err(err) => return err,
    };
    if let Err(err) = check_read_allowed(caller, &path, "realpath") {
        return err;
    }
    match path.canonicalize() {
        Ok(path) => store_runtime_string(caller, path.to_string_lossy().into_owned()),
        Err(err) => io_error(caller, err, "realpath", &path),
    }
}

fn readlink_sync(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let path = match path_arg(caller, args.first().copied(), "readlink") {
        Ok(path) => path,
        Err(err) => return err,
    };
    if let Err(err) = check_read_allowed(caller, &path, "readlink") {
        return err;
    }
    match fs::read_link(&path) {
        Ok(target) => store_runtime_string(caller, target.to_string_lossy().into_owned()),
        Err(err) => io_error(caller, err, "readlink", &path),
    }
}

fn symlink_sync(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let target = match path_arg(caller, args.first().copied(), "symlink") {
        Ok(path) => path,
        Err(err) => return err,
    };
    let path = match path_arg(caller, args.get(1).copied(), "symlink") {
        Ok(path) => path,
        Err(err) => return err,
    };
    if let Err(err) = check_write_allowed(caller, &path, "symlink") {
        return err;
    }
    match create_symlink(&target, &path, args.get(2).copied(), caller) {
        Ok(()) => value::encode_undefined(),
        Err(err) => io_error(caller, err, "symlink", &path),
    }
}

fn chmod_sync(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let path = match path_arg(caller, args.first().copied(), "chmod") {
        Ok(path) => path,
        Err(err) => return err,
    };
    if let Err(err) = check_write_allowed(caller, &path, "chmod") {
        return err;
    }
    let mode = args
        .get(1)
        .copied()
        .map(|value| value::decode_f64(to_number(caller, value)).trunc() as u32)
        .unwrap_or(0);
    match chmod_path(&path, mode) {
        Ok(()) => value::encode_undefined(),
        Err(err) => io_error(caller, err, "chmod", &path),
    }
}

fn chown_sync(caller: &mut Caller<'_, RuntimeState>, args: &[i64]) -> i64 {
    let path = match path_arg(caller, args.first().copied(), "chown") {
        Ok(path) => path,
        Err(err) => return err,
    };
    if let Err(err) = check_write_allowed(caller, &path, "chown") {
        return err;
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let uid = args
            .get(1)
            .copied()
            .map(|value| value::decode_f64(to_number(caller, value)).trunc() as libc::uid_t)
            .unwrap_or(u32::MAX as libc::uid_t);
        let gid = args
            .get(2)
            .copied()
            .map(|value| value::decode_f64(to_number(caller, value)).trunc() as libc::gid_t)
            .unwrap_or(u32::MAX as libc::gid_t);
        let c_path = match std::ffi::CString::new(path.as_os_str().as_bytes()) {
            Ok(path) => path,
            Err(_) => return node_fs_error(caller, "EINVAL", -22, "chown", &path, None),
        };
        // SAFETY: `c_path` is a NUL-terminated path buffer valid for this call.
        let rc = unsafe { libc::chown(c_path.as_ptr(), uid, gid) };
        return if rc == 0 {
            value::encode_undefined()
        } else {
            io_error(caller, std::io::Error::last_os_error(), "chown", &path)
        };
    }
    #[cfg(not(unix))]
    {
        node_fs_error(caller, "ENOSYS", -38, "chown", &path, None)
    }
}

fn path_arg(
    caller: &mut Caller<'_, RuntimeState>,
    value_raw: Option<i64>,
    _syscall: &'static str,
) -> Result<PathBuf, i64> {
    let value_raw = value_raw.unwrap_or_else(value::encode_undefined);
    let raw = if let Some(bytes) =
        visible_bytes(caller, value_raw).or_else(|| arraybuffer_visible_bytes(caller, value_raw))
    {
        String::from_utf8_lossy(&bytes).into_owned()
    } else if value::is_string(value_raw) || value::is_runtime_string_handle(value_raw) {
        js_string_lossy(caller, value_raw)
    } else {
        return Err(make_type_error_exception(
            caller,
            "The path argument must be a string, Buffer, or Uint8Array",
        ));
    };
    let path = PathBuf::from(raw);
    if path.is_absolute() {
        Ok(path)
    } else {
        let cwd = caller
            .data()
            .process
            .cwd
            .as_deref()
            .map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        Ok(cwd.join(path))
    }
}

fn data_arg(
    caller: &mut Caller<'_, RuntimeState>,
    value_raw: Option<i64>,
    options: Option<i64>,
) -> Result<Vec<u8>, i64> {
    let value_raw = value_raw.unwrap_or_else(value::encode_undefined);
    if let Some(bytes) =
        visible_bytes(caller, value_raw).or_else(|| arraybuffer_visible_bytes(caller, value_raw))
    {
        return Ok(bytes);
    }
    let encoding = write_encoding_option(caller, options)?;
    encode_js_string(caller, value_raw, encoding).map_err(|label| unknown_encoding(caller, &label))
}

fn read_encoding_option(
    caller: &mut Caller<'_, RuntimeState>,
    value_raw: Option<i64>,
) -> Result<Option<BufferEncoding>, i64> {
    let Some(value_raw) = value_raw else {
        return Ok(None);
    };
    if value::is_undefined(value_raw) || value::is_null(value_raw) {
        return Ok(None);
    }
    if value::is_string(value_raw) || value::is_runtime_string_handle(value_raw) {
        return encoding_from_value(caller, value_raw)
            .map(Some)
            .map_err(|label| unknown_encoding(caller, &label));
    }
    let Some(ptr) = value::is_object(value_raw)
        .then(|| resolve_handle(caller, value_raw))
        .flatten()
    else {
        return Ok(None);
    };
    let Some(encoding) = read_object_property_by_name(caller, ptr, "encoding") else {
        return Ok(None);
    };
    if value::is_undefined(encoding) || value::is_null(encoding) {
        Ok(None)
    } else {
        encoding_from_value(caller, encoding)
            .map(Some)
            .map_err(|label| unknown_encoding(caller, &label))
    }
}

fn write_encoding_option(
    caller: &mut Caller<'_, RuntimeState>,
    value_raw: Option<i64>,
) -> Result<BufferEncoding, i64> {
    match read_encoding_option(caller, value_raw)? {
        Some(encoding) => Ok(encoding),
        None => Ok(BufferEncoding::Utf8),
    }
}

fn write_flag_append(caller: &mut Caller<'_, RuntimeState>, value_raw: Option<i64>) -> bool {
    let Some(value_raw) = value_raw else {
        return false;
    };
    let Some(ptr) = value::is_object(value_raw)
        .then(|| resolve_handle(caller, value_raw))
        .flatten()
    else {
        return false;
    };
    let Some(flag) = read_object_property_by_name(caller, ptr, "flag") else {
        return false;
    };
    js_string_lossy(caller, flag).starts_with('a')
}

fn object_bool_option(
    caller: &mut Caller<'_, RuntimeState>,
    value_raw: Option<i64>,
    name: &str,
) -> bool {
    let Some(value_raw) = value_raw else {
        return false;
    };
    if value::is_bool(value_raw) {
        return value::decode_bool(value_raw);
    }
    let Some(ptr) = value::is_object(value_raw)
        .then(|| resolve_handle(caller, value_raw))
        .flatten()
    else {
        return false;
    };
    read_object_property_by_name(caller, ptr, name).is_some_and(|value| to_boolean(caller, value))
}

fn check_read_allowed(
    caller: &mut Caller<'_, RuntimeState>,
    path: &Path,
    syscall: &'static str,
) -> Result<(), i64> {
    let canonical = path
        .canonicalize()
        .map_err(|err| io_error(caller, err, syscall, path))?;
    if is_under_any_root(&canonical, caller.data().process.fs_read_roots.iter()) {
        Ok(())
    } else {
        Err(node_fs_error(caller, "EACCES", -13, syscall, path, None))
    }
}

fn check_write_allowed(
    caller: &mut Caller<'_, RuntimeState>,
    path: &Path,
    syscall: &'static str,
) -> Result<(), i64> {
    if caller.data().process.fs_allow_write_anywhere {
        return Ok(());
    }
    let Some(anchor) = canonical_write_anchor(path) else {
        return Err(node_fs_error(caller, "EACCES", -13, syscall, path, None));
    };
    if is_under_any_root(&anchor, caller.data().process.fs_write_roots.iter()) {
        Ok(())
    } else {
        Err(node_fs_error(caller, "EACCES", -13, syscall, path, None))
    }
}

fn canonical_write_anchor(path: &Path) -> Option<PathBuf> {
    if let Ok(canonical) = path.canonicalize() {
        return Some(canonical);
    }
    let mut cursor = path.parent();
    while let Some(parent) = cursor {
        if let Ok(canonical) = parent.canonicalize() {
            return Some(canonical);
        }
        cursor = parent.parent();
    }
    None
}

fn is_under_any_root<'a>(path: &Path, roots: impl Iterator<Item = &'a PathBuf>) -> bool {
    roots
        .map(|root| root.canonicalize().unwrap_or_else(|_| root.clone()))
        .any(|root| path.starts_with(root))
}

fn metadata_object(caller: &mut Caller<'_, RuntimeState>, metadata: &fs::Metadata) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 7);
    let _ = define_host_data_property_from_caller(
        caller,
        obj,
        "size",
        value::encode_f64(metadata.len() as f64),
    );
    let _ = define_host_data_property_from_caller(
        caller,
        obj,
        "mode",
        value::encode_f64(metadata_mode(metadata) as f64),
    );
    let _ = define_host_data_property_from_caller(
        caller,
        obj,
        "mtimeMs",
        value::encode_f64(system_time_ms(metadata.modified())),
    );
    let _ = define_host_data_property_from_caller(
        caller,
        obj,
        "atimeMs",
        value::encode_f64(system_time_ms(metadata.accessed())),
    );
    let _ = define_host_data_property_from_caller(
        caller,
        obj,
        "ctimeMs",
        value::encode_f64(system_time_ms(metadata.created())),
    );
    let _ = define_host_data_property_from_caller(
        caller,
        obj,
        "birthtimeMs",
        value::encode_f64(system_time_ms(metadata.created())),
    );
    let kind = store_runtime_string(caller, file_kind(&metadata.file_type()).to_string());
    let _ = define_host_data_property_from_caller(caller, obj, "kind", kind);
    obj
}

fn dirent_object(caller: &mut Caller<'_, RuntimeState>, name: String, kind: &'static str) -> i64 {
    let env = WasmEnv::from_caller(caller).expect("WasmEnv");
    let obj = alloc_host_object(caller, &env, 2);
    let name = store_runtime_string(caller, name);
    let kind = store_runtime_string(caller, kind.to_string());
    let _ = define_host_data_property_from_caller(caller, obj, "name", name);
    let _ = define_host_data_property_from_caller(caller, obj, "kind", kind);
    obj
}

fn system_time_ms(time: std::io::Result<std::time::SystemTime>) -> f64 {
    time.ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

#[cfg(unix)]
fn metadata_mode(metadata: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode()
}

#[cfg(not(unix))]
fn metadata_mode(metadata: &fs::Metadata) -> u32 {
    if metadata.permissions().readonly() {
        0o444
    } else {
        0o666
    }
}

fn file_kind(file_type: &fs::FileType) -> &'static str {
    if file_type.is_file() {
        "file"
    } else if file_type.is_dir() {
        "directory"
    } else if file_type.is_symlink() {
        "symlink"
    } else {
        file_kind_platform(file_type)
    }
}

#[cfg(unix)]
fn file_kind_platform(file_type: &fs::FileType) -> &'static str {
    use std::os::unix::fs::FileTypeExt;
    if file_type.is_block_device() {
        "block"
    } else if file_type.is_char_device() {
        "character"
    } else if file_type.is_fifo() {
        "fifo"
    } else if file_type.is_socket() {
        "socket"
    } else {
        "other"
    }
}

#[cfg(not(unix))]
fn file_kind_platform(_file_type: &fs::FileType) -> &'static str {
    "other"
}

#[cfg(unix)]
fn access_path(path: &Path, mode: i32) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    // SAFETY: `c_path` is a NUL-terminated path buffer valid for this call.
    let rc = unsafe { libc::access(c_path.as_ptr(), mode) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(unix))]
fn access_path(path: &Path, mode: i32) -> std::io::Result<()> {
    let metadata = fs::metadata(path)?;
    if (mode & 2) != 0 && metadata.permissions().readonly() {
        return Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
    }
    Ok(())
}

#[cfg(unix)]
fn create_symlink(
    target: &Path,
    path: &Path,
    _kind: Option<i64>,
    _caller: &mut Caller<'_, RuntimeState>,
) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, path)
}

#[cfg(windows)]
fn create_symlink(
    target: &Path,
    path: &Path,
    kind: Option<i64>,
    caller: &mut Caller<'_, RuntimeState>,
) -> std::io::Result<()> {
    if kind
        .map(|kind| js_string_lossy(caller, kind) == "dir")
        .unwrap_or(false)
    {
        std::os::windows::fs::symlink_dir(target, path)
    } else {
        std::os::windows::fs::symlink_file(target, path)
    }
}

#[cfg(unix)]
fn chmod_path(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn chmod_path(path: &Path, mode: u32) -> std::io::Result<()> {
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_readonly((mode & 0o222) == 0);
    fs::set_permissions(path, permissions)
}

fn io_error(
    caller: &mut Caller<'_, RuntimeState>,
    err: std::io::Error,
    syscall: &'static str,
    path: &Path,
) -> i64 {
    let (code, errno) = match err.kind() {
        std::io::ErrorKind::NotFound => ("ENOENT", -2),
        std::io::ErrorKind::PermissionDenied => ("EACCES", -13),
        std::io::ErrorKind::AlreadyExists => ("EEXIST", -17),
        std::io::ErrorKind::NotADirectory => ("ENOTDIR", -20),
        std::io::ErrorKind::IsADirectory => ("EISDIR", -21),
        std::io::ErrorKind::DirectoryNotEmpty => ("ENOTEMPTY", -39),
        std::io::ErrorKind::InvalidInput => ("EINVAL", -22),
        _ => ("EIO", -5),
    };
    node_fs_error(caller, code, errno, syscall, path, Some(err))
}

fn node_fs_error(
    caller: &mut Caller<'_, RuntimeState>,
    code: &str,
    errno: i32,
    syscall: &'static str,
    path: &Path,
    source: Option<std::io::Error>,
) -> i64 {
    let message = source
        .as_ref()
        .map(|err| err.to_string())
        .unwrap_or_else(|| format!("{code}: {syscall} '{}'", path.display()));
    let msg_val = store_runtime_string(caller, message.clone());
    let error_obj = create_error_object(caller, "Error", msg_val, value::encode_undefined());
    let code_val = store_runtime_string(caller, code.to_string());
    let syscall_val = store_runtime_string(caller, syscall.to_string());
    let path_val = store_runtime_string(caller, path.to_string_lossy().into_owned());
    let _ = define_host_data_property_from_caller(caller, error_obj, "code", code_val);
    let _ = define_host_data_property_from_caller(
        caller,
        error_obj,
        "errno",
        value::encode_f64(errno as f64),
    );
    let _ = define_host_data_property_from_caller(caller, error_obj, "syscall", syscall_val);
    let _ = define_host_data_property_from_caller(caller, error_obj, "path", path_val);
    let mut errors = caller
        .data()
        .error_table
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let idx = errors.len() as u32;
    errors.push(ErrorEntry {
        name: "Error".to_string(),
        message,
        value: error_obj,
    });
    value::encode_handle(value::TAG_EXCEPTION, idx)
}

fn unknown_encoding(caller: &mut Caller<'_, RuntimeState>, label: &str) -> i64 {
    make_type_error_exception(caller, &format!("Unknown encoding: {label}"))
}
