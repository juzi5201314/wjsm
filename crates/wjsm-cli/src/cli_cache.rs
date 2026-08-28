use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};

use crate::cli_args::CacheCommand;

struct CacheEntry {
    path: PathBuf,
    bytes: u64,
    modified: u128,
}

pub(crate) fn run(command: CacheCommand, directory: Option<&Path>, quiet: bool) -> Result<()> {
    let directory = cache_directory(directory)?;
    match command {
        CacheCommand::Stats => print_stats(&directory),
        CacheCommand::Clear => clear(&directory, quiet),
        CacheCommand::Prune { max_bytes } => prune(&directory, max_bytes, quiet),
    }
}

fn cache_directory(explicit: Option<&Path>) -> Result<PathBuf> {
    explicit
        .map(Path::to_path_buf)
        .or_else(wjsm_module::resolve_cache_dir)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "disk cache is disabled (WJSM_CACHE_DIR is empty and no user cache directory \
                 is available); pass --dir or set WJSM_CACHE_DIR"
            )
        })
}

fn print_stats(directory: &Path) -> Result<()> {
    let entries = entries(directory)?;
    let bytes = entries.iter().map(|entry| entry.bytes).sum::<u64>();
    println!("Cache directory: {}", directory.display());
    println!("Entries: {}", entries.len());
    println!("Bytes: {bytes}");
    Ok(())
}

fn clear(directory: &Path, quiet: bool) -> Result<()> {
    let entries = entries(directory)?;
    let bytes = entries.iter().map(|entry| entry.bytes).sum::<u64>();
    for entry in &entries {
        fs::remove_file(&entry.path)
            .with_context(|| format!("failed to remove '{}'", entry.path.display()))?;
    }
    if !quiet {
        println!("Cleared {} cache entries ({bytes} bytes)", entries.len());
    }
    Ok(())
}

fn prune(directory: &Path, max_bytes: u64, quiet: bool) -> Result<()> {
    let mut entries = entries(directory)?;
    entries.sort_by_key(|entry| (entry.modified, entry.path.clone()));
    let mut bytes = entries.iter().map(|entry| entry.bytes).sum::<u64>();
    let mut removed_entries = 0_u64;
    let mut removed_bytes = 0_u64;
    for entry in entries {
        if bytes <= max_bytes {
            break;
        }
        fs::remove_file(&entry.path)
            .with_context(|| format!("failed to remove '{}'", entry.path.display()))?;
        bytes = bytes.saturating_sub(entry.bytes);
        removed_entries += 1;
        removed_bytes += entry.bytes;
    }
    if bytes > max_bytes {
        bail!("native cache remains above the requested byte limit");
    }
    if !quiet {
        println!(
            "Pruned {removed_entries} cache entries ({removed_bytes} bytes); retained {bytes} bytes"
        );
    }
    Ok(())
}

fn entries(directory: &Path) -> Result<Vec<CacheEntry>> {
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut entries = Vec::new();
    collect_entries(directory, &mut entries)?;
    Ok(entries)
}

/// 缓存子目录及各自的条目扩展名：`builtin_ir/*.bin`（wjsm-module 的 lower
/// 产物缓存）、`artifact/*.{wjsm,dep}`（输入寻址 artifact 缓存）。顶层只认
/// `*.wnat`。与 backend 的自动 LRU 淘汰范围保持一致。
fn cache_subdir_extensions(name: &str) -> Option<&'static [&'static str]> {
    match name {
        "builtin_ir" => Some(&["bin"]),
        "artifact" => Some(&["wjsm", "dep"]),
        _ => None,
    }
}

