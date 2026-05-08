use anyhow::{bail, Result};
use std::collections::{BTreeSet, HashMap};

use crate::graph::ModuleGraph;
use crate::resolver::ExportEntry;
use wjsm_ir::{ImportBinding, ModuleId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleLinkResult {
    pub import_map: HashMap<ModuleId, Vec<ImportBinding>>,
    pub export_names: HashMap<ModuleId, BTreeSet<String>>,
}

#[derive(Debug, Clone)]
struct CollectedExports {
    names: BTreeSet<String>,
    has_wildcard_reexport: bool,
}

impl CollectedExports {
    fn new() -> Self {
        Self {
            names: BTreeSet::new(),
            has_wildcard_reexport: false,
        }
    }

    fn supports_name(&self, exported: &str) -> bool {
        self.names.contains(exported) || self.has_wildcard_reexport
    }
}

pub fn analyze_module_links(graph: &ModuleGraph) -> Result<ModuleLinkResult> {
    let mut exports_by_module: HashMap<ModuleId, CollectedExports> = HashMap::new();

    for module_id in graph.all_module_ids() {
        let node = graph
            .get_module(module_id)
            .expect("module id from iterator should always exist");
        let mut collected = CollectedExports::new();

        for export in &node.exports {
            match export {
                ExportEntry::Named { exported, .. } => {
                    if !collected.names.insert(exported.clone()) {
                        bail!(
                            "Duplicate export '{}' in module '{}'",
                            exported,
                            node.path.display()
                        );
                    }
                }
                ExportEntry::Declaration { name } => {
                    if !collected.names.insert(name.clone()) {
                        bail!(
                            "Duplicate export '{}' in module '{}'",
                            name,
                            node.path.display()
                        );
                    }
                }
                ExportEntry::Default { .. } => {
                    if !collected.names.insert("default".to_string()) {
                        bail!("Duplicate export 'default' in module '{}'", node.path.display());
                    }
                }
                ExportEntry::All { .. } => {
                    // 存在 wildcard re-export 时，无法在静态阶段穷举导出名；
                    // 对导入名校验按“可能存在”处理，避免误报缺失导出。
                    collected.has_wildcard_reexport = true;
                }
            }
        }

        exports_by_module.insert(module_id, collected);
    }

    let mut import_map: HashMap<ModuleId, Vec<ImportBinding>> = HashMap::new();

    for module_id in graph.all_module_ids() {
        let node = graph
            .get_module(module_id)
            .expect("module id from iterator should always exist");
        let mut local_aliases: HashMap<String, (String, String)> = HashMap::new();
        let mut bindings = Vec::new();

        for (source_module, import) in &node.imports {
            let source_node = graph
                .get_module(*source_module)
                .expect("import source module should exist");
            let source_exports = exports_by_module
                .get(source_module)
                .expect("exports should be collected for every module");

            for (local_name, imported_name) in &import.names {
                if let Some((prev_specifier, prev_imported)) = local_aliases.insert(
                    local_name.clone(),
                    (import.specifier.clone(), imported_name.clone()),
                ) {
                    bail!(
                        "Duplicate import alias '{}' in module '{}': '{}' imports '{}' and '{}' imports '{}'",
                        local_name,
                        node.path.display(),
                        prev_specifier,
                        prev_imported,
                        import.specifier,
                        imported_name
                    );
                }

                if imported_name != "*" && !source_exports.supports_name(imported_name) {
                    bail!(
                        "Missing export '{}' in module '{}' (imported by '{}')",
                        imported_name,
                        source_node.path.display(),
                        node.path.display()
                    );
                }
            }

            bindings.push(ImportBinding {
                source_module: *source_module,
                names: import.names.clone(),
            });
        }

        import_map.insert(module_id, bindings);
    }

    let export_names = exports_by_module
        .into_iter()
        .map(|(module_id, collected)| (module_id, collected.names))
        .collect();

    Ok(ModuleLinkResult {
        import_map,
        export_names,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::ModuleGraph;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn reports_missing_export() {
        let root = create_temp_project("missing_export");
        write_file(
            &root,
            "main.js",
            "import { missing } from './dep.js';\nconsole.log(missing);\n",
        );
        write_file(&root, "dep.js", "export const ok = 1;\n");

        let graph = ModuleGraph::build("./main.js", &root).expect("graph should build");
        let error = analyze_module_links(&graph).expect_err("should report missing export");
        let message = error.to_string();
        assert!(message.contains("Missing export 'missing'"));
    }

    #[test]
    fn reports_duplicate_export() {
        let root = create_temp_project("duplicate_export");
        write_file(
            &root,
            "main.js",
            "import { a } from './dep.js';\nconsole.log(a);\n",
        );
        write_file(
            &root,
            "dep.js",
            "const a = 1;\nexport { a };\nexport { a as a };\n",
        );

        let graph = ModuleGraph::build("./main.js", &root).expect("graph should build");
        let error = analyze_module_links(&graph).expect_err("should report duplicate export");
        let message = error.to_string();
        assert!(message.contains("Duplicate export 'a'"));
    }

    #[test]
    fn reports_duplicate_import_alias() {
        let root = create_temp_project("duplicate_import_alias");
        write_file(
            &root,
            "main.js",
            "import { a as same } from './a.js';\nimport { b as same } from './b.js';\nconsole.log(same);\n",
        );
        write_file(&root, "a.js", "export const a = 1;\n");
        write_file(&root, "b.js", "export const b = 2;\n");

        let graph = ModuleGraph::build("./main.js", &root).expect("graph should build");
        let error = analyze_module_links(&graph).expect_err("should report duplicate alias");
        let message = error.to_string();
        assert!(message.contains("Duplicate import alias 'same'"));
    }

    #[test]
    fn produces_runtime_consumable_link_result() {
        let root = create_temp_project("link_result");
        write_file(
            &root,
            "main.js",
            "import { value as localValue } from './dep.js';\nconsole.log(localValue);\n",
        );
        write_file(&root, "dep.js", "export const value = 42;\n");

        let graph = ModuleGraph::build("./main.js", &root).expect("graph should build");
        let link = analyze_module_links(&graph).expect("linking should succeed");

        let entry_id = graph.entry_id();
        let imports = link
            .import_map
            .get(&entry_id)
            .expect("entry module should have import bindings");
        assert_eq!(imports.len(), 1);
        assert_eq!(imports[0].names, vec![("localValue".to_string(), "value".to_string())]);

        let dep_id = graph
            .all_module_ids()
            .find(|id| *id != entry_id)
            .expect("dep module id should exist");
        let exports = link
            .export_names
            .get(&dep_id)
            .expect("dep exports should be collected");
        assert!(exports.contains("value"));
    }

    fn create_temp_project(case: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be monotonic enough for tests")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "wjsm_module_semantic_{case}_{}_{}",
            std::process::id(),
            nanos
        ));
        std::fs::create_dir_all(&dir).expect("temp project dir should be creatable");
        dir
    }

    fn write_file(root: &Path, relative: &str, content: &str) {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("parent dir should be created");
        }
        std::fs::write(path, content).expect("fixture file should be writable");
    }

    #[test]
    fn wildcard_reexport_allows_any_import() {
        let root = create_temp_project("wildcard_reexport");
        write_file(
            &root,
            "main.js",
            "import { anything } from './reexport.js';\nconsole.log(anything);\n",
        );
        write_file(&root, "base.js", "export const anything = 1;\n");
        write_file(&root, "reexport.js", "export * from './base.js';\n");

        let graph = ModuleGraph::build("./main.js", &root).expect("graph should build");
        let result = analyze_module_links(&graph);
        assert!(result.is_ok(), "wildcard re-export should allow any import name");
    }

    #[test]
    fn namespace_import_skips_missing_check() {
        let root = create_temp_project("namespace_import");
        write_file(
            &root,
            "main.js",
            "import * as ns from './dep.js';\nconsole.log(ns);\n",
        );
        write_file(&root, "dep.js", "export const x = 1;\n");

        let graph = ModuleGraph::build("./main.js", &root).expect("graph should build");
        let result = analyze_module_links(&graph);
        assert!(result.is_ok(), "namespace import should skip missing export check");
    }

    #[test]
    fn duplicate_default_export_detected() {
        let root = create_temp_project("dup_default");
        write_file(
            &root,
            "main.js",
            "import d from './dep.js';\nconsole.log(d);\n",
        );
        write_file(&root, "dep.js", "export default 1;\nexport default 2;\n");

        let graph = ModuleGraph::build("./main.js", &root).expect("graph should build");
        let error = analyze_module_links(&graph).expect_err("should report duplicate default export");
        let message = error.to_string();
        assert!(message.contains("Duplicate export 'default'"));
    }

    #[test]
    fn link_result_contains_correct_export_names() {
        let root = create_temp_project("export_names");
        write_file(
            &root,
            "main.js",
            "import { x, y } from './dep.js';\nconsole.log(x, y);\n",
        );
        write_file(&root, "dep.js", "export const x = 1;\nexport const y = 2;\n");

        let graph = ModuleGraph::build("./main.js", &root).expect("graph should build");
        let link = analyze_module_links(&graph).expect("linking should succeed");

        let dep_id = graph
            .all_module_ids()
            .find(|id| *id != graph.entry_id())
            .expect("dep module id should exist");
        let exports = link.export_names.get(&dep_id).expect("dep exports should exist");
        assert!(exports.contains("x"));
        assert!(exports.contains("y"));
    }

    #[test]
    fn empty_module_links_successfully() {
        let root = create_temp_project("empty_module");
        write_file(&root, "main.js", "const x = 1;\nconsole.log(x);\n");

        let graph = ModuleGraph::build("./main.js", &root).expect("graph should build");
        let result = analyze_module_links(&graph);
        assert!(result.is_ok(), "module with no imports/exports should link successfully");
    }
}
