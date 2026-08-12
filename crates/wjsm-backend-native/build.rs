use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use walkdir::WalkDir;

fn main() {
    let crate_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let workspace = crate_dir
        .parent()
        .and_then(Path::parent)
        .expect("backend crate is under workspace/crates");
    let roots = [
        "crates/wjsm-backend-native",
        "crates/wjsm-ir",
        "crates/wjsm-artifact-format",
        "crates/wjsm-host",
        "crates/wjsm-native-abi",
    ];
    let mut files = Vec::new();
    for root in roots {
        let absolute = workspace.join(root);
        println!("cargo:rerun-if-changed={}", absolute.display());
        for entry in WalkDir::new(&absolute) {
            let entry = entry.expect("codegen source tree should be readable");
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            if matches!(
                path.extension().and_then(|extension| extension.to_str()),
                Some("rs" | "toml")
            ) {
                files.push(path.to_path_buf());
            }
        }
    }
    let lock = workspace.join("Cargo.lock");
    println!("cargo:rerun-if-changed={}", lock.display());
    files.push(lock);
    files.sort_by(|left, right| {
        relative_bytes(workspace, left).cmp(&relative_bytes(workspace, right))
    });

    let mut hasher = Sha256::new();
    for file in files {
        let relative = file
            .strip_prefix(workspace)
            .expect("hashed source is inside workspace");
        let relative = relative
            .to_str()
            .expect("workspace source path should be UTF-8")
            .replace('\\', "/");
        let bytes = fs::read(&file).expect("codegen source should be readable");
        hasher.update(relative.as_bytes());
        hasher.update([0]);
        hasher.update(
            u64::try_from(bytes.len())
                .expect("source length fits u64")
                .to_le_bytes(),
        );
        hasher.update(bytes);
    }
    let digest: [u8; 32] = hasher.finalize().into();
    let output =
        PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR")).join("native_codegen_hash.rs");
    fs::write(
        output,
        format!("pub const NATIVE_CODEGEN_HASH: [u8; 32] = {digest:?};\n"),
    )
    .expect("generated codegen hash should be writable");
}

fn relative_bytes(workspace: &Path, path: &Path) -> Vec<u8> {
    path.strip_prefix(workspace)
        .expect("hashed source is inside workspace")
        .to_string_lossy()
        .replace('\\', "/")
        .into_bytes()
}
