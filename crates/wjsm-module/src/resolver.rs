// 模块解析器：文件系统模块解析、import/export 提取

use anyhow::{Context, Result, bail};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use swc_core::common::Span;
use swc_core::ecma::ast;

use crate::builtin_modules::{self, BuiltinLookup};
use crate::exports::{resolve_package_exports, resolve_package_imports};
use crate::module_format::{ModuleFormat, detect_module_format};
use crate::package_json::{self, BrowserField, PackageInfo};
use crate::resolution_options::{ResolutionKind, ResolutionOptions};
/// 尝试作为模块入口解析的路径扩展名（顺序优先）
const MODULE_EXTENSIONS: &[&str] = &["js", "ts", "mjs", "cjs", "jsx", "tsx"];

pub use wjsm_ir::ModuleId;

/// 解析后的模块信息
#[derive(Debug)]
pub struct ResolvedModule {
    pub id: ModuleId,
    pub source: String,
    pub path: PathBuf,
    pub ast: ast::Module,
    pub imports: Vec<ImportEntry>,
    pub exports: Vec<ExportEntry>,
    /// 动态 import() 的 specifier 列表（不合并进 imports）
    pub dynamic_imports: Vec<String>,
    pub is_cjs: bool,
}

/// Import 声明条目
#[derive(Debug, Clone)]
pub struct ImportEntry {
    /// 模块说明符（如 './foo'）
    pub specifier: String,
    /// 导入的名称列表：(local_name, imported_name)
    /// - `import { x } from './foo'` → ("x", "x")
    /// - `import { y as z } from './foo'` → ("z", "y")
    /// - `import * as ns from './foo'` → ("ns", "*")
    /// - `import defaultExport from './foo'` → ("defaultExport", "default")
    pub names: Vec<(String, String)>,
    pub source_span: Span,
}

/// Export 声明条目
#[derive(Debug, Clone)]
pub enum ExportEntry {
    /// export { x } / export { x as y }
    Named { local: String, exported: String },
    /// export default expr
    Default {
        local: String, // 内部生成的变量名
    },
    /// export * from './foo'
    All { source: String },
    /// export { x, y } from './foo' — 命名重导出
    NamedReExport {
        local: String,
        exported: String,
        source: String,
    },
    /// export const/let/var/function/class
    Declaration { name: String },
}

/// 模块解析器
pub struct ModuleResolver {
    root_path: PathBuf,
    options: ResolutionOptions,
    package_cache: RefCell<HashMap<PathBuf, Option<PackageInfo>>>,
    next_id: u32,
    visited: HashMap<PathBuf, ModuleId>,
    modules: HashMap<ModuleId, ResolvedModule>,
}

pub(crate) enum ResolvedSpecifier {
    Builtin(&'static builtin_modules::BuiltinModule),
    Path(PathBuf),
}

impl ModuleResolver {
    pub fn new(root_path: &Path) -> Self {
        Self::with_options(root_path, ResolutionOptions::default())
    }

    pub(crate) fn with_options(root_path: &Path, options: ResolutionOptions) -> Self {
        let root_path = root_path
            .canonicalize()
            .unwrap_or_else(|_| root_path.to_path_buf());
        Self {
            root_path,
            options,
            package_cache: RefCell::new(HashMap::new()),
            next_id: 0,
            visited: HashMap::new(),
            modules: HashMap::new(),
        }
    }

    /// 解析模块路径（相对路径、目录 index、node_modules bare specifier）
    pub fn resolve_path(specifier: &str, parent: &Path) -> Result<PathBuf> {
        let root = Self::path_resolution_root(parent);
        let resolver = Self::new(&root);
        match resolver.resolve_specifier(specifier, parent)? {
            ResolvedSpecifier::Builtin(module) => {
                Ok(builtin_modules::virtual_path(module.canonical))
            }
            ResolvedSpecifier::Path(path) => Ok(path),
        }
    }

    fn path_resolution_root(parent: &Path) -> PathBuf {
        let start = parent.parent().unwrap_or(parent);
        start
            .ancestors()
            .last()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(start)
            .to_path_buf()
    }

    fn resolve_specifier(&self, specifier: &str, parent: &Path) -> Result<ResolvedSpecifier> {
        self.resolve_specifier_with_conditions(specifier, parent, self.options.conditions())
    }

    pub(crate) fn resolve_specifier_with_kind(
        &self,
        specifier: &str,
        parent: &Path,
        kind: ResolutionKind,
    ) -> Result<ResolvedSpecifier> {
        self.resolve_specifier_with_conditions(
            specifier,
            parent,
            self.options.conditions_for_kind(kind),
        )
    }

    fn resolve_specifier_with_conditions(
        &self,
        specifier: &str,
        parent: &Path,
        conditions: &[String],
    ) -> Result<ResolvedSpecifier> {
        match builtin_modules::lookup(specifier) {
            BuiltinLookup::Found(module) => Ok(ResolvedSpecifier::Builtin(module)),
            BuiltinLookup::UnknownNodeBuiltin(name) => {
                bail!("Unknown built-in module 'node:{}'", name)
            }
            BuiltinLookup::NotBuiltin => self
                .resolve_path_non_builtin(specifier, parent, conditions)
                .map(ResolvedSpecifier::Path),
        }
    }

    fn resolve_path_non_builtin(
        &self,
        specifier: &str,
        parent: &Path,
        conditions: &[String],
    ) -> Result<PathBuf> {
        if specifier.starts_with('/') {
            bail!(
                "Module specifier '{}' is not supported. Absolute path imports are not supported.",
                specifier
            );
        }

        if specifier.starts_with('#') {
            return self.resolve_imports_specifier(specifier, parent, conditions);
        }

        if Self::is_bare_specifier(specifier) {
            return self.resolve_bare_specifier(specifier, parent, conditions);
        }

        let base = parent.parent().unwrap_or(parent);
        let resolved = base.join(specifier);

        self.resolve_mapped_file_or_directory(&resolved, specifier, parent)
    }

    pub(crate) fn is_bare_specifier(specifier: &str) -> bool {
        !specifier.starts_with('.')
    }

