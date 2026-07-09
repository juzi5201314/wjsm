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
        canonical: "util",
        source: include_str!("../builtin_js/node_util.js"),
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

pub(crate) fn virtual_path(canonical: &str) -> PathBuf {
    PathBuf::from(format!("/__wjsm_builtin__/node/{canonical}.mjs"))
}

pub(crate) fn is_builtin_virtual_path(path: &Path) -> bool {
    path.starts_with("/__wjsm_builtin__/node")
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
            "util",
            "events",
            "assert",
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
        ] {
            assert!(
                seen.insert(virtual_path(canonical)),
                "virtual path for {canonical} should be unique"
            );
        }
    }
}
