use std::collections::{BTreeSet, HashMap};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use wjsm_runtime::{
    RuntimeInstantiatedModule, RuntimeInstantiationEnv, RuntimeModuleFormat,
    RuntimeModuleImportLink, RuntimeModuleInstantiationContext, RuntimeModuleKey,
    RuntimeModuleLoadError, RuntimeModuleLoadErrorCode, RuntimeModuleLoader, RuntimeModuleReferrer,
    RuntimeModuleResolutionKind, RuntimeResolvedModule,
};

pub(crate) struct CliRuntimeModuleLoader {
    root: PathBuf,
    read_roots: Vec<PathBuf>,
    resolution_options: wjsm_module::ResolutionOptions,
}

impl CliRuntimeModuleLoader {
    pub(crate) fn new(
        root: PathBuf,
        read_roots: Vec<PathBuf>,
        resolution_options: wjsm_module::ResolutionOptions,
    ) -> Self {
        Self {
            root,
            read_roots,
            resolution_options,
        }
    }

    fn referrer_path(
        &self,
        referrer: RuntimeModuleReferrer,
    ) -> Result<PathBuf, RuntimeModuleLoadError> {
        match referrer {
            RuntimeModuleReferrer::Path(path) => Ok(path),
            RuntimeModuleReferrer::Module(RuntimeModuleKey::File(path))
            | RuntimeModuleReferrer::Module(RuntimeModuleKey::Json(path)) => Ok(path),
            RuntimeModuleReferrer::None => Ok(self.root.clone()),
            RuntimeModuleReferrer::Module(RuntimeModuleKey::Builtin(specifier)) => {
                Err(RuntimeModuleLoadError::new(
                    RuntimeModuleLoadErrorCode::Unsupported,
                    format!("runtime loader cannot resolve relative to builtin module {specifier}"),
                ))
            }
            RuntimeModuleReferrer::Module(RuntimeModuleKey::PrecompiledModuleId(_))
            | RuntimeModuleReferrer::Module(RuntimeModuleKey::RuntimeModuleId(_)) => {
                Err(RuntimeModuleLoadError::new(
                    RuntimeModuleLoadErrorCode::Unsupported,
                    "runtime loader cannot resolve relative to a compiled module id",
                ))
            }
            _ => Err(RuntimeModuleLoadError::new(
                RuntimeModuleLoadErrorCode::Unsupported,
                "runtime loader does not support this referrer kind",
            )),
        }
    }

    fn compile_runtime_module(
        &self,
        resolved: &RuntimeResolvedModule,
        context: &mut RuntimeModuleInstantiationContext<'_, '_>,
    ) -> Result<(Vec<u8>, Option<u32>), RuntimeModuleLoadError> {
        match resolved.format {
            RuntimeModuleFormat::CommonJs => {
                let path = file_backed_runtime_path(resolved)?;
                self.compile_runtime_commonjs_module(resolved, path, context)
            }
            RuntimeModuleFormat::EsModule => {
                let path = file_backed_runtime_path(resolved)?;
                self.compile_runtime_esm_module(path, context)
            }
            RuntimeModuleFormat::Builtin => self.compile_runtime_builtin_module(resolved, context),
            RuntimeModuleFormat::Json => Err(RuntimeModuleLoadError::new(
                RuntimeModuleLoadErrorCode::Unsupported,
                "CLI runtime loader cannot compile this module format as WASM",
            )),
            _ => Err(RuntimeModuleLoadError::new(
                RuntimeModuleLoadErrorCode::Unsupported,
                "CLI runtime loader does not support this module format",
            )),
        }
    }

    fn compile_runtime_esm_module(
        &self,
        path: &Path,
        context: &mut RuntimeModuleInstantiationContext<'_, '_>,
    ) -> Result<(Vec<u8>, Option<u32>), RuntimeModuleLoadError> {
        reject_unsupported_runtime_extension(path)?;
        let mut bundle = wjsm_module::lower_runtime_entry_bundle_with_options(
            path,
            &self.root,
            self.resolution_options.clone(),
        )
        .map_err(invalid_module_error)?;
        self.compile_runtime_esm_bundle(&mut bundle, context)
    }