    /// 将 bare specifier 拆为 npm 包名与包内子路径（若有）
    fn split_npm_specifier(specifier: &str) -> (String, Option<String>) {
        if let Some(rest) = specifier.strip_prefix('@') {
            let mut parts = rest.split('/');
            let scope = parts.next().unwrap_or("");
            let name = parts.next();
            match name {
                Some(n) => {
                    let pkg = format!("@{scope}/{n}");
                    let sub: String = parts.collect::<Vec<_>>().join("/");
                    (pkg, if sub.is_empty() { None } else { Some(sub) })
                }
                None => (format!("@{scope}"), None),
            }
        } else {
            match specifier.split_once('/') {
                Some((pkg, sub)) => (pkg.to_string(), Some(sub.to_string())),
                None => (specifier.to_string(), None),
            }
        }
    }

    fn resolve_imports_specifier(
        &self,
        specifier: &str,
        parent: &Path,
        conditions: &[String],
    ) -> Result<PathBuf> {
        let Some(package) = self.find_nearest_package(parent).with_context(|| {
            format!(
                "resolve package import `{specifier}` from {}",
                parent.display()
            )
        })?
        else {
            bail!(
                "ERR_PACKAGE_IMPORT_NOT_DEFINED: package import `{specifier}` is not defined from {}",
                parent.display()
            )
        };

        let target =
            resolve_package_imports(&package, specifier, conditions).with_context(|| {
                format!(
                    "resolve package import `{specifier}` from {}",
                    parent.display()
                )
            })?;
        let target_path = package.root.join(target.relative_path);
        Self::resolve_package_target_path(&target_path, specifier, parent)
    }

    fn resolve_bare_specifier(
        &self,
        specifier: &str,
        parent: &Path,
        conditions: &[String],
    ) -> Result<PathBuf> {
        let (package_name, subpath) = Self::split_npm_specifier(specifier);
        if let Some(package) = self.find_nearest_package(parent)?
            && package.exports.is_some()
            && package.name.as_deref() == Some(package_name.as_str())
        {
            return self.resolve_package_specifier(
                &package.root,
                Some(&package),
                subpath.as_deref(),
                specifier,
                parent,
                conditions,
            );
        }

        let start_dir = parent.parent().unwrap_or(parent);
        let package_dir = self
            .find_package_in_node_modules(&package_name, start_dir)?
            .ok_or_else(|| anyhow::anyhow!("Cannot find module '{}'", specifier))?;
        let package_info = self.read_package_info(&package_dir)?;

        self.resolve_package_specifier(
            &package_dir,
            package_info.as_ref(),
            subpath.as_deref(),
            specifier,
            parent,
            conditions,
        )
    }

    fn resolve_package_specifier(
        &self,
        package_dir: &Path,
        package_info: Option<&PackageInfo>,
        subpath: Option<&str>,
        specifier: &str,
        parent: &Path,
        conditions: &[String],
    ) -> Result<PathBuf> {
        if let Some(package) = package_info
            && package.exports.is_some()
        {
            return self
                .resolve_package_exports_target(package, subpath, specifier, parent, conditions);
        }

        let package_root = package_info
            .map(|package| package.root.as_path())
            .unwrap_or(package_dir);
        if let Some(subpath) = subpath {
            let target = package_root.join(subpath);
            return self.resolve_file_or_directory(&target, specifier, parent);
        }

        self.resolve_legacy_package_entry(package_root, package_info, specifier, parent)
    }

    fn resolve_package_exports_target(
        &self,
        package: &PackageInfo,
        subpath: Option<&str>,
        specifier: &str,
        parent: &Path,
        conditions: &[String],
    ) -> Result<PathBuf> {
        let package_subpath = subpath
            .map(|subpath| format!("./{subpath}"))
            .unwrap_or_else(|| ".".to_string());
        let target = resolve_package_exports(package, &package_subpath, conditions).with_context(|| {
            format!(
                "resolve package export `{package_subpath}` for specifier `{specifier}` from {}",
                parent.display()
            )
        })?;
        let target_path = package.root.join(target.relative_path);
        Self::resolve_package_target_path(&target_path, specifier, parent)
    }

    /// 从 `from_dir` 起向上遍历，查找 `node_modules/<package_name>` 目录。
    fn find_package_in_node_modules(
        &self,
        package_name: &str,
        from_dir: &Path,
    ) -> Result<Option<PathBuf>> {
        let mut dir = from_dir
            .canonicalize()
            .unwrap_or_else(|_| from_dir.to_path_buf());
        loop {
            if !dir.starts_with(&self.root_path) {
                return Ok(None);
            }

            let candidate = dir.join("node_modules").join(package_name);
            if candidate.is_dir() {
                return candidate.canonicalize().map(Some).with_context(|| {
                    format!("canonicalize package directory {}", candidate.display())
                });
            }
            if dir == self.root_path || !dir.pop() {
                return Ok(None);
            }
        }
    }

    pub(crate) fn find_nearest_package(&self, start: &Path) -> Result<Option<PackageInfo>> {
        let start_dir = if start.is_dir() {
            start
        } else {
            start.parent().unwrap_or(start)
        };
        let mut current = start_dir.canonicalize().with_context(|| {
            format!("canonicalize package search start {}", start_dir.display())
        })?;

        loop {
            if !current.starts_with(&self.root_path) {
                return Ok(None);
            }

            if let Some(package) = self.read_package_info(&current)? {
                return Ok(Some(package));
            }

            if current == self.root_path || !current.pop() {
                return Ok(None);
            }
        }
    }

    fn read_package_info(&self, package_dir: &Path) -> Result<Option<PackageInfo>> {
        let canonical_dir = package_dir
            .canonicalize()
            .with_context(|| format!("canonicalize package directory {}", package_dir.display()))?;
        if let Some(package) = self.package_cache.borrow().get(&canonical_dir) {
            return Ok(package.clone());
        }

        let package = package_json::read_package_info(&canonical_dir)?;
        self.package_cache
            .borrow_mut()
            .insert(canonical_dir, package.clone());
        Ok(package)
    }

    /// 解析包目录入口（package.json 的 module/main，或 index.*）
    fn resolve_legacy_package_entry(
        &self,
        package_dir: &Path,
        package_info: Option<&PackageInfo>,
        specifier: &str,
        parent: &Path,
    ) -> Result<PathBuf> {
        if let Some(package) = package_info {
            let browser_entry = if self.options.browser() {
                match package.browser.as_ref() {
                    Some(BrowserField::Entry(entry)) => Some(entry.as_str()),
                    _ => None,
                }
            } else {
                None
            };
            for entry in [
                browser_entry,
                package.module.as_deref(),
                package.main.as_deref(),
            ]
            .into_iter()
            .flatten()
            {
                let entry_path = package.root.join(entry);
                if self.options.browser()
                    && let Some(path) = self.resolve_browser_mapping_for_package(
                        package,
                        &entry_path,
                        specifier,
                        parent,
                    )?
                {
                    return Ok(path);
                }
                if let Ok(path) = self.resolve_existing_module_path(&entry_path, specifier, parent)
                {
                    if self.options.browser()
                        && let Some(mapped_path) = self.resolve_browser_mapping_for_package(
                            package, &path, specifier, parent,
                        )?
                    {
                        return Ok(mapped_path);
                    }
                    return Ok(path);
                }
            }
        }

        Self::resolve_directory_index(package_dir, specifier, parent)
    }

