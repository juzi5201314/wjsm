use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use tokio::runtime::Builder;
use wjsm_runtime::{
    RuntimeInstantiatedModule, RuntimeInstantiationEnv, RuntimeModuleFormat, RuntimeModuleKey,
    RuntimeModuleLoadError, RuntimeModuleLoadErrorCode, RuntimeModuleLoader, RuntimeModuleReferrer,
    RuntimeModuleResolutionKind, RuntimeOptions, RuntimeResolvedModule,
    execute_with_writer_with_options,
};

struct ExternalStyleLoader;

impl RuntimeModuleLoader for ExternalStyleLoader {
    fn resolve_for_runtime(
        &self,
        referrer: RuntimeModuleReferrer,
        specifier: &str,
        kind: RuntimeModuleResolutionKind,
    ) -> Result<RuntimeResolvedModule, RuntimeModuleLoadError> {
        assert_eq!(referrer, RuntimeModuleReferrer::None);
        assert_eq!(specifier, "./dep.js");
        assert_eq!(kind, RuntimeModuleResolutionKind::Require);

        let path = PathBuf::from("/project/dep.js");
        Ok(RuntimeResolvedModule::new(
            RuntimeModuleKey::File(path.clone()),
            "file:///project/dep.js",
            Some(path),
            RuntimeModuleFormat::CommonJs,
        ))
    }

    fn instantiate_runtime_module(
        &self,
        resolved: &RuntimeResolvedModule,
        env: RuntimeInstantiationEnv,
    ) -> Result<RuntimeInstantiatedModule, RuntimeModuleLoadError> {
        assert_eq!(resolved.format, RuntimeModuleFormat::CommonJs);
        assert_eq!(
            env,
            RuntimeInstantiationEnv::new(RuntimeModuleReferrer::Module(resolved.key.clone()))
        );

        Ok(RuntimeInstantiatedModule::new(Some(42), 10, 11, 12))
    }
}

struct ResolveFixtureLoader;

impl RuntimeModuleLoader for ResolveFixtureLoader {
    fn resolve_for_runtime(
        &self,
        referrer: RuntimeModuleReferrer,
        specifier: &str,
        kind: RuntimeModuleResolutionKind,
    ) -> Result<RuntimeResolvedModule, RuntimeModuleLoadError> {
        assert_eq!(
            referrer,
            RuntimeModuleReferrer::Path(PathBuf::from("/project/main.cjs"))
        );
        assert_eq!(kind, RuntimeModuleResolutionKind::Require);
        if specifier == "./dep.js" {
            let path = PathBuf::from("/project/dep.js");
            return Ok(RuntimeResolvedModule::new(
                RuntimeModuleKey::File(path.clone()),
                "file:///project/dep.js",
                Some(path),
                RuntimeModuleFormat::CommonJs,
            ));
        }
        Err(RuntimeModuleLoadError::new(
            RuntimeModuleLoadErrorCode::NotFound,
            format!("module not found: {specifier}"),
        ))
    }

    fn resolve_paths_for_runtime(
        &self,
        referrer: RuntimeModuleReferrer,
        specifier: &str,
    ) -> Result<Option<Vec<PathBuf>>, RuntimeModuleLoadError> {
        assert_eq!(
            referrer,
            RuntimeModuleReferrer::Path(PathBuf::from("/project/main.cjs"))
        );
        assert_eq!(specifier, "./dep.js");
        Ok(Some(vec![
            PathBuf::from("/project/node_modules"),
            PathBuf::from("/node_modules"),
        ]))
    }

    fn instantiate_runtime_module(
        &self,
        _resolved: &RuntimeResolvedModule,
        _env: RuntimeInstantiationEnv,
    ) -> Result<RuntimeInstantiatedModule, RuntimeModuleLoadError> {
        Err(RuntimeModuleLoadError::new(
            RuntimeModuleLoadErrorCode::Unsupported,
            "fixture loader only supports resolution",
        ))
    }
}

struct SelfReferenceLoader;

