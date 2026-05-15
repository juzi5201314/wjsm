use super::*;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

fn create_temp_project(case: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be monotonic enough for tests")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "wjsm_module_resolver_{case}_{}_{}",
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
fn resolve_path_rejects_non_relative_specifier() {
    let root = create_temp_project("non_relative");
    let parent = root.join("main.js");
    let result = ModuleResolver::resolve_path("lodash", &parent);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("not supported"));
}

#[test]
fn resolve_path_finds_js_extension() {
    let root = create_temp_project("js_ext");
    write_file(&root, "dep.js", "export const x = 1;\n");
    let parent = root.join("main.js");
    let result = ModuleResolver::resolve_path("./dep", &parent);
    assert!(result.is_ok());
    let path = result.unwrap();
    assert!(path.to_string_lossy().ends_with("dep.js"));
}

#[test]
fn resolve_path_finds_file_with_extension() {
    let root = create_temp_project("with_ext");
    write_file(&root, "dep.js", "export const x = 1;\n");
    let parent = root.join("main.js");
    let result = ModuleResolver::resolve_path("./dep.js", &parent);
    assert!(result.is_ok());
}

#[test]
fn resolve_path_fails_when_module_not_found() {
    let root = create_temp_project("not_found");
    let parent = root.join("main.js");
    let result = ModuleResolver::resolve_path("./nonexistent", &parent);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("Cannot find module"));
}

#[test]
fn resolve_path_resolves_parent_directory() {
    let root = create_temp_project("parent_dir");
    write_file(&root, "sibling.js", "export const x = 1;\n");
    let sub_dir = root.join("sub");
    std::fs::create_dir_all(&sub_dir).expect("sub dir should be created");
    let parent = sub_dir.join("main.js");
    let result = ModuleResolver::resolve_path("../sibling", &parent);
    assert!(result.is_ok());
}

#[test]
fn resolve_rejects_path_outside_root() {
    let root = create_temp_project("outside_root");
    let outside_name = format!("{}_sibling", root.file_name().unwrap().to_string_lossy());
    let outside = root.parent().unwrap().join(&outside_name);
    std::fs::create_dir_all(&outside).expect("outside dir should be created");
    write_file(&outside, "dep.js", "export const x = 1;\n");
    let parent = root.join("main.js");
    let mut resolver = ModuleResolver::new(&root);

    let result = resolver.resolve(&format!("../{outside_name}/dep.js"), &parent);

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("outside root"));
}

#[test]
fn resolve_returns_cached_id_on_second_call() {
    let root = create_temp_project("cache_test");
    write_file(&root, "dep.js", "export const x = 1;\n");
    let parent = root.join("main.js");
    let mut resolver = ModuleResolver::new(&root);
    let id1 = resolver
        .resolve("./dep.js", &parent)
        .expect("first resolve should succeed");
    let id2 = resolver
        .resolve("./dep.js", &parent)
        .expect("second resolve should succeed");
    assert_eq!(id1, id2, "cached resolve should return same ID");
}

#[test]
fn resolve_detects_cjs_module() {
    let root = create_temp_project("cjs_detect");
    write_file(&root, "cjs.js", "module.exports.x = 1;\n");
    let parent = root.join("main.js");
    let mut resolver = ModuleResolver::new(&root);
    let id = resolver
        .resolve("./cjs.js", &parent)
        .expect("resolve should succeed");
    let module = resolver.get_module(id).expect("module should exist");
    assert!(module.is_cjs, "module should be detected as CJS");
}

#[test]
fn resolve_parses_esm_module() {
    let root = create_temp_project("esm_detect");
    write_file(&root, "esm.js", "export const x = 1;\n");
    let parent = root.join("main.js");
    let mut resolver = ModuleResolver::new(&root);
    let id = resolver
        .resolve("./esm.js", &parent)
        .expect("resolve should succeed");
    let module = resolver.get_module(id).expect("module should exist");
    assert!(!module.is_cjs, "module should not be detected as CJS");
}

#[test]
fn get_module_returns_some_for_existing() {
    let root = create_temp_project("get_mod_some");
    write_file(&root, "dep.js", "export const x = 1;\n");
    let parent = root.join("main.js");
    let mut resolver = ModuleResolver::new(&root);
    let id = resolver
        .resolve("./dep.js", &parent)
        .expect("resolve should succeed");
    assert!(resolver.get_module(id).is_some());
}

