use std::collections::HashMap;
use std::path::{Path, PathBuf};

use wjsm_artifact_format::{
    ArtifactBuildInput, BuildOptions, ManifestModule, ModuleKind, ModuleManifest, PortableArtifact,
};
use wjsm_ir::{Builtin, ModuleId, is_module_entry_ir_function, value};
use wjsm_module::{
    ResolutionOptions, RuntimeModuleFormat, RuntimeModuleKey, RuntimeResolveKind,
    RuntimeResolvePaths, RuntimeResolvedModule, logical_url_path,
    lower_runtime_builtin_bundle_with_options, lower_runtime_entry_bundle_with_options,
    resolve_runtime_paths, resolve_runtime_specifier,
};
use wjsm_native_abi::{NativeVmContext, PendingExceptionKind};

use super::promise::{
    NativeMicrotask, PromiseState, drain_microtasks, enqueue_microtask, new_promise, settle_promise,
};
use super::runtime::fail_dispatch;
use crate::{NativeAgentState, NativeCallableKind, NativeHostRegistry};

#[derive(Clone, Copy)]
enum CjsModuleStatus {
    Loading,
    Loaded,
    Errored(i64),
}

#[derive(Clone, Copy)]
struct CjsModuleRecord {
    module_object: i64,
    status: CjsModuleStatus,
}

#[derive(Clone, Copy)]
pub(crate) struct NativeScopeBinding {
    pub(crate) value: i64,
    pub(crate) initialized: bool,
    pub(crate) constant: bool,
}

pub(crate) struct NativeScopeRecord {
    bindings: HashMap<u32, NativeScopeBinding>,
    outer: i64,
    super_base: Option<i64>,
    new_target: Option<i64>,
    has_arguments_binding: bool,
    is_strict: bool,
}

pub(crate) enum ScopeBindingRead {
    Missing,
    Uninitialized,
    Value(i64),
}

pub(crate) enum ScopeBindingWrite {
    Missing,
    Constant,
    Updated,
}

pub(crate) struct NativeModuleState {
    root_path: PathBuf,
    options: ResolutionOptions,
    module_keys: HashMap<(u64, ModuleId), RuntimeModuleKey>,
    namespaces: HashMap<RuntimeModuleKey, i64>,
    cjs_modules: HashMap<RuntimeModuleKey, CjsModuleRecord>,
    cjs_cache_object: Option<i64>,
    referrers: Vec<PathBuf>,
    referrer_ids: HashMap<PathBuf, u32>,
}

pub(crate) fn create_scope_record(state: &mut NativeAgentState) -> Option<i64> {
    let record = state.allocate_object(0, false).ok()?;
    state.scope_records.insert(
        value::decode_handle(record),
        NativeScopeRecord {
            bindings: HashMap::new(),
            outer: state.global_object.unwrap_or_else(value::encode_undefined),
            super_base: None,
            new_target: None,
            has_arguments_binding: false,
            is_strict: false,
        },
    );
    Some(record)
}

pub(crate) fn create_scope_record_with_outer(
    state: &mut NativeAgentState,
    outer: i64,
) -> Option<i64> {
    let record = create_scope_record(state)?;
    state
        .scope_records
        .get_mut(&value::decode_handle(record))?
        .outer = outer;
    Some(record)
}

pub(crate) fn scope_record_add(
    state: &mut NativeAgentState,
    record: i64,
    key: i64,
    stored: i64,
    initialized: i64,
    constant: i64,
) -> bool {
    let (Some(record), Some(key)) = (object_handle(record), property_key(state, key)) else {
        return false;
    };
    let Some(scope) = state.scope_records.get_mut(&record) else {
        return false;
    };
    scope.bindings.insert(
        key,
        NativeScopeBinding {
            value: stored,
            initialized: initialized == 0,
            constant: constant != 0,
        },
    );
    true
}

pub(crate) fn scope_record_set(
    state: &mut NativeAgentState,
    record: i64,
    key: i64,
    stored: i64,
) -> ScopeBindingWrite {
    let (Some(record), Some(key)) = (object_handle(record), property_key(state, key)) else {
        return ScopeBindingWrite::Missing;
    };
    let Some(binding) = state
        .scope_records
        .get_mut(&record)
        .and_then(|scope| scope.bindings.get_mut(&key))
    else {
        return ScopeBindingWrite::Missing;
    };
    if binding.constant && binding.initialized {
        return ScopeBindingWrite::Constant;
    }
    binding.value = stored;
    binding.initialized = true;
    ScopeBindingWrite::Updated
}

pub(crate) fn scope_record_get(
    state: &NativeAgentState,
    record: i64,
    key: i64,
) -> ScopeBindingRead {
    let Some(binding) = object_handle(record)
        .and_then(|record| state.scope_records.get(&record))
        .and_then(|scope| property_key(state, key).and_then(|key| scope.bindings.get(&key)))
    else {
        return ScopeBindingRead::Missing;
    };
    if binding.initialized {
        ScopeBindingRead::Value(binding.value)
    } else {
        ScopeBindingRead::Uninitialized
    }
}

pub(crate) fn scope_record_contains(state: &NativeAgentState, record: i64, key: i64) -> bool {
    object_handle(record)
        .and_then(|record| state.scope_records.get(&record))
        .is_some_and(|scope| {
            property_key(state, key).is_some_and(|key| scope.bindings.contains_key(&key))
        })
}