impl RuntimeModuleLoader for SelfReferenceLoader {
    fn resolve_for_runtime(
        &self,
        referrer: RuntimeModuleReferrer,
        specifier: &str,
        kind: RuntimeModuleResolutionKind,
    ) -> Result<RuntimeResolvedModule, RuntimeModuleLoadError> {
        assert_eq!(
            referrer,
            RuntimeModuleReferrer::Path(PathBuf::from("/project/main.cjs"))
        );
        assert_eq!(specifier, "./self.js");
        assert_eq!(kind, RuntimeModuleResolutionKind::Require);

        let path = PathBuf::from("/project/main.cjs");
        Ok(RuntimeResolvedModule::new(
            RuntimeModuleKey::File(path.clone()),
            "file:///project/main.cjs",
            Some(path),
            RuntimeModuleFormat::CommonJs,
        ))
    }

    fn instantiate_runtime_module(
        &self,
        _resolved: &RuntimeResolvedModule,
        _env: RuntimeInstantiationEnv,
    ) -> Result<RuntimeInstantiatedModule, RuntimeModuleLoadError> {
        panic!("self-reference require should hit the existing CJS registry entry")
    }
}

struct LaterLoadedCacheEntryLoader;

impl RuntimeModuleLoader for LaterLoadedCacheEntryLoader {
    fn resolve_for_runtime(
        &self,
        referrer: RuntimeModuleReferrer,
        specifier: &str,
        kind: RuntimeModuleResolutionKind,
    ) -> Result<RuntimeResolvedModule, RuntimeModuleLoadError> {
        assert_eq!(
            referrer,
            RuntimeModuleReferrer::Path(PathBuf::from("/project/main.cjs"))
        );
        assert_eq!(specifier, "./later.js");
        assert_eq!(kind, RuntimeModuleResolutionKind::Require);

        let path = PathBuf::from("/project/later.js");
        Ok(RuntimeResolvedModule::new(
            RuntimeModuleKey::File(path.clone()),
            "file:///project/later.js",
            Some(path),
            RuntimeModuleFormat::CommonJs,
        ))
    }

    fn instantiate_runtime_module(
        &self,
        resolved: &RuntimeResolvedModule,
        env: RuntimeInstantiationEnv,
    ) -> Result<RuntimeInstantiatedModule, RuntimeModuleLoadError> {
        assert_eq!(resolved.format, RuntimeModuleFormat::CommonJs);
        assert_eq!(
            env,
            RuntimeInstantiationEnv::new(RuntimeModuleReferrer::Path(PathBuf::from(
                "/project/main.cjs"
            )))
        );

        Ok(RuntimeInstantiatedModule::new(
            Some(43),
            wjsm_ir::value::encode_null(),
            wjsm_ir::value::encode_undefined(),
            wjsm_ir::value::encode_undefined(),
        ))
    }
}

struct DynamicImportLoader;

impl RuntimeModuleLoader for DynamicImportLoader {
    fn resolve_for_runtime(
        &self,
        referrer: RuntimeModuleReferrer,
        specifier: &str,
        kind: RuntimeModuleResolutionKind,
    ) -> Result<RuntimeResolvedModule, RuntimeModuleLoadError> {
        assert_eq!(
            referrer,
            RuntimeModuleReferrer::Path(PathBuf::from("/project/main.js"))
        );
        assert_eq!(specifier, "./dep.js");
        assert_eq!(kind, RuntimeModuleResolutionKind::Import);

        let path = PathBuf::from("/project/dep.js");
        Ok(RuntimeResolvedModule::new(
            RuntimeModuleKey::File(path.clone()),
            "file:///project/dep.js",
            Some(path),
            RuntimeModuleFormat::EsModule,
        ))
    }

