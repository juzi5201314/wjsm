//! 从已记录源码里收集静态 Worker / fork / cluster.exec 相对路径。

use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::source_store::ModuleSourceStore;

/// 把静态相对入口补进 Recording store；缺文件则失败。
pub fn include_static_runtime_entries(store: &ModuleSourceStore) -> Result<()> {
    let recorded = store.recorded_files();
    let mut extras = Vec::new();
    for (logical, bytes) in &recorded {
        if !is_javascript_logical(logical) {
            continue;
        }
        let Ok(source) = std::str::from_utf8(bytes) else {
            continue;
        };
        for spec in static_relative_specs(source) {
            extras.push((logical.clone(), spec));
        }
    }
    for (referrer, spec) in extras {
        let logical = join_logical(&referrer, &spec)?;
        let path = store.root().join(&logical);
        store.include_file(&path).with_context(|| {
            format!("static runtime entry '{spec}' from '{referrer}' is missing or outside root")
        })?;
    }
    Ok(())
}

fn is_javascript_logical(logical: &str) -> bool {
    matches!(
        Path::new(logical)
            .extension()
            .and_then(|extension| extension.to_str()),
        Some("js" | "mjs" | "cjs" | "ts" | "mts" | "cts" | "jsx" | "tsx")
    )
}

fn static_relative_specs(source: &str) -> Vec<String> {
    let mut specs = Vec::new();
    collect_call_relative(source, "new Worker", &mut specs);
    collect_call_relative(source, "fork", &mut specs);
    collect_cluster_exec(source, &mut specs);
    specs
}

fn collect_call_relative(source: &str, callee: &str, specs: &mut Vec<String>) {
    let mut rest = source;
    while let Some(index) = rest.find(callee) {
        rest = &rest[index + callee.len()..];
        let trimmed = rest.trim_start();
        if !trimmed.starts_with('(') {
            continue;
        }
        if let Some(spec) = first_relative_string(trimmed.trim_start_matches('(').trim_start()) {
            specs.push(spec);
        }
    }
}

fn collect_cluster_exec(source: &str, specs: &mut Vec<String>) {
    for marker in ["setupMaster", "setupPrimary"] {
        let mut rest = source;
        while let Some(index) = rest.find(marker) {
            rest = &rest[index + marker.len()..];
            if let Some(exec) = rest.find("exec") {
                let after = rest[exec + 4..].trim_start();
                if let Some(after) = after.strip_prefix(':')
                    && let Some(spec) = first_relative_string(after.trim_start())
                {
                    specs.push(spec);
                }
                rest = &rest[exec + 4..];
            }
        }
    }
}

fn first_relative_string(source: &str) -> Option<String> {
    let bytes = source.as_bytes();
    let quote = *bytes.first()?;
    if quote != b'\'' && quote != b'"' {
        return None;
    }
    let end = source[1..].find(quote as char)? + 1;
    let spec = &source[1..end];
    (spec.starts_with("./") || spec.starts_with("../")).then(|| spec.to_string())
}

fn join_logical(referrer: &str, spec: &str) -> Result<String> {
    if !spec.starts_with("./") && !spec.starts_with("../") {
        bail!("static runtime entry '{spec}' is not a relative path");
    }
    let parent = Path::new(referrer).parent().unwrap_or(Path::new(""));
    let joined = parent.join(spec);
    let mut normalized = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    bail!("static runtime entry '{spec}' escapes '{referrer}'");
                }
            }
            Component::Normal(name) => normalized.push(name),
            Component::RootDir | Component::Prefix(_) => {}
        }
    }
    normalized
        .to_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("static runtime entry is not UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn scratch() -> PathBuf {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir()
            .join("wjsm-test-cache")
            .join("static-entries")
            .join(format!("{}-{id}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("scratch");
        path
    }

    #[test]
    fn collects_static_worker_and_fork_paths() {
        let root = scratch();
        fs::write(
            root.join("main.js"),
            "new Worker('./worker.js');\nfork('./child.js');\n",
        )
        .expect("main");
        fs::write(root.join("worker.js"), "console.log(1);\n").expect("worker");
        fs::write(root.join("child.js"), "console.log(2);\n").expect("child");
        let store = ModuleSourceStore::recording(&root);
        let _ = store.read_to_string(&root.join("main.js")).expect("read");
        include_static_runtime_entries(&store).expect("include");
        let files = store.recorded_files();
        assert!(files.contains_key("worker.js"));
        assert!(files.contains_key("child.js"));
    }

    #[test]
    fn missing_static_worker_fails() {
        let root = scratch();
        fs::write(root.join("main.js"), "new Worker('./missing.js');\n").expect("main");
        let store = ModuleSourceStore::recording(&root);
        let _ = store.read_to_string(&root.join("main.js")).expect("read");
        assert!(include_static_runtime_entries(&store).is_err());
    }
}