pub(crate) fn scope_record_set_meta(
    state: &mut NativeAgentState,
    record: i64,
    key: i64,
    stored: i64,
) -> bool {
    let Some(scope) = object_handle(record).and_then(|record| state.scope_records.get_mut(&record))
    else {
        return false;
    };
    match value::decode_f64(key) as u8 {
        0 => scope.is_strict = metadata_bool(stored),
        1 => scope.has_arguments_binding = metadata_bool(stored),
        2 => scope.super_base = Some(stored),
        3 => scope.new_target = Some(stored),
        _ => return false,
    }
    true
}

pub(crate) fn scope_record_set_strict(state: &mut NativeAgentState, record: i64, is_strict: bool) {
    if let Some(scope) =
        object_handle(record).and_then(|record| state.scope_records.get_mut(&record))
    {
        scope.is_strict = is_strict;
    }
}

pub(crate) fn scope_record_is_strict(state: &NativeAgentState, record: i64) -> bool {
    object_handle(record)
        .and_then(|record| state.scope_records.get(&record))
        .is_some_and(|scope| scope.is_strict)
}

pub(crate) fn scope_record_outer(state: &NativeAgentState, record: i64) -> Option<i64> {
    object_handle(record)
        .and_then(|record| state.scope_records.get(&record))
        .map(|scope| scope.outer)
}

pub(crate) fn scope_record_super_base(state: &NativeAgentState, record: i64) -> Option<i64> {
    object_handle(record)
        .and_then(|record| state.scope_records.get(&record))
        .and_then(|scope| scope.super_base)
}

pub(crate) fn scope_record_new_target(state: &NativeAgentState, record: i64) -> Option<i64> {
    object_handle(record)
        .and_then(|record| state.scope_records.get(&record))
        .and_then(|scope| scope.new_target)
}

pub(crate) fn destroy_scope_record(state: &mut NativeAgentState, record: i64) {
    if let Some(record) = object_handle(record) {
        state.scope_records.remove(&record);
    }
}
fn scope_record_is_retained_by_closure(state: &NativeAgentState, target: i64) -> bool {
    state.closures.iter().any(|closure| {
        let mut record = closure.environment;
        loop {
            if record == target {
                return true;
            }
            let Some(outer) = scope_record_outer(state, record) else {
                return false;
            };
            record = outer;
        }
    })
}

fn metadata_bool(encoded: i64) -> bool {
    if value::is_bool(encoded) {
        value::decode_bool(encoded)
    } else if value::is_f64(encoded) {
        value::decode_f64(encoded) != 0.0
    } else {
        false
    }
}
fn property_key(state: &NativeAgentState, key: i64) -> Option<u32> {
    value::is_string(key)
        .then(|| value::decode_handle(key))
        .or_else(|| state.string(key).map(|_| value::decode_handle(key)))
}

fn object_handle(value: i64) -> Option<u32> {
    value::is_js_object(value).then(|| value::decode_handle(value))
}

impl Default for NativeModuleState {
    fn default() -> Self {
        Self {
            root_path: PathBuf::from("."),
            options: ResolutionOptions::default(),
            module_keys: HashMap::new(),
            namespaces: HashMap::new(),
            cjs_modules: HashMap::new(),
            cjs_cache_object: None,
            referrers: Vec::new(),
            referrer_ids: HashMap::new(),
        }
    }
}

impl NativeModuleState {
    pub(crate) fn clear(&mut self) {
        self.module_keys.clear();
        self.namespaces.clear();
        self.cjs_modules.clear();
        self.cjs_cache_object = None;
        self.referrers.clear();
        self.referrer_ids.clear();
    }
}

pub(crate) fn configure(
    state: &mut NativeAgentState,
    root_path: &Path,
    image_id: u64,
    manifest: &ModuleManifest,
) -> Result<(), String> {
    let root = root_path
        .canonicalize()
        .unwrap_or_else(|_| root_path.to_path_buf());
    let keys = manifest_entries(&root, image_id, manifest)?;
    state.runtime_modules.root_path = root;
    state.runtime_modules.options =
        ResolutionOptions::new().with_conditions(manifest.resolution_conditions.iter().cloned());
    state.runtime_modules.module_keys.extend(keys);
    Ok(())
}

pub(super) fn dispatch_module(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    builtin: Builtin,
    args: &[i64],
) -> Option<i64> {
    Some(match builtin {
        Builtin::RegisterModuleNamespace => register_namespace(ctx, state, args),
        Builtin::DynamicImport => static_dynamic_import(ctx, state, args),
        Builtin::DynamicImportRuntime => runtime_dynamic_import(ctx, state, args),
        Builtin::ImportMetaResolve => create_import_meta_resolve(ctx, state, args),
        Builtin::CjsCreateRequire => create_require(ctx, state, args),
        Builtin::CjsRegisterModule => register_cjs_module(ctx, state, args),
        _ => return None,
    })
}

pub(crate) fn callable_property(
    state: &mut NativeAgentState,
    callable: i64,
    key: &str,
) -> Option<i64> {
    match state.native_callable_kind(callable)? {
        NativeCallableKind::CjsRequire(referrer) => match key {
            "cache" => ensure_cjs_cache(state),
            "resolve" => state.native_callable(NativeCallableKind::CjsResolve(referrer)),
            _ => None,
        },
        NativeCallableKind::CjsResolve(referrer) if key == "paths" => {
            state.native_callable(NativeCallableKind::CjsResolvePaths(referrer))
        }
        _ => None,
    }
}