    fn instantiate_runtime_module(
        &self,
        resolved: &RuntimeResolvedModule,
        env: RuntimeInstantiationEnv,
    ) -> Result<RuntimeInstantiatedModule, RuntimeModuleLoadError> {
        assert_eq!(resolved.format, RuntimeModuleFormat::EsModule);
        assert_eq!(
            env,
            RuntimeInstantiationEnv::new(RuntimeModuleReferrer::Path(PathBuf::from(
                "/project/main.js"
            )))
        );

        Ok(RuntimeInstantiatedModule::new(
            Some(7),
            wjsm_ir::value::encode_undefined(),
            wjsm_ir::value::encode_undefined(),
            wjsm_ir::value::encode_f64(77.0),
        ))
    }
}

struct NumericDynamicImportLoader;

impl RuntimeModuleLoader for NumericDynamicImportLoader {
    fn resolve_for_runtime(
        &self,
        referrer: RuntimeModuleReferrer,
        specifier: &str,
        kind: RuntimeModuleResolutionKind,
    ) -> Result<RuntimeResolvedModule, RuntimeModuleLoadError> {
        assert_eq!(
            referrer,
            RuntimeModuleReferrer::Path(PathBuf::from("/project/main.js"))
        );
        assert_eq!(specifier, "123");
        assert_eq!(kind, RuntimeModuleResolutionKind::Import);

        let path = PathBuf::from("/project/number.js");
        Ok(RuntimeResolvedModule::new(
            RuntimeModuleKey::File(path.clone()),
            "file:///project/number.js",
            Some(path),
            RuntimeModuleFormat::EsModule,
        ))
    }

    fn instantiate_runtime_module(
        &self,
        resolved: &RuntimeResolvedModule,
        env: RuntimeInstantiationEnv,
    ) -> Result<RuntimeInstantiatedModule, RuntimeModuleLoadError> {
        assert_eq!(resolved.format, RuntimeModuleFormat::EsModule);
        assert_eq!(
            env,
            RuntimeInstantiationEnv::new(RuntimeModuleReferrer::Path(PathBuf::from(
                "/project/main.js"
            )))
        );

        Ok(RuntimeInstantiatedModule::new(
            Some(8),
            wjsm_ir::value::encode_undefined(),
            wjsm_ir::value::encode_undefined(),
            wjsm_ir::value::encode_f64(88.0),
        ))
    }
}

struct ImportMetaResolveLoader;

impl RuntimeModuleLoader for ImportMetaResolveLoader {
    fn resolve_for_runtime(
        &self,
        referrer: RuntimeModuleReferrer,
        specifier: &str,
        kind: RuntimeModuleResolutionKind,
    ) -> Result<RuntimeResolvedModule, RuntimeModuleLoadError> {
        assert_eq!(
            referrer,
            RuntimeModuleReferrer::Path(PathBuf::from("/project/main.js"))
        );
        assert_eq!(specifier, "./dep.js");
        assert_eq!(kind, RuntimeModuleResolutionKind::Import);

        let path = PathBuf::from("/project/dep.js");
        Ok(RuntimeResolvedModule::new(
            RuntimeModuleKey::File(path.clone()),
            "file:///project/dep.js",
            Some(path),
            RuntimeModuleFormat::EsModule,
        ))
    }

    fn instantiate_runtime_module(
        &self,
        _resolved: &RuntimeResolvedModule,
        _env: RuntimeInstantiationEnv,
    ) -> Result<RuntimeInstantiatedModule, RuntimeModuleLoadError> {
        Err(RuntimeModuleLoadError::new(
            RuntimeModuleLoadErrorCode::Unsupported,
            "resolve-only loader",
        ))
    }
}

struct MissingDynamicImportLoader;

impl RuntimeModuleLoader for MissingDynamicImportLoader {
    fn resolve_for_runtime(
        &self,
        referrer: RuntimeModuleReferrer,
        specifier: &str,
        kind: RuntimeModuleResolutionKind,
    ) -> Result<RuntimeResolvedModule, RuntimeModuleLoadError> {
        assert_eq!(
            referrer,
            RuntimeModuleReferrer::Path(PathBuf::from("/project/main.js"))
        );
        assert_eq!(specifier, "./missing.js");
        assert_eq!(kind, RuntimeModuleResolutionKind::Import);
        Err(RuntimeModuleLoadError::new(
            RuntimeModuleLoadErrorCode::NotFound,
            "missing module",
        ))
    }

