use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};
use url::Url;

use crate::builtin_modules::{self, BuiltinLookup};
use crate::module_format::{ModuleFormat, detect_module_format};
use crate::resolution_options::{ResolutionKind, ResolutionOptions};
use crate::resolver::{ModuleResolver, ResolvedSpecifier};

/// 运行时解析入口类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuntimeResolveKind {
    Import,
    Require,
}

impl From<RuntimeResolveKind> for ResolutionKind {
    fn from(kind: RuntimeResolveKind) -> Self {
        match kind {
            RuntimeResolveKind::Import => Self::Import,
            RuntimeResolveKind::Require => Self::Require,
        }
    }
}

/// 运行时模块规范身份。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RuntimeModuleKey {
    File(PathBuf),
    Json(PathBuf),
    Builtin(String),
}

/// 运行时加载器可直接消费的模块格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuntimeModuleFormat {
    Esm,
    CommonJs,
    Json,
    Builtin,
}

/// 运行时解析后的普通 DTO，不携带 AST 或编译期 ModuleId。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeResolvedModule {
    pub key: RuntimeModuleKey,
    pub path: Option<PathBuf>,
    pub url: String,
    pub format: RuntimeModuleFormat,
}

/// `require.resolve.paths()` 的运行时返回形态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeResolvePaths {
    Null,
    Search(Vec<PathBuf>),
}

/// 按现有 package/path resolver 语义解析运行时 specifier。
pub fn resolve_runtime_specifier(
    specifier: &str,
    referrer_path: &Path,
    root_path: &Path,
    options: &ResolutionOptions,
    kind: RuntimeResolveKind,
) -> Result<RuntimeResolvedModule> {
    let root = canonical_root(root_path);
    let resolver = ModuleResolver::with_options(&root, options.clone());
    match resolver.resolve_specifier_with_kind(specifier, referrer_path, kind.into())? {
        ResolvedSpecifier::Builtin(module) => Ok(resolve_builtin_module(module.canonical)),
        ResolvedSpecifier::Path(path) => resolve_file_module(&resolver, specifier, &root, path),
    }
}

/// 返回 bare package 从 referrer 起可搜索的 `node_modules` 目录。
pub fn resolve_runtime_paths(
    specifier: &str,
    referrer_path: &Path,
    root_path: &Path,
) -> RuntimeResolvePaths {
    if !uses_node_modules_search_paths(specifier) {
        return RuntimeResolvePaths::Null;
    }

    let root = canonical_root(root_path);
    let mut dir = referrer_path
        .parent()
        .unwrap_or(referrer_path)
        .canonicalize()
        .unwrap_or_else(|_| referrer_path.parent().unwrap_or(referrer_path).to_path_buf());
    let mut paths = Vec::new();

    loop {
        if !dir.starts_with(&root) {
            break;
        }
        paths.push(dir.join("node_modules"));
        if dir == root || !dir.pop() {
            break;
        }
    }

    RuntimeResolvePaths::Search(paths)
}

fn resolve_builtin_module(canonical: &str) -> RuntimeResolvedModule {
    let key = format!("node:{canonical}");
    RuntimeResolvedModule {
        key: RuntimeModuleKey::Builtin(key.clone()),
        path: None,
        url: key,
        format: RuntimeModuleFormat::Builtin,
    }
}

fn resolve_file_module(
    resolver: &ModuleResolver,
    specifier: &str,
    root: &Path,
    path: PathBuf,
) -> Result<RuntimeResolvedModule> {
    if !path.starts_with(root) {
        bail!(
            "Module '{}' resolves outside root '{}': {}",
            specifier,
            root.display(),
            path.display()
        );
    }

    let format = runtime_module_format(resolver, &path)?;
    let key = match format {
        RuntimeModuleFormat::Json => RuntimeModuleKey::Json(path.clone()),
        RuntimeModuleFormat::Esm | RuntimeModuleFormat::CommonJs => {
            RuntimeModuleKey::File(path.clone())
        }
        RuntimeModuleFormat::Builtin => unreachable!("builtins do not have filesystem paths"),
    };

    Ok(RuntimeResolvedModule {
        key,
        url: file_url(&path)?,
        path: Some(path),
        format,
    })
}

fn runtime_module_format(resolver: &ModuleResolver, path: &Path) -> Result<RuntimeModuleFormat> {
    if path.extension().and_then(|extension| extension.to_str()) == Some("json") {
        return Ok(RuntimeModuleFormat::Json);
    }

    let package = resolver.find_nearest_package(path)?;
    Ok(match detect_module_format(path, package.as_ref(), false) {
        ModuleFormat::Esm => RuntimeModuleFormat::Esm,
        ModuleFormat::CommonJs => RuntimeModuleFormat::CommonJs,
    })
}

fn file_url(path: &Path) -> Result<String> {
    Url::from_file_path(path)
        .map(|url| url.to_string())
        .map_err(|()| anyhow!("cannot build file URL for {}", path.display()))
}

fn canonical_root(root_path: &Path) -> PathBuf {
    root_path
        .canonicalize()
        .unwrap_or_else(|_| root_path.to_path_buf())
}

fn uses_node_modules_search_paths(specifier: &str) -> bool {
    if specifier.starts_with('#') || Path::new(specifier).is_absolute() {
        return false;
    }
    if !ModuleResolver::is_bare_specifier(specifier) {
        return false;
    }
    matches!(builtin_modules::lookup(specifier), BuiltinLookup::NotBuiltin)
}