pub(crate) fn invoke_module_callable(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    kind: NativeCallableKind,
    args: &[i64],
) -> Option<i64> {
    Some(match kind {
        NativeCallableKind::CjsRequire(referrer) => require(ctx, state, referrer, args),
        NativeCallableKind::CjsResolve(referrer) => resolve_for_require(ctx, state, referrer, args),
        NativeCallableKind::CjsResolvePaths(referrer) => {
            resolve_paths_for_require(ctx, state, referrer, args)
        }
        NativeCallableKind::ImportMetaResolve(referrer) => {
            resolve_for_import_meta(ctx, state, referrer, args)
        }
        _ => return None,
    })
}

pub(crate) fn run_dynamic_import(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    specifier: String,
    referrer: PathBuf,
    promise: u32,
) -> i64 {
    let result = resolve_module(state, &specifier, &referrer, RuntimeResolveKind::Import).and_then(
        |resolved| {
            if resolved.format == RuntimeModuleFormat::Json {
                return Err(ModuleLoadFailure::Message(
                    "JSON import requires import assertions, which are not supported".into(),
                ));
            }
            load_resolved_module(ctx, state, &resolved, true)
        },
    );
    match result {
        Ok(namespace) => settle_promise(state, promise, namespace, false),
        Err(error) => {
            let reason = error.into_value(state);
            settle_promise(state, promise, reason, true);
        }
    }
    value::encode_undefined()
}

fn register_namespace(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let [module_id, namespace] = args else {
        return fail_dispatch(ctx);
    };
    let Some(module_id) = decode_module_id(*module_id) else {
        return fail_dispatch(ctx);
    };
    let Some(key) = state
        .runtime_modules
        .module_keys
        .get(&(state.current_image_id, module_id))
        .cloned()
    else {
        return fail_dispatch(ctx);
    };
    state.runtime_modules.namespaces.insert(key, *namespace);
    value::encode_undefined()
}

fn static_dynamic_import(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let [module_id] = args else {
        return fail_dispatch(ctx);
    };
    let Some(module_id) = decode_module_id(*module_id) else {
        return fail_dispatch(ctx);
    };
    let Some(key) = state
        .runtime_modules
        .module_keys
        .get(&(state.current_image_id, module_id))
    else {
        return fail_dispatch(ctx);
    };
    let Some(namespace) = state.runtime_modules.namespaces.get(key).copied() else {
        return fail_dispatch(ctx);
    };
    let Some(promise) = new_promise(ctx, state) else {
        return fail_dispatch(ctx);
    };
    settle_promise(state, value::decode_handle(promise), namespace, false);
    promise
}

fn runtime_dynamic_import(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let [referrer, specifier] = args else {
        return fail_dispatch(ctx);
    };
    let Some(referrer) = state.string(*referrer).and_then(|text| text.to_utf8()) else {
        return fail_dispatch(ctx);
    };
    let Some(specifier) = state.string(*specifier).and_then(|text| text.to_utf8()) else {
        return fail_dispatch(ctx);
    };
    let referrer = normalize_referrer(state, Path::new(&referrer));
    let Some(promise_value) = new_promise(ctx, state) else {
        return fail_dispatch(ctx);
    };
    enqueue_microtask(
        state,
        NativeMicrotask::DynamicImport {
            specifier,
            referrer,
            promise: value::decode_handle(promise_value),
        },
    );
    promise_value
}