    fn instantiate_runtime_module(
        &self,
        _resolved: &RuntimeResolvedModule,
        _env: RuntimeInstantiationEnv,
    ) -> Result<RuntimeInstantiatedModule, RuntimeModuleLoadError> {
        panic!("missing dynamic import should not instantiate")
    }
}


fn compile_cjs_source(source: &str) -> Result<Vec<u8>> {
    let ast = wjsm_parser::parse_module(source)?;
    let program = wjsm_semantic::lower_modules(
        vec![wjsm_semantic::ModuleLoweringInput {
            id: wjsm_ir::ModuleId(0),
            ast,
            metadata: wjsm_semantic::ModuleMetadata {
                filename: "/project/main.cjs".to_string(),
                dirname: "/project".to_string(),
                url: "file:///project/main.cjs".to_string(),
                kind: wjsm_semantic::ModuleKind::CommonJs,
            },
        }],
        &HashMap::<wjsm_ir::ModuleId, Vec<wjsm_ir::ImportBinding>>::new(),
        &HashMap::<wjsm_ir::ModuleId, Vec<wjsm_ir::ModuleId>>::new(),
        &HashMap::<wjsm_ir::ModuleId, BTreeSet<String>>::new(),
        &HashMap::<wjsm_ir::ModuleId, Vec<(String, wjsm_ir::ModuleId)>>::new(),
        &HashMap::<wjsm_ir::ModuleId, Vec<wjsm_ir::ReExportBinding>>::new(),
    )?;
    wjsm_backend_wasm::compile(&program)
}

fn compile_esm_source(source: &str) -> Result<Vec<u8>> {
    let ast = wjsm_parser::parse_module(source)?;
    let program = wjsm_semantic::lower_modules(
        vec![wjsm_semantic::ModuleLoweringInput {
            id: wjsm_ir::ModuleId(0),
            ast,
            metadata: wjsm_semantic::ModuleMetadata {
                filename: "/project/main.js".to_string(),
                dirname: "/project".to_string(),
                url: "file:///project/main.js".to_string(),
                kind: wjsm_semantic::ModuleKind::Esm,
            },
        }],
        &HashMap::<wjsm_ir::ModuleId, Vec<wjsm_ir::ImportBinding>>::new(),
        &HashMap::<wjsm_ir::ModuleId, Vec<wjsm_ir::ModuleId>>::new(),
        &HashMap::<wjsm_ir::ModuleId, BTreeSet<String>>::new(),
        &HashMap::<wjsm_ir::ModuleId, Vec<(String, wjsm_ir::ModuleId)>>::new(),
        &HashMap::<wjsm_ir::ModuleId, Vec<wjsm_ir::ReExportBinding>>::new(),
    )?;
    wjsm_backend_wasm::compile(&program)
}

fn run_esm_source(source: &str, options: RuntimeOptions) -> Result<String> {
    let wasm = compile_esm_source(source)?;
    let rt = Builder::new_current_thread().enable_all().build()?;
    let (out, _) =
        rt.block_on(async { execute_with_writer_with_options(&wasm, Vec::new(), options).await })?;
    Ok(String::from_utf8(out)?)
}


fn run_cjs_source(source: &str, options: RuntimeOptions) -> Result<String> {
    let wasm = compile_cjs_source(source)?;
    let rt = Builder::new_current_thread().enable_all().build()?;
    let (out, _) =
        rt.block_on(async { execute_with_writer_with_options(&wasm, Vec::new(), options).await })?;
    Ok(String::from_utf8(out)?)
}