    /// 将路径解析为具体模块文件：扩展名补全、目录 index、package.json main/module
    fn resolve_file_or_directory(
        &self,
        resolved: &Path,
        specifier: &str,
        parent: &Path,
    ) -> Result<PathBuf> {
        if resolved.is_file() {
            return Ok(resolved.canonicalize()?);
        }
        if resolved.is_dir() {
            let package_info = self.read_package_info(resolved)?;
            return self.resolve_legacy_package_entry(
                resolved,
                package_info.as_ref(),
                specifier,
                parent,
            );
        }

        let file_candidates = Self::file_candidates(resolved);
        for candidate in &file_candidates {
            if candidate.is_file() {
                return Ok(candidate.canonicalize()?);
            }
        }

        for candidate in &file_candidates {
            if candidate.is_dir() {
                let package_info = self.read_package_info(candidate)?;
                return self.resolve_legacy_package_entry(
                    candidate,
                    package_info.as_ref(),
                    specifier,
                    parent,
                );
            }
        }

        bail!(
            "Cannot find module '{}' from '{}'. Tried: {:?}",
            specifier,
            parent.display(),
            file_candidates
        );
    }

    fn resolve_mapped_file_or_directory(
        &self,
        resolved: &Path,
        specifier: &str,
        parent: &Path,
    ) -> Result<PathBuf> {
        if self.options.browser()
            && let Some(path) = self.resolve_browser_mapping(parent, resolved, specifier)?
        {
            return Ok(path);
        }

        let path = self.resolve_file_or_directory(resolved, specifier, parent)?;
        if self.options.browser()
            && let Some(path) = self.resolve_browser_mapping(parent, &path, specifier)?
        {
            return Ok(path);
        }

        Ok(path)
    }

    fn resolve_browser_mapping(
        &self,
        parent: &Path,
        requested_path: &Path,
        specifier: &str,
    ) -> Result<Option<PathBuf>> {
        let Some(package) = self.find_nearest_package(parent)? else {
            return Ok(None);
        };
        self.resolve_browser_mapping_for_package(&package, requested_path, specifier, parent)
    }

    fn resolve_browser_mapping_for_package(
        &self,
        package: &PackageInfo,
        requested_path: &Path,
        specifier: &str,
        parent: &Path,
    ) -> Result<Option<PathBuf>> {
        let Some(BrowserField::Map(map)) = package.browser.as_ref() else {
            return Ok(None);
        };
        let Some(key) = Self::browser_map_key(package, requested_path) else {
            return Ok(None);
        };
        let Some(replacement) = map.get(&key) else {
            return Ok(None);
        };

        match replacement {
            Some(replacement) => {
                let target_path = Self::join_package_relative(&package.root, replacement);
                Self::resolve_package_target_path(&target_path, specifier, parent).map(Some)
            }
            None => bail!(
                "ERR_PACKAGE_PATH_DISABLED_BY_BROWSER: package path `{key}` is disabled by browser field in {}",
                package.path.display()
            ),
        }
    }

    fn browser_map_key(package: &PackageInfo, path: &Path) -> Option<String> {
        let root = Self::normalize_path_lexically(&package.root);
        let path = Self::normalize_path_lexically(path);
        let relative = path.strip_prefix(root).ok()?;
        if relative.as_os_str().is_empty() {
            return None;
        }
        let relative = relative.to_str()?.replace('\\', "/");
        Some(format!("./{relative}"))
    }

    fn join_package_relative(package_root: &Path, target: &str) -> PathBuf {
        let target = target.strip_prefix("./").unwrap_or(target);
        package_root.join(target)
    }

    fn normalize_path_lexically(path: &Path) -> PathBuf {
        let mut normalized = PathBuf::new();
        for component in path.components() {
            match component {
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => {
                    normalized.pop();
                }
                component => normalized.push(component.as_os_str()),
            }
        }
        normalized
    }

    fn resolve_existing_module_path(
        &self,
        path: &Path,
        specifier: &str,
        parent: &Path,
    ) -> Result<PathBuf> {
        if path.is_file() {
            return Ok(path.canonicalize()?);
        }
        self.resolve_file_or_directory(path, specifier, parent)
    }

    fn resolve_package_target_path(path: &Path, specifier: &str, parent: &Path) -> Result<PathBuf> {
        if path.is_file() {
            return Ok(path.canonicalize()?);
        }
        if path.is_dir() {
            return Self::resolve_directory_index(path, specifier, parent);
        }

        let file_candidates = Self::file_candidates(path);
        for candidate in &file_candidates {
            if candidate.is_file() {
                return Ok(candidate.canonicalize()?);
            }
        }

        for candidate in &file_candidates {
            if candidate.is_dir() {
                return Self::resolve_directory_index(candidate, specifier, parent);
            }
        }

        bail!(
            "Cannot find module '{}' from '{}'. Tried: {:?}",
            specifier,
            parent.display(),
            file_candidates
        );
    }

    fn resolve_directory_index(dir: &Path, specifier: &str, parent: &Path) -> Result<PathBuf> {
        for ext in MODULE_EXTENSIONS {
            let index = dir.join(format!("index.{ext}"));
            if index.is_file() {
                return Ok(index.canonicalize()?);
            }
        }
        bail!(
            "Cannot find module '{}' from '{}'. No index file in directory '{}'",
            specifier,
            parent.display(),
            dir.display()
        );
    }

    fn file_candidates(resolved: &Path) -> Vec<PathBuf> {
        if resolved.extension().is_some() {
            vec![resolved.to_path_buf()]
        } else {
            let mut candidates: Vec<PathBuf> = MODULE_EXTENSIONS
                .iter()
                .map(|ext| resolved.with_extension(ext))
                .collect();
            candidates.push(resolved.to_path_buf());
            candidates
        }
    }

