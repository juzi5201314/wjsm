use std::path::Path;

use crate::package_json::{PackageInfo, PackageType};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModuleFormat {
    Esm,
    CommonJs,
}

pub(crate) fn detect_module_format(
    path: &Path,
    package: Option<&PackageInfo>,
    ast_is_cjs: bool,
) -> ModuleFormat {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("mjs") => ModuleFormat::Esm,
        Some("cjs") => ModuleFormat::CommonJs,
        Some("js") => match package.map(|package| package.package_type) {
            Some(PackageType::Module) => ModuleFormat::Esm,
            Some(PackageType::CommonJs) => ModuleFormat::CommonJs,
            None if ast_is_cjs => ModuleFormat::CommonJs,
            None => ModuleFormat::Esm,
        },
        _ if ast_is_cjs => ModuleFormat::CommonJs,
        _ => ModuleFormat::Esm,
    }
}

#[cfg(test)]
mod tests {
    use super::{ModuleFormat, detect_module_format};
    use crate::package_json::{PackageInfo, PackageType};

    use std::path::{Path, PathBuf};

    fn package_with_type(package_type: PackageType) -> PackageInfo {
        PackageInfo {
            root: PathBuf::from("/pkg"),
            path: PathBuf::from("/pkg/package.json"),
            name: None,
            package_type,
            module: None,
            main: None,
            exports: None,
            imports: None,
            browser: None,
        }
    }

    #[test]
    fn mjs_is_esm() {
        assert_eq!(
            detect_module_format(Path::new("/pkg/index.mjs"), None, true),
            ModuleFormat::Esm
        );
    }

    #[test]
    fn cjs_is_commonjs() {
        assert_eq!(
            detect_module_format(Path::new("/pkg/index.cjs"), None, false),
            ModuleFormat::CommonJs
        );
    }

    #[test]
    fn js_uses_package_type_module() {
        let package = package_with_type(PackageType::Module);

        assert_eq!(
            detect_module_format(Path::new("/pkg/index.js"), Some(&package), true),
            ModuleFormat::Esm
        );
    }

    #[test]
    fn js_package_without_type_defaults_to_commonjs() {
        let package = package_with_type(PackageType::CommonJs);

        assert_eq!(
            detect_module_format(Path::new("/pkg/index.js"), Some(&package), false),
            ModuleFormat::CommonJs
        );
    }

    #[test]
    fn js_without_package_uses_ast_detection_marker() {
        assert_eq!(
            detect_module_format(Path::new("/pkg/index.js"), None, true),
            ModuleFormat::CommonJs
        );
        assert_eq!(
            detect_module_format(Path::new("/pkg/index.js"), None, false),
            ModuleFormat::Esm
        );
    }

    #[test]
    fn tsx_uses_ast_detection_marker() {
        assert_eq!(
            detect_module_format(Path::new("/pkg/index.tsx"), None, true),
            ModuleFormat::CommonJs
        );
        assert_eq!(
            detect_module_format(Path::new("/pkg/index.tsx"), None, false),
            ModuleFormat::Esm
        );
    }
}