#[test]
fn get_module_returns_none_for_missing() {
    let root = create_temp_project("get_mod_none");
    let resolver = ModuleResolver::new(&root);
    assert!(resolver.get_module(ModuleId(999)).is_none());
}

#[test]
fn all_modules_iterates_all() {
    let root = create_temp_project("all_mods");
    write_file(&root, "a.js", "export const a = 1;\n");
    write_file(&root, "b.js", "export const b = 2;\n");
    let parent = root.join("main.js");
    let mut resolver = ModuleResolver::new(&root);
    resolver
        .resolve("./a.js", &parent)
        .expect("resolve a should succeed");
    resolver
        .resolve("./b.js", &parent)
        .expect("resolve b should succeed");
    let count = resolver.all_modules().count();
    assert_eq!(count, 2);
}

#[test]
fn get_id_by_path_returns_some_for_visited() {
    let root = create_temp_project("id_by_path_some");
    write_file(&root, "dep.js", "export const x = 1;\n");
    let parent = root.join("main.js");
    let mut resolver = ModuleResolver::new(&root);
    let id = resolver
        .resolve("./dep.js", &parent)
        .expect("resolve should succeed");
    let dep_path = root
        .join("dep.js")
        .canonicalize()
        .expect("canonicalize should work");
    assert_eq!(resolver.get_id_by_path(&dep_path), Some(id));
}

#[test]
fn get_id_by_path_returns_none_for_unknown() {
    let root = create_temp_project("id_by_path_none");
    let resolver = ModuleResolver::new(&root);
    let unknown_path = root.join("nonexistent.js");
    assert!(resolver.get_id_by_path(&unknown_path).is_none());
}

#[test]
fn ensure_default_export_adds_when_no_default() {
    let root = create_temp_project("ensure_default_add");
    write_file(&root, "dep.js", "export const x = 1;\n");
    let parent = root.join("main.js");
    let mut resolver = ModuleResolver::new(&root);
    let id = resolver
        .resolve("./dep.js", &parent)
        .expect("resolve should succeed");
    let before_count = resolver.get_module(id).unwrap().exports.len();
    resolver
        .ensure_default_export_for(id)
        .expect("ensure default export should succeed");
    let after_count = resolver.get_module(id).unwrap().exports.len();
    assert!(
        after_count > before_count,
        "should have added a default export"
    );
}

#[test]
fn ensure_default_export_skips_when_has_default() {
    let root = create_temp_project("ensure_default_skip_has");
    write_file(&root, "dep.js", "export default 42;\n");
    let parent = root.join("main.js");
    let mut resolver = ModuleResolver::new(&root);
    let id = resolver
        .resolve("./dep.js", &parent)
        .expect("resolve should succeed");
    let before_count = resolver.get_module(id).unwrap().exports.len();
    resolver
        .ensure_default_export_for(id)
        .expect("ensure default export should succeed");
    let after_count = resolver.get_module(id).unwrap().exports.len();
    assert_eq!(
        after_count, before_count,
        "should not add default export when one exists"
    );
}

#[test]
fn ensure_default_export_skips_when_no_exports() {
    let root = create_temp_project("ensure_default_skip_empty");
    write_file(&root, "dep.js", "const x = 1;\nconsole.log(x);\n");
    let parent = root.join("main.js");
    let mut resolver = ModuleResolver::new(&root);
    let id = resolver
        .resolve("./dep.js", &parent)
        .expect("resolve should succeed");
    let before_count = resolver.get_module(id).unwrap().exports.len();
    assert_eq!(before_count, 0, "module should have no exports");
    resolver
        .ensure_default_export_for(id)
        .expect("ensure default export should succeed");
    let after_count = resolver.get_module(id).unwrap().exports.len();
    assert_eq!(
        after_count, 0,
        "should not add default export when no exports exist"
    );
}

#[test]
fn extract_imports_handles_named_import() {
    let root = create_temp_project("import_named");
    write_file(&root, "dep.js", "export const x = 1;\n");
    write_file(
        &root,
        "main.js",
        "import { x } from './dep.js';\nconsole.log(x);\n",
    );
    let parent = root.join("main.js");
    let mut resolver = ModuleResolver::new(&root);
    let id = resolver
        .resolve("./main.js", &parent)
        .expect("resolve should succeed");
    let module = resolver.get_module(id).expect("module should exist");
    assert_eq!(module.imports.len(), 1);
    assert_eq!(
        module.imports[0].names,
        vec![("x".to_string(), "x".to_string())]
    );
}

