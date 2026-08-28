// Node.js 内置模块元数据 owner（含当前核心模块封装源）。

use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct BuiltinModule {
    pub canonical: &'static str,
    pub source: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BuiltinLookup {
    Found(&'static BuiltinModule),
    UnknownNodeBuiltin(String),
    NotBuiltin,
}

const BUILTIN_MODULES: &[BuiltinModule] = &[
    BuiltinModule {
        canonical: "path",
        source: include_str!("../builtin_js/node_path.js"),
    },
    BuiltinModule {
        canonical: "path/posix",
        source: include_str!("../builtin_js/node_path_posix.js"),
    },
    BuiltinModule {
        canonical: "path/win32",
        source: include_str!("../builtin_js/node_path_win32.js"),
    },
    BuiltinModule {
        canonical: "util",
        source: include_str!("../builtin_js/node_util.js"),
    },
    BuiltinModule {
        canonical: "util/types",
        source: include_str!("../builtin_js/node_util_types.js"),
    },
    BuiltinModule {
        canonical: "events",
        source: include_str!("../builtin_js/node_events.js"),
    },
    BuiltinModule {
        canonical: "assert",
        source: include_str!("../builtin_js/node_assert.js"),
    },
    BuiltinModule {
        canonical: "assert/strict",
        source: include_str!("../builtin_js/node_assert_strict.js"),
    },
    BuiltinModule {
        canonical: "buffer",
        source: include_str!("../builtin_js/node_buffer.js"),
    },
    BuiltinModule {
        canonical: "url",
        source: include_str!("../builtin_js/node_url.js"),
    },
    BuiltinModule {
        canonical: "querystring",
        source: include_str!("../builtin_js/node_querystring.js"),
    },
    BuiltinModule {
        canonical: "os",
        source: include_str!("../builtin_js/node_os.js"),
    },
    BuiltinModule {
        canonical: "fs",
        source: include_str!("../builtin_js/node_fs.js"),
    },
    BuiltinModule {
        canonical: "fs/promises",
        source: include_str!("../builtin_js/node_fs_promises.js"),
    },
    BuiltinModule {
        canonical: "crypto",
        source: include_str!("../builtin_js/node_crypto.js"),
    },
    BuiltinModule {
        canonical: "stream",
        source: include_str!("../builtin_js/node_stream.js"),
    },
    BuiltinModule {
        canonical: "http",
        source: include_str!("../builtin_js/node_http.js"),
    },
    BuiltinModule {
        canonical: "net",
        source: include_str!("../builtin_js/node_net.js"),
    },
    BuiltinModule {
        canonical: "https",
        source: include_str!("../builtin_js/node_https.js"),
    },
    BuiltinModule {
        canonical: "zlib",
        source: include_str!("../builtin_js/node_zlib.js"),
    },
    BuiltinModule {
        canonical: "child_process",
        source: include_str!("../builtin_js/node_child_process.js"),
    },
    BuiltinModule {
        canonical: "dgram",
        source: include_str!("../builtin_js/node_dgram.js"),
    },
    BuiltinModule {
        canonical: "tls",
        source: include_str!("../builtin_js/node_tls.js"),
    },
    BuiltinModule {
        canonical: "worker_threads",
        source: include_str!("../builtin_js/node_worker_threads.js"),
    },
    BuiltinModule {
        canonical: "inspector",
        source: include_str!("../builtin_js/node_inspector.js"),
    },
    BuiltinModule {
        canonical: "cluster",
        source: include_str!("../builtin_js/node_cluster.js"),
    },
    BuiltinModule {
        canonical: "vm",
        source: include_str!("../builtin_js/node_vm.js"),
    },
    BuiltinModule {
        canonical: "async_hooks",
        source: include_str!("../builtin_js/node_async_hooks.js"),
    },
    BuiltinModule {
        canonical: "perf_hooks",
        source: concat!(
            include_str!("../builtin_js/node_perf_hooks/internal.js"),
            include_str!("../builtin_js/node_perf_hooks/entries.js"),
            include_str!("../builtin_js/node_perf_hooks/observer.js"),
            include_str!("../builtin_js/node_perf_hooks/histogram.js"),
            include_str!("../builtin_js/node_perf_hooks/resource.js"),
            include_str!("../builtin_js/node_perf_hooks/fetch.js"),
            include_str!("../builtin_js/node_perf_hooks/performance.js"),
            include_str!("../builtin_js/node_perf_hooks/exports.js"),
        ),
    },
    BuiltinModule {
        canonical: "string_decoder",
        source: include_str!("../builtin_js/node_string_decoder.js"),
    },
    BuiltinModule {
        canonical: "timers",
        source: include_str!("../builtin_js/node_timers.js"),
    },
    BuiltinModule {
        canonical: "timers/promises",
        source: include_str!("../builtin_js/node_timers_promises.js"),
    },
    BuiltinModule {
        canonical: "punycode",
        source: include_str!("../builtin_js/node_punycode.js"),
    },
    BuiltinModule {
        canonical: "process",
        source: include_str!("../builtin_js/node_process.js"),
    },
    BuiltinModule {
        canonical: "console",
        source: include_str!("../builtin_js/node_console.js"),
    },
    BuiltinModule {
        canonical: "constants",
        source: include_str!("../builtin_js/node_constants.js"),
    },
    BuiltinModule {
        canonical: "diagnostics_channel",
        source: include_str!("../builtin_js/node_diagnostics_channel.js"),
    },
];

pub(crate) fn lookup(specifier: &str) -> BuiltinLookup {
    let canonical = specifier.strip_prefix("node:").unwrap_or(specifier);
    if let Some(module) = BUILTIN_MODULES
        .iter()
        .find(|module| module.canonical == canonical)
    {
        return BuiltinLookup::Found(module);
    }
    if specifier.starts_with("node:") {
        return BuiltinLookup::UnknownNodeBuiltin(canonical.to_string());
    }
    BuiltinLookup::NotBuiltin
}

/// 按 canonical 名取 builtin 模块源码（不存在时返回 `None`）。
///
/// canonical 可带或不带 `node:` 前缀，与 [`lookup`] 的归一化规则一致。
/// 供 builtin 段缓存键计算与模块布局记录使用。
pub(crate) fn source_for_canonical(canonical: &str) -> Option<&'static str> {
    match lookup(canonical) {
        BuiltinLookup::Found(module) => Some(module.source),
        BuiltinLookup::UnknownNodeBuiltin(_) | BuiltinLookup::NotBuiltin => None,
    }
}

pub(crate) fn virtual_path(canonical: &str) -> PathBuf {
    PathBuf::from(format!("/__wjsm_builtin__/node/{canonical}.mjs"))
}

pub(crate) fn is_builtin_virtual_path(path: &Path) -> bool {
    path.starts_with("/__wjsm_builtin__/node")
}

/// 从 builtin 虚拟路径 `/__wjsm_builtin__/node/<canonical>.mjs` 提取 canonical。
pub(crate) fn canonical_from_virtual_path(path: &Path) -> Option<&str> {
    let text = path.to_str()?;
    const PREFIX: &str = "/__wjsm_builtin__/node/";
    text.strip_prefix(PREFIX)?.strip_suffix(".mjs")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn builtin_lookup_accepts_bare_and_node_prefix() {
        let bare = match lookup("path") {
            BuiltinLookup::Found(module) => module,
            other => panic!("bare lookup should find path, got {other:?}"),
        };
        let prefixed = match lookup("node:path") {
            BuiltinLookup::Found(module) => module,
            other => panic!("node: lookup should find path, got {other:?}"),
        };

        assert_eq!(bare.canonical, "path");
        assert!(std::ptr::eq(bare, prefixed));
    }

    #[test]
    fn builtin_lookup_rejects_unknown_node_prefix() {
        let err = match lookup("node:not_real") {
            BuiltinLookup::UnknownNodeBuiltin(name) => {
                format!("Unknown built-in module 'node:{name}'")
            }
            other => panic!("unknown node: lookup should be rejected, got {other:?}"),
        };

        assert!(err.contains("Unknown built-in module 'node:not_real'"));
    }

    #[test]
    fn builtin_virtual_paths_are_stable() {
        assert_eq!(
            virtual_path("path"),
            PathBuf::from("/__wjsm_builtin__/node/path.mjs")
        );

        assert_eq!(
            virtual_path("fs/promises"),
            PathBuf::from("/__wjsm_builtin__/node/fs/promises.mjs")
        );

        let mut seen = HashSet::new();
        for canonical in [
            "path",
            "path/posix",
            "path/win32",
            "util",
            "util/types",
            "events",
            "assert",
            "assert/strict",
            "url",
            "querystring",
            "os",
            "fs",
            "fs/promises",
            "crypto",
            "stream",
            "http",
            "net",
            "https",
            "zlib",
            "child_process",
            "dgram",
            "tls",
            "worker_threads",
            "inspector",
            "cluster",
            "perf_hooks",
            "string_decoder",
            "timers",
            "timers/promises",
            "punycode",
            "process",
            "console",
            "constants",
            "diagnostics_channel",
        ] {
            assert!(
                seen.insert(virtual_path(canonical)),
                "virtual path for {canonical} should be unique"
            );
        }
    }

    #[test]
    fn builtin_lookup_resolves_subpath_modules_with_and_without_prefix() {
        for canonical in ["timers/promises", "assert/strict", "util/types", "path/posix"] {
            let bare = match lookup(canonical) {
                BuiltinLookup::Found(module) => module,
                other => panic!("bare lookup should find {canonical}, got {other:?}"),
            };
            let prefixed = match lookup(&format!("node:{canonical}")) {
                BuiltinLookup::Found(module) => module,
                other => panic!("node: lookup should find {canonical}, got {other:?}"),
            };
            assert_eq!(bare.canonical, canonical);
            assert!(
                std::ptr::eq(bare, prefixed),
                "{canonical} should resolve to the same wrapper with or without node: prefix"
            );
        }
    }

    #[test]
    fn every_registered_builtin_has_nonempty_source() {
        for module in BUILTIN_MODULES {
            assert!(
                !module.source.trim().is_empty(),
                "builtin {} should have wrapper source",
                module.canonical
            );
        }
    }
}