fn create_import_meta_resolve(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let Some(filename) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let Some(filename) = state.string(filename).and_then(|text| text.to_utf8()) else {
        return fail_dispatch(ctx);
    };
    let referrer = intern_referrer(state, normalize_referrer(state, Path::new(&filename)));
    referrer
        .and_then(|referrer| state.native_callable(NativeCallableKind::ImportMetaResolve(referrer)))
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn create_require(ctx: &mut NativeVmContext, state: &mut NativeAgentState, args: &[i64]) -> i64 {
    let Some(filename) = args.first().copied() else {
        return fail_dispatch(ctx);
    };
    let Some(filename) = state.string(filename).and_then(|text| text.to_utf8()) else {
        return fail_dispatch(ctx);
    };
    let path = normalize_referrer(state, Path::new(&filename));
    let Some(referrer) = intern_referrer(state, path) else {
        return fail_dispatch(ctx);
    };
    if ensure_cjs_cache(state).is_none() {
        return fail_dispatch(ctx);
    }
    state
        .native_callable(NativeCallableKind::CjsRequire(referrer))
        .unwrap_or_else(|| fail_dispatch(ctx))
}

fn register_cjs_module(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    args: &[i64],
) -> i64 {
    let [filename, module_object, _initial_exports] = args else {
        return fail_dispatch(ctx);
    };
    let Some(filename) = state.string(*filename).and_then(|text| text.to_utf8()) else {
        return fail_dispatch(ctx);
    };
    let path = normalize_referrer(state, Path::new(&filename));
    let key = RuntimeModuleKey::File(path);
    if cache_module_object(state, &key, *module_object).is_none() {
        return fail_dispatch(ctx);
    }
    value::encode_undefined()
}

fn require(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    referrer: u32,
    args: &[i64],
) -> i64 {
    let Some(specifier) = args.first().and_then(|value| state.string(*value)) else {
        return fail_dispatch(ctx);
    };
    let Some(specifier) = specifier.to_utf8() else {
        return fail_dispatch(ctx);
    };
    let Some(referrer) = referrer_path(state, referrer) else {
        return fail_dispatch(ctx);
    };
    let resolved = match resolve_module(state, &specifier, &referrer, RuntimeResolveKind::Require) {
        Ok(resolved) => resolved,
        Err(error) => {
            return error
                .into_exception(state)
                .unwrap_or_else(|| fail_dispatch(ctx));
        }
    };
    match load_resolved_module(ctx, state, &resolved, false) {
        Ok(exports) => exports,
        Err(error) => error
            .into_exception(state)
            .unwrap_or_else(|| fail_dispatch(ctx)),
    }
}

fn resolve_for_require(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    referrer: u32,
    args: &[i64],
) -> i64 {
    resolve_callable(
        ctx,
        state,
        referrer,
        args,
        RuntimeResolveKind::Require,
        false,
    )
}

fn resolve_for_import_meta(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    referrer: u32,
    args: &[i64],
) -> i64 {
    resolve_callable(ctx, state, referrer, args, RuntimeResolveKind::Import, true)
}

fn resolve_callable(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    referrer: u32,
    args: &[i64],
    kind: RuntimeResolveKind,
    as_url: bool,
) -> i64 {
    let Some(specifier) = args.first().and_then(|value| state.string(*value)) else {
        return fail_dispatch(ctx);
    };
    let Some(specifier) = specifier.to_utf8() else {
        return fail_dispatch(ctx);
    };
    let Some(referrer) = referrer_path(state, referrer) else {
        return fail_dispatch(ctx);
    };
    match resolve_module(state, &specifier, &referrer, kind) {
        Ok(resolved) => {
            let text = if as_url {
                resolved.url
            } else {
                resolved
                    .path
                    .map_or(resolved.url, |path| path.to_string_lossy().into_owned())
            };
            state
                .intern_text(text, value::TAG_STRING)
                .unwrap_or_else(|| fail_dispatch(ctx))
        }
        Err(error) => error
            .into_exception(state)
            .unwrap_or_else(|| fail_dispatch(ctx)),
    }
}

fn resolve_paths_for_require(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    referrer: u32,
    args: &[i64],
) -> i64 {
    let Some(specifier) = args.first().and_then(|value| state.string(*value)) else {
        return fail_dispatch(ctx);
    };
    let Some(specifier) = specifier.to_utf8() else {
        return fail_dispatch(ctx);
    };
    let Some(referrer) = referrer_path(state, referrer) else {
        return fail_dispatch(ctx);
    };
    let root = state.runtime_modules.root_path.clone();
    match resolve_runtime_paths(&specifier, &referrer, &root) {
        RuntimeResolvePaths::Null => value::encode_null(),
        RuntimeResolvePaths::Search(paths) => {
            let Ok(array) =
                state.allocate_object(u32::try_from(paths.len()).unwrap_or(u32::MAX), true)
            else {
                return fail_dispatch(ctx);
            };
            for (index, path) in paths.into_iter().enumerate() {
                let Some(path) =
                    state.intern_text(path.to_string_lossy().into_owned(), value::TAG_STRING)
                else {
                    return fail_dispatch(ctx);
                };
                let Ok(index) = u32::try_from(index) else {
                    return fail_dispatch(ctx);
                };
                if state
                    .heap
                    .set_element(value::decode_handle(array), index, path as u64)
                    .is_err()
                {
                    return fail_dispatch(ctx);
                }
            }
            array
        }
    }
}

fn resolve_module(
    state: &NativeAgentState,
    specifier: &str,
    referrer: &Path,
    kind: RuntimeResolveKind,
) -> Result<RuntimeResolvedModule, ModuleLoadFailure> {
    resolve_runtime_specifier(
        specifier,
        referrer,
        &state.runtime_modules.root_path,
        &state.runtime_modules.options,
        kind,
    )
    .map_err(|error| ModuleLoadFailure::TypeError(error.to_string()))
}

fn load_resolved_module(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    resolved: &RuntimeResolvedModule,
    namespace: bool,
) -> Result<i64, ModuleLoadFailure> {
    if let Some(value) = cached_module_value(state, resolved, namespace) {
        return Ok(value);
    }
    match resolved.format {
        RuntimeModuleFormat::Json => load_json_module(ctx, state, resolved),
        RuntimeModuleFormat::Esm | RuntimeModuleFormat::CommonJs | RuntimeModuleFormat::Builtin => {
            execute_module_bundle(ctx, state, resolved)?;
            cached_module_value(state, resolved, namespace).ok_or_else(|| {
                ModuleLoadFailure::Message(format!(
                    "runtime module '{}' did not register its exports",
                    resolved.url
                ))
            })
        }
    }
}

fn execute_module_bundle(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    resolved: &RuntimeResolvedModule,
) -> Result<(), ModuleLoadFailure> {
    let root = state.runtime_modules.root_path.clone();
    let options = state.runtime_modules.options.clone();
    let bundle = match (&resolved.format, &resolved.key, resolved.path.as_deref()) {
        (RuntimeModuleFormat::Builtin, RuntimeModuleKey::Builtin(specifier), _) => {
            lower_runtime_builtin_bundle_with_options(specifier, &root, options)
        }
        (_, _, Some(path)) => lower_runtime_entry_bundle_with_options(path, &root, options),
        _ => {
            return Err(ModuleLoadFailure::Message(format!(
                "runtime module '{}' has no loadable source",
                resolved.url
            )));
        }
    }
    .map_err(|error| ModuleLoadFailure::Message(error.to_string()))?;
    let entry_module_id = bundle.entry_module_id;
    let manifest = bundle.manifest.clone();
    let input = ArtifactBuildInput::new(bundle.program, bundle.manifest, BuildOptions::default());
    let artifact = PortableArtifact::from_input(&input)
        .map_err(|error| ModuleLoadFailure::Message(error.to_string()))?;
    let image = state
        .repository
        .prepare(&artifact, &NativeHostRegistry)
        .map_err(|error| ModuleLoadFailure::Message(error.to_string()))?;
    let entry = image
        .entries()
        .get(
            artifact
                .program()
                .functions()
                .iter()
                .position(|function| is_module_entry_ir_function(function.name()))
                .unwrap_or(0),
        )
        .ok_or_else(|| ModuleLoadFailure::Message("runtime module entry is missing".into()))?
        .slow_entry;
    let image_id = image.image_id();
    let caller_image_id = state.current_image_id;
    state.install_program(image, artifact.program());
    register_manifest(state, image_id, &manifest).map_err(ModuleLoadFailure::Message)?;
    state
        .runtime_modules
        .module_keys
        .insert((image_id, entry_module_id), resolved.key.clone());
    state
        .activate_image(ctx, image_id)
        .ok_or_else(|| ModuleLoadFailure::Message("runtime module image is missing".into()))?;
    state
        .prepare_entry_call(ctx, caller_image_id)
        .ok_or_else(|| ModuleLoadFailure::Message("Maximum call stack size exceeded".into()))?;
    // SAFETY: entry 属于 state 持有的 RX image，vmctx 已 pinned，且本次调用不传实参。
    let result = unsafe { entry(ctx, 0, value::encode_undefined(), 0, 0) };
    state
        .finish_call(ctx)
        .ok_or_else(|| ModuleLoadFailure::Message("runtime module activation is missing".into()))?;
    if ctx.pending_exception_kind != PendingExceptionKind::None {
        let kind = ctx.pending_exception_kind;
        ctx.pending_exception_kind = PendingExceptionKind::None;
        remove_cached_module(state, &resolved.key);
        return Err(ModuleLoadFailure::Message(format!(
            "runtime module host failure: {kind:?}"
        )));
    }
    if value::is_exception(result) {
        mark_errored_module(state, &resolved.key, result);
        return Err(ModuleLoadFailure::JavaScript(result));
    }
    if state.promises.contains_key(&value::decode_handle(result)) {
        let drained = drain_microtasks(ctx, state);
        if value::is_exception(drained) {
            remove_cached_module(state, &resolved.key);
            return Err(ModuleLoadFailure::JavaScript(drained));
        }
        match state
            .promises
            .get(&value::decode_handle(result))
            .map(|promise| promise.state)
        {
            Some(PromiseState::Rejected(reason)) => {
                remove_cached_module(state, &resolved.key);
                return Err(ModuleLoadFailure::JavaScript(reason));
            }
            Some(PromiseState::Fulfilled(_)) => {}
            Some(PromiseState::Pending) | None => {
                remove_cached_module(state, &resolved.key);
                return Err(ModuleLoadFailure::Message(
                    "runtime module top-level await did not settle".into(),
                ));
            }
        }
    }
    if let Some(record) = state.runtime_modules.cjs_modules.get_mut(&resolved.key) {
        record.status = CjsModuleStatus::Loaded;
    }
    Ok(())
}

fn load_json_module(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    resolved: &RuntimeResolvedModule,
) -> Result<i64, ModuleLoadFailure> {
    let path = resolved.path.as_deref().ok_or_else(|| {
        ModuleLoadFailure::Message(format!("JSON module '{}' has no path", resolved.url))
    })?;
    let text = std::fs::read_to_string(path).map_err(|error| {
        ModuleLoadFailure::Message(format!(
            "failed to read JSON module '{}': {error}",
            path.display()
        ))
    })?;
    let encoded = state
        .intern_text(text, value::TAG_STRING)
        .ok_or_else(|| ModuleLoadFailure::Message("JSON source string table overflow".into()))?;
    let parsed = super::dispatch_builtin(ctx, state, Builtin::JsonParse, &[encoded]);
    if value::is_exception(parsed) {
        return Err(ModuleLoadFailure::JavaScript(parsed));
    }
    let module_object = create_module_object(state, &resolved.url, parsed)?;
    cache_module_object(state, &resolved.key, module_object)
        .ok_or_else(|| ModuleLoadFailure::Message("failed to cache JSON module".into()))?;
    if let Some(record) = state.runtime_modules.cjs_modules.get_mut(&resolved.key) {
        record.status = CjsModuleStatus::Loaded;
    }
    Ok(parsed)
}

fn create_module_object(
    state: &mut NativeAgentState,
    id: &str,
    exports: i64,
) -> Result<i64, ModuleLoadFailure> {
    let module = state
        .allocate_object(4, false)
        .map_err(|error| ModuleLoadFailure::Message(error.to_string()))?;
    for (name, property) in [
        (
            "id",
            state
                .intern_text(id.to_string(), value::TAG_STRING)
                .ok_or_else(|| ModuleLoadFailure::Message("module id overflow".into()))?,
        ),
        ("exports", exports),
        ("loaded", value::encode_bool(true)),
    ] {
        set_named_property(state, module, name, property)?;
    }
    Ok(module)
}

fn cached_module_value(
    state: &mut NativeAgentState,
    resolved: &RuntimeResolvedModule,
    namespace: bool,
) -> Option<i64> {
    if namespace
        || matches!(
            resolved.format,
            RuntimeModuleFormat::Esm | RuntimeModuleFormat::Builtin
        )
    {
        return state
            .runtime_modules
            .namespaces
            .get(&resolved.key)
            .copied()
            .or_else(|| {
                state
                    .runtime_modules
                    .cjs_modules
                    .get(&resolved.key)
                    .copied()
                    .and_then(|record| module_exports(state, record.module_object))
            });
    }
    let record = state
        .runtime_modules
        .cjs_modules
        .get(&resolved.key)
        .copied()?;
    match record.status {
        CjsModuleStatus::Errored(error) => {
            state.runtime_modules.cjs_modules.remove(&resolved.key);
            Some(error)
        }
        CjsModuleStatus::Loading => module_exports(state, record.module_object),
        CjsModuleStatus::Loaded if !cache_contains(state, &resolved.key) => {
            state.runtime_modules.cjs_modules.remove(&resolved.key);
            None
        }
        CjsModuleStatus::Loaded => module_exports(state, record.module_object),
    }
}

fn cache_module_object(
    state: &mut NativeAgentState,
    key: &RuntimeModuleKey,
    module_object: i64,
) -> Option<()> {
    let cache = ensure_cjs_cache(state)?;
    let property = module_cache_key(state, key)?;
    state
        .heap
        .set_property(
            value::decode_handle(cache),
            value::decode_handle(property),
            module_object as u64,
        )
        .ok()?;
    state.runtime_modules.cjs_modules.insert(
        key.clone(),
        CjsModuleRecord {
            module_object,
            status: CjsModuleStatus::Loading,
        },
    );
    Some(())
}

fn ensure_cjs_cache(state: &mut NativeAgentState) -> Option<i64> {
    if let Some(cache) = state.runtime_modules.cjs_cache_object {
        return Some(cache);
    }
    let cache = state.allocate_object(8, false).ok()?;
    state.runtime_modules.cjs_cache_object = Some(cache);
    Some(cache)
}

fn cache_contains(state: &mut NativeAgentState, key: &RuntimeModuleKey) -> bool {
    let Some(cache) = state.runtime_modules.cjs_cache_object else {
        return false;
    };
    let Some(property) = module_cache_key(state, key) else {
        return false;
    };
    state
        .heap
        .get_property_slot(value::decode_handle(cache), value::decode_handle(property))
        .ok()
        .flatten()
        .is_some()
}

fn remove_cached_module(state: &mut NativeAgentState, key: &RuntimeModuleKey) {
    state.runtime_modules.cjs_modules.remove(key);
    state.runtime_modules.namespaces.remove(key);
    let Some(cache) = state.runtime_modules.cjs_cache_object else {
        return;
    };
    let Some(property) = module_cache_key(state, key) else {
        return;
    };
    let _ = state
        .heap
        .delete_property(value::decode_handle(cache), value::decode_handle(property));
}

fn mark_errored_module(state: &mut NativeAgentState, key: &RuntimeModuleKey, error: i64) {
    state.runtime_modules.namespaces.remove(key);
    if let Some(record) = state.runtime_modules.cjs_modules.get_mut(key) {
        record.status = CjsModuleStatus::Errored(error);
    }
    let Some(cache) = state.runtime_modules.cjs_cache_object else {
        return;
    };
    let Some(property) = module_cache_key(state, key) else {
        return;
    };
    let _ = state
        .heap
        .delete_property(value::decode_handle(cache), value::decode_handle(property));
}

fn module_exports(state: &mut NativeAgentState, module_object: i64) -> Option<i64> {
    named_property(state, module_object, "exports")
}

fn module_cache_key(state: &mut NativeAgentState, key: &RuntimeModuleKey) -> Option<i64> {
    let text = match key {
        RuntimeModuleKey::File(path) | RuntimeModuleKey::Json(path) => {
            path.to_string_lossy().into_owned()
        }
        RuntimeModuleKey::Builtin(specifier) => specifier.clone(),
    };
    state.intern_text(text, value::TAG_STRING)
}

pub(crate) fn named_property(state: &mut NativeAgentState, object: i64, name: &str) -> Option<i64> {
    let key = state.intern_text(name.to_string(), value::TAG_STRING)?;
    state
        .heap
        .get_property_slot(value::decode_handle(object), value::decode_handle(key))
        .ok()
        .flatten()
        .map(|slot| slot.value as i64)
}

pub(crate) fn set_named_property(
    state: &mut NativeAgentState,
    object: i64,
    name: &str,
    stored: i64,
) -> Result<(), ModuleLoadFailure> {
    let key = state
        .intern_text(name.to_string(), value::TAG_STRING)
        .ok_or_else(|| ModuleLoadFailure::Message("property key overflow".into()))?;
    state
        .heap
        .set_property(
            value::decode_handle(object),
            value::decode_handle(key),
            stored as u64,
        )
        .map_err(|error| ModuleLoadFailure::Message(error.to_string()))
}

fn register_manifest(
    state: &mut NativeAgentState,
    image_id: u64,
    manifest: &ModuleManifest,
) -> Result<(), String> {
    let keys = manifest_entries(&state.runtime_modules.root_path, image_id, manifest)?;
    state.runtime_modules.module_keys.extend(keys);
    Ok(())
}

fn manifest_entries(
    root: &Path,
    image_id: u64,
    manifest: &ModuleManifest,
) -> Result<Vec<((u64, ModuleId), RuntimeModuleKey)>, String> {
    manifest
        .modules
        .iter()
        .map(|module| manifest_key(root, module).map(|key| ((image_id, module.id), key)))
        .collect()
}

fn manifest_key(root: &Path, module: &ManifestModule) -> Result<RuntimeModuleKey, String> {
    match module.kind {
        ModuleKind::Builtin => Ok(RuntimeModuleKey::Builtin(module.logical_url.clone())),
        ModuleKind::Script | ModuleKind::EsModule | ModuleKind::CommonJs => {
            logical_url_path(root, &module.logical_url)
                .map(RuntimeModuleKey::File)
                .map_err(|error| error.to_string())
        }
    }
}

fn normalize_referrer(state: &NativeAgentState, path: &Path) -> PathBuf {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        state.runtime_modules.root_path.join(path)
    };
    path.canonicalize().unwrap_or(path)
}

