use std::borrow::Cow;

use anyhow::{Result, anyhow, bail};
use serde_json::{Map, Value};

use crate::package_json::PackageInfo;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PackageTarget {
    pub(crate) relative_path: String,
}

#[derive(Debug, Clone, Copy)]
enum MissingTarget {
    Export,
    Import,
}

impl MissingTarget {
    fn missing<T>(self, request: &str) -> Result<T> {
        match self {
            Self::Export => {
                bail!("ERR_PACKAGE_PATH_NOT_EXPORTED: package subpath `{request}` is not exported")
            }
            Self::Import => {
                bail!("ERR_PACKAGE_IMPORT_NOT_DEFINED: package import `{request}` is not defined")
            }
        }
    }
}

fn normalize_package_subpath(package_subpath: &str) -> Cow<'_, str> {
    if package_subpath.is_empty() || package_subpath == "." {
        Cow::Borrowed(".")
    } else if package_subpath.starts_with("./") {
        Cow::Borrowed(package_subpath)
    } else {
        Cow::Owned(format!("./{package_subpath}"))
    }
}

fn package_error_context(
    error: anyhow::Error,
    package: &PackageInfo,
    field: &str,
) -> anyhow::Error {
    let package_name = package.name.as_deref().unwrap_or("<anonymous>");
    anyhow!(
        "{error}; package `{package_name}` at {}, field `{field}`",
        package.path.display()
    )
}

pub(crate) fn resolve_package_exports<C>(
    package: &PackageInfo,
    package_subpath: &str,
    conditions: &[C],
) -> Result<PackageTarget>
where
    C: AsRef<str>,
{
    resolve_package_exports_inner(package, package_subpath, conditions)
        .map_err(|error| package_error_context(error, package, "exports"))
}

fn resolve_package_exports_inner<C>(
    package: &PackageInfo,
    package_subpath: &str,
    conditions: &[C],
) -> Result<PackageTarget>
where
    C: AsRef<str>,
{
    let normalized_subpath = normalize_package_subpath(package_subpath);
    let package_subpath = normalized_subpath.as_ref();

    let Some(exports) = package.exports.as_ref() else {
        return MissingTarget::Export.missing(package_subpath);
    };

    match exports {
        Value::String(_) | Value::Null | Value::Array(_) => {
            if package_subpath != "." {
                return MissingTarget::Export.missing(package_subpath);
            }
            resolve_target_value(
                exports,
                conditions,
                None,
                package_subpath,
                MissingTarget::Export,
            )
        }
        Value::Object(entries) => resolve_exports_object(entries, package_subpath, conditions),
        _ => invalid_package_config("exports must be a string, object, array, or null"),
    }
}

pub(crate) fn resolve_package_imports<C>(
    package: &PackageInfo,
    specifier: &str,
    conditions: &[C],
) -> Result<PackageTarget>
where
    C: AsRef<str>,
{
    resolve_package_imports_inner(package, specifier, conditions)
        .map_err(|error| package_error_context(error, package, "imports"))
}

fn resolve_package_imports_inner<C>(
    package: &PackageInfo,
    specifier: &str,
    conditions: &[C],
) -> Result<PackageTarget>
where
    C: AsRef<str>,
{
    let Some(imports) = package.imports.as_ref() else {
        return MissingTarget::Import.missing(specifier);
    };
    let Value::Object(entries) = imports else {
        return invalid_package_config("imports must be an object");
    };

    let Some((target, pattern_match)) = find_subpath_match(entries, specifier) else {
        return MissingTarget::Import.missing(specifier);
    };

    resolve_target_value(
        target,
        conditions,
        pattern_match,
        specifier,
        MissingTarget::Import,
    )
}