    /// 解析入口文件路径（如果已解析过则返回缓存的 ID）。
    pub fn resolve_entry_path(&mut self, entry: &Path) -> Result<ModuleId> {
        let path = Self::canonical_entry_path(entry)
            .with_context(|| format!("Failed to resolve input file: {}", entry.display()))?;
        self.load_resolved_module(&entry.display().to_string(), path)
    }

    fn canonical_entry_path(entry: &Path) -> Result<PathBuf> {
        if !entry.is_file() {
            bail!("Input file '{}' does not exist", entry.display());
        }
        entry
            .canonicalize()
            .with_context(|| format!("Failed to canonicalize input file: {}", entry.display()))
    }

    pub fn resolve(&mut self, specifier: &str, parent: &Path) -> Result<ModuleId> {
        let options = self.options.clone();
        self.resolve_with_options(specifier, parent, options.conditions())
    }

    pub(crate) fn resolve_with_kind(
        &mut self,
        specifier: &str,
        parent: &Path,
        kind: ResolutionKind,
    ) -> Result<ModuleId> {
        match self.resolve_specifier_with_kind(specifier, parent, kind)? {
            ResolvedSpecifier::Builtin(module) => self.load_builtin_module(module),
            ResolvedSpecifier::Path(path) => self.load_resolved_module(specifier, path),
        }
    }

    fn resolve_with_options(
        &mut self,
        specifier: &str,
        parent: &Path,
        conditions: &[String],
    ) -> Result<ModuleId> {
        match self.resolve_specifier_with_conditions(specifier, parent, conditions)? {
            ResolvedSpecifier::Builtin(module) => self.load_builtin_module(module),
            ResolvedSpecifier::Path(path) => self.load_resolved_module(specifier, path),
        }
    }

    pub(crate) fn resolve_builtin_entry(&mut self, specifier: &str) -> Result<ModuleId> {
        match builtin_modules::lookup(specifier) {
            BuiltinLookup::Found(module) => self.load_builtin_module(module),
            BuiltinLookup::UnknownNodeBuiltin(name) => {
                bail!("Unknown built-in module 'node:{name}'")
            }
            BuiltinLookup::NotBuiltin => bail!("Not a built-in module: {specifier}"),
        }
    }

    fn load_builtin_module(
        &mut self,
        module: &'static builtin_modules::BuiltinModule,
    ) -> Result<ModuleId> {
        let path = builtin_modules::virtual_path(module.canonical);
        debug_assert!(builtin_modules::is_builtin_virtual_path(&path));
        if let Some(&id) = self.visited.get(&path) {
            return Ok(id);
        }

        let ast = wjsm_parser::parse_module_with_path(module.source, &path)
            .with_context(|| format!("Failed to parse built-in module: {}", module.canonical))?;
        let imports = Self::extract_imports(&ast);
        let exports = Self::extract_exports(&ast);
        let dynamic_imports = Self::extract_dynamic_imports(&ast)?;
        let id = ModuleId(self.next_id);
        self.next_id += 1;

        self.visited.insert(path.clone(), id);
        self.modules.insert(
            id,
            ResolvedModule {
                id,
                source: module.source.to_string(),
                path,
                ast,
                imports,
                exports,
                dynamic_imports,
                is_cjs: false,
            },
        );
        Ok(id)
    }

    fn load_resolved_module(&mut self, specifier: &str, path: PathBuf) -> Result<ModuleId> {
        if !path.starts_with(&self.root_path) {
            bail!(
                "Module '{}' resolves outside root '{}': {}",
                specifier,
                self.root_path.display(),
                path.display()
            );
        }

        // 检查缓存
        if let Some(&id) = self.visited.get(&path) {
            return Ok(id);
        }

        // 查找当前文件所属 package（路径已由 resolver canonicalize，可复用 package cache）。
        let package = self.find_nearest_package(&path)?;

        // 读取文件
        let source = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read module: {}", path.display()))?;

        // 解析 AST
        let ast = wjsm_parser::parse_module_with_path(&source, &path)
            .with_context(|| format!("Failed to parse module: {}", path.display()))?;

        Self::validate_typescript_module_syntax(&ast, &path)?;

        // 检测模块格式；.mjs/.cjs/package type 优先，TS/JSX 继续使用 AST 检测。
        let ast_is_cjs = crate::cjs_transform::is_commonjs_module(&ast);
        let format = detect_module_format(&path, package.as_ref(), ast_is_cjs);
        let is_cjs = matches!(format, ModuleFormat::CommonJs);
        if is_cjs {
            Self::validate_commonjs_goal_syntax(&ast, &path)?;
        }
        let ast = if is_cjs {
            let prefix = format!("_{}_", self.next_id);
            crate::cjs_transform::transform_with_prefix(&ast, &prefix)
        } else {
            ast
        };

        // 提取 import/export
        let imports = Self::extract_imports(&ast);
        let exports = Self::extract_exports(&ast);

        // 提取动态 import() specifier（不合并进 imports，保持图语义正确）
        let dynamic_imports = Self::extract_dynamic_imports(&ast)?;

        // 分配 ID
        let id = ModuleId(self.next_id);
        self.next_id += 1;

        // 保存模块
        let module = ResolvedModule {
            id,
            source,
            path: path.clone(),
            ast,
            imports,
            exports,
            dynamic_imports,
            is_cjs,
        };

        self.visited.insert(path, id);
        self.modules.insert(id, module);

        Ok(id)
    }

    /// 获取已解析的模块
    pub fn get_module(&self, id: ModuleId) -> Option<&ResolvedModule> {
        self.modules.get(&id)
    }

    /// 获取所有已解析的模块
    pub fn all_modules(&self) -> impl Iterator<Item = &ResolvedModule> {
        self.modules.values()
    }

    /// 根据路径查找已解析的模块 ID
    pub fn get_id_by_path(&self, path: &Path) -> Option<ModuleId> {
        self.visited.get(path).copied()
    }

    #[cfg(test)]
    pub(crate) fn get_id_for_specifier(
        &self,
        specifier: &str,
        parent: &Path,
    ) -> Result<Option<ModuleId>> {
        self.get_id_for_specifier_with_conditions(specifier, parent, self.options.conditions())
    }

    pub(crate) fn get_id_for_specifier_with_kind(
        &self,
        specifier: &str,
        parent: &Path,
        kind: ResolutionKind,
    ) -> Result<Option<ModuleId>> {
        self.get_id_for_specifier_with_conditions(
            specifier,
            parent,
            self.options.conditions_for_kind(kind),
        )
    }