/// 递归收集缓存条目：顶层 `*.wnat` + 已知缓存子目录内的对应扩展名。
fn collect_entries(directory: &Path, out: &mut Vec<CacheEntry>) -> Result<()> {
    for entry in fs::read_dir(directory)
        .with_context(|| format!("failed to read native cache '{}'", directory.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(cache_subdir_extensions)
                .is_some()
            {
                collect_entries(&path, out)?;
            }
            continue;
        }
        let extension = path.extension().and_then(|extension| extension.to_str());
        let subdir_extensions = path
            .parent()
            .and_then(|parent| parent.file_name())
            .and_then(|name| name.to_str())
            .and_then(cache_subdir_extensions);
        let is_cache_file = match (extension, subdir_extensions) {
            (Some(extension), Some(allowed)) => allowed.contains(&extension),
            (Some(extension), None) => extension == "wnat",
            (None, _) => false,
        };
        if !is_cache_file {
            continue;
        }
        let metadata = entry.metadata()?;
        out.push(CacheEntry {
            path,
            bytes: metadata.len(),
            modified: modified_key(metadata.modified().unwrap_or(UNIX_EPOCH)),
        });
    }
    Ok(())
}

fn modified_key(time: SystemTime) -> u128 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestCache {
        path: PathBuf,
    }

    impl TestCache {
        fn new() -> Self {
            let path = std::env::temp_dir()
                .join("wjsm-test-cache")
                .join("cli")
                .join(format!(
                    "cache-{}-{}",
                    std::process::id(),
                    modified_key(SystemTime::now()),
                ));
            fs::create_dir_all(&path).expect("cache directory should be created");
            Self { path }
        }
    }

    impl Drop for TestCache {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn prune_and_clear_only_manage_native_cache_entries() {
        let cache = TestCache::new();
        fs::write(cache.path.join("a.wnat"), [0_u8; 4]).expect("first entry should be written");
        fs::write(cache.path.join("b.wnat"), [0_u8; 6]).expect("second entry should be written");
        let builtin_ir = cache.path.join("builtin_ir");
        fs::create_dir_all(&builtin_ir).expect("builtin_ir dir should be created");
        fs::write(builtin_ir.join("c.bin"), [0_u8; 8]).expect("builtin ir entry should be written");
        let artifact = cache.path.join("artifact");
        fs::create_dir_all(&artifact).expect("artifact dir should be created");
        fs::write(artifact.join("d.wjsm"), [0_u8; 10]).expect("artifact entry should be written");
        fs::write(artifact.join("d.dep"), [0_u8; 2]).expect("deps entry should be written");
        fs::write(cache.path.join("keep.txt"), b"keep").expect("unrelated file should be written");

        prune(&cache.path, 6, true).expect("cache should prune to byte limit");
        let retained = entries(&cache.path).expect("entries should remain readable");
        assert!(retained.iter().map(|entry| entry.bytes).sum::<u64>() <= 6);
        assert!(cache.path.join("keep.txt").exists());

        clear(&cache.path, true).expect("cache entries should clear");
        assert!(
            entries(&cache.path)
                .expect("cache should be readable")
                .is_empty()
        );
        assert!(cache.path.join("keep.txt").exists());
    }

    #[test]
    fn stats_and_entries_include_builtin_ir_and_artifact() {
        let cache = TestCache::new();
        fs::write(cache.path.join("a.wnat"), [0_u8; 4]).expect("wnat entry should be written");
        let builtin_ir = cache.path.join("builtin_ir");
        fs::create_dir_all(&builtin_ir).expect("builtin_ir dir should be created");
        fs::write(builtin_ir.join("c.bin"), [0_u8; 8]).expect("builtin ir entry should be written");
        let artifact = cache.path.join("artifact");
        fs::create_dir_all(&artifact).expect("artifact dir should be created");
        fs::write(artifact.join("d.wjsm"), [0_u8; 16]).expect("artifact entry should be written");
        fs::write(artifact.join("d.dep"), [0_u8; 2]).expect("deps entry should be written");
        fs::write(artifact.join("skip.txt"), b"x").expect("unrelated file should be written");

        let all = entries(&cache.path).expect("entries should include cache subdirectories");
        assert_eq!(all.len(), 4, "wnat + builtin_ir/*.bin + artifact/*.{{wjsm,dep}}");
        assert_eq!(all.iter().map(|entry| entry.bytes).sum::<u64>(), 30);
    }
}