fn resolve_exports_object<C>(
    entries: &Map<String, Value>,
    package_subpath: &str,
    conditions: &[C],
) -> Result<PackageTarget>
where
    C: AsRef<str>,
{
    let has_subpath_key = entries.keys().any(|key| key.starts_with('.'));
    let has_condition_key = entries.keys().any(|key| !key.starts_with('.'));
    if has_subpath_key && has_condition_key {
        return invalid_package_config("exports cannot mix condition keys and subpath keys");
    }

    if !has_subpath_key {
        if package_subpath != "." {
            return MissingTarget::Export.missing(package_subpath);
        }
        return resolve_condition_object(
            entries,
            conditions,
            None,
            package_subpath,
            MissingTarget::Export,
        );
    }

    let Some((target, pattern_match)) = find_subpath_match(entries, package_subpath) else {
        return MissingTarget::Export.missing(package_subpath);
    };

    resolve_target_value(
        target,
        conditions,
        pattern_match,
        package_subpath,
        MissingTarget::Export,
    )
}

fn resolve_target_value<C>(
    value: &Value,
    conditions: &[C],
    pattern_match: Option<&str>,
    request: &str,
    missing_target: MissingTarget,
) -> Result<PackageTarget>
where
    C: AsRef<str>,
{
    match value {
        Value::String(target) => resolve_string_target(target, pattern_match),
        Value::Object(entries) => {
            resolve_condition_object(entries, conditions, pattern_match, request, missing_target)
        }
        Value::Null => missing_target.missing(request),
        Value::Array(_) => invalid_package_target_shape("package target arrays are not supported"),
        _ => {
            invalid_package_target_shape("package target must be a string, object, array, or null")
        }
    }
}

fn resolve_condition_object<C>(
    entries: &Map<String, Value>,
    conditions: &[C],
    pattern_match: Option<&str>,
    request: &str,
    missing_target: MissingTarget,
) -> Result<PackageTarget>
where
    C: AsRef<str>,
{
    for condition in conditions {
        if let Some(target) = entries.get(condition.as_ref()) {
            return resolve_target_value(
                target,
                conditions,
                pattern_match,
                request,
                missing_target,
            );
        }
    }

    missing_target.missing(request)
}

fn find_subpath_match<'a, 'b>(
    entries: &'a Map<String, Value>,
    request: &'b str,
) -> Option<(&'a Value, Option<&'b str>)> {
    if let Some(target) = entries.get(request) {
        return Some((target, None));
    }

    let mut candidates = Vec::new();
    for (index, (key, target)) in entries.iter().enumerate() {
        let Some((prefix, suffix)) = split_single_pattern(key) else {
            continue;
        };
        if request.len() < prefix.len() + suffix.len() {
            continue;
        }
        if request.starts_with(prefix) && request.ends_with(suffix) {
            let capture = &request[prefix.len()..request.len() - suffix.len()];
            candidates.push((prefix.len(), index, target, capture));
        }
    }

    candidates.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    candidates
        .into_iter()
        .next()
        .map(|(_, _, target, capture)| (target, Some(capture)))
}

fn split_single_pattern(pattern: &str) -> Option<(&str, &str)> {
    let star = pattern.find('*')?;
    if pattern[star + 1..].contains('*') {
        return None;
    }
    Some((&pattern[..star], &pattern[star + 1..]))
}

fn resolve_string_target(target: &str, pattern_match: Option<&str>) -> Result<PackageTarget> {
    let target = match pattern_match {
        Some(capture) => target.replace('*', capture),
        None => target.to_string(),
    };
    validate_package_target(&target)?;

    Ok(PackageTarget {
        relative_path: target
            .strip_prefix("./")
            .expect("target was validated")
            .to_string(),
    })
}

