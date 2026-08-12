use std::path::PathBuf;

use wjsm_host::{
    CjsRequireCacheTrapKind, ExecContext, NativeCallableRef, PromiseSettlement,
    RuntimeInstantiationEnv, RuntimeModuleFormat, RuntimeModuleImportResult,
    RuntimeModuleLoadError, RuntimeModuleLoadErrorCode, RuntimeModuleReferrer,
    RuntimeModuleRequireResult, RuntimeModuleResolutionKind, RuntimeResolvedModule, Value,
};
use wjsm_ir::value;

pub fn cjs_require_property<E: ExecContext>(
    ctx: &mut E,
    referrer: RuntimeModuleReferrer,
    property: &str,
) -> Option<Value> {
    match property {
        "resolve" => {
            Some(ctx.create_native_callable(NativeCallableRef::CjsRequireResolve { referrer }))
        }
        "cache" => Some(ctx.create_require_cache_proxy()),
        _ => None,
    }
}

pub fn cjs_require_resolve_property<E: ExecContext>(
    ctx: &mut E,
    referrer: RuntimeModuleReferrer,
    property: &str,
) -> Option<Value> {
    (property == "paths")
        .then(|| ctx.create_native_callable(NativeCallableRef::CjsRequireResolvePaths { referrer }))
}

pub fn call_cjs_require<E: ExecContext>(
    ctx: &mut E,
    referrer: RuntimeModuleReferrer,
    args: &[Value],
) -> Value {
    let specifier = match require_specifier_to_string(ctx, args.first().copied()) {
        Ok(specifier) => specifier,
        Err(exception) => return exception,
    };
    let resolved = match ctx.module_resolve(
        referrer.clone(),
        &specifier,
        RuntimeModuleResolutionKind::Require,
    ) {
        Ok(resolved) => resolved,
        Err(error) => return module_load_error_exception(ctx, &specifier, error),
    };
    match require_cache_result(ctx, &resolved) {
        RuntimeModuleRequireResult::Missing => {
            let env = RuntimeInstantiationEnv::new(referrer);
            match ctx.module_instantiate(resolved.clone(), env) {
                Ok(instantiated) => {
                    let exports = instantiated.exports_object;
                    ctx.module_finish_loaded(resolved.key, instantiated);
                    exports
                }
                Err(error) => module_load_error_exception(ctx, &specifier, error),
            }
        }
        RuntimeModuleRequireResult::Exports(exports) => exports,
        RuntimeModuleRequireResult::LoadedModule {
            module_object,
            exports_object,
        } => loaded_module_exports(ctx, module_object, exports_object),
        RuntimeModuleRequireResult::Errored(error) => as_exception(ctx, error),
    }
}

pub fn create_import_meta_resolve<E: ExecContext>(ctx: &mut E, filename: Value) -> Value {
    let referrer = match module_referrer_from_value(ctx, filename) {
        Ok(referrer) => referrer,
        Err(exception) => return exception,
    };
    ctx.create_native_callable(NativeCallableRef::ImportMetaResolve { referrer })
}

pub fn call_import_meta_resolve<E: ExecContext>(
    ctx: &mut E,
    referrer: RuntimeModuleReferrer,
    args: &[Value],
) -> Value {
    let specifier = match require_specifier_to_string(ctx, args.first().copied()) {
        Ok(specifier) => specifier,
        Err(exception) => return exception,
    };
    match ctx.module_resolve(referrer, &specifier, RuntimeModuleResolutionKind::Import) {
        Ok(resolved) => ctx.store_string_owned(resolved.url),
        Err(error) => module_load_error_exception(ctx, &specifier, error),
    }
}

