use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PackageInfo {
    pub(crate) root: PathBuf,
    pub(crate) path: PathBuf,
    pub(crate) name: Option<String>,
    pub(crate) package_type: PackageType,
    pub(crate) module: Option<String>,
    pub(crate) main: Option<String>,
    pub(crate) exports: Option<Value>,
    pub(crate) imports: Option<Value>,
    pub(crate) browser: Option<BrowserField>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PackageType {
    CommonJs,
    Module,
}

impl Default for PackageType {
    fn default() -> Self {
        Self::CommonJs
    }
}

impl PackageType {
    fn from_package_json(value: Option<&Value>) -> Self {
        match value.and_then(Value::as_str) {
            Some("module") => Self::Module,
            _ => Self::CommonJs,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BrowserField {
    Entry(String),
    Map(BTreeMap<String, Option<String>>),
}

pub(crate) fn read_package_info(package_dir: &Path) -> Result<Option<PackageInfo>> {
    let path = package_dir.join("package.json");
    match fs::metadata(&path) {
        Ok(_) => read_package_info_manifest(&path).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => {
            Err(error).with_context(|| format!("stat package.json at {}", path.display()))
        }
    }
}

fn read_package_info_manifest(path: &Path) -> Result<PackageInfo> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("read package.json at {}", path.display()))?;
    let value: Value = serde_json::from_str(&text)
        .with_context(|| format!("parse package.json at {}", path.display()))?;

    Ok(PackageInfo {
        root: path.parent().unwrap_or_else(|| Path::new("")).to_path_buf(),
        path: path.to_path_buf(),
        name: value
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_string),
        package_type: PackageType::from_package_json(value.get("type")),
        module: value
            .get("module")
            .and_then(Value::as_str)
            .map(str::to_string),
        main: value
            .get("main")
            .and_then(Value::as_str)
            .map(str::to_string),
        exports: value.get("exports").cloned(),
        imports: value.get("imports").cloned(),
        browser: parse_browser_field(value.get("browser"), path)?,
    })
}

fn parse_browser_field(value: Option<&Value>, path: &Path) -> Result<Option<BrowserField>> {
    match value {
        None => Ok(None),
        Some(Value::String(entry)) => Ok(Some(BrowserField::Entry(entry.clone()))),
        Some(Value::Object(entries)) => {
            let mut browser = BTreeMap::new();
            for (key, value) in entries {
                let replacement = match value {
                    Value::String(replacement) => Some(replacement.clone()),
                    Value::Bool(false) => None,
                    _ => bail!(
                        "invalid browser field entry `{}` in {}: expected string or false",
                        key,
                        path.display()
                    ),
                };
                browser.insert(key.clone(), replacement);
            }
            Ok(Some(BrowserField::Map(browser)))
        }
        Some(_) => bail!(
            "invalid browser field in {}: expected string or object",
            path.display()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{BrowserField, PackageType, read_package_info};

    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_TEST_DIR: AtomicUsize = AtomicUsize::new(0);

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new(name: &str) -> Self {
            let id = NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "wjsm_module_package_json_{name}_{}_{id}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir(&path).unwrap();
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn read_package_info_reads_name_type_exports_imports_browser() {
        let temp = TestDir::new("metadata");
        let package_json = temp.path().join("package.json");
        fs::write(
            &package_json,
            r##"{
                "name": "demo-pkg",
                "type": "module",
                "module": "./module.js",
                "main": "./main.cjs",
                "exports": {".": "./esm.js"},
                "imports": {"#dep": "./dep.js"},
                "browser": {
                    "./server.js": "./browser.js",
                    "./native.js": false
                }
            }"##,
        )
        .unwrap();

        let info = read_package_info(temp.path()).unwrap().unwrap();

        assert_eq!(info.name.as_deref(), Some("demo-pkg"));
        assert_eq!(info.package_type, PackageType::Module);
        assert_eq!(info.module.as_deref(), Some("./module.js"));
        assert_eq!(info.main.as_deref(), Some("./main.cjs"));
        assert_eq!(info.exports.unwrap()["."], "./esm.js");
        assert_eq!(info.imports.unwrap()["#dep"], "./dep.js");
        assert_eq!(info.root, temp.path());
        assert_eq!(info.path, package_json);
        let mut expected_browser = BTreeMap::new();
        expected_browser.insert("./native.js".to_string(), None);
        expected_browser.insert("./server.js".to_string(), Some("./browser.js".to_string()));
        assert_eq!(info.browser, Some(BrowserField::Map(expected_browser)));
    }

    #[test]
    fn read_package_info_defaults_to_commonjs_without_type() {
        let temp = TestDir::new("default_type");
        let package_json = temp.path().join("package.json");
        fs::write(&package_json, r#"{"name":"demo-pkg"}"#).unwrap();

        let info = read_package_info(temp.path()).unwrap().unwrap();

        assert_eq!(info.package_type, PackageType::CommonJs);
        assert_eq!(info.browser, None);
        assert_eq!(info.exports, None);
        assert_eq!(info.imports, None);
        assert_eq!(info.module, None);
        assert_eq!(info.main, None);
    }

    #[test]
    fn read_package_info_returns_none_without_package_json() {
        let temp = TestDir::new("missing_package_json");

        let info = read_package_info(temp.path()).unwrap();

        assert_eq!(info, None);
    }

    #[test]
    fn read_package_info_reads_browser_string_entry() {
        let temp = TestDir::new("browser_string");
        fs::write(
            temp.path().join("package.json"),
            r#"{"browser":"./browser.js"}"#,
        )
        .unwrap();

        let info = read_package_info(temp.path()).unwrap().unwrap();

        assert_eq!(
            info.browser,
            Some(BrowserField::Entry("./browser.js".to_string()))
        );
    }
}