#[test]
fn module_registry_external_loader_impl_constructs_runtime_dtos() {
    let loader = ExternalStyleLoader;
    let resolved = loader
        .resolve_for_runtime(
            RuntimeModuleReferrer::None,
            "./dep.js",
            RuntimeModuleResolutionKind::Require,
        )
        .expect("external-style loader should resolve module");

    assert_eq!(resolved.url, "file:///project/dep.js");
    assert_eq!(resolved.format, RuntimeModuleFormat::CommonJs);

    let instantiated = loader
        .instantiate_runtime_module(
            &resolved,
            RuntimeInstantiationEnv::new(RuntimeModuleReferrer::Module(resolved.key.clone())),
        )
        .expect("external-style loader should instantiate module");

    assert_eq!(instantiated.module_id, Some(42));
    assert_eq!(instantiated.module_object, 10);
    assert_eq!(instantiated.exports_object, 11);
    assert_eq!(instantiated.namespace_object, 12);
}

#[test]
fn require_resolve_uses_installed_loader_and_paths() -> Result<()> {
    let output = run_cjs_source(
        r#"
console.log(require.resolve('./dep.js'));
console.log(require.resolve.paths('./dep.js').join('|'));
try { require('./missing.js'); } catch (e) { console.log(e.name, e.message.includes("Cannot find module './missing.js'")); }
"#,
        RuntimeOptions {
            module_loader: Some(Arc::new(ResolveFixtureLoader)),
            ..RuntimeOptions::default()
        },
    )?;

    assert_eq!(
        output,
        "/project/dep.js\n/project/node_modules|/node_modules\nTypeError true\n"
    );
    Ok(())
}

#[test]
fn require_cache_held_reference_reflects_delete() -> Result<()> {
    let output = run_cjs_source(
        r#"
const cache = require.cache;
console.log(cache[__filename] === module);
console.log(__filename in cache);
console.log(delete require.cache[__filename]);
console.log(cache[__filename]);
console.log(__filename in cache);
"#,
        RuntimeOptions::default(),
    )?;

    assert_eq!(output, "true\ntrue\ntrue\nundefined\nfalse\n");
    Ok(())
}

#[test]
fn require_cache_held_reference_observes_later_loaded_entry() -> Result<()> {
    let output = run_cjs_source(
        r#"
const cache = require.cache;
console.log(cache['/project/later.js']);
require('./later.js');
console.log(cache['/project/later.js'] === null);
console.log(Object.keys(cache).join('|'));
const desc = Object.getOwnPropertyDescriptor(cache, '/project/later.js');
console.log(desc.value === null, desc.enumerable, desc.configurable, desc.writable);
"#,
        RuntimeOptions {
            module_loader: Some(Arc::new(LaterLoadedCacheEntryLoader)),
            ..RuntimeOptions::default()
        },
    )?;

    assert_eq!(
        output,
        "undefined\ntrue\n/project/later.js|/project/main.cjs\ntrue true true true\n"
    );
    Ok(())
}

#[test]
fn require_cache_exposes_module_object_and_deletes_loaded_entry() -> Result<()> {
    let output = run_cjs_source(
        r#"
console.log(require.cache[__filename] === module);
console.log(delete require.cache[__filename]);
console.log(require.cache[__filename]);
"#,
        RuntimeOptions::default(),
    )?;

    assert_eq!(output, "true\ntrue\nundefined\n");
    Ok(())
}

#[test]
fn require_cache_returns_replaced_module_exports_for_loaded_cjs() -> Result<()> {
    let output = run_cjs_source(
        r#"
module.exports = { marker: "replacement" };
const self = require('./self.js');
console.log(self.marker);
console.log(self === module.exports);
"#,
        RuntimeOptions {
            module_loader: Some(Arc::new(SelfReferenceLoader)),
            ..RuntimeOptions::default()
        },
    )?;

    assert_eq!(output, "replacement\ntrue\n");
    Ok(())
}

#[test]
fn require_resolve_without_loader_is_catchable() -> Result<()> {
    let output = run_cjs_source(
        r#"
try { require.resolve('./dep.js'); } catch (e) { console.log(e.name, e.message.includes('module loader')); }
"#,
        RuntimeOptions::default(),
    )?;

    assert_eq!(output, "TypeError true\n");
    Ok(())
}