#[test]
fn extract_imports_handles_default_import() {
    let root = create_temp_project("import_default");
    write_file(&root, "dep.js", "export default 42;\n");
    write_file(
        &root,
        "main.js",
        "import answer from './dep.js';\nconsole.log(answer);\n",
    );
    let parent = root.join("main.js");
    let mut resolver = ModuleResolver::new(&root);
    let id = resolver
        .resolve("./main.js", &parent)
        .expect("resolve should succeed");
    let module = resolver.get_module(id).expect("module should exist");
    assert_eq!(module.imports.len(), 1);
    assert_eq!(
        module.imports[0].names,
        vec![("answer".to_string(), "default".to_string())]
    );
}

#[test]
fn extract_imports_handles_namespace_import() {
    let root = create_temp_project("import_ns");
    write_file(&root, "dep.js", "export const x = 1;\n");
    write_file(
        &root,
        "main.js",
        "import * as ns from './dep.js';\nconsole.log(ns);\n",
    );
    let parent = root.join("main.js");
    let mut resolver = ModuleResolver::new(&root);
    let id = resolver
        .resolve("./main.js", &parent)
        .expect("resolve should succeed");
    let module = resolver.get_module(id).expect("module should exist");
    assert_eq!(module.imports.len(), 1);
    assert_eq!(
        module.imports[0].names,
        vec![("ns".to_string(), "*".to_string())]
    );
}

#[test]
fn extract_imports_handles_aliased_named_import() {
    let root = create_temp_project("import_alias");
    write_file(&root, "dep.js", "export const x = 1;\n");
    write_file(
        &root,
        "main.js",
        "import { x as y } from './dep.js';\nconsole.log(y);\n",
    );
    let parent = root.join("main.js");
    let mut resolver = ModuleResolver::new(&root);
    let id = resolver
        .resolve("./main.js", &parent)
        .expect("resolve should succeed");
    let module = resolver.get_module(id).expect("module should exist");
    assert_eq!(module.imports.len(), 1);
    assert_eq!(
        module.imports[0].names,
        vec![("y".to_string(), "x".to_string())]
    );
}

#[test]
fn extract_exports_handles_named_export() {
    let root = create_temp_project("export_named");
    write_file(&root, "dep.js", "const x = 1;\nexport { x };\n");
    let parent = root.join("main.js");
    let mut resolver = ModuleResolver::new(&root);
    let id = resolver
        .resolve("./dep.js", &parent)
        .expect("resolve should succeed");
    let module = resolver.get_module(id).expect("module should exist");
    assert!(
        module
            .exports
            .iter()
            .any(|e| matches!(e, ExportEntry::Named { exported, .. } if exported == "x"))
    );
}

#[test]
fn extract_exports_handles_default_expr_export() {
    let root = create_temp_project("export_default_expr");
    write_file(&root, "dep.js", "export default 99;\n");
    let parent = root.join("main.js");
    let mut resolver = ModuleResolver::new(&root);
    let id = resolver
        .resolve("./dep.js", &parent)
        .expect("resolve should succeed");
    let module = resolver.get_module(id).expect("module should exist");
    assert!(
        module
            .exports
            .iter()
            .any(|e| matches!(e, ExportEntry::Default { .. }))
    );
}

#[test]
fn extract_exports_handles_default_fn_export() {
    let root = create_temp_project("export_default_fn");
    write_file(&root, "dep.js", "export default function hello() {}\n");
    let parent = root.join("main.js");
    let mut resolver = ModuleResolver::new(&root);
    let id = resolver
        .resolve("./dep.js", &parent)
        .expect("resolve should succeed");
    let module = resolver.get_module(id).expect("module should exist");
    assert!(
        module
            .exports
            .iter()
            .any(|e| matches!(e, ExportEntry::Default { local, .. } if local == "hello"))
    );
}

#[test]
fn extract_exports_handles_default_class_export() {
    let root = create_temp_project("export_default_class");
    write_file(&root, "dep.js", "export default class MyClass {}\n");
    let parent = root.join("main.js");
    let mut resolver = ModuleResolver::new(&root);
    let id = resolver
        .resolve("./dep.js", &parent)
        .expect("resolve should succeed");
    let module = resolver.get_module(id).expect("module should exist");
    assert!(
        module
            .exports
            .iter()
            .any(|e| matches!(e, ExportEntry::Default { local, .. } if local == "MyClass"))
    );
}

