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
    println!("cargo::rerun-if-changed=../../.gitmodules");

    let test262_dir = test262_directory();

    if test262_dir.is_dir() {
        // 检查目录是否非空（submodule 已 checkout）
        let has_content = std::fs::read_dir(&test262_dir)
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(false);
        if has_content {
            println!("cargo::rustc-cfg=test262_available");
        } else {
            println!(
                "cargo::warning=test262 submodule directory exists but is empty. \
                 Run: git submodule update --init test262"
            );
            println!(
                "cargo::warning=Test262 conformance tests will be non-functional until the submodule is initialized."
            );
        }
    } else {
        println!(
            "cargo::warning=test262 submodule not found at {}. \
             Run: git submodule update --init test262",
            test262_dir.display()
        );
        println!(
            "cargo::warning=Test262 conformance tests will be non-functional until the submodule is initialized."
        );
    }
}