pub fn call_runtime_dynamic_import<E: ExecContext>(
    ctx: &mut E,
    referrer_value: Value,
    specifier_value: Value,
) -> Value {
    let promise = ctx.alloc_dynamic_import_promise();
    let referrer = match module_referrer_from_value(ctx, referrer_value) {
        Ok(referrer) => referrer,
        Err(exception) => {
            reject_with_value(ctx, promise, exception);
            return promise;
        }
    };
    let specifier = match js_to_string(ctx, specifier_value) {
        Ok(specifier) => specifier,
        Err(exception) => {
            reject_with_value(ctx, promise, exception);
            return promise;
        }
    };
    let resolved = match ctx.module_resolve(
        referrer.clone(),
        &specifier,
        RuntimeModuleResolutionKind::Import,
    ) {
        Ok(resolved) => resolved,
        Err(error) => {
            let exception = module_load_error_exception(ctx, &specifier, error);
            reject_with_value(ctx, promise, exception);
            return promise;
        }
    };
    if resolved.format == RuntimeModuleFormat::Json {
        let error = RuntimeModuleLoadError::new(
            RuntimeModuleLoadErrorCode::Unsupported,
            "runtime JSON import is unsupported without import assertions",
        );
        let exception = module_load_error_exception(ctx, &specifier, error);
        reject_with_value(ctx, promise, exception);
        return promise;
    }
    match ctx.module_cached_import(&resolved.key) {
        RuntimeModuleImportResult::Namespace(namespace) => ctx.resolve_promise(promise, namespace),
        RuntimeModuleImportResult::Errored(error) => reject_with_value(ctx, promise, error),
        RuntimeModuleImportResult::Missing => {
            let env = RuntimeInstantiationEnv::new(referrer);
            match ctx.module_instantiate(resolved.clone(), env) {
                Ok(instantiated) => {
                    let namespace = instantiated.namespace_object;
                    ctx.module_finish_loaded(resolved.key, instantiated);
                    ctx.resolve_promise(promise, namespace);
                }
                Err(error) => {
                    let exception = module_load_error_exception(ctx, &specifier, error);
                    let reason = ctx.exception_reason(exception);
                    ctx.module_finish_errored(resolved.key, None, reason);
                    ctx.settle_promise(promise, PromiseSettlement::Reject(reason));
                }
            }
        }
    }
    promise
}

pub fn call_cjs_require_resolve<E: ExecContext>(
    ctx: &mut E,
    referrer: RuntimeModuleReferrer,
    args: &[Value],
) -> Value {
    let specifier = match require_specifier_to_string(ctx, args.first().copied()) {
        Ok(specifier) => specifier,
        Err(exception) => return exception,
    };
    match ctx.module_resolve(referrer, &specifier, RuntimeModuleResolutionKind::Require) {
        Ok(resolved) => {
            let resolved_id = resolved
                .path
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or(resolved.url);
            ctx.store_string_owned(resolved_id)
        }
        Err(error) => module_load_error_exception(ctx, &specifier, error),
    }
}

pub fn call_cjs_require_resolve_paths<E: ExecContext>(
    ctx: &mut E,
    referrer: RuntimeModuleReferrer,
    args: &[Value],
) -> Value {
    let specifier = match require_specifier_to_string(ctx, args.first().copied()) {
        Ok(specifier) => specifier,
        Err(exception) => return exception,
    };
    match ctx.module_resolve_paths(referrer, &specifier) {
        Ok(Some(paths)) => paths_array(ctx, paths),
        Ok(None) => value::encode_null(),
        Err(error) => module_load_error_exception(ctx, &specifier, error),
    }
}

pub fn call_cjs_require_cache_trap<E: ExecContext>(
    ctx: &mut E,
    kind: CjsRequireCacheTrapKind,
    args: &[Value],
) -> Value {
    match kind {
        CjsRequireCacheTrapKind::Get => cache_key(ctx, args)
            .ok()
            .flatten()
            .and_then(|key| ctx.module_require_cache_entry(&key))
            .map(|entry| entry.module_object)
            .unwrap_or_else(value::encode_undefined),
        CjsRequireCacheTrapKind::Has => {
            let exists = cache_key(ctx, args)
                .ok()
                .flatten()
                .is_some_and(|key| ctx.module_require_cache_entry(&key).is_some());
            value::encode_bool(exists)
        }
        CjsRequireCacheTrapKind::DeleteProperty => {
            let key = match cache_key(ctx, args) {
                Ok(Some(key)) => key,
                Ok(None) => return value::encode_bool(true),
                Err(exception) => return exception,
            };
            value::encode_bool(ctx.module_delete_require_cache_entry(&key))
        }
        CjsRequireCacheTrapKind::OwnKeys => require_cache_keys_array(ctx),
        CjsRequireCacheTrapKind::GetOwnPropertyDescriptor => {
            let Ok(Some(key)) = cache_key(ctx, args) else {
                return value::encode_undefined();
            };
            require_cache_descriptor(ctx, &key)
        }
    }
}

fn require_cache_result<E: ExecContext>(
    ctx: &mut E,
    resolved: &RuntimeResolvedModule,
) -> RuntimeModuleRequireResult {
    ctx.module_cached_require(&resolved.key)
}