    fn get_id_for_specifier_with_conditions(
        &self,
        specifier: &str,
        parent: &Path,
        conditions: &[String],
    ) -> Result<Option<ModuleId>> {
        match self.resolve_specifier_with_conditions(specifier, parent, conditions)? {
            ResolvedSpecifier::Builtin(module) => Ok(self
                .visited
                .get(&builtin_modules::virtual_path(module.canonical))
                .copied()),
            ResolvedSpecifier::Path(path) => Ok(self.get_id_by_path(&path)),
        }
    }

    /// 为指定模块添加合成默认导出（如果它没有默认导出但有其他导出）
    pub fn ensure_default_export_for(&mut self, module_id: ModuleId) -> Result<()> {
        let module = self
            .modules
            .get_mut(&module_id)
            .ok_or_else(|| anyhow::anyhow!("invalid ModuleId: {:?}", module_id))?;
        let has_default = module
            .exports
            .iter()
            .any(|e| matches!(e, ExportEntry::Default { .. }));
        if !has_default && !module.exports.is_empty() {
            Self::add_synthetic_default_export(&mut module.ast, &module.exports);
            module.exports = Self::extract_exports(&module.ast);
        }
        Ok(())
    }

    /// 为没有默认导出的模块添加合成默认导出
    fn add_synthetic_default_export(ast: &mut ast::Module, exports: &[ExportEntry]) {
        use swc_core::common::{DUMMY_SP, SyntaxContext};

        let mut export_names: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for entry in exports {
            match entry {
                ExportEntry::Named { exported, .. } => {
                    if exported != "default" && seen.insert(exported.clone()) {
                        export_names.push(exported.clone());
                    }
                }
                ExportEntry::Declaration { name } if seen.insert(name.clone()) => {
                    export_names.push(name.clone());
                }
                _ => {}
            }
        }

        if export_names.is_empty() {
            return;
        }

        // 创建合成默认导出表达式：export default { name1, name2, ... }
        let props: Vec<ast::PropOrSpread> = export_names
            .iter()
            .map(|name| {
                ast::PropOrSpread::Prop(Box::new(ast::Prop::Shorthand(ast::Ident::new(
                    name.clone().into(),
                    DUMMY_SP,
                    SyntaxContext::default(),
                ))))
            })
            .collect();

        let default_export = ast::ModuleItem::ModuleDecl(ast::ModuleDecl::ExportDefaultExpr(
            ast::ExportDefaultExpr {
                span: DUMMY_SP,
                expr: Box::new(ast::Expr::Object(ast::ObjectLit {
                    span: DUMMY_SP,
                    props,
                })),
            },
        ));

        ast.body.push(default_export);
    }

    /// 从 AST 中提取 import 声明
    fn extract_imports(module: &ast::Module) -> Vec<ImportEntry> {
        module
            .body
            .iter()
            .filter_map(|item| match item {
                ast::ModuleItem::ModuleDecl(ast::ModuleDecl::Import(import_decl)) => {
                    let specifier = import_decl.src.value.to_string_lossy().into_owned();
                    let mut names = Vec::new();

                    for spec in &import_decl.specifiers {
                        match spec {
                            ast::ImportSpecifier::Named(named) => {
                                let local = named.local.sym.to_string();
                                let imported = named
                                    .imported
                                    .as_ref()
                                    .map(|id| match id {
                                        ast::ModuleExportName::Ident(ident) => {
                                            ident.sym.to_string()
                                        }
                                        ast::ModuleExportName::Str(s) => {
                                            s.value.to_string_lossy().into_owned()
                                        }
                                    })
                                    .unwrap_or_else(|| local.clone());
                                names.push((local, imported));
                            }
                            ast::ImportSpecifier::Default(default) => {
                                let local = default.local.sym.to_string();
                                names.push((local, "default".to_string()));
                            }
                            ast::ImportSpecifier::Namespace(ns) => {
                                let local = ns.local.sym.to_string();
                                names.push((local, "*".to_string()));
                            }
                        }
                    }

                    Some(ImportEntry {
                        specifier,
                        names,
                        source_span: import_decl.span,
                    })
                }
                _ => None,
            })
            .collect()
    }
    /// 从 AST 中提取静态可预解析的动态 import() specifier。
    ///
    /// 运行时表达式 import(expr) 由 runtime loader 处理；resolver 只收集能形成
    /// AOT 图边的静态字符串，不能再把表达式路径当作编译期错误。
    /// - 字符串字面量 → 直接提取
    /// - 无插值模板字符串 → 静态求值
    /// - 其他表达式 / 有插值模板 → 跳过；语义层会降级到运行时路径
    pub fn extract_dynamic_imports(module: &ast::Module) -> Result<Vec<String>> {
        let mut specifiers = Vec::new();
        for item in &module.body {
            Self::extract_dynamic_imports_from_item(item, &mut specifiers)?;
        }
        Ok(specifiers)
    }

    fn extract_dynamic_imports_from_item(
        item: &ast::ModuleItem,
        specifiers: &mut Vec<String>,
    ) -> Result<()> {
        match item {
            ast::ModuleItem::ModuleDecl(decl) => {
                Self::extract_dynamic_imports_from_module_decl(decl, specifiers)?;
            }
            ast::ModuleItem::Stmt(stmt) => {
                Self::extract_dynamic_imports_from_stmt(stmt, specifiers)?;
            }
        }
        Ok(())
    }

