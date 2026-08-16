//! `wjsm build --format native-executable`：复制 stub、缝 overlay、原子写出。

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, bail};
use wjsm_artifact_format::{ArtifactLimits, PortableArtifact};
use wjsm_exec_format::{locate_exec_stub, pack};
use wjsm_host_native::{compile_native_exec_images, exec_payload_from_images};

/// 打包失败时不创建或覆盖 `output`。
pub(crate) fn write_native_executable(
    artifact_bytes: &[u8],
    files: BTreeMap<String, Vec<u8>>,
    output: &Path,
) -> Result<()> {
    if output.as_os_str() == "-" {
        bail!("refusing to write a native executable to stdout; use `-o <path>`");
    }
    let packed = pack_native_executable(artifact_bytes, files)?;
    write_atomically(output, &packed)
}

fn pack_native_executable(
    artifact_bytes: &[u8],
    files: BTreeMap<String, Vec<u8>>,
) -> Result<Vec<u8>> {
    let artifact =
        PortableArtifact::decode(artifact_bytes.to_vec().into(), &ArtifactLimits::default())
            .map_err(|error| anyhow::anyhow!("invalid portable artifact: {error}"))?;
    let stub_path = locate_exec_stub().context("failed to locate wjsm-exec stub")?;
    let stub = fs::read(&stub_path)
        .with_context(|| format!("failed to read wjsm-exec stub '{}'", stub_path.display()))?;
    let images = compile_native_exec_images(&artifact)
        .context("failed to compile native executable images")?;
    let payload = exec_payload_from_images(&artifact, &images, files)
        .context("failed to encode native executable payload")?;
    pack(&stub, &payload).context("failed to pack native executable")
}

fn write_atomically(output: &Path, bytes: &[u8]) -> Result<()> {
    let parent = output.parent().filter(|path| !path.as_os_str().is_empty());
    if let Some(parent) = parent {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create '{}'", parent.display()))?;
    }
    let directory = parent.unwrap_or_else(|| Path::new("."));
    let temp = directory.join(format!(
        ".{}.{}.wjsm-exec.tmp",
        std::process::id(),
        output
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("out")
    ));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        let mut file = options
            .open(&temp)
            .with_context(|| format!("failed to create '{}'", temp.display()))?;
        file.write_all(bytes)?;
        file.sync_all()?;
        set_executable(&temp)?;
        fs::rename(&temp, output)
            .with_context(|| format!("failed to write '{}'", output.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<()> {
    Ok(())
}