    fn compile_runtime_builtin_module(
        &self,
        resolved: &RuntimeResolvedModule,
        context: &mut RuntimeModuleInstantiationContext<'_, '_>,
    ) -> Result<(Vec<u8>, Option<u32>), RuntimeModuleLoadError> {
        let RuntimeModuleKey::Builtin(specifier) = &resolved.key else {
            return Err(RuntimeModuleLoadError::new(
                RuntimeModuleLoadErrorCode::Unsupported,
                "CLI runtime loader requires a built-in module key",
            ));
        };
        let mut bundle = wjsm_module::lower_runtime_builtin_bundle_with_options(
            specifier,
            &self.root,
            self.resolution_options.clone(),
        )
        .map_err(invalid_module_error)?;
        self.compile_runtime_esm_bundle(&mut bundle, context)
    }

    fn compile_runtime_esm_bundle(
        &self,
        bundle: &mut wjsm_module::RuntimeEntryBundle,
        context: &mut RuntimeModuleInstantiationContext<'_, '_>,
    ) -> Result<(Vec<u8>, Option<u32>), RuntimeModuleLoadError> {
        let module_id_base = context.reserve_runtime_module_ids(bundle.module_id_span)?;
        bundle
            .program
            .offset_module_ids(module_id_base)
            .map_err(module_id_offset_error)?;
        let entry_module_id = wjsm_ir::offset_module_id(bundle.entry_module_id, module_id_base)
            .map_err(module_id_offset_error)?;
        let wasm = compile_runtime_program(&bundle.program, context)?;
        Ok((wasm, Some(entry_module_id.0)))
    }

    fn compile_runtime_commonjs_module(
        &self,
        resolved: &RuntimeResolvedModule,
        path: &Path,
        context: &mut RuntimeModuleInstantiationContext<'_, '_>,
    ) -> Result<(Vec<u8>, Option<u32>), RuntimeModuleLoadError> {
        reject_unsupported_runtime_extension(path)?;
        let source = read_runtime_source(path)?;
        let ast =
            wjsm_parser::parse_module_with_path(&source, path).map_err(invalid_module_error)?;
        let module_id = wjsm_ir::ModuleId(0);
        let dirname = path
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .to_string_lossy()
            .into_owned();
        let program = wjsm_semantic::lower_modules(
            vec![wjsm_semantic::ModuleLoweringInput {
                id: module_id,
                ast,
                metadata: wjsm_semantic::ModuleMetadata {
                    filename: path.to_string_lossy().into_owned(),
                    dirname,
                    url: resolved.url.clone(),
                    kind: wjsm_semantic::ModuleKind::CommonJs,
                },
            }],
            &HashMap::<wjsm_ir::ModuleId, Vec<wjsm_ir::ImportBinding>>::new(),
            &HashMap::<wjsm_ir::ModuleId, Vec<wjsm_ir::ModuleId>>::new(),
            &HashMap::<wjsm_ir::ModuleId, BTreeSet<String>>::new(),
            &HashMap::<wjsm_ir::ModuleId, Vec<(String, wjsm_ir::ModuleId)>>::new(),
            &HashMap::<wjsm_ir::ModuleId, Vec<wjsm_ir::ReExportBinding>>::new(),
        )
        .map_err(|error| invalid_module_error(error.into()))?;
        let wasm = compile_runtime_program(&program, context)?;
        Ok((wasm, None))
    }
}

impl RuntimeModuleLoader for CliRuntimeModuleLoader {
    fn resolve_for_runtime(
        &self,
        referrer: RuntimeModuleReferrer,
        specifier: &str,
        kind: RuntimeModuleResolutionKind,
    ) -> Result<RuntimeResolvedModule, RuntimeModuleLoadError> {
        let referrer_path = self.referrer_path(referrer)?;
        let kind = match kind {
            RuntimeModuleResolutionKind::Import
            | RuntimeModuleResolutionKind::ImportMetaResolve => {
                wjsm_module::RuntimeResolveKind::Import
            }
            RuntimeModuleResolutionKind::Require => wjsm_module::RuntimeResolveKind::Require,
            _ => wjsm_module::RuntimeResolveKind::Import,
        };
        let resolved = wjsm_module::resolve_runtime_specifier(
            specifier,
            &referrer_path,
            &self.root,
            &self.resolution_options,
            kind,
        )
        .map_err(|error| {
            RuntimeModuleLoadError::new(RuntimeModuleLoadErrorCode::NotFound, error.to_string())
        })?;
        let mut converted = convert_resolved_module(resolved);
        if let Some(path) = &converted.path {
            self.check_read_allowed(path)?;
            converted.format = detect_runtime_file_format(path, converted.format, &self.root);
        }
        Ok(converted)
    }