    fn extract_dynamic_imports_from_module_decl(
        decl: &ast::ModuleDecl,
        specifiers: &mut Vec<String>,
    ) -> Result<()> {
        match decl {
            ast::ModuleDecl::ExportDecl(export_decl) => {
                Self::extract_dynamic_imports_from_decl(&export_decl.decl, specifiers)?;
            }
            ast::ModuleDecl::ExportDefaultExpr(default_expr) => {
                Self::extract_dynamic_imports_from_expr(&default_expr.expr, specifiers)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn extract_dynamic_imports_from_decl(
        decl: &ast::Decl,
        specifiers: &mut Vec<String>,
    ) -> Result<()> {
        match decl {
            ast::Decl::Fn(fn_decl) => {
                Self::extract_dynamic_imports_from_function(&fn_decl.function, specifiers)?;
            }
            ast::Decl::Class(class_decl) => {
                Self::extract_dynamic_imports_from_class(&class_decl.class, specifiers)?;
            }
            ast::Decl::Var(var_decl) => {
                for declarator in &var_decl.decls {
                    if let Some(init) = &declarator.init {
                        Self::extract_dynamic_imports_from_expr(init, specifiers)?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn extract_dynamic_imports_from_stmt(
        stmt: &ast::Stmt,
        specifiers: &mut Vec<String>,
    ) -> Result<()> {
        match stmt {
            ast::Stmt::Expr(expr_stmt) => {
                Self::extract_dynamic_imports_from_expr(&expr_stmt.expr, specifiers)?;
            }
            ast::Stmt::Decl(decl) => {
                Self::extract_dynamic_imports_from_decl(decl, specifiers)?;
            }
            ast::Stmt::Block(block) => {
                for s in &block.stmts {
                    Self::extract_dynamic_imports_from_stmt(s, specifiers)?;
                }
            }
            ast::Stmt::If(if_stmt) => {
                Self::extract_dynamic_imports_from_expr(&if_stmt.test, specifiers)?;
                Self::extract_dynamic_imports_from_stmt(&if_stmt.cons, specifiers)?;
                if let Some(alt) = &if_stmt.alt {
                    Self::extract_dynamic_imports_from_stmt(alt, specifiers)?;
                }
            }
            ast::Stmt::For(for_stmt) => {
                if let Some(init) = &for_stmt.init {
                    match init {
                        ast::VarDeclOrExpr::VarDecl(var_decl) => {
                            Self::extract_dynamic_imports_from_decl(
                                &ast::Decl::Var(var_decl.clone()),
                                specifiers,
                            )?;
                        }
                        ast::VarDeclOrExpr::Expr(expr) => {
                            Self::extract_dynamic_imports_from_expr(expr, specifiers)?;
                        }
                    }
                }
                if let Some(test) = &for_stmt.test {
                    Self::extract_dynamic_imports_from_expr(test, specifiers)?;
                }
                if let Some(update) = &for_stmt.update {
                    Self::extract_dynamic_imports_from_expr(update, specifiers)?;
                }
                Self::extract_dynamic_imports_from_stmt(&for_stmt.body, specifiers)?;
            }
            ast::Stmt::ForIn(for_in) => {
                Self::extract_dynamic_imports_from_stmt(&for_in.body, specifiers)?;
            }
            ast::Stmt::ForOf(for_of) => {
                Self::extract_dynamic_imports_from_stmt(&for_of.body, specifiers)?;
            }
            ast::Stmt::While(while_stmt) => {
                Self::extract_dynamic_imports_from_expr(&while_stmt.test, specifiers)?;
                Self::extract_dynamic_imports_from_stmt(&while_stmt.body, specifiers)?;
            }
            ast::Stmt::DoWhile(do_while) => {
                Self::extract_dynamic_imports_from_stmt(&do_while.body, specifiers)?;
                Self::extract_dynamic_imports_from_expr(&do_while.test, specifiers)?;
            }
            ast::Stmt::Switch(switch) => {
                Self::extract_dynamic_imports_from_expr(&switch.discriminant, specifiers)?;
                for case in &switch.cases {
                    for s in &case.cons {
                        Self::extract_dynamic_imports_from_stmt(s, specifiers)?;
                    }
                }
            }
            ast::Stmt::Try(try_stmt) => {
                Self::extract_dynamic_imports_from_stmt(
                    &ast::Stmt::Block(try_stmt.block.clone()),
                    specifiers,
                )?;
                if let Some(handler) = &try_stmt.handler {
                    Self::extract_dynamic_imports_from_stmt(
                        &ast::Stmt::Block(handler.body.clone()),
                        specifiers,
                    )?;
                }
                if let Some(finalizer) = &try_stmt.finalizer {
                    Self::extract_dynamic_imports_from_stmt(
                        &ast::Stmt::Block(finalizer.clone()),
                        specifiers,
                    )?;
                }
            }
            ast::Stmt::Labeled(labeled) => {
                Self::extract_dynamic_imports_from_stmt(&labeled.body, specifiers)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn extract_dynamic_imports_from_expr(
        expr: &ast::Expr,
        specifiers: &mut Vec<String>,
    ) -> Result<()> {
        match expr {
            ast::Expr::Call(call) => {
                // 检测 import() 调用；只有静态 specifier 形成 AOT 图边。
                if matches!(call.callee, ast::Callee::Import(_)) {
                    if let Some(specifier) = Self::extract_import_call_specifier(call)? {
                        specifiers.push(specifier);
                    } else {
                        for arg in &call.args {
                            Self::extract_dynamic_imports_from_expr(&arg.expr, specifiers)?;
                        }
                    }
                } else {
                    // 递归进入被调用者和参数
                    if let ast::Callee::Expr(callee_expr) = &call.callee {
                        Self::extract_dynamic_imports_from_expr(callee_expr, specifiers)?;
                    }
                    for arg in &call.args {
                        Self::extract_dynamic_imports_from_expr(&arg.expr, specifiers)?;
                    }
                }
            }
            ast::Expr::Bin(bin) => {
                Self::extract_dynamic_imports_from_expr(&bin.left, specifiers)?;
                Self::extract_dynamic_imports_from_expr(&bin.right, specifiers)?;
            }
            ast::Expr::Unary(unary) => {
                Self::extract_dynamic_imports_from_expr(&unary.arg, specifiers)?;
            }
            ast::Expr::Assign(assign) => {
                Self::extract_dynamic_imports_from_expr(assign.right.as_ref(), specifiers)?;
            }
            ast::Expr::Cond(cond) => {
                Self::extract_dynamic_imports_from_expr(&cond.test, specifiers)?;
                Self::extract_dynamic_imports_from_expr(&cond.cons, specifiers)?;
                Self::extract_dynamic_imports_from_expr(&cond.alt, specifiers)?;
            }
            ast::Expr::Member(member) => {
                Self::extract_dynamic_imports_from_expr(&member.obj, specifiers)?;
                // 计算成员属性：obj[import(...)] 中的表达式也可能包含动态 import
                if let ast::MemberProp::Computed(computed) = &member.prop {
                    Self::extract_dynamic_imports_from_expr(&computed.expr, specifiers)?;
                }
            }
            ast::Expr::Object(obj) => {
                for prop in &obj.props {
                    match prop {
                        ast::PropOrSpread::Prop(p) => match p.as_ref() {
                            ast::Prop::KeyValue(kv) => {
                                Self::extract_dynamic_imports_from_expr(&kv.value, specifiers)?;
                            }
                            ast::Prop::Shorthand(_) => {}
                            ast::Prop::Getter(getter) => {
                                if let Some(body) = &getter.body {
                                    for s in &body.stmts {
                                        Self::extract_dynamic_imports_from_stmt(s, specifiers)?;
                                    }
                                }
                            }
                            ast::Prop::Setter(setter) => {
                                if let Some(body) = &setter.body {
                                    for s in &body.stmts {
                                        Self::extract_dynamic_imports_from_stmt(s, specifiers)?;
                                    }
                                }
                            }
                            ast::Prop::Method(method) => {
                                Self::extract_dynamic_imports_from_function(
                                    &method.function,
                                    specifiers,
                                )?;
                            }
                            _ => {}
                        },
                        ast::PropOrSpread::Spread(spread) => {
                            Self::extract_dynamic_imports_from_expr(&spread.expr, specifiers)?;
                        }
                    }
                }
            }
            ast::Expr::Array(arr) => {
                for elem in arr.elems.iter().flatten() {
                    Self::extract_dynamic_imports_from_expr(&elem.expr, specifiers)?;
                }
            }
            ast::Expr::Arrow(arrow) => match &*arrow.body {
                ast::BlockStmtOrExpr::BlockStmt(block) => {
                    for s in &block.stmts {
                        Self::extract_dynamic_imports_from_stmt(s, specifiers)?;
                    }
                }
                ast::BlockStmtOrExpr::Expr(expr) => {
                    Self::extract_dynamic_imports_from_expr(expr, specifiers)?;
                }
            },
            ast::Expr::Fn(fn_expr) => {
                Self::extract_dynamic_imports_from_function(&fn_expr.function, specifiers)?;
            }
            ast::Expr::Class(class_expr) => {
                Self::extract_dynamic_imports_from_class(&class_expr.class, specifiers)?;
            }
            ast::Expr::Tpl(tpl) => {
                for expr in &tpl.exprs {
                    Self::extract_dynamic_imports_from_expr(expr, specifiers)?;
                }
            }
            ast::Expr::TaggedTpl(tagged) => {
                Self::extract_dynamic_imports_from_expr(&tagged.tag, specifiers)?;
                for expr in &tagged.tpl.exprs {
                    Self::extract_dynamic_imports_from_expr(expr, specifiers)?;
                }
            }
            ast::Expr::Paren(paren) => {
                Self::extract_dynamic_imports_from_expr(&paren.expr, specifiers)?;
            }
            ast::Expr::Seq(seq) => {
                for expr in &seq.exprs {
                    Self::extract_dynamic_imports_from_expr(expr, specifiers)?;
                }
            }
            ast::Expr::New(new) => {
                Self::extract_dynamic_imports_from_expr(&new.callee, specifiers)?;
                if let Some(args) = &new.args {
                    for arg in args {
                        Self::extract_dynamic_imports_from_expr(&arg.expr, specifiers)?;
                    }
                }
            }
            ast::Expr::Await(await_expr) => {
                Self::extract_dynamic_imports_from_expr(&await_expr.arg, specifiers)?;
            }
            ast::Expr::Yield(yield_expr) => {
                if let Some(arg) = &yield_expr.arg {
                    Self::extract_dynamic_imports_from_expr(arg, specifiers)?;
                }
            }
            ast::Expr::MetaProp(_) | ast::Expr::Ident(_) | ast::Expr::Lit(_) => {}
            _ => {}
        }
        Ok(())
    }

    fn extract_dynamic_imports_from_function(
        function: &ast::Function,
        specifiers: &mut Vec<String>,
    ) -> Result<()> {
        if let Some(body) = &function.body {
            for s in &body.stmts {
                Self::extract_dynamic_imports_from_stmt(s, specifiers)?;
            }
        }
        Ok(())
    }

    fn extract_dynamic_imports_from_class(
        class: &ast::Class,
        specifiers: &mut Vec<String>,
    ) -> Result<()> {
        for member in &class.body {
            match member {
                ast::ClassMember::Method(method) => {
                    Self::extract_dynamic_imports_from_function(&method.function, specifiers)?;
                }
                ast::ClassMember::Constructor(ctor) => {
                    if let Some(body) = &ctor.body {
                        for s in &body.stmts {
                            Self::extract_dynamic_imports_from_stmt(s, specifiers)?;
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn extract_import_call_specifier(call: &ast::CallExpr) -> Result<Option<String>> {
        let first_arg = call
            .args
            .first()
            .ok_or_else(|| anyhow::anyhow!("import() requires a module specifier"))?;

        match first_arg.expr.as_ref() {
            ast::Expr::Lit(ast::Lit::Str(s)) => Ok(Some(s.value.to_string_lossy().into_owned())),
            ast::Expr::Tpl(tpl) => {
                if tpl.exprs.is_empty() {
                    // 无插值的模板字符串：静态求值。
                    let mut result = String::new();
                    for quasi in &tpl.quasis {
                        result.push_str(&quasi.raw);
                    }
                    Ok(Some(result))
                } else {
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    }
    /// 检测 TypeScript 特有模块语法；不支持时返回明确错误（避免静默丢弃）
    fn validate_typescript_module_syntax(module: &ast::Module, path: &Path) -> Result<()> {
        for item in &module.body {
            if let ast::ModuleItem::ModuleDecl(decl) = item {
                match decl {
                    ast::ModuleDecl::TsImportEquals(ts_import) => {
                        let local = ts_import.id.sym.to_string();
                        bail!(
                            "TypeScript `import {local} = ...` (import assignment) is not supported in module bundling ({}); \
                             use ESM `import` or CommonJS `require()` instead",
                            path.display()
                        );
                    }
                    ast::ModuleDecl::TsExportAssignment(_) => {
                        bail!(
                            "TypeScript `export = ...` is not supported in module bundling ({}); \
                             use `export default` instead",
                            path.display()
                        );
                    }
                    ast::ModuleDecl::TsNamespaceExport(ns_export) => {
                        let name = ns_export.id.sym.to_string();
                        bail!(
                            "TypeScript `export as namespace {name}` is not supported in module bundling ({}); \
                             it is a global ambient declaration and does not affect ESM module semantics",
                            path.display()
                        );
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    fn validate_commonjs_goal_syntax(module: &ast::Module, path: &Path) -> Result<()> {
        let has_static_import_export = module.body.iter().any(|item| {
            matches!(
                item,
                ast::ModuleItem::ModuleDecl(
                    ast::ModuleDecl::Import(_)
                        | ast::ModuleDecl::ExportDecl(_)
                        | ast::ModuleDecl::ExportNamed(_)
                        | ast::ModuleDecl::ExportDefaultDecl(_)
                        | ast::ModuleDecl::ExportDefaultExpr(_)
                        | ast::ModuleDecl::ExportAll(_)
                )
            )
        });
        if has_static_import_export {
            bail!(
                "SyntaxError: Cannot use import/export syntax in CommonJS module {}",
                path.display()
            );
        }
        Ok(())
    }

    /// 从 AST 中提取 export 声明
    fn extract_exports(module: &ast::Module) -> Vec<ExportEntry> {
        let mut exports = Vec::new();

        for item in &module.body {
            if let ast::ModuleItem::ModuleDecl(decl) = item {
                match decl {
                    ast::ModuleDecl::ExportNamed(named_export) => {
                        if let Some(src) = &named_export.src {
                            // export { ... } from './foo' — 命名重导出
                            let source = src.value.to_string_lossy().into_owned();
                            for spec in &named_export.specifiers {
                                match spec {
                                    ast::ExportSpecifier::Named(named) => {
                                        let local = match &named.orig {
                                            ast::ModuleExportName::Ident(ident) => {
                                                ident.sym.to_string()
                                            }
                                            ast::ModuleExportName::Str(s) => {
                                                s.value.to_string_lossy().into_owned()
                                            }
                                        };
                                        let exported = named
                                            .exported
                                            .as_ref()
                                            .map(|id| match id {
                                                ast::ModuleExportName::Ident(ident) => {
                                                    ident.sym.to_string()
                                                }
                                                ast::ModuleExportName::Str(s) => {
                                                    s.value.to_string_lossy().into_owned()
                                                }
                                            })
                                            .unwrap_or_else(|| local.clone());
                                        exports.push(ExportEntry::NamedReExport {
                                            local,
                                            exported,
                                            source: source.clone(),
                                        });
                                    }
                                    ast::ExportSpecifier::Namespace(ns) => {
                                        // export * as ns from './foo'
                                        let name = match &ns.name {
                                            ast::ModuleExportName::Ident(ident) => {
                                                ident.sym.to_string()
                                            }
                                            ast::ModuleExportName::Str(s) => {
                                                s.value.to_string_lossy().into_owned()
                                            }
                                        };
                                        exports.push(ExportEntry::NamedReExport {
                                            local: "*".to_string(),
                                            exported: name,
                                            source: source.clone(),
                                        });
                                    }
                                    ast::ExportSpecifier::Default(default) => {
                                        // export { default } from './foo'
                                        let local = default.exported.sym.to_string();
                                        exports.push(ExportEntry::NamedReExport {
                                            local: local.clone(),
                                            exported: "default".to_string(),
                                            source: source.clone(),
                                        });
                                    }
                                }
                            }
                        } else {
                            // export { x } / export { x as y }
                            for spec in &named_export.specifiers {
                                match spec {
                                    ast::ExportSpecifier::Named(named) => {
                                        let local = match &named.orig {
                                            ast::ModuleExportName::Ident(ident) => {
                                                ident.sym.to_string()
                                            }
                                            ast::ModuleExportName::Str(s) => {
                                                s.value.to_string_lossy().into_owned()
                                            }
                                        };
                                        let exported = named
                                            .exported
                                            .as_ref()
                                            .map(|id| match id {
                                                ast::ModuleExportName::Ident(ident) => {
                                                    ident.sym.to_string()
                                                }
                                                ast::ModuleExportName::Str(s) => {
                                                    s.value.to_string_lossy().into_owned()
                                                }
                                            })
                                            .unwrap_or_else(|| local.clone());
                                        exports.push(ExportEntry::Named { local, exported });
                                    }
                                    ast::ExportSpecifier::Default(default) => {
                                        // export { x as default }
                                        let local = default.exported.sym.to_string();
                                        exports.push(ExportEntry::Named {
                                            local,
                                            exported: "default".to_string(),
                                        });
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    ast::ModuleDecl::ExportDefaultExpr(_default_expr) => {
                        // export default expr
                        // 需要生成一个内部变量名
                        exports.push(ExportEntry::Default {
                            local: "_default_export".to_string(),
                        });
                    }
                    ast::ModuleDecl::ExportDefaultDecl(default_decl) => {
                        // export default function/class
                        let local = match &default_decl.decl {
                            ast::DefaultDecl::Class(class) => class
                                .ident
                                .as_ref()
                                .map(|i| i.sym.to_string())
                                .unwrap_or_else(|| "_default_export".to_string()),
                            ast::DefaultDecl::Fn(func) => func
                                .ident
                                .as_ref()
                                .map(|i| i.sym.to_string())
                                .unwrap_or_else(|| "_default_export".to_string()),
                            ast::DefaultDecl::TsInterfaceDecl(_) => "_default_export".to_string(),
                        };
                        exports.push(ExportEntry::Default { local });
                    }
                    ast::ModuleDecl::ExportAll(all) => {
                        // export * from './foo'
                        exports.push(ExportEntry::All {
                            source: all.src.value.to_string_lossy().into_owned(),
                        });
                    }
                    ast::ModuleDecl::ExportDecl(export_decl) => {
                        // export const/let/var/function/class
                        let name = match &export_decl.decl {
                            ast::Decl::Class(class) => class.ident.sym.to_string(),
                            ast::Decl::Fn(func) => func.ident.sym.to_string(),
                            ast::Decl::Var(var) => {
                                // 变量声明可能有多个
                                for decl in &var.decls {
                                    if let ast::Pat::Ident(ident) = &decl.name {
                                        exports.push(ExportEntry::Declaration {
                                            name: ident.id.sym.to_string(),
                                        });
                                    }
                                }
                                continue;
                            }
                            ast::Decl::TsInterface(_) => continue,
                            ast::Decl::TsTypeAlias(_) => continue,
                            ast::Decl::TsEnum(_) => continue,
                            ast::Decl::TsModule(_) => continue,
                            ast::Decl::Using(_) => continue,
                        };
                        exports.push(ExportEntry::Declaration { name });
                    }
                    _ => {}
                }
            }
        }

        exports
    }
}

#[cfg(test)]
#[cfg(test)]
#[path = "resolver_tests.rs"]
mod tests;