fn intern_referrer(state: &mut NativeAgentState, path: PathBuf) -> Option<u32> {
    if let Some(index) = state.runtime_modules.referrer_ids.get(&path).copied() {
        return Some(index);
    }
    let index = u32::try_from(state.runtime_modules.referrers.len()).ok()?;
    state.runtime_modules.referrers.push(path.clone());
    state.runtime_modules.referrer_ids.insert(path, index);
    Some(index)
}

fn referrer_path(state: &NativeAgentState, index: u32) -> Option<PathBuf> {
    state
        .runtime_modules
        .referrers
        .get(usize::try_from(index).ok()?)
        .cloned()
}

fn decode_module_id(encoded: i64) -> Option<ModuleId> {
    let number = value::decode_f64(encoded);
    if !number.is_finite() || number.fract() != 0.0 || number < 0.0 {
        return None;
    }
    let id = u32::try_from(number as u64).ok()?;
    Some(ModuleId(id))
}

pub(crate) fn error_object(state: &mut NativeAgentState, message: String) -> Option<i64> {
    named_error_object(state, "Error", message)
}

pub(crate) fn named_error_object(
    state: &mut NativeAgentState,
    name: &str,
    message: String,
) -> Option<i64> {
    let error = state.allocate_object(3, false).ok()?;
    let prototype = state.ensure_error_prototype(name)?;
    state
        .heap
        .set_prototype(value::decode_handle(error), value::decode_handle(prototype))
        .ok()?;
    initialize_error_object(state, error, name, message)
}