fn validate_package_target(target: &str) -> Result<()> {
    if target.starts_with('/') || target.starts_with('\\') {
        return invalid_package_target(target, "absolute paths are not allowed");
    }
    if target.contains('\\') {
        return invalid_package_target(
            target,
            "backslashes are not allowed; use `/` path separators",
        );
    }
    if is_url_like(target) {
        return invalid_package_target(target, "URL-like targets are not allowed");
    }
    if !target.starts_with("./") {
        return invalid_package_target(target, "target must start with `./`");
    }

    for segment in target[2..].split('/') {
        if segment == ".." {
            return invalid_package_target(target, "parent traversal is not allowed");
        }
        if segment == "node_modules" {
            return invalid_package_target(target, "node_modules segments are not allowed");
        }
    }

    Ok(())
}

fn is_url_like(target: &str) -> bool {
    let Some(colon) = target.find(':') else {
        return false;
    };
    let first_slash = target.find('/').unwrap_or(usize::MAX);
    let first_backslash = target.find('\\').unwrap_or(usize::MAX);
    if colon > first_slash.min(first_backslash) {
        return false;
    }

    let scheme = &target[..colon];
    scheme
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic())
        && scheme.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.')
        })
}

fn invalid_package_config<T>(reason: &str) -> Result<T> {
    bail!("ERR_INVALID_PACKAGE_CONFIG: {reason}")
}

fn invalid_package_target<T>(target: &str, reason: &str) -> Result<T> {
    bail!("ERR_INVALID_PACKAGE_TARGET: invalid package target `{target}`: {reason}")
}

fn invalid_package_target_shape<T>(reason: &str) -> Result<T> {
    bail!("ERR_INVALID_PACKAGE_TARGET: {reason}")
}

#[cfg(test)]
mod tests {
    use super::{resolve_package_exports, resolve_package_imports};
    use crate::package_json::{PackageInfo, PackageType};

    use std::path::PathBuf;

    use serde_json::{Value, json};

    fn package_with(exports: Option<Value>, imports: Option<Value>) -> PackageInfo {
        PackageInfo {
            root: PathBuf::from("/pkg"),
            path: PathBuf::from("/pkg/package.json"),
            name: Some("pkg".to_string()),
            package_type: PackageType::CommonJs,
            module: None,
            main: None,
            exports,
            imports,
            browser: None,
        }
    }

    fn assert_error_contains(error: anyhow::Error, expected: &str) {
        assert_error_contains_all(error, &[expected]);
    }

    fn assert_error_contains_all(error: anyhow::Error, expected: &[&str]) {
        let message = error.to_string();
        for expected in expected {
            assert!(
                message.contains(expected),
                "expected `{message}` to contain `{expected}`"
            );
        }
    }

    #[test]
    fn exports_string_resolves_main() {
        let package = package_with(Some(json!("./dist/index.js")), None);

        let target = resolve_package_exports(&package, ".", &["wjsm", "node", "import", "default"])
            .expect("main export resolves");

        assert_eq!(target.relative_path, "dist/index.js");
    }

    #[test]
    fn exports_condition_prefers_wjsm_then_node_then_import_then_default() {
        let conditions = ["wjsm", "node", "import", "default"];
        let cases = [
            (
                json!({
                    ".": {
                        "default": "./default.js",
                        "import": "./import.js",
                        "node": "./node.js",
                        "wjsm": "./wjsm.js"
                    }
                }),
                "wjsm.js",
            ),
            (
                json!({
                    ".": {
                        "default": "./default.js",
                        "import": "./import.js",
                        "node": "./node.js"
                    }
                }),
                "node.js",
            ),
            (
                json!({
                    ".": {
                        "default": "./default.js",
                        "import": "./import.js"
                    }
                }),
                "import.js",
            ),
            (
                json!({
                    ".": {
                        "default": "./default.js"
                    }
                }),
                "default.js",
            ),
        ];

        for (exports, expected) in cases {
            let package = package_with(Some(exports), None);
            let target = resolve_package_exports(&package, ".", &conditions)
                .expect("conditional export resolves");

            assert_eq!(target.relative_path, expected);
        }
    }