fn require_specifier_to_string<E: ExecContext>(
    ctx: &mut E,
    specifier: Option<Value>,
) -> Result<String, Value> {
    js_to_string(ctx, specifier.unwrap_or_else(value::encode_undefined))
}

fn js_to_string<E: ExecContext>(ctx: &mut E, raw: Value) -> Result<String, Value> {
    if value::is_exception(raw) {
        return Err(raw);
    }
    if value::is_symbol(raw) {
        return Err(ctx.make_type_error("TypeError: Cannot convert a Symbol value to a string"));
    }
    Ok(ctx.render_value(raw))
}

fn module_referrer_from_value<E: ExecContext>(
    ctx: &mut E,
    raw: Value,
) -> Result<RuntimeModuleReferrer, Value> {
    if value::is_undefined(raw) || value::is_null(raw) {
        return Ok(RuntimeModuleReferrer::None);
    }
    let filename = js_to_string(ctx, raw)?;
    if filename.is_empty() {
        Ok(RuntimeModuleReferrer::None)
    } else {
        Ok(RuntimeModuleReferrer::Path(PathBuf::from(filename)))
    }
}

fn module_load_error_exception<E: ExecContext>(
    ctx: &mut E,
    specifier: &str,
    error: RuntimeModuleLoadError,
) -> Value {
    let message = match error.code {
        RuntimeModuleLoadErrorCode::NotFound => {
            format!("Error: Cannot find module '{specifier}': {}", error.message)
        }
        _ => format!("Error: {}", error.message),
    };
    ctx.make_type_error(&message)
}

fn as_exception<E: ExecContext>(ctx: &mut E, error: Value) -> Value {
    if value::is_exception(error) {
        error
    } else {
        ctx.make_exception(error)
    }
}

fn reject_with_value<E: ExecContext>(ctx: &mut E, promise: Value, raw: Value) {
    let reason = if value::is_exception(raw) {
        ctx.exception_reason(raw)
    } else {
        raw
    };
    ctx.settle_promise(promise, PromiseSettlement::Reject(reason));
}

fn loaded_module_exports<E: ExecContext>(
    ctx: &mut E,
    module_object: Value,
    fallback: Value,
) -> Value {
    ctx.read_property_for_render(module_object, "exports")
        .unwrap_or(fallback)
}

fn paths_array<E: ExecContext>(ctx: &mut E, paths: Vec<PathBuf>) -> Value {
    let length = paths.len() as u32;
    let array = ctx.alloc_array(length);
    let root_length = ctx.push_host_temp_roots(&[array]);
    for (index, path) in paths.into_iter().enumerate() {
        let path = ctx.store_string_owned(path.to_string_lossy().into_owned());
        ctx.array_write_elem(array, index as u32, path);
    }
    ctx.array_write_length(array, length);
    ctx.truncate_host_temp_roots(root_length);
    array
}

fn cache_key<E: ExecContext>(ctx: &mut E, args: &[Value]) -> Result<Option<String>, Value> {
    let Some(raw) = args.get(1).copied().or_else(|| args.first().copied()) else {
        return Ok(None);
    };
    if value::is_symbol(raw) {
        return Ok(None);
    }
    js_to_string(ctx, raw).map(Some)
}

fn require_cache_keys_array<E: ExecContext>(ctx: &mut E) -> Value {
    let entries = ctx.module_require_cache_entries();
    let length = entries.len() as u32;
    let array = ctx.alloc_array(length);
    let root_length = ctx.push_host_temp_roots(&[array]);
    for (index, entry) in entries.into_iter().enumerate() {
        let key = ctx.store_string_owned(entry.id);
        ctx.array_write_elem(array, index as u32, key);
    }
    ctx.array_write_length(array, length);
    ctx.truncate_host_temp_roots(root_length);
    array
}

fn require_cache_descriptor<E: ExecContext>(ctx: &mut E, key: &str) -> Value {
    let Some(entry) = ctx.module_require_cache_entry(key) else {
        return value::encode_undefined();
    };
    let descriptor = ctx.alloc_object(4);
    let Some(handle) = ctx.handle_index_of(descriptor) else {
        return value::encode_undefined();
    };
    ctx.set_property(handle, "enumerable", value::encode_bool(true));
    ctx.set_property(handle, "configurable", value::encode_bool(true));
    ctx.set_property(handle, "writable", value::encode_bool(true));
    ctx.set_property(handle, "value", entry.module_object);
    descriptor
}
