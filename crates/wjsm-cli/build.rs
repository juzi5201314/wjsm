use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// 生成 CLI 管线源码指纹：输入寻址 artifact 缓存（issue #376）的键需要覆盖
/// artifact 生产方自身——CLI 决定 manifest 形态、logical URL、source map 选项等，
/// 这些行为变化不会体现在 wjsm-module 的语义 ABI 指纹里。本 crate 源码任一
/// 变化都会切换缓存命名空间，杜绝跨编译器版本的脏命中。
fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    println!("cargo:rerun-if-changed={}", manifest_dir.display());

    let mut files = Vec::new();
    for entry in WalkDir::new(&manifest_dir) {
        let entry = entry.expect("CLI source tree should be readable");
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if !matches!(
            path.extension().and_then(|extension| extension.to_str()),
            Some("rs" | "toml")
        ) {
            continue;
        }
        files.push(path.to_path_buf());
    }
    files.sort();

    let mut hasher = Sha256::new();
    hasher.update(b"wjsm-cli-pipeline-v1\0");
    for file in files {
        let relative = file
            .strip_prefix(&manifest_dir)
            .expect("hashed source is inside the CLI crate");
        let bytes = fs::read(&file).expect("CLI source should be readable");
        hasher.update(relative_bytes(relative));
        hasher.update(
            u64::try_from(bytes.len())
                .expect("CLI source length fits u64")
                .to_le_bytes(),
        );
        hasher.update(bytes);
    }

    let digest: [u8; 32] = hasher.finalize().into();
    let output =
        PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR")).join("cli_pipeline_hash.rs");
    fs::write(
        output,
        format!("const CLI_PIPELINE_SOURCE_HASH: [u8; 32] = {digest:?};\n"),
    )
    .expect("generated CLI pipeline hash should be writable");
}

fn relative_bytes(path: &Path) -> Vec<u8> {
    path.to_string_lossy().replace('\\', "/").into_bytes()
}