pub(crate) fn initialize_error_object(
    state: &mut NativeAgentState,
    error: i64,
    name: &str,
    message: String,
) -> Option<i64> {
    let stack = if message.is_empty() {
        name.to_owned()
    } else {
        format!("{name}: {message}")
    };
    state.error_objects.insert(value::decode_handle(error));
    let name = state.intern_text(name.into(), value::TAG_STRING)?;
    let message = state.intern_text(message, value::TAG_STRING)?;
    let stack = state.intern_text(stack, value::TAG_STRING)?;
    set_named_property(state, error, "name", name).ok()?;
    set_named_property(state, error, "message", message).ok()?;
    set_named_property(state, error, "stack", stack).ok()?;
    Some(error)
}

pub(crate) fn set_error_stack(
    state: &mut NativeAgentState,
    error: i64,
    stack: String,
) -> Option<()> {
    let stack = state.intern_text(stack, value::TAG_STRING)?;
    set_named_property(state, error, "stack", stack).ok()
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum VmExecutionError {
    #[error(transparent)]
    Compile(anyhow::Error),
    #[error("vm runtime invariant failed: {0}")]
    Invariant(&'static str),
    #[error("vm script host failure: {0:?}")]
    Host(PendingExceptionKind),
    #[error("vm script threw a JavaScript exception")]
    JavaScript(i64),
}

pub(crate) fn execute_vm_script(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    source: &str,
    global: i64,
    logical_url: &str,
) -> Result<i64, VmExecutionError> {
    let environment = create_scope_record_with_outer(state, global).ok_or(
        VmExecutionError::Invariant("script scope allocation failed"),
    )?;
    let result = (|| {
        let (program, is_strict) = compile_vm_script(source, logical_url, false)?;
        scope_record_set_strict(state, environment, is_strict);
        execute_vm_program(ctx, state, program, environment, global, logical_url)
    })();
    if !scope_record_is_retained_by_closure(state, environment) {
        destroy_scope_record(state, environment);
    }
    result
}

pub(crate) fn execute_eval_script(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    source: &str,
    environment: i64,
    global: i64,
    logical_url: &str,
) -> Result<i64, VmExecutionError> {
    let inherited_strict = scope_record_is_strict(state, environment);
    let (program, is_strict) = compile_vm_script(source, logical_url, inherited_strict)?;
    scope_record_set_strict(state, environment, is_strict);
    execute_vm_program(ctx, state, program, environment, global, logical_url)
}

pub(crate) fn compile_vm_function(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    body: &str,
    params: &[String],
    global: i64,
    logical_url: &str,
) -> Result<i64, VmExecutionError> {
    const FUNCTION_PROPERTY: &str = "__wjsm_vm_function__";
    let source = format!(
        "globalThis.{FUNCTION_PROPERTY} = function({}) {{ {body} }};",
        params.join(",")
    );
    execute_vm_script(ctx, state, &source, global, logical_url)?;
    named_property(state, global, FUNCTION_PROPERTY)
        .filter(|function| value::is_callable(*function))
        .ok_or(VmExecutionError::Invariant("compiled function is missing"))
}

fn compile_vm_script(
    source: &str,
    _logical_url: &str,
    inherited_strict: bool,
) -> Result<(wjsm_ir::Program, bool), VmExecutionError> {
    let module = wjsm_parser::parse_script_as_module(source).map_err(VmExecutionError::Compile)?;
    let is_strict =
        inherited_strict || wjsm_semantic::eval_module_has_use_strict_directive(&module);
    let program = wjsm_semantic::lower_eval_module_with_scope_and_strict(
        module,
        true,
        true,
        inherited_strict,
    )
    .map_err(|error| VmExecutionError::Compile(anyhow::Error::new(error)))?;
    program
        .verify()
        .map_err(|error| VmExecutionError::Compile(anyhow::Error::new(error)))?;
    Ok((program, is_strict))
}

fn execute_vm_program(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    program: wjsm_ir::Program,
    environment: i64,
    global: i64,
    logical_url: &str,
) -> Result<i64, VmExecutionError> {
    let artifact = PortableArtifact::from_input(&ArtifactBuildInput::new(
        program,
        ModuleManifest::single(logical_url, true),
        BuildOptions::default(),
    ))
    .map_err(|error| VmExecutionError::Compile(anyhow::Error::new(error)))?;
    let image = state
        .repository
        .prepare(&artifact, &NativeHostRegistry)
        .map_err(|error| VmExecutionError::Compile(anyhow::Error::new(error)))?;
    let entry = image
        .entries()
        .get(
            artifact
                .program()
                .functions()
                .iter()
                .position(|function| is_module_entry_ir_function(function.name()))
                .unwrap_or(0),
        )
        .ok_or(VmExecutionError::Invariant("script entry is missing"))?
        .slow_entry;
    let image_id = image.image_id();
    let caller_image_id = state.current_image_id;
    state.install_program(image, artifact.program());
    state
        .activate_image(ctx, image_id)
        .ok_or(VmExecutionError::Invariant("script image is missing"))?;
    state
        .prepare_entry_call(ctx, caller_image_id)
        .ok_or(VmExecutionError::Invariant("call stack limit exceeded"))?;
    let previous_global = state.global_object.replace(global);
    let result = unsafe { entry(ctx, environment, value::encode_undefined(), 0, 0) };
    state.global_object = previous_global;
    state
        .finish_call(ctx)
        .ok_or(VmExecutionError::Invariant("script activation is missing"))?;
    if ctx.pending_exception_kind != PendingExceptionKind::None {
        let kind = ctx.pending_exception_kind;
        ctx.pending_exception_kind = PendingExceptionKind::None;
        return Err(VmExecutionError::Host(kind));
    }
    if value::is_exception(result) {
        return Err(VmExecutionError::JavaScript(result));
    }
    Ok(result)
}

pub(crate) enum ModuleLoadFailure {
    JavaScript(i64),
    Message(String),
    TypeError(String),
}

impl ModuleLoadFailure {
    fn into_value(self, state: &mut NativeAgentState) -> i64 {
        match self {
            Self::JavaScript(value) if value::is_exception(value) => {
                state.exception_value(value).unwrap_or(value)
            }
            Self::JavaScript(value) => value,
            Self::Message(message) => {
                error_object(state, message).unwrap_or_else(value::encode_undefined)
            }
            Self::TypeError(message) => named_error_object(state, "TypeError", message)
                .unwrap_or_else(value::encode_undefined),
        }
    }

    fn into_exception(self, state: &mut NativeAgentState) -> Option<i64> {
        match self {
            Self::JavaScript(value) if value::is_exception(value) => Some(value),
            Self::JavaScript(value) => state.create_exception(value),
            Self::Message(message) => {
                let error = error_object(state, message)?;
                state.create_exception(error)
            }
            Self::TypeError(message) => {
                let error = named_error_object(state, "TypeError", message)?;
                state.create_exception(error)
            }
        }
    }
}

pub(crate) fn exception_text(state: &mut NativeAgentState, exception: i64) -> String {
    let value = state.exception_value(exception).unwrap_or(exception);
    let Some(name) = named_property(state, value, "name")
        .and_then(|name| state.string(name))
        .and_then(|name| name.to_utf8())
    else {
        return super::runtime::render_value(state, value);
    };
    if let Some(stack) = named_property(state, value, "stack")
        .and_then(|stack| state.string(stack))
        .and_then(|stack| stack.to_utf8())
    {
        return stack;
    }
    let message = named_property(state, value, "message")
        .and_then(|message| state.string(message))
        .and_then(|message| message.to_utf8())
        .unwrap_or_default();
    if message.is_empty() {
        name
    } else {
        format!("{name}: {message}")
    }
}

pub(crate) fn named_exception_text(state: &mut NativeAgentState, exception: i64) -> String {
    let value = state.exception_value(exception).unwrap_or(exception);
    let name = named_property(state, value, "name")
        .and_then(|name| state.string(name))
        .and_then(|name| name.to_utf8())
        .unwrap_or_else(|| "Error".into());
    if let Some(stack) = named_property(state, value, "stack")
        .and_then(|stack| state.string(stack))
        .and_then(|stack| stack.to_utf8())
    {
        return stack;
    }
    let message = named_property(state, value, "message")
        .and_then(|message| state.string(message))
        .and_then(|message| message.to_utf8())
        .unwrap_or_else(|| super::runtime::render_value(state, value));
    if message.is_empty() {
        name
    } else {
        format!("{name}: {message}")
    }
}
