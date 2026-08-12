use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use walkdir::WalkDir;
use wjsm_gc::{ManagedHeapLayout, ShapeTable};
use wjsm_snapshot_format::{
    NativeStartupSnapshot, SnapshotEndian, SnapshotGeneration, SnapshotHandle, encode_snapshot,
};

const DEFAULT_MAX_HEAP_BYTES: u64 = 64 * 1024 * 1024;
const HOST_STATE_MAGIC: &[u8; 8] = b"WJSMHST\0";
const HOST_STATE_VERSION: u32 = 1;

fn main() {
    let crate_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let workspace = crate_dir
        .parent()
        .and_then(Path::parent)
        .expect("host-native crate is under workspace/crates");
    let bootstrap_hash = hash_bootstrap_sources(workspace);
    let layout = ManagedHeapLayout::new(DEFAULT_MAX_HEAP_BYTES, 64 * 1024)
        .expect("native startup heap layout is valid");
    let object_heap_base = layout.object_heap_base();
    let target = format!(
        "{}-{}",
        env::var("CARGO_CFG_TARGET_ARCH").expect("target arch"),
        env::var("CARGO_CFG_TARGET_OS").expect("target OS")
    );
    let endian = match env::var("CARGO_CFG_TARGET_ENDIAN")
        .expect("target endian")
        .as_str()
    {
        "little" => SnapshotEndian::Little,
        "big" => SnapshotEndian::Big,
        endian => panic!("unsupported target endian {endian}"),
    };
    let mut object_bytes = vec![0; wjsm_ir::constants::HEAP_OBJECT_HEADER_SIZE as usize];
    object_bytes[..4].copy_from_slice(&u32::MAX.to_le_bytes());
    let snapshot = NativeStartupSnapshot {
        bootstrap_hash,
        lowering_hash: wjsm_backend_native::NATIVE_CODEGEN_HASH,
        semantic_abi_hash: wjsm_artifact_format::semantic_abi_hash(),
        native_abi_hash: wjsm_native_abi::native_abi_hash(),
        target,
        endian,
        object_heap_base,
        object_heap_end: layout.object_heap_end(),
        next_handle: 1,
        global_object: wjsm_ir::value::encode_handle(wjsm_ir::value::TAG_OBJECT, 0),
        object_bytes,
        handles: vec![SnapshotHandle {
            handle: 0,
            address: object_heap_base,
            generation: SnapshotGeneration::Young,
        }],
        shape_table_bytes: serde_json::to_vec(&ShapeTable::new().export())
            .expect("empty shape table serializes"),
        host_state_bytes: encode_host_state(),
    };
    let bytes = encode_snapshot(&snapshot).expect("native startup snapshot encodes");
    let output_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    fs::write(output_dir.join("startup_snapshot.bin"), bytes)
        .expect("startup snapshot should be writable");
    fs::write(
        output_dir.join("bootstrap_hash.rs"),
        format!("pub const BOOTSTRAP_HASH: [u8; 32] = {bootstrap_hash:?};\n"),
    )
    .expect("bootstrap hash should be writable");
}

fn encode_host_state() -> Vec<u8> {
    let mut bytes = Vec::with_capacity(23);
    bytes.extend_from_slice(HOST_STATE_MAGIC);
    bytes.extend_from_slice(&HOST_STATE_VERSION.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u32.to_le_bytes());
    bytes.extend_from_slice(&wjsm_ir::Builtin::EvalIndirect.wire_id().to_le_bytes());
    bytes.push(0);
    bytes
}

fn hash_bootstrap_sources(workspace: &Path) -> [u8; 32] {
    let roots = [
        "crates/wjsm-host-native",
        "crates/wjsm-builtins",
        "crates/wjsm-gc",
        "crates/wjsm-module/builtin_js",
        "crates/wjsm-semantic",
        "crates/wjsm-snapshot-format",
    ];
    let mut files = Vec::new();
    for root in roots {
        let absolute = workspace.join(root);
        println!("cargo:rerun-if-changed={}", absolute.display());
        for entry in WalkDir::new(&absolute) {
            let entry = entry.expect("bootstrap source tree should be readable");
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            if matches!(
                path.extension().and_then(|extension| extension.to_str()),
                Some("js" | "rs" | "toml")
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
        let relative = relative_bytes(workspace, &file);
        let bytes = fs::read(&file).expect("bootstrap source should be readable");
        hasher.update(relative);
        hasher.update([0]);
        hasher.update(
            u64::try_from(bytes.len())
                .expect("source length fits u64")
                .to_le_bytes(),
        );
        hasher.update(bytes);
    }
    hasher.finalize().into()
}

fn relative_bytes(workspace: &Path, path: &Path) -> Vec<u8> {
    path.strip_prefix(workspace)
        .expect("hashed source is inside workspace")
        .to_string_lossy()
        .replace('\\', "/")
        .into_bytes()
}
