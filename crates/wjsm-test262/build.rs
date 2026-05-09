use std::path::PathBuf;

/// test262 submodule 位于项目根目录
fn test262_directory() -> PathBuf {
    // build.rs 的 CWD 是 package 根目录 (crates/wjsm-test262/)
    // 项目根目录是其父目录的父目录
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("manifest dir should have a parent")
        .parent()
        .expect("manifest dir parent should have a parent")
        .join("test262")
}

fn main() {
    println!("cargo::rerun-if-changed=build.rs");

    let test262_dir = test262_directory();

    if !test262_dir.is_dir() {
        println!(
            "cargo::warning=test262 submodule not initialized. Run: git submodule update --init test262"
        );
    }
}
