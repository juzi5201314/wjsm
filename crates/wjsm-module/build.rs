use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let workspace = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("module crate is under workspace/crates");
    let roots = [
        "crates/wjsm-module",
        "crates/wjsm-parser",
        "crates/wjsm-semantic",
        "crates/wjsm-ir",
        "crates/wjsm-artifact-format",
    ];
    let mut files = Vec::new();

    for root in roots {
        let absolute = workspace.join(root);
        println!("cargo:rerun-if-changed={}", absolute.display());
        for entry in WalkDir::new(&absolute) {
            let entry = entry.expect("builtin cache ABI source tree should be readable");
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            if !matches!(
                path.extension().and_then(|extension| extension.to_str()),
                Some("js" | "rs" | "toml")
            ) {
                continue;
            }
            files.push(path.to_path_buf());
        }
    }

    let lock = workspace.join("Cargo.lock");
    println!("cargo:rerun-if-changed={}", lock.display());
    files.push(lock);
    files.sort();

    let mut hasher = Sha256::new();
    hasher.update(b"wjsm-builtin-cache-abi-v1\0");
    for file in files {
        let relative = file
            .strip_prefix(workspace)
            .expect("hashed source is inside workspace");
        let bytes = fs::read(&file).expect("builtin cache ABI source should be readable");
        hasher.update(relative_bytes(relative));
        hasher.update(
            u64::try_from(bytes.len())
                .expect("builtin cache ABI source length fits u64")
                .to_le_bytes(),
        );
        hasher.update(bytes);
    }

    let digest: [u8; 32] = hasher.finalize().into();
    let output =
        PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR")).join("builtin_cache_abi_hash.rs");
    fs::write(
        output,
        format!("const BUILTIN_CACHE_ABI_HASH: [u8; 32] = {digest:?};\n"),
    )
    .expect("generated builtin cache ABI hash should be writable");
}

fn relative_bytes(path: &Path) -> Vec<u8> {
    path.to_string_lossy().replace('\\', "/").into_bytes()
}
