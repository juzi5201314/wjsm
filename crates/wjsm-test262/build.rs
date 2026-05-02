use std::path::Path;
use std::process::Command;

const TEST262_COMMIT: &str = "d0c1b4555b03dd404873fd6422a4b5da00136500";
const TEST262_REPOSITORY: &str = "https://github.com/tc39/test262.git";
const TEST262_DIRECTORY: &str = "test262";

fn main() {
    println!("cargo::rerun-if-changed=build.rs");

    if Path::new(TEST262_DIRECTORY).is_dir() {
        update_test262();
    } else {
        clone_test262();
    }
}

fn clone_test262() {
    println!("Cloning test262 repository...");
    let status = Command::new("git")
        .args(["clone", TEST262_REPOSITORY, TEST262_DIRECTORY])
        .status()
        .expect("failed to execute git clone");

    if !status.success() {
        panic!("failed to clone test262 repository");
    }

    reset_to_commit();
}

fn update_test262() {
    println!("Updating test262 repository...");

    let fetch_status = Command::new("git")
        .args(["fetch", "origin"])
        .current_dir(TEST262_DIRECTORY)
        .status()
        .expect("failed to execute git fetch");

    if !fetch_status.success() {
        panic!("failed to fetch test262 repository");
    }

    reset_to_commit();
}

fn reset_to_commit() {
    let status = Command::new("git")
        .args(["reset", "--hard", TEST262_COMMIT])
        .current_dir(TEST262_DIRECTORY)
        .status()
        .expect("failed to execute git reset");

    if !status.success() {
        panic!("failed to reset test262 to commit {}", TEST262_COMMIT);
    }

    println!("test262 is at commit {}", TEST262_COMMIT);
}