    #[test]
    fn exports_condition_prefers_require_for_require_edges() {
        let package = package_with(
            Some(json!({
                ".": {
                    "default": "./default.js",
                    "import": "./esm.js",
                    "require": "./cjs.js"
                }
            })),
            None,
        );

        let target =
            resolve_package_exports(&package, ".", &["wjsm", "node", "require", "default"])
                .expect("require condition resolves");

        assert_eq!(target.relative_path, "cjs.js");
    }
    #[test]
    fn exports_nested_condition_object_resolves() {
        let package = package_with(
            Some(json!({
                ".": {
                    "node": {
                        "import": "./nested-import.js"
                    },
                    "default": "./default.js"
                }
            })),
            None,
        );

        let target = resolve_package_exports(&package, ".", &["wjsm", "node", "import", "default"])
            .expect("nested conditional export resolves");

        assert_eq!(target.relative_path, "nested-import.js");
    }

    #[test]
    fn exports_subpath_resolves_exact_key() {
        let package = package_with(
            Some(json!({
                ".": "./index.js",
                "./feature": "./src/feature.js"
            })),
            None,
        );

        let target = resolve_package_exports(&package, "./feature", &["default"])
            .expect("subpath export resolves");

        assert_eq!(target.relative_path, "src/feature.js");
    }
    #[test]
    fn exports_bare_subpath_is_normalized() {
        let package = package_with(
            Some(json!({
                ".": "./index.js",
                "./feature": "./src/feature.js"
            })),
            None,
        );

        let target = resolve_package_exports(&package, "feature", &["default"])
            .expect("bare subpath export resolves");

        assert_eq!(target.relative_path, "src/feature.js");
    }

    #[test]
    fn exports_pattern_replaces_star() {
        let package = package_with(
            Some(json!({
                "./features/*": "./src/features/*.js"
            })),
            None,
        );

        let target = resolve_package_exports(&package, "./features/foo/bar", &["default"])
            .expect("pattern export resolves");

        assert_eq!(target.relative_path, "src/features/foo/bar.js");
    }
    #[test]
    fn exports_pattern_longest_prefix_wins() {
        let package = package_with(
            Some(json!({
                "./features/*": "./src/features/*.js",
                "./features/internal/*": "./src/internal/*.js"
            })),
            None,
        );

        let target = resolve_package_exports(&package, "./features/internal/cache", &["default"])
            .expect("longest prefix pattern resolves");

        assert_eq!(target.relative_path, "src/internal/cache.js");
    }

    #[test]
    fn exports_pattern_same_prefix_uses_manifest_order() {
        let package = package_with(
            Some(json!({
                "./pkg/*.js": "./src/extension/*.js",
                "./pkg/*": "./src/wildcard/*.js"
            })),
            None,
        );

        let target = resolve_package_exports(&package, "./pkg/tool.js", &["default"])
            .expect("same-prefix pattern resolves by manifest order");

        assert_eq!(target.relative_path, "src/extension/tool.js");
    }

    #[test]
    fn exports_null_reports_not_exported() {
        let package = package_with(
            Some(json!({
                "./hidden": null
            })),
            None,
        );

        let error = resolve_package_exports(&package, "./hidden", &["default"])
            .expect_err("null export should be blocked");

        assert_error_contains_all(
            error,
            &[
                "ERR_PACKAGE_PATH_NOT_EXPORTED",
                "/pkg/package.json",
                "pkg",
                "exports",
            ],
        );
    }

    #[test]
    fn exports_rejects_absolute_target() {
        let package = package_with(Some(json!("/abs.js")), None);

        let error = resolve_package_exports(&package, ".", &["default"])
            .expect_err("absolute target should be rejected");

        assert_error_contains_all(
            error,
            &[
                "ERR_INVALID_PACKAGE_TARGET",
                "/pkg/package.json",
                "pkg",
                "exports",
            ],
        );
    }
    #[test]
    fn exports_rejects_array_target() {
        let package = package_with(Some(json!(["./fallback.js"])), None);

        let error = resolve_package_exports(&package, ".", &["default"])
            .expect_err("array targets should be rejected");

        assert_error_contains(error, "ERR_INVALID_PACKAGE_TARGET");
    }