#[test]
fn require_loader_unavailable() -> Result<()> {
    let output = run_cjs_source(
        r#"
try {
    require('./dep.js');
} catch (e) {
    console.log(e.name);
    console.log(e.message.includes('runtime module loader is not installed'));
    console.log(e.message.includes('CommonJS'));
}
"#,
        RuntimeOptions::default(),
    )?;

    assert_eq!(output, "TypeError\ntrue\nfalse\n");
    Ok(())
}

#[test]
fn dynamic_module_loader_unavailable() -> Result<()> {
    let output = run_esm_source(
        r#"
const path = './dep.js';
import(path).catch(e => {
    console.log(e.name);
    console.log(e.message.includes('runtime module loader is not installed'));
    console.log(e.message.includes('CommonJS'));
});
"#,
        RuntimeOptions::default(),
    )?;

    assert_eq!(output, "TypeError\ntrue\nfalse\n");
    Ok(())
}


#[test]
fn dynamic_module_import_expression_uses_loader_namespace() -> Result<()> {
    let output = run_esm_source(
        r#"
const path = './dep.js';
import(path).then(ns => console.log(ns));
"#,
        RuntimeOptions {
            module_loader: Some(Arc::new(DynamicImportLoader)),
            ..RuntimeOptions::default()
        },
    )?;

    assert_eq!(output, "77\n");
    Ok(())
}

#[test]
fn dynamic_module_import_to_string_converts_non_exception_specifier() -> Result<()> {
    let output = run_esm_source(
        r#"
const path = 123;
import(path).then(ns => console.log(ns));
"#,
        RuntimeOptions {
            module_loader: Some(Arc::new(NumericDynamicImportLoader)),
            ..RuntimeOptions::default()
        },
    )?;

    assert_eq!(output, "88\n");
    Ok(())
}

#[test]
fn dynamic_module_import_json_parse_abrupt_rejects_promise() -> Result<()> {
    let output = run_esm_source(
        r#"
try {
    const promise = import(JSON.parse('bad'));
    console.log(typeof promise.then);
    promise.catch(e => console.log('caught', e.name));
    console.log('after');
} catch (e) {
    console.log('sync', e.name);
}
"#,
        RuntimeOptions::default(),
    )?;

    assert_eq!(output, "function\nafter\ncaught SyntaxError\n");
    Ok(())
}

#[test]
fn dynamic_module_import_composed_json_parse_abrupt_rejects_original_reason() -> Result<()> {
    let output = run_esm_source(
        r#"
try {
    const promise = import(JSON.parse('bad') + './never.js');
    console.log(typeof promise.then);
    promise.catch(e => console.log('caught', e.name));
    console.log('after');
} catch (e) {
    console.log('sync', e.name);
}
"#,
        RuntimeOptions::default(),
    )?;

    assert_eq!(output, "function\nafter\ncaught SyntaxError\n");
    Ok(())
}

#[test]
fn dynamic_module_import_conditional_json_parse_abrupt_rejects_original_reason() -> Result<()> {
    let output = run_esm_source(
        r#"
try {
    const promise = import((true ? JSON.parse('bad') : './dep.js') + '?x');
    console.log(typeof promise.then);
    promise.catch(e => console.log('caught', e.name));
    console.log('after');
} catch (e) {
    console.log('sync', e.name);
}
"#,
        RuntimeOptions::default(),
    )?;

    assert_eq!(output, "function\nafter\ncaught SyntaxError\n");
    Ok(())
}

#[test]
fn dynamic_module_import_conditional_normal_completion_uses_selected_specifier() -> Result<()> {
    let output = run_esm_source(
        r#"
const promise = import((false ? JSON.parse('bad') : './dep.js') + '');
console.log(typeof promise.then);
promise.then(ns => console.log('loaded', ns));
"#,
        RuntimeOptions {
            module_loader: Some(Arc::new(DynamicImportLoader)),
            ..RuntimeOptions::default()
        },
    )?;

    assert_eq!(output, "function\nloaded 77\n");
    Ok(())
}