#[test]
fn extract_exports_handles_declaration_export() {
    let root = create_temp_project("export_decl");
    write_file(
        &root,
        "dep.js",
        "export const x = 1;\nexport function foo() {}\nexport class Bar {}\n",
    );
    let parent = root.join("main.js");
    let mut resolver = ModuleResolver::new(&root);
    let id = resolver
        .resolve("./dep.js", &parent)
        .expect("resolve should succeed");
    let module = resolver.get_module(id).expect("module should exist");
    let names: Vec<&str> = module
        .exports
        .iter()
        .filter_map(|e| {
            if let ExportEntry::Declaration { name } = e {
                Some(name.as_str())
            } else {
                None
            }
        })
        .collect();
    assert!(names.contains(&"x"));
    assert!(names.contains(&"foo"));
    assert!(names.contains(&"Bar"));
}

#[test]
fn extract_exports_handles_export_all() {
    let root = create_temp_project("export_all");
    write_file(&root, "base.js", "export const x = 1;\n");
    write_file(&root, "dep.js", "export * from './base.js';\n");
    let parent = root.join("main.js");
    let mut resolver = ModuleResolver::new(&root);
    let id = resolver
        .resolve("./dep.js", &parent)
        .expect("resolve should succeed");
    let module = resolver.get_module(id).expect("module should exist");
    assert!(
        module
            .exports
            .iter()
            .any(|e| matches!(e, ExportEntry::All { source } if source == "./base.js"))
    );
}

#[test]
fn extract_exports_handles_re_export_with_source() {
    let root = create_temp_project("re_export");
    write_file(&root, "base.js", "export const x = 1;\n");
    write_file(&root, "dep.js", "export { x } from './base.js';\n");
    let parent = root.join("main.js");
    let mut resolver = ModuleResolver::new(&root);
    let id = resolver
        .resolve("./dep.js", &parent)
        .expect("resolve should succeed");
    let module = resolver.get_module(id).expect("module should exist");
    assert!(module.exports.iter().any(|e| matches!(
        e,
        ExportEntry::NamedReExport { local, exported, source }
            if local == "x" && exported == "x" && source == "./base.js"
    )));
}

#[test]
fn extract_exports_handles_default_anonymous_fn() {
    let root = create_temp_project("export_default_anon_fn");
    write_file(&root, "dep.js", "export default function() {}\n");
    let parent = root.join("main.js");
    let mut resolver = ModuleResolver::new(&root);
    let id = resolver
        .resolve("./dep.js", &parent)
        .expect("resolve should succeed");
    let module = resolver.get_module(id).expect("module should exist");
    assert!(module.exports.iter().any(
        |e| matches!(e, ExportEntry::Default { local, .. } if local == "_default_export")
    ));
}

#[test]
fn extract_exports_handles_default_anonymous_class() {
    let root = create_temp_project("export_default_anon_class");
    write_file(&root, "dep.js", "export default class {}\n");
    let parent = root.join("main.js");
    let mut resolver = ModuleResolver::new(&root);
    let id = resolver
        .resolve("./dep.js", &parent)
        .expect("resolve should succeed");
    let module = resolver.get_module(id).expect("module should exist");
    assert!(module.exports.iter().any(
        |e| matches!(e, ExportEntry::Default { local, .. } if local == "_default_export")
    ));
}

#[test]
fn extract_exports_handles_multiple_var_declarations() {
    let root = create_temp_project("export_multi_var");
    write_file(&root, "dep.js", "export const a = 1, b = 2;\n");
    let parent = root.join("main.js");
    let mut resolver = ModuleResolver::new(&root);
    let id = resolver
        .resolve("./dep.js", &parent)
        .expect("resolve should succeed");
    let module = resolver.get_module(id).expect("module should exist");
    let names: Vec<&str> = module
        .exports
        .iter()
        .filter_map(|e| {
            if let ExportEntry::Declaration { name } = e {
                Some(name.as_str())
            } else {
                None
            }
        })
        .collect();
    assert!(names.contains(&"a"));
    assert!(names.contains(&"b"));
}
