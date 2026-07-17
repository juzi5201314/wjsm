use anyhow::{Context, Result};
use serde::Serialize;
use std::fs;
use std::path::Path;

pub fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    let payload = serde_json::to_vec_pretty(value).context("serialize benchmark JSON")?;
    fs::write(path, payload).with_context(|| format!("write {}", path.display()))
}

pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let payload = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&payload).with_context(|| format!("decode {}", path.display()))
}