    fn resolve_paths_for_runtime(
        &self,
        referrer: RuntimeModuleReferrer,
        specifier: &str,
    ) -> Result<Option<Vec<PathBuf>>, RuntimeModuleLoadError> {
        let referrer_path = self.referrer_path(referrer)?;
        Ok(
            match wjsm_module::resolve_runtime_paths(specifier, &referrer_path, &self.root) {
                wjsm_module::RuntimeResolvePaths::Null => None,
                wjsm_module::RuntimeResolvePaths::Search(paths) => Some(paths),
            },
        )
    }

    fn instantiate_runtime_module(
        &self,
        _resolved: &RuntimeResolvedModule,
        _env: RuntimeInstantiationEnv,
    ) -> Result<RuntimeInstantiatedModule, RuntimeModuleLoadError> {
        Err(RuntimeModuleLoadError::new(
            RuntimeModuleLoadErrorCode::Unsupported,
            "CLI runtime module loader requires the current runtime instantiation context",
        ))
    }

    fn instantiate_runtime_module_with_context<'a, 'b>(
        &'a self,
        resolved: &'a RuntimeResolvedModule,
        _env: RuntimeInstantiationEnv,
        mut context: RuntimeModuleInstantiationContext<'a, 'b>,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<RuntimeInstantiatedModule, RuntimeModuleLoadError>>
                + Send
                + 'a,
        >,
    >
    where
        'b: 'a,
    {
        Box::pin(async move {
            if resolved.format == RuntimeModuleFormat::Json {
                let path = resolved.path.as_deref().ok_or_else(|| {
                    RuntimeModuleLoadError::new(
                        RuntimeModuleLoadErrorCode::Unsupported,
                        "CLI runtime loader only supports file-backed JSON modules",
                    )
                })?;
                let source = read_runtime_source(path)?;
                return context.instantiate_json_module(resolved, &source);
            }
            let (wasm, entry_module_id) = self.compile_runtime_module(resolved, &mut context)?;
            context
                .instantiate_compiled_module_with_imports(
                    resolved,
                    &wasm,
                    entry_module_id,
                    backend_runtime_import_links(),
                )
                .await
        })
    }
}

impl CliRuntimeModuleLoader {
    fn check_read_allowed(&self, path: &Path) -> Result<(), RuntimeModuleLoadError> {
        if self.read_roots.iter().any(|root| path.starts_with(root)) {
            Ok(())
        } else {
            Err(RuntimeModuleLoadError::new(
                RuntimeModuleLoadErrorCode::NotFound,
                format!(
                    "runtime module '{}' is outside configured read roots",
                    path.display()
                ),
            ))
        }
    }
}

fn backend_runtime_import_links() -> impl Iterator<Item = RuntimeModuleImportLink> {
    wjsm_backend_wasm::host_import_registry::host_import_specs()
        .iter()
        .map(|spec| RuntimeModuleImportLink::env(spec.name))
}

fn compile_runtime_program(
    program: &wjsm_ir::Program,
    context: &mut RuntimeModuleInstantiationContext<'_, '_>,
) -> Result<Vec<u8>, RuntimeModuleLoadError> {
    let measured = wjsm_backend_wasm::compile_runtime_module_at(program, 0, 0)
        .map_err(invalid_module_error)?;
    let placement = context.reserve_module_layout(measured.table_len, measured.data_len)?;
    let compiled = wjsm_backend_wasm::compile_runtime_module_at(
        program,
        placement.data_base,
        placement.table_base,
    )
    .map_err(invalid_module_error)?;
    Ok(compiled.wasm)
}

fn file_backed_runtime_path(
    resolved: &RuntimeResolvedModule,
) -> Result<&Path, RuntimeModuleLoadError> {
    resolved.path.as_deref().ok_or_else(|| {
        RuntimeModuleLoadError::new(
            RuntimeModuleLoadErrorCode::Unsupported,
            "CLI runtime loader only supports file-backed modules for this format",
        )
    })
}

fn read_runtime_source(path: &Path) -> Result<String, RuntimeModuleLoadError> {
    std::fs::read_to_string(path).map_err(|error| {
        RuntimeModuleLoadError::new(
            RuntimeModuleLoadErrorCode::NotFound,
            format!(
                "failed to read runtime module '{}': {error}",
                path.display()
            ),
        )
    })
}

fn convert_resolved_module(resolved: wjsm_module::RuntimeResolvedModule) -> RuntimeResolvedModule {
    RuntimeResolvedModule::new(
        convert_key(resolved.key),
        resolved.url,
        resolved.path,
        convert_format(resolved.format),
    )
}

fn convert_key(key: wjsm_module::RuntimeModuleKey) -> RuntimeModuleKey {
    match key {
        wjsm_module::RuntimeModuleKey::File(path) => RuntimeModuleKey::File(path),
        wjsm_module::RuntimeModuleKey::Json(path) => RuntimeModuleKey::Json(path),
        wjsm_module::RuntimeModuleKey::Builtin(specifier) => RuntimeModuleKey::Builtin(specifier),
    }
}

fn convert_format(format: wjsm_module::RuntimeModuleFormat) -> RuntimeModuleFormat {
    match format {
        wjsm_module::RuntimeModuleFormat::Esm => RuntimeModuleFormat::EsModule,
        wjsm_module::RuntimeModuleFormat::CommonJs => RuntimeModuleFormat::CommonJs,
        wjsm_module::RuntimeModuleFormat::Json => RuntimeModuleFormat::Json,
        wjsm_module::RuntimeModuleFormat::Builtin => RuntimeModuleFormat::Builtin,
    }
}

fn detect_runtime_file_format(
    path: &Path,
    fallback: RuntimeModuleFormat,
    root: &Path,
) -> RuntimeModuleFormat {
    if !should_probe_runtime_commonjs(path, fallback, root) {
        return fallback;
    }

    let Ok(source) = std::fs::read_to_string(path) else {
        return fallback;
    };
    let Ok(module) = wjsm_parser::parse_module_with_path(&source, path) else {
        return fallback;
    };
    if wjsm_module::is_commonjs_module(&module) {
        RuntimeModuleFormat::CommonJs
    } else {
        fallback
    }
}

// The resolver already owns explicit extension/package goals. The AST probe is
// only a second pass for ambiguous no-package `.js` and extensionless files,
// where source shape is the CommonJS signal.
fn should_probe_runtime_commonjs(path: &Path, fallback: RuntimeModuleFormat, root: &Path) -> bool {
    let extension = path.extension().and_then(|extension| extension.to_str());

    fallback == RuntimeModuleFormat::EsModule
        && matches!(extension, Some("js") | None)
        && !has_nearest_package_manifest(path, root)
}

fn has_nearest_package_manifest(path: &Path, root: &Path) -> bool {
    let Some(mut current) = path.parent() else {
        return false;
    };

    loop {
        if !current.starts_with(root) {
            return false;
        }
        if current.join("package.json").is_file() {
            return true;
        }
        if current == root {
            return false;
        }
        let Some(parent) = current.parent() else {
            return false;
        };
        current = parent;
    }
}

fn reject_unsupported_runtime_extension(path: &Path) -> Result<(), RuntimeModuleLoadError> {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("ts" | "tsx" | "jsx") => Err(RuntimeModuleLoadError::new(
            RuntimeModuleLoadErrorCode::Unsupported,
            format!(
                "runtime loader does not compile TypeScript/JSX modules: {}",
                path.display()
            ),
        )),
        _ => Ok(()),
    }
}

