use super::*;
use crate::{
    RuntimeModuleFormat, RuntimeModuleKey, RuntimeResolveKind, RuntimeResolvePaths,
    resolve_runtime_paths, resolve_runtime_specifier,
};
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEST_PROJECT: AtomicUsize = AtomicUsize::new(0);

struct TestProject {
    path: PathBuf,
}

impl TestProject {
    fn new(case: &str) -> Self {
        let id = NEXT_TEST_PROJECT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "wjsm_module_resolver_{case}_{}_{id}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp project dir should be creatable");
        Self { path: dir }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Deref for TestProject {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        self.path()
    }
}

impl Drop for TestProject {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn create_temp_project(case: &str) -> TestProject {
    TestProject::new(case)
}

fn write_file(root: &Path, relative: &str, content: &str) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("parent dir should be created");
    }
    std::fs::write(path, content).expect("fixture file should be writable");
}

fn write_type_module_package(root: &Path) {
    write_file(root, "package.json", r#"{"type":"module"}"#);
}

fn resolve_loaded_path(root: &Path, parent: &Path, specifier: &str) -> PathBuf {
    let mut resolver = ModuleResolver::new(root);
    let id = resolver
        .resolve(specifier, parent)
        .expect("specifier should resolve");
    resolver
        .get_module(id)
        .expect("module should be loaded")
        .path
        .clone()
}

fn resolve_error(root: &Path, parent: &Path, specifier: &str) -> String {
    let mut resolver = ModuleResolver::new(root);
    let error = resolver.resolve(specifier, parent).unwrap_err();
    format!("{error:#}")
}

#[test]
fn resolve_path_rejects_non_relative_specifier() {
    let root = create_temp_project("non_relative");
    let parent = root.join("main.js");
    let result = ModuleResolver::resolve_path("lodash", &parent);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("Cannot find module"));
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
fn resolve_path_directory_index() {
    let root = create_temp_project("dir_index");
    write_file(&root, "lib/index.js", "export const x = 1;\n");
    let parent = root.join("main.js");
    let result = ModuleResolver::resolve_path("./lib", &parent);
    assert!(result.is_ok());
    let path = result.unwrap();
    assert!(path.to_string_lossy().ends_with("lib/index.js"));
}

#[test]
fn resolve_path_node_modules_package() {
    let root = create_temp_project("node_modules_pkg");
    write_file(
        &root,
        "node_modules/some-pkg/index.js",
        "export const x = 1;\n",
    );
    let parent = root.join("main.js");
    let result = ModuleResolver::resolve_path("some-pkg", &parent);
    assert!(result.is_ok());
    let path = result.unwrap();
    assert!(path.to_string_lossy().contains("node_modules/some-pkg"));
}

#[test]
fn resolve_path_node_modules_package_json_main() {
    let root = create_temp_project("node_modules_main");
    write_file(
        &root,
        "node_modules/foo-pkg/package.json",
        r#"{"main":"lib/entry.js"}"#,
    );
    write_file(&root, "node_modules/foo-pkg/lib/entry.js", "export {};\n");
    let parent = root.join("main.js");
    let result = ModuleResolver::resolve_path("foo-pkg", &parent);
    assert!(result.is_ok());
    let path = result.unwrap();
    assert!(path.to_string_lossy().ends_with("entry.js"));
}

#[test]
fn exports_blocks_main_fallback() {
    let root = create_temp_project("exports_blocks_main");
    write_file(&root, "main.js", "import 'pkg';\n");
    write_file(
        &root,
        "node_modules/pkg/package.json",
        r#"{"main":"legacy.js","exports":{"./only":"./only.js"}}"#,
    );
    write_file(
        &root,
        "node_modules/pkg/legacy.js",
        "export const legacy = true;\n",
    );
    write_file(
        &root,
        "node_modules/pkg/only.js",
        "export const only = true;\n",
    );

    let err = resolve_error(&root, &root.join("main.js"), "pkg");

    assert!(err.contains("ERR_PACKAGE_PATH_NOT_EXPORTED"));
    assert!(err.contains(".`"));
}

#[test]
fn exports_resolves_package_main_dot() {
    let root = create_temp_project("exports_dot");
    write_file(&root, "main.js", "import 'pkg';\n");
    write_file(
        &root,
        "node_modules/pkg/package.json",
        r#"{"type":"module","main":"legacy.js","exports":{".":"./esm.js"}}"#,
    );
    write_file(
        &root,
        "node_modules/pkg/legacy.js",
        "export const value = 'legacy';\n",
    );
    write_file(
        &root,
        "node_modules/pkg/esm.js",
        "export const value = 'esm';\n",
    );

    let path = resolve_loaded_path(&root, &root.join("main.js"), "pkg");

    assert!(path.ends_with(Path::new("node_modules/pkg/esm.js")));
}

#[test]
fn exports_resolves_directory_target_to_index_without_package_entry() {
    let root = create_temp_project("exports_dir_target_index");
    write_file(&root, "main.js", "import 'pkg';\n");
    write_file(
        &root,
        "node_modules/pkg/package.json",
        r#"{"type":"module","main":"legacy.js","exports":{".":"./dir"}}"#,
    );
    write_file(
        &root,
        "node_modules/pkg/legacy.js",
        "export const value = 'legacy';\n",
    );
    write_file(
        &root,
        "node_modules/pkg/dir/package.json",
        r#"{"type":"module","module":"wrong-module.js","main":"wrong.js"}"#,
    );
    write_file(
        &root,
        "node_modules/pkg/dir/wrong-module.js",
        "export const value = 'wrong-module';\n",
    );
    write_file(
        &root,
        "node_modules/pkg/dir/wrong.js",
        "export const value = 'wrong';\n",
    );
    write_file(
        &root,
        "node_modules/pkg/dir/index.js",
        "export const value = 'index';\n",
    );

    let path = resolve_loaded_path(&root, &root.join("main.js"), "pkg");

    assert!(path.ends_with(Path::new("node_modules/pkg/dir/index.js")));
}

#[test]
fn exports_resolves_get_id_for_specifier_matches_resolve_id() {
    let root = create_temp_project("exports_get_id_consistency");
    write_file(&root, "main.js", "import 'pkg';\n");
    write_file(
        &root,
        "node_modules/pkg/package.json",
        r#"{"type":"module","exports":{".":"./esm.js"}}"#,
    );
    write_file(
        &root,
        "node_modules/pkg/esm.js",
        "export const value = 'esm';\n",
    );
    let parent = root.join("main.js");
    let mut resolver = ModuleResolver::new(&root);

    let id = resolver
        .resolve("pkg", &parent)
        .expect("package exports should resolve");
    let cached_id = resolver
        .get_id_for_specifier("pkg", &parent)
        .expect("specifier id lookup should resolve");

    assert_eq!(cached_id, Some(id));
}

#[test]
fn exports_resolves_subpath() {
    let root = create_temp_project("exports_subpath");
    write_file(&root, "main.js", "import 'pkg/feature';\n");
    write_file(
        &root,
        "node_modules/pkg/package.json",
        r#"{"type":"module","exports":{"./feature":"./src/feature.js"}}"#,
    );
    write_file(
        &root,
        "node_modules/pkg/src/feature.js",
        "export const feature = true;\n",
    );

    let path = resolve_loaded_path(&root, &root.join("main.js"), "pkg/feature");

    assert!(path.ends_with(Path::new("node_modules/pkg/src/feature.js")));
}

#[test]
fn exports_resolves_pattern() {
    let root = create_temp_project("exports_pattern");
    write_file(&root, "main.js", "import 'pkg/features/a';\n");
    write_file(
        &root,
        "node_modules/pkg/package.json",
        r#"{"type":"module","exports":{"./features/*":"./src/features/*.js"}}"#,
    );
    write_file(
        &root,
        "node_modules/pkg/src/features/a.js",
        "export const a = true;\n",
    );

    let path = resolve_loaded_path(&root, &root.join("main.js"), "pkg/features/a");

    assert!(path.ends_with(Path::new("node_modules/pkg/src/features/a.js")));
}

#[test]
fn exports_null_blocks_subpath() {
    let root = create_temp_project("exports_null");
    write_file(&root, "main.js", "import 'pkg/blocked';\n");
    write_file(
        &root,
        "node_modules/pkg/package.json",
        r#"{"exports":{"./blocked":null}}"#,
    );
    write_file(
        &root,
        "node_modules/pkg/blocked.js",
        "export const blocked = true;\n",
    );

    let parent = root.join("main.js");
    let err = resolve_error(&root, &parent, "pkg/blocked");

    assert!(err.contains("ERR_PACKAGE_PATH_NOT_EXPORTED"));
    assert!(err.contains("./blocked"));
    assert!(err.contains(&parent.display().to_string()));
}

#[test]
fn imports_resolves_hash_alias_within_parent_package() {
    let root = create_temp_project("imports_alias");
    write_file(
        &root,
        "package.json",
        r##"{"name":"app","type":"module","imports":{"#alias":"./src/alias.js"}}"##,
    );
    write_file(&root, "src/main.js", "import '#alias';\n");
    write_file(&root, "src/alias.js", "export const alias = true;\n");

    let path = resolve_loaded_path(&root, &root.join("src/main.js"), "#alias");

    assert!(path.ends_with(Path::new("src/alias.js")));
}

#[test]
fn imports_missing_reports_import_not_defined() {
    let root = create_temp_project("imports_missing");
    write_file(
        &root,
        "package.json",
        r##"{"name":"app","type":"module","imports":{"#present":"./src/present.js"}}"##,
    );
    write_file(&root, "src/main.js", "import '#missing';\n");
    write_file(&root, "src/present.js", "export const present = true;\n");

    let parent = root.join("src/main.js");
    let err = resolve_error(&root, &parent, "#missing");

    assert!(err.contains("ERR_PACKAGE_IMPORT_NOT_DEFINED"));
    assert!(err.contains("#missing"));
    assert!(err.contains(&parent.display().to_string()));
}

#[test]
fn self_reference_uses_own_exports_before_node_modules() {
    let root = create_temp_project("self_reference");
    write_file(
        &root,
        "package.json",
        r#"{"name":"pkg","type":"module","exports":{"./self":"./src/self.js"}}"#,
    );
    write_file(&root, "src/main.js", "import 'pkg/self';\n");
    write_file(&root, "src/self.js", "export const source = 'self';\n");
    write_file(
        &root,
        "node_modules/pkg/package.json",
        r#"{"name":"pkg","exports":{"./self":"./wrong.js"}}"#,
    );
    write_file(
        &root,
        "node_modules/pkg/wrong.js",
        "export const source = 'node_modules';\n",
    );

    let path = resolve_loaded_path(&root, &root.join("src/main.js"), "pkg/self");

    assert!(path.ends_with(Path::new("src/self.js")));
}

#[test]
fn self_reference_without_exports_uses_node_modules_package() {
    let root = create_temp_project("self_reference_no_exports");
    write_file(&root, "package.json", r#"{"name":"pkg","type":"module"}"#);
    write_file(&root, "src/main.js", "import 'pkg';\n");
    write_file(
        &root,
        "node_modules/pkg/package.json",
        r#"{"name":"pkg","type":"module","exports":{".":"./node.js"}}"#,
    );
    write_file(
        &root,
        "node_modules/pkg/node.js",
        "export const source = 'node_modules';\n",
    );

    let path = resolve_loaded_path(&root, &root.join("src/main.js"), "pkg");

    assert!(path.ends_with(Path::new("node_modules/pkg/node.js")));
}

#[test]
fn legacy_main_still_works_without_exports() {
    let root = create_temp_project("legacy_main_without_exports");
    write_file(&root, "main.js", "import 'pkg';\n");
    write_file(
        &root,
        "node_modules/pkg/package.json",
        r#"{"type":"module","main":"legacy.js"}"#,
    );
    write_file(
        &root,
        "node_modules/pkg/legacy.js",
        "export const value = 'legacy';\n",
    );

    let path = resolve_loaded_path(&root, &root.join("main.js"), "pkg");

    assert!(path.ends_with(Path::new("node_modules/pkg/legacy.js")));
}

#[test]
fn browser_string_replaces_package_entry_when_enabled() {
    let root = create_temp_project("browser_string_entry");
    write_file(&root, "main.js", "import 'pkg';\n");
    write_file(
        &root,
        "node_modules/pkg/package.json",
        r#"{"type":"module","module":"module.js","main":"main.js","browser":"browser.js"}"#,
    );
    write_file(
        &root,
        "node_modules/pkg/module.js",
        "export const value = 'module';\n",
    );
    write_file(
        &root,
        "node_modules/pkg/main.js",
        "export const value = 'main';\n",
    );
    write_file(
        &root,
        "node_modules/pkg/browser.js",
        "export const value = 'browser';\n",
    );

    let default_path = resolve_loaded_path(&root, &root.join("main.js"), "pkg");
    assert!(default_path.ends_with(Path::new("node_modules/pkg/module.js")));

    let mut resolver =
        ModuleResolver::with_options(&root, ResolutionOptions::default().with_browser(true));
    let id = resolver
        .resolve("pkg", &root.join("main.js"))
        .expect("browser entry should resolve");
    let path = &resolver
        .get_module(id)
        .expect("module should be loaded")
        .path;

    assert!(path.ends_with(Path::new("node_modules/pkg/browser.js")));
}

#[test]
fn browser_object_replaces_package_entry_when_enabled() {
    let root = create_temp_project("browser_object_entry");
    write_file(&root, "main.js", "import 'pkg';\n");
    write_file(
        &root,
        "node_modules/pkg/package.json",
        r#"{"type":"module","main":"main.js","browser":{"./main.js":"./browser.js"}}"#,
    );
    write_file(
        &root,
        "node_modules/pkg/main.js",
        "export const value = 'main';\n",
    );
    write_file(
        &root,
        "node_modules/pkg/browser.js",
        "export const value = 'browser';\n",
    );

    let default_path = resolve_loaded_path(&root, &root.join("main.js"), "pkg");
    assert!(default_path.ends_with(Path::new("node_modules/pkg/main.js")));

    let mut resolver =
        ModuleResolver::with_options(&root, ResolutionOptions::default().with_browser(true));
    let id = resolver
        .resolve("pkg", &root.join("main.js"))
        .expect("browser object entry should resolve");
    let path = &resolver
        .get_module(id)
        .expect("module should be loaded")
        .path;

    assert!(path.ends_with(Path::new("node_modules/pkg/browser.js")));
}

#[test]
fn browser_object_false_disables_package_entry_when_enabled() {
    let root = create_temp_project("browser_object_entry_false");
    write_file(&root, "main.js", "import 'pkg';\n");
    write_file(
        &root,
        "node_modules/pkg/package.json",
        r#"{"type":"module","main":"main.js","browser":{"./main.js":false}}"#,
    );
    write_file(
        &root,
        "node_modules/pkg/main.js",
        "export const value = 'main';\n",
    );

    let mut resolver =
        ModuleResolver::with_options(&root, ResolutionOptions::default().with_browser(true));
    let error = resolver
        .resolve("pkg", &root.join("main.js"))
        .expect_err("browser false package entry should disable the package path");
    let message = format!("{error:#}");

    assert!(message.contains("ERR_PACKAGE_PATH_DISABLED_BY_BROWSER"));
    assert!(message.contains("./main.js"));
}

#[test]
fn browser_object_replaces_extensionless_package_entry_when_enabled() {
    let root = create_temp_project("browser_object_extensionless_entry");
    write_file(&root, "main.js", "import 'pkg';\n");
    write_file(
        &root,
        "node_modules/pkg/package.json",
        r#"{"type":"module","main":"main","browser":{"./main.js":"./browser.js"}}"#,
    );
    write_file(
        &root,
        "node_modules/pkg/main.js",
        "export const value = 'main';\n",
    );
    write_file(
        &root,
        "node_modules/pkg/browser.js",
        "export const value = 'browser';\n",
    );

    let mut resolver =
        ModuleResolver::with_options(&root, ResolutionOptions::default().with_browser(true));
    let id = resolver
        .resolve("pkg", &root.join("main.js"))
        .expect("browser object entry should resolve after extension lookup");
    let path = &resolver
        .get_module(id)
        .expect("module should be loaded")
        .path;

    assert!(path.ends_with(Path::new("node_modules/pkg/browser.js")));
}

#[test]
fn browser_object_false_disables_extensionless_package_entry_when_enabled() {
    let root = create_temp_project("browser_object_extensionless_false");
    write_file(&root, "main.js", "import 'pkg';\n");
    write_file(
        &root,
        "node_modules/pkg/package.json",
        r#"{"type":"module","main":"main","browser":{"./main.js":false}}"#,
    );
    write_file(
        &root,
        "node_modules/pkg/main.js",
        "export const value = 'main';\n",
    );

    let mut resolver =
        ModuleResolver::with_options(&root, ResolutionOptions::default().with_browser(true));
    let error = resolver
        .resolve("pkg", &root.join("main.js"))
        .expect_err("browser false package entry should disable resolved extension path");
    let message = format!("{error:#}");

    assert!(message.contains("ERR_PACKAGE_PATH_DISABLED_BY_BROWSER"));
    assert!(message.contains("./main.js"));
}

#[test]
fn browser_object_replaces_extensionless_module_entry_when_enabled() {
    let root = create_temp_project("browser_object_extensionless_module_entry");
    write_file(&root, "main.js", "import 'pkg';\n");
    write_file(
        &root,
        "node_modules/pkg/package.json",
        r#"{"type":"module","module":"module","main":"main.js","browser":{"./module.js":"./browser.js"}}"#,
    );
    write_file(
        &root,
        "node_modules/pkg/module.js",
        "export const value = 'module';\n",
    );
    write_file(
        &root,
        "node_modules/pkg/main.js",
        "export const value = 'main';\n",
    );
    write_file(
        &root,
        "node_modules/pkg/browser.js",
        "export const value = 'browser';\n",
    );

    let mut resolver =
        ModuleResolver::with_options(&root, ResolutionOptions::default().with_browser(true));
    let id = resolver
        .resolve("pkg", &root.join("main.js"))
        .expect("browser object module entry should resolve after extension lookup");
    let path = &resolver
        .get_module(id)
        .expect("module should be loaded")
        .path;

    assert!(path.ends_with(Path::new("node_modules/pkg/browser.js")));
}

#[test]
fn browser_object_false_disables_extensionless_module_entry_when_enabled() {
    let root = create_temp_project("browser_object_extensionless_module_false");
    write_file(&root, "main.js", "import 'pkg';\n");
    write_file(
        &root,
        "node_modules/pkg/package.json",
        r#"{"type":"module","module":"module","main":"main.js","browser":{"./module.js":false}}"#,
    );
    write_file(
        &root,
        "node_modules/pkg/module.js",
        "export const value = 'module';\n",
    );
    write_file(
        &root,
        "node_modules/pkg/main.js",
        "export const value = 'main';\n",
    );

    let mut resolver =
        ModuleResolver::with_options(&root, ResolutionOptions::default().with_browser(true));
    let error = resolver
        .resolve("pkg", &root.join("main.js"))
        .expect_err("browser false module entry should disable resolved extension path");
    let message = format!("{error:#}");

    assert!(message.contains("ERR_PACKAGE_PATH_DISABLED_BY_BROWSER"));
    assert!(message.contains("./module.js"));
}

#[test]
fn browser_mapping_replaces_relative_dependency_when_enabled() {
    let root = create_temp_project("browser_map_replace");
    write_type_module_package(&root);
    write_file(&root, "main.js", "import './server.js';\n");
    write_file(&root, "server.js", "export const target = 'server';\n");
    write_file(&root, "browser.js", "export const target = 'browser';\n");
    write_file(
        &root,
        "package.json",
        r#"{"type":"module","browser":{"./server.js":"./browser.js"}}"#,
    );

    let default_path = resolve_loaded_path(&root, &root.join("main.js"), "./server.js");
    assert!(default_path.ends_with(Path::new("server.js")));

    let mut resolver =
        ModuleResolver::with_options(&root, ResolutionOptions::default().with_browser(true));
    let id = resolver
        .resolve("./server.js", &root.join("main.js"))
        .expect("browser mapping should resolve");
    let path = &resolver
        .get_module(id)
        .expect("module should be loaded")
        .path;

    assert!(path.ends_with(Path::new("browser.js")));
}

#[test]
fn browser_mapping_false_reports_disabled() {
    let root = create_temp_project("browser_map_false");
    write_file(
        &root,
        "package.json",
        r#"{"type":"module","browser":{"./native.js":false}}"#,
    );
    write_file(&root, "main.js", "import './native.js';\n");
    write_file(&root, "native.js", "export const native = true;\n");

    let mut resolver =
        ModuleResolver::with_options(&root, ResolutionOptions::default().with_browser(true));
    let error = resolver
        .resolve("./native.js", &root.join("main.js"))
        .expect_err("browser false mapping should disable the package path");
    let message = format!("{error:#}");

    assert!(message.contains("ERR_PACKAGE_PATH_DISABLED_BY_BROWSER"));
    assert!(message.contains("./native.js"));
}
#[test]
fn node_prefix_still_resolves_builtin_without_node_modules() {
    let root = create_temp_project("node_prefix_builtin");
    write_file(&root, "main.js", "import 'node:fs';\n");
    write_file(
        &root,
        "node_modules/fs/index.js",
        "export const wrong = true;\n",
    );
    #[cfg(not(windows))]
    write_file(
        &root,
        "node_modules/node:fs/index.js",
        "export const wrong = true;\n",
    );
    let path = resolve_loaded_path(&root, &root.join("main.js"), "node:fs");

    assert!(
        path.to_string_lossy()
            .ends_with("/__wjsm_builtin__/node/fs.mjs")
    );
}

#[test]
fn runtime_resolve_relative_file_returns_file_key() {
    let root = create_temp_project("runtime_relative_file");
    write_type_module_package(&root);
    write_file(&root, "src/main.js", "import './dep.js';\n");
    write_file(&root, "src/dep.js", "export const value = 1;\n");
    let dep_path = root
        .join("src/dep.js")
        .canonicalize()
        .expect("dep path should canonicalize");

    let resolved = resolve_runtime_specifier(
        "./dep.js",
        &root.join("src/main.js"),
        &root,
        &ResolutionOptions::default(),
        RuntimeResolveKind::Import,
    )
    .expect("relative runtime import should resolve");

    assert_eq!(resolved.key, RuntimeModuleKey::File(dep_path.clone()));
    assert_eq!(resolved.path.as_deref(), Some(dep_path.as_path()));
    assert_eq!(resolved.format, RuntimeModuleFormat::Esm);
    assert_eq!(resolved.url, url::Url::from_file_path(&dep_path).unwrap().to_string());
}

#[test]
fn runtime_resolve_json_file_returns_json_key() {
    let root = create_temp_project("runtime_json_file");
    write_file(&root, "main.cjs", "require('./config.json');\n");
    write_file(&root, "config.json", r#"{"value":1}"#);
    let json_path = root
        .join("config.json")
        .canonicalize()
        .expect("json path should canonicalize");

    let resolved = resolve_runtime_specifier(
        "./config.json",
        &root.join("main.cjs"),
        &root,
        &ResolutionOptions::default(),
        RuntimeResolveKind::Require,
    )
    .expect("json require should resolve");

    assert_eq!(resolved.key, RuntimeModuleKey::Json(json_path.clone()));
    assert_eq!(resolved.path.as_deref(), Some(json_path.as_path()));
    assert_eq!(resolved.format, RuntimeModuleFormat::Json);
    assert_eq!(resolved.url, url::Url::from_file_path(&json_path).unwrap().to_string());
}

#[test]
fn runtime_resolve_package_uses_require_condition() {
    let root = create_temp_project("runtime_require_condition");
    write_file(&root, "main.cjs", "require('pkg');\n");
    write_file(
        &root,
        "node_modules/pkg/package.json",
        r#"{"exports":{".":{"import":"./esm.mjs","require":"./cjs.cjs","default":"./default.js"}}}"#,
    );
    write_file(&root, "node_modules/pkg/esm.mjs", "export const value = 'esm';\n");
    write_file(&root, "node_modules/pkg/cjs.cjs", "module.exports = 'cjs';\n");
    write_file(&root, "node_modules/pkg/default.js", "export default 'default';\n");

    let resolved = resolve_runtime_specifier(
        "pkg",
        &root.join("main.cjs"),
        &root,
        &ResolutionOptions::default(),
        RuntimeResolveKind::Require,
    )
    .expect("runtime require should resolve package");

    assert!(resolved.path.as_deref().unwrap().ends_with(Path::new("node_modules/pkg/cjs.cjs")));
    assert_eq!(resolved.format, RuntimeModuleFormat::CommonJs);
}

#[test]
fn runtime_resolve_import_meta_uses_import_condition() {
    let root = create_temp_project("runtime_import_condition");
    write_type_module_package(&root);
    write_file(&root, "main.js", "import.meta.resolve('pkg');\n");
    write_file(
        &root,
        "node_modules/pkg/package.json",
        r#"{"exports":{".":{"import":"./esm.mjs","require":"./cjs.cjs","default":"./default.js"}}}"#,
    );
    write_file(&root, "node_modules/pkg/esm.mjs", "export const value = 'esm';\n");
    write_file(&root, "node_modules/pkg/cjs.cjs", "module.exports = 'cjs';\n");
    write_file(&root, "node_modules/pkg/default.js", "export default 'default';\n");

    let resolved = resolve_runtime_specifier(
        "pkg",
        &root.join("main.js"),
        &root,
        &ResolutionOptions::default(),
        RuntimeResolveKind::Import,
    )
    .expect("import.meta.resolve should use import conditions");

    assert!(resolved.path.as_deref().unwrap().ends_with(Path::new("node_modules/pkg/esm.mjs")));
    assert_eq!(resolved.format, RuntimeModuleFormat::Esm);
}

#[test]
fn runtime_resolve_builtin_returns_builtin_key() {
    let root = create_temp_project("runtime_builtin");
    write_file(&root, "main.js", "import 'node:path';\n");

    let resolved = resolve_runtime_specifier(
        "node:path",
        &root.join("main.js"),
        &root,
        &ResolutionOptions::default(),
        RuntimeResolveKind::Import,
    )
    .expect("builtin should resolve");

    assert_eq!(resolved.key, RuntimeModuleKey::Builtin("node:path".to_string()));
    assert_eq!(resolved.path, None);
    assert_eq!(resolved.url, "node:path");
    assert_eq!(resolved.format, RuntimeModuleFormat::Builtin);
}

#[test]
fn resolve_paths_for_bare_package_lists_node_modules_parents() {
    let root = create_temp_project("runtime_resolve_paths_bare");
    let parent = root.join("packages/app/src/main.js");

    let paths = resolve_runtime_paths("pkg", &parent, &root);

    assert_eq!(
        paths,
        RuntimeResolvePaths::Search(vec![
            root.join("packages/app/src/node_modules"),
            root.join("packages/app/node_modules"),
            root.join("packages/node_modules"),
            root.join("node_modules"),
        ])
    );
}

#[test]
fn resolve_paths_for_relative_returns_null_marker() {
    let root = create_temp_project("runtime_resolve_paths_null");
    let parent = root.join("src/main.js");

    assert_eq!(
        resolve_runtime_paths("./dep.js", &parent, &root),
        RuntimeResolvePaths::Null
    );
    assert_eq!(
        resolve_runtime_paths(&root.join("src/dep.js").display().to_string(), &parent, &root),
        RuntimeResolvePaths::Null
    );
    assert_eq!(
        resolve_runtime_paths("node:path", &parent, &root),
        RuntimeResolvePaths::Null
    );
}

#[test]
fn resolve_rejects_path_outside_root() {
    let root = create_temp_project("outside_root");
    let outside = create_temp_project("outside_root_sibling");
    let outside_name = outside.file_name().unwrap().to_string_lossy();
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
    write_type_module_package(&root);
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
    write_type_module_package(&root);
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
fn mjs_forces_esm_even_with_package_commonjs() {
    let root = create_temp_project("mjs_forces_esm");
    write_file(&root, "package.json", r#"{"type":"commonjs"}"#);
    write_file(&root, "entry.mjs", "module.exports = { value: 1 };\n");
    let parent = root.join("main.js");
    let mut resolver = ModuleResolver::new(&root);

    let id = resolver
        .resolve("./entry.mjs", &parent)
        .expect("resolve should succeed");
    let module = resolver.get_module(id).expect("module should exist");

    assert!(!module.is_cjs, ".mjs should force ESM goal");
    assert!(
        module.exports.is_empty(),
        "ESM goal should not transform module.exports into synthetic exports"
    );
}

#[test]
fn cjs_forces_commonjs_even_with_package_module() {
    let root = create_temp_project("cjs_forces_commonjs");
    write_file(&root, "package.json", r#"{"type":"module"}"#);
    write_file(&root, "entry.cjs", "const value = 1;\n");
    let parent = root.join("main.js");
    let mut resolver = ModuleResolver::new(&root);

    let id = resolver
        .resolve("./entry.cjs", &parent)
        .expect("resolve should succeed");
    let module = resolver.get_module(id).expect("module should exist");

    assert!(module.is_cjs, ".cjs should force CommonJS goal");
}

#[test]
fn type_module_js_is_esm() {
    let root = create_temp_project("type_module_js_is_esm");
    write_file(&root, "package.json", r#"{"type":"module"}"#);
    write_file(&root, "entry.js", "module.exports = { value: 1 };\n");
    let parent = root.join("main.js");
    let mut resolver = ModuleResolver::new(&root);

    let id = resolver
        .resolve("./entry.js", &parent)
        .expect("resolve should succeed");
    let module = resolver.get_module(id).expect("module should exist");

    assert!(!module.is_cjs, "package type=module should force ESM goal");
    assert!(
        module.exports.is_empty(),
        "ESM goal should not transform module.exports into synthetic exports"
    );
}

#[test]
fn type_commonjs_js_is_commonjs() {
    let root = create_temp_project("type_commonjs_js_is_commonjs");
    write_file(&root, "package.json", r#"{"type":"commonjs"}"#);
    write_file(&root, "entry.js", "const value = 1;\n");
    let parent = root.join("main.js");
    let mut resolver = ModuleResolver::new(&root);

    let id = resolver
        .resolve("./entry.js", &parent)
        .expect("resolve should succeed");
    let module = resolver.get_module(id).expect("module should exist");

    assert!(
        module.is_cjs,
        "package type=commonjs should force CommonJS goal"
    );
}

#[test]
fn commonjs_goal_rejects_static_import_syntax() {
    let root = create_temp_project("commonjs_rejects_static_import");
    write_file(&root, "package.json", r#"{"type":"commonjs"}"#);
    write_file(&root, "dep.js", "module.exports.value = 1;\n");
    write_file(&root, "entry.js", "import { value } from './dep.js';\n");
    let parent = root.join("main.js");
    let mut resolver = ModuleResolver::new(&root);

    let err = resolver
        .resolve("./entry.js", &parent)
        .expect_err("static import should fail under CommonJS goal");
    let msg = err.to_string();
    let entry = root.join("entry.js");

    assert!(
        msg.contains(&format!(
            "SyntaxError: Cannot use import/export syntax in CommonJS module {}",
            entry.display()
        )),
        "unexpected error: {msg}"
    );
}

#[test]
fn get_module_returns_some_for_existing() {
    let root = create_temp_project("get_mod_some");
    write_type_module_package(&root);
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
    write_type_module_package(&root);
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
    write_type_module_package(&root);
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
    write_type_module_package(&root);
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
    write_type_module_package(&root);
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
    write_type_module_package(&root);
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
    write_type_module_package(&root);
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
    write_type_module_package(&root);
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
    write_type_module_package(&root);
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
    write_type_module_package(&root);
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
    write_type_module_package(&root);
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
    write_type_module_package(&root);
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
    write_type_module_package(&root);
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
    write_type_module_package(&root);
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
    write_type_module_package(&root);
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
    write_type_module_package(&root);
    write_file(&root, "base.js", "export const x = 1;\n");
    write_file(&root, "dep.js", "export { x } from './base.js';\n");
    let parent = root.join("main.js");
    let mut resolver = ModuleResolver::new(&root);
    let id = resolver
        .resolve("./dep.js", &parent)
        .expect("resolve should succeed");
    let module = resolver.get_module(id).expect("module should exist");
    // export { x } from './base.js' 应该产生 NamedReExport，而不是 All
    assert!(module.exports.iter().any(|e| matches!(
        e,
        ExportEntry::NamedReExport { local, exported, source }
            if local == "x" && exported == "x" && source == "./base.js"
    )));
}

#[test]
fn extract_exports_handles_default_anonymous_fn() {
    let root = create_temp_project("export_default_anon_fn");
    write_type_module_package(&root);
    write_file(&root, "dep.js", "export default function() {}\n");
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
            .any(|e| matches!(e, ExportEntry::Default { local, .. } if local == "_default_export"))
    );
}

#[test]
fn extract_exports_handles_default_anonymous_class() {
    let root = create_temp_project("export_default_anon_class");
    write_type_module_package(&root);
    write_file(&root, "dep.js", "export default class {}\n");
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
            .any(|e| matches!(e, ExportEntry::Default { local, .. } if local == "_default_export"))
    );
}

#[test]
fn extract_exports_handles_multiple_var_declarations() {
    let root = create_temp_project("export_multi_var");
    write_type_module_package(&root);
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

#[test]
fn resolve_rejects_ts_import_equals() {
    let root = create_temp_project("ts_import_eq");
    write_file(&root, "foo.ts", "export const x = 1;\n");
    write_file(&root, "main.ts", "import x = require('./foo');\n");
    let parent = root.join("main.ts");
    let mut resolver = ModuleResolver::new(&root);
    let err = resolver
        .resolve("./main.ts", &parent)
        .expect_err("ts import equals should fail");
    let msg = err.to_string();
    assert!(
        msg.contains("import assignment") || msg.contains("import x"),
        "unexpected error: {msg}"
    );
}

#[test]
fn resolve_rejects_ts_export_assignment() {
    let root = create_temp_project("ts_export_eq");
    write_file(&root, "mod.ts", "const x = 1;\nexport = x;\n");
    let parent = root.join("entry.ts");
    write_file(&root, "entry.ts", "import x = require('./mod');\n");
    let mut resolver = ModuleResolver::new(&root);
    // 先解析 mod.ts 会因 export = 失败
    let err = resolver
        .resolve("./mod.ts", &parent)
        .expect_err("export = should fail");
    assert!(
        err.to_string().contains("export ="),
        "unexpected error: {}",
        err
    );
}

fn extracted_dynamic_imports(source: &str) -> Vec<String> {
    let module = wjsm_parser::parse_module(source).expect("source should parse");
    ModuleResolver::extract_dynamic_imports(&module).expect("dynamic import extraction should succeed")
}

#[test]
fn dynamic_import_expression_is_runtime_not_resolver_diagnostic() {
    let specifiers = extracted_dynamic_imports("const path = './dep.js'; import(path);\n");

    assert!(
        specifiers.is_empty(),
        "runtime expression import must not create AOT graph edges: {specifiers:?}"
    );
}

#[test]
fn dynamic_import_template_expression_is_runtime_not_resolver_diagnostic() {
    let specifiers = extracted_dynamic_imports("const name = 'dep'; import(`./${name}.js`);\n");

    assert!(
        specifiers.is_empty(),
        "template expression import must not create AOT graph edges: {specifiers:?}"
    );
}

#[test]
fn dynamic_import_static_literal_still_creates_graph_edge() {
    let specifiers = extracted_dynamic_imports("import('./dep.js'); import(`./other.js`);\n");

    assert_eq!(specifiers, vec!["./dep.js", "./other.js"]);
}

#[test]
fn dynamic_import_without_specifier_still_reports_malformed_call() {
    let error = wjsm_parser::parse_module("import();\n")
        .expect_err("zero-argument import() should remain malformed");

    assert!(error.to_string().contains("exactly one or two arguments"));
}
