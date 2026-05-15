mod extract;
mod types;

use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub use types::{ExportEntry, ImportEntry, ModuleId, ResolvedModule};

pub struct ModuleResolver {
    root_path: PathBuf,
    next_id: u32,
    visited: HashMap<PathBuf, ModuleId>,
    modules: HashMap<ModuleId, types::ResolvedModule>,
}

impl ModuleResolver {
    pub fn new(root_path: &Path) -> Self {
        let root_path = root_path
            .canonicalize()
            .unwrap_or_else(|_| root_path.to_path_buf());
        Self {
            root_path,
            next_id: 0,
            visited: HashMap::new(),
            modules: HashMap::new(),
        }
    }

    pub fn resolve_path(specifier: &str, parent: &Path) -> Result<PathBuf> {
        if !specifier.starts_with('.') {
            bail!(
                "Module specifier '{}' is not supported. Only relative imports (starting with './' or '../') are supported.",
                specifier
            );
        }

        let base = parent.parent().unwrap_or(parent);
        let resolved = base.join(specifier);

        let candidates = if resolved.extension().is_some() {
            vec![resolved.clone()]
        } else {
            vec![
                resolved.with_extension("js"),
                resolved.with_extension("ts"),
                resolved.clone(),
            ]
        };

        for candidate in &candidates {
            if candidate.exists() {
                return Ok(candidate.canonicalize()?);
            }
        }

        bail!(
            "Cannot find module '{}' from '{}'. Tried: {:?}",
            specifier,
            parent.display(),
            candidates
        );
    }

    pub fn resolve(&mut self, specifier: &str, parent: &Path) -> Result<ModuleId> {
        let path = Self::resolve_path(specifier, parent)?;
        if !path.starts_with(&self.root_path) {
            bail!(
                "Module '{}' resolves outside root '{}': {}",
                specifier,
                self.root_path.display(),
                path.display()
            );
        }

        if let Some(&id) = self.visited.get(&path) {
            return Ok(id);
        }

        let source = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read module: {}", path.display()))?;

        let ast = wjsm_parser::parse_module(&source)
            .with_context(|| format!("Failed to parse module: {}", path.display()))?;

        let is_cjs = crate::cjs_transform::is_commonjs_module(&ast);
        let ast = if is_cjs {
            let prefix = format!("_{}_", self.next_id);
            crate::cjs_transform::transform_with_prefix(&ast, &prefix)
        } else {
            ast
        };

        let imports = extract::extract_imports(&ast);
        let exports = extract::extract_exports(&ast);
        let dynamic_imports = extract::extract_dynamic_imports(&ast)?;

        let id = ModuleId(self.next_id);
        self.next_id += 1;

        let module = types::ResolvedModule {
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

    pub fn get_module(&self, id: ModuleId) -> Option<&types::ResolvedModule> {
        self.modules.get(&id)
    }

    pub fn all_modules(&self) -> impl Iterator<Item = &types::ResolvedModule> {
        self.modules.values()
    }

    pub fn get_id_by_path(&self, path: &Path) -> Option<ModuleId> {
        self.visited.get(path).copied()
    }

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
            extract::add_synthetic_default_export(&mut module.ast, &module.exports);
            module.exports = extract::extract_exports(&module.ast);
        }
        Ok(())
    }

    pub fn extract_dynamic_imports(module: &swc_core::ecma::ast::Module) -> Result<Vec<String>> {
        extract::extract_dynamic_imports(module)
    }
}

#[cfg(test)]
mod tests;