    #[test]
    fn exports_rejects_url_like_target() {
        let package = package_with(Some(json!("https://example.com/index.js")), None);

        let error = resolve_package_exports(&package, ".", &["default"])
            .expect_err("URL-like target should be rejected");

        assert_error_contains(error, "ERR_INVALID_PACKAGE_TARGET");
    }

    #[test]
    fn exports_rejects_parent_traversal() {
        let package = package_with(Some(json!("./src/../secret.js")), None);

        let error = resolve_package_exports(&package, ".", &["default"])
            .expect_err("parent traversal should be rejected");

        assert_error_contains(error, "ERR_INVALID_PACKAGE_TARGET");
    }
    #[test]
    fn exports_rejects_node_modules_segment() {
        let package = package_with(Some(json!("./node_modules/dep/index.js")), None);

        let error = resolve_package_exports(&package, ".", &["default"])
            .expect_err("node_modules segment should be rejected");

        assert_error_contains(error, "ERR_INVALID_PACKAGE_TARGET");
    }

    #[test]
    fn exports_rejects_backslash_targets() {
        for target in ["./x\\..\\y.js", "./node_modules\\dep.js"] {
            let package = package_with(Some(json!(target)), None);

            let error = resolve_package_exports(&package, ".", &["default"])
                .expect_err("backslash target should be rejected");

            assert_error_contains(error, "ERR_INVALID_PACKAGE_TARGET");
        }
    }

    #[test]
    fn exports_rejects_mixed_condition_and_subpath_keys() {
        let package = package_with(
            Some(json!({
                ".": "./index.js",
                "default": "./default.js"
            })),
            None,
        );

        let error = resolve_package_exports(&package, ".", &["default"])
            .expect_err("mixed keys should be invalid");

        assert_error_contains_all(
            error,
            &[
                "ERR_INVALID_PACKAGE_CONFIG",
                "/pkg/package.json",
                "pkg",
                "exports",
            ],
        );
    }

    #[test]
    fn imports_hash_alias_resolves() {
        let package = package_with(
            None,
            Some(json!({
                "#dep": "./src/dep.js"
            })),
        );

        let target =
            resolve_package_imports(&package, "#dep", &["wjsm", "node", "import", "default"])
                .expect("hash alias resolves");

        assert_eq!(target.relative_path, "src/dep.js");
    }

    #[test]
    fn imports_hash_pattern_resolves() {
        let package = package_with(
            None,
            Some(json!({
                "#lib/*": "./src/lib/*.js"
            })),
        );

        let target = resolve_package_imports(&package, "#lib/util", &["default"])
            .expect("hash pattern resolves");

        assert_eq!(target.relative_path, "src/lib/util.js");
    }

    #[test]
    fn imports_missing_reports_import_not_defined() {
        let package = package_with(
            None,
            Some(json!({
                "#dep": "./src/dep.js"
            })),
        );

        let error = resolve_package_imports(&package, "#missing", &["default"])
            .expect_err("missing import should report not defined");

        assert_error_contains_all(
            error,
            &[
                "ERR_PACKAGE_IMPORT_NOT_DEFINED",
                "/pkg/package.json",
                "pkg",
                "imports",
            ],
        );
    }
    #[test]
    fn imports_null_reports_import_not_defined() {
        let package = package_with(
            None,
            Some(json!({
                "#dep": null
            })),
        );

        let error = resolve_package_imports(&package, "#dep", &["default"])
            .expect_err("null import should report not defined");

        assert_error_contains(error, "ERR_PACKAGE_IMPORT_NOT_DEFINED");
    }
}