#[test]
fn dynamic_module_import_sequence_json_parse_abrupt_rejects_original_reason() -> Result<()> {
    let output = run_esm_source(
        r#"
try {
    const promise = import((JSON.parse('bad'), './dep.js'));
    console.log(typeof promise.then);
    promise.catch(e => console.log('caught', e.name));
    console.log('after');
} catch (e) {
    console.log('sync', e.name);
}
"#,
        RuntimeOptions::default(),
    )?;

    assert_eq!(output, "function\nafter\ncaught SyntaxError\n");
    Ok(())
}

#[test]
fn dynamic_module_import_sequence_normal_completion_uses_final_specifier() -> Result<()> {
    let output = run_esm_source(
        r#"
let seen = 0;
function sideEffect() { seen = 1; }
const promise = import((sideEffect(), './dep.js'));
console.log(typeof promise.then, seen);
promise.then(ns => console.log('loaded', ns));
"#,
        RuntimeOptions {
            module_loader: Some(Arc::new(DynamicImportLoader)),
            ..RuntimeOptions::default()
        },
    )?;

    assert_eq!(output, "function 1\nloaded 77\n");
    Ok(())
}

#[test]
fn dynamic_module_import_meta_resolve_composed_abrupt_rejects_original_reason() -> Result<()> {
    let output = run_esm_source(
        r#"
try {
    const promise = import(import.meta.resolve('./missing.js') + '?x');
    console.log(typeof promise.then);
    promise.catch(e => console.log('caught', e.name, e.message.includes('missing module')));
    console.log('after');
} catch (e) {
    console.log('sync', e.name, e.message.includes('missing module'));
}
"#,
        RuntimeOptions {
            module_loader: Some(Arc::new(MissingDynamicImportLoader)),
            ..RuntimeOptions::default()
        },
    )?;

    assert_eq!(output, "function\nafter\ncaught TypeError true\n");
    Ok(())
}

#[test]
fn dynamic_module_import_meta_resolve_abrupt_rejects_promise() -> Result<()> {
    let output = run_esm_source(
        r#"
try {
    const promise = import(import.meta.resolve('./missing.js'));
    console.log(typeof promise.then);
    promise.catch(e => console.log('caught', e.name, e.message.includes('missing module')));
    console.log('after');
} catch (e) {
    console.log('sync', e.name, e.message.includes('missing module'));
}
"#,
        RuntimeOptions {
            module_loader: Some(Arc::new(MissingDynamicImportLoader)),
            ..RuntimeOptions::default()
        },
    )?;

    assert_eq!(output, "function\nafter\ncaught TypeError true\n");
    Ok(())
}

#[test]
fn import_meta_resolve_uses_loader_import_condition() -> Result<()> {
    let output = run_esm_source(
        r#"
console.log(import.meta.resolve('./dep.js'));
"#,
        RuntimeOptions {
            module_loader: Some(Arc::new(ImportMetaResolveLoader)),
            ..RuntimeOptions::default()
        },
    )?;

    assert_eq!(output, "file:///project/dep.js\n");
    Ok(())
}

#[test]
fn import_meta_resolve_missing_nested_expression_throws_before_outer_call() -> Result<()> {
    let output = run_esm_source(
        r#"
try {
    console.log(import.meta.resolve('./missing.js'));
    console.log('outer-observed');
} catch (e) {
    console.log(e.name);
    console.log(e.message.includes('missing'));
}
"#,
        RuntimeOptions {
            module_loader: Some(Arc::new(MissingDynamicImportLoader)),
            ..RuntimeOptions::default()
        },
    )?;

    assert_eq!(output, "TypeError\ntrue\n");
    Ok(())
}

#[test]
fn dynamic_module_import_missing_rejects_promise() -> Result<()> {
    let output = run_esm_source(
        r#"
const path = './missing.js';
import(path).catch(e => console.log(e.name, e.message.includes('missing')));
"#,
        RuntimeOptions {
            module_loader: Some(Arc::new(MissingDynamicImportLoader)),
            ..RuntimeOptions::default()
        },
    )?;

    assert_eq!(output, "TypeError true\n");
    Ok(())
}