fn invalid_module_error(error: anyhow::Error) -> RuntimeModuleLoadError {
    RuntimeModuleLoadError::new(RuntimeModuleLoadErrorCode::InvalidModule, error.to_string())
}

fn module_id_offset_error(error: wjsm_ir::ModuleIdOffsetError) -> RuntimeModuleLoadError {
    RuntimeModuleLoadError::new(RuntimeModuleLoadErrorCode::InvalidModule, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_commonjs_probe_includes_extensionless_no_package() {
        assert!(should_probe_runtime_commonjs(
            Path::new("/repo/child"),
            RuntimeModuleFormat::EsModule,
            Path::new("/repo"),
        ));
    }

    #[test]
    fn runtime_commonjs_probe_preserves_explicit_esm_boundaries() {
        assert!(!should_probe_runtime_commonjs(
            Path::new("/repo/entry.mjs"),
            RuntimeModuleFormat::EsModule,
            Path::new("/repo"),
        ));

        assert!(!should_probe_runtime_commonjs(
            Path::new("/repo/entry.js"),
            RuntimeModuleFormat::CommonJs,
            Path::new("/repo"),
        ));

        let root =
            std::env::temp_dir().join(format!("wjsm-runtime-loader-probe-{}", std::process::id()));
        let package_root = root.join("pkg");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&package_root).expect("temp package root should be created");
        std::fs::write(package_root.join("package.json"), r#"{"type":"module"}"#)
            .expect("temp package manifest should be written");

        assert!(!should_probe_runtime_commonjs(
            &package_root.join("entry.js"),
            RuntimeModuleFormat::EsModule,
            &root,
        ));

        let _ = std::fs::remove_dir_all(&root);
    }
}
