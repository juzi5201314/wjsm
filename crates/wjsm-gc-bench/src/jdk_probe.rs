use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct JdkProbeMetadata {
    pub jdk_home: PathBuf,
    pub java_version: String,
    pub patch_sha256: String,
    pub diagnostic_counters_available: bool,
    pub classes_dir: PathBuf,
}

pub fn prepare_jdk(jdk_home: &Path, probe_home: &Path) -> Result<JdkProbeMetadata> {
    let classes_dir = probe_home.join("classes");
    std::fs::create_dir_all(&classes_dir)
        .with_context(|| format!("create {}", classes_dir.display()))?;
    let java_version = java_version(jdk_home)?;
    let source = source_path();
    let javac = jdk_home.join("bin/javac");
    let status = Command::new(&javac)
        .arg("--release")
        .arg("25")
        .arg("-d")
        .arg(&classes_dir)
        .arg(&source)
        .status()
        .with_context(|| format!("spawn {}", javac.display()))?;
    if !status.success() {
        anyhow::bail!("javac failed with {status}");
    }
    Ok(JdkProbeMetadata {
        jdk_home: jdk_home.to_owned(),
        java_version,
        patch_sha256: patch_sha256()?,
        diagnostic_counters_available: supports_diagnostic_counters(jdk_home)?,
        classes_dir,
    })
}

pub fn inspect_jdk(jdk_home: &Path, probe_home: &Path) -> Result<JdkProbeMetadata> {
    Ok(JdkProbeMetadata {
        jdk_home: jdk_home.to_owned(),
        java_version: java_version(jdk_home)?,
        patch_sha256: patch_sha256()?,
        diagnostic_counters_available: supports_diagnostic_counters(jdk_home)?,
        classes_dir: probe_home.join("classes"),
    })
}

fn java_version(jdk_home: &Path) -> Result<String> {
    let java = jdk_home.join("bin/java");
    let output = Command::new(&java)
        .arg("-version")
        .output()
        .with_context(|| format!("spawn {}", java.display()))?;
    if !output.status.success() {
        anyhow::bail!("java -version failed with {}", output.status);
    }
    let version = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if !is_jdk_25(&version) {
        anyhow::bail!("需要 JDK 25，实际版本为 `{version}`");
    }
    Ok(version)
}

fn is_jdk_25(version_output: &str) -> bool {
    version_output
        .split('"')
        .nth(1)
        .and_then(|release| release.split(['.', '-', '+']).next())
        == Some("25")
}

fn supports_diagnostic_counters(jdk_home: &Path) -> Result<bool> {
    let java = jdk_home.join("bin/java");
    let output = Command::new(&java)
        .args([
            "-XX:+UnlockDiagnosticVMOptions",
            "-XX:+WjsmGcBenchmarkCounters",
            "-version",
        ])
        .output()
        .with_context(|| format!("probe {} diagnostic counters", java.display()))?;
    Ok(output.status.success())
}

fn source_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("java/src/WjsmGcBench.java")
}

fn patch_sha256() -> Result<String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("jdk-probe/0001-zgc-benchmark-counters.patch");
    let bytes = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    let digest = Sha256::digest(bytes);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::is_jdk_25;

    #[test]
    #[ignore = "GC benchmark 契约只通过专用 CLI 入口验证"]
    fn recognizes_only_major_jdk_25() {
        assert!(is_jdk_25("openjdk version \"25\" 2025-09-16"));
        assert!(is_jdk_25("openjdk version \"25.0.1\" 2025-10-21"));
        assert!(!is_jdk_25("openjdk version \"21.0.25\""));
    }
}
