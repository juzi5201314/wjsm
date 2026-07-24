use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use wasmtime::Store;
use wasmtime::{Caller, Func, Linker};

use crate::runtime_module_registry::RuntimeModuleImportResult;
use crate::*;

pub(crate) fn define_modules(
    linker: &mut Linker<RuntimeState>,
    mut store: &mut Store<RuntimeState>,
) -> Result<()> {
    let create_require_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, filename_val: i64| -> i64 {
            let filename = js_to_string(&mut caller, filename_val).unwrap_or_default();
            create_native_callable(
                caller.data(),
                NativeCallable::CjsRequire {
                    referrer: RuntimeModuleReferrer::Path(PathBuf::from(filename)),
                },
            )
        },
    );
    linker.define(&mut store, "env", "cjs_create_require", create_require_fn)?;

    let register_module_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>,
         filename_val: i64,
         module_obj: i64,
         exports_obj: i64| {
            let filename = js_to_string(&mut caller, filename_val).unwrap_or_default();
            let key = RuntimeModuleKey::File(PathBuf::from(filename));
            let mut registry = caller
                .data()
                .module_registry
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if registry.is_loading(&key) {
                registry.begin_loading(key, None, module_obj, exports_obj);
            } else {
                registry.finish_loaded(key, None, module_obj, exports_obj, exports_obj);
            }
        },
    );
    linker.define(&mut store, "env", "cjs_register_module", register_module_fn)?;

    let import_meta_resolve_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, filename_val: i64| -> i64 {
            create_import_meta_resolve(&mut caller, filename_val)
        },
    );
    linker.define(
        &mut store,
        "env",
        "import_meta_resolve",
        import_meta_resolve_fn,
    )?;

    linker.func_wrap_async(
        "env",
        "dynamic_import_runtime",
        |mut caller: Caller<'_, RuntimeState>, (referrer_val, specifier_val): (i64, i64)| {
            Box::new(async move {
                call_runtime_dynamic_import(&mut caller, referrer_val, specifier_val).await
            })
        },
    )?;

    Ok(())
}

pub(crate) fn cjs_require_property(
    caller: &mut Caller<'_, RuntimeState>,
    referrer: RuntimeModuleReferrer,
    prop_name: &str,
) -> Option<i64> {
    match prop_name {
        "resolve" => Some(create_native_callable(
            caller.data(),
            NativeCallable::CjsRequireResolve { referrer },
        )),
        "cache" => Some(create_require_cache_proxy(caller)),
        _ => None,
    }
}

pub(crate) fn cjs_require_resolve_property(
    caller: &mut Caller<'_, RuntimeState>,
    referrer: RuntimeModuleReferrer,
    prop_name: &str,
) -> Option<i64> {
    if prop_name == "paths" {
        return Some(create_native_callable(
            caller.data(),
            NativeCallable::CjsRequireResolvePaths { referrer },
        ));
    }
    None
}

pub(crate) fn call_cjs_require(
    caller: &mut Caller<'_, RuntimeState>,
    referrer: RuntimeModuleReferrer,
    args: Vec<i64>,
) -> i64 {
    let specifier = match require_specifier_to_string(caller, args.first().copied()) {
        Ok(specifier) => specifier,
        Err(exception) => return exception,
    };
    let loader = match module_loader(caller) {
        Ok(loader) => loader,
        Err(exception) => return exception,
    };
    let resolved = match loader.resolve_for_runtime(
        referrer.clone(),
        &specifier,
        RuntimeModuleResolutionKind::Require,
    ) {
        Ok(resolved) => resolved,
        Err(error) => return module_load_error_exception(caller, &specifier, error),
    };

    match cached_require_result(caller, &resolved.key) {
        RuntimeModuleRequireResult::Exports(exports) => exports,
        RuntimeModuleRequireResult::LoadedModule {
            module_object,
            exports_object,
        } => loaded_module_exports(caller, module_object, exports_object),
        RuntimeModuleRequireResult::Errored(error) => {
            if value::is_exception(error) {
                error
            } else {
                make_exception_value(caller, error)
            }
        }
        RuntimeModuleRequireResult::Missing => {
            let env = RuntimeInstantiationEnv::new(referrer);
            let instantiated = match loader.instantiate_runtime_module(&resolved, env) {
                Ok(instantiated) => instantiated,
                Err(error) => return module_load_error_exception(caller, &specifier, error),
            };
            let mut registry = caller
                .data()
                .module_registry
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            registry.finish_loaded(
                resolved.key,
                instantiated.module_id,
                instantiated.module_object,
                instantiated.exports_object,
                instantiated.namespace_object,
            );
            instantiated.exports_object
        }
    }
}

pub(crate) async fn call_cjs_require_async(
    caller: &mut Caller<'_, RuntimeState>,
    referrer: RuntimeModuleReferrer,
    args: Vec<i64>,
) -> i64 {
    let specifier = match require_specifier_to_string(caller, args.first().copied()) {
        Ok(specifier) => specifier,
        Err(exception) => return exception,
    };
    let loader = match module_loader(caller) {
        Ok(loader) => loader,
        Err(exception) => return exception,
    };
    let resolved = match loader.resolve_for_runtime(
        referrer.clone(),
        &specifier,
        RuntimeModuleResolutionKind::Require,
    ) {
        Ok(resolved) => resolved,
        Err(error) => return module_load_error_exception(caller, &specifier, error),
    };

    match cached_require_result(caller, &resolved.key) {
        RuntimeModuleRequireResult::Exports(exports) => exports,
        RuntimeModuleRequireResult::LoadedModule {
            module_object,
            exports_object,
        } => loaded_module_exports(caller, module_object, exports_object),
        RuntimeModuleRequireResult::Errored(error) => {
            if value::is_exception(error) {
                error
            } else {
                make_exception_value(caller, error)
            }
        }
        RuntimeModuleRequireResult::Missing => {
            let env = RuntimeInstantiationEnv::new(referrer);
            let context = RuntimeModuleInstantiationContext::new(caller);
            let instantiated = match loader
                .instantiate_runtime_module_with_context(&resolved, env, context)
                .await
            {
                Ok(instantiated) => instantiated,
                Err(error) => return module_load_error_exception(caller, &specifier, error),
            };
            let mut registry = caller
                .data()
                .module_registry
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            registry.finish_loaded(
                resolved.key,
                instantiated.module_id,
                instantiated.module_object,
                instantiated.exports_object,
                instantiated.namespace_object,
            );
            instantiated.exports_object
        }
    }
}

pub(crate) fn create_import_meta_resolve(
    caller: &mut Caller<'_, RuntimeState>,
    filename_val: i64,
) -> i64 {
    let referrer = match module_referrer_from_value(caller, filename_val) {
        Ok(referrer) => referrer,
        Err(exception) => return exception,
    };
    create_native_callable(
        caller.data(),
        NativeCallable::ImportMetaResolve { referrer },
    )
}

pub(crate) fn call_import_meta_resolve(
    caller: &mut Caller<'_, RuntimeState>,
    referrer: RuntimeModuleReferrer,
    args: Vec<i64>,
) -> i64 {
    let specifier = match require_specifier_to_string(caller, args.first().copied()) {
        Ok(specifier) => specifier,
        Err(exception) => return exception,
    };
    let loader = match module_loader(caller) {
        Ok(loader) => loader,
        Err(exception) => return exception,
    };
    let resolved =
        match loader.resolve_for_runtime(referrer, &specifier, RuntimeModuleResolutionKind::Import)
        {
            Ok(resolved) => resolved,
            Err(error) => return module_load_error_exception(caller, &specifier, error),
        };
    store_runtime_string(caller, resolved.url)
}

pub(crate) async fn call_runtime_dynamic_import(
    caller: &mut Caller<'_, RuntimeState>,
    referrer_val: i64,
    specifier_val: i64,
) -> i64 {
    let promise = alloc_dynamic_import_promise(caller);
    let referrer = match module_referrer_from_value(caller, referrer_val) {
        Ok(referrer) => referrer,
        Err(exception) => {
            reject_dynamic_import_with_value(caller, promise, exception);
            return promise;
        }
    };
    let specifier = match js_to_string(caller, specifier_val) {
        Ok(specifier) => specifier,
        Err(exception) => {
            reject_dynamic_import_with_value(caller, promise, exception);
            return promise;
        }
    };
    let loader = match module_loader(caller) {
        Ok(loader) => loader,
        Err(exception) => {
            reject_dynamic_import_with_value(caller, promise, exception);
            return promise;
        }
    };
    let resolved = match loader.resolve_for_runtime(
        referrer.clone(),
        &specifier,
        RuntimeModuleResolutionKind::Import,
    ) {
        Ok(resolved) => resolved,
        Err(error) => {
            let exception = module_load_error_exception(caller, &specifier, error);
            reject_dynamic_import_with_value(caller, promise, exception);
            return promise;
        }
    };

    if resolved.format == RuntimeModuleFormat::Json {
        let exception = module_load_error_exception(
            caller,
            &specifier,
            RuntimeModuleLoadError::new(
                RuntimeModuleLoadErrorCode::Unsupported,
                "runtime JSON import is unsupported without import assertions",
            ),
        );
        reject_dynamic_import_with_value(caller, promise, exception);
        return promise;
    }

    match cached_import_result(caller, &resolved.key) {
        RuntimeModuleImportResult::Namespace(namespace) => {
            resolve_promise_from_caller(caller, promise, namespace);
        }
        RuntimeModuleImportResult::Errored(error) => {
            reject_dynamic_import_with_value(caller, promise, error);
        }
        RuntimeModuleImportResult::Missing => {
            let env = RuntimeInstantiationEnv::new(referrer);
            let context = RuntimeModuleInstantiationContext::new(caller);
            let instantiated = match loader
                .instantiate_runtime_module_with_context(&resolved, env, context)
                .await
            {
                Ok(instantiated) => instantiated,
                Err(error) => {
                    let exception = module_load_error_exception(caller, &specifier, error);
                    let reason = rejection_reason(caller, exception);
                    let mut registry = caller
                        .data()
                        .module_registry
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    registry.finish_errored(resolved.key, None, reason);
                    drop(registry);
                    settle_promise(caller.data(), promise, PromiseSettlement::Reject(reason));
                    return promise;
                }
            };
            let namespace = instantiated.namespace_object;
            let mut registry = caller
                .data()
                .module_registry
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            registry.finish_loaded(
                resolved.key,
                instantiated.module_id,
                instantiated.module_object,
                instantiated.exports_object,
                namespace,
            );
            drop(registry);
            resolve_promise_from_caller(caller, promise, namespace);
        }
    }

    promise
}

pub(crate) fn call_cjs_require_resolve(
    caller: &mut Caller<'_, RuntimeState>,
    referrer: RuntimeModuleReferrer,
    args: Vec<i64>,
) -> i64 {
    let specifier = match require_specifier_to_string(caller, args.first().copied()) {
        Ok(specifier) => specifier,
        Err(exception) => return exception,
    };
    let loader = match module_loader(caller) {
        Ok(loader) => loader,
        Err(exception) => return exception,
    };
    let resolved = match loader.resolve_for_runtime(
        referrer,
        &specifier,
        RuntimeModuleResolutionKind::Require,
    ) {
        Ok(resolved) => resolved,
        Err(error) => return module_load_error_exception(caller, &specifier, error),
    };
    let resolved_id = resolved
        .path
        .as_ref()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or(resolved.url);
    store_runtime_string(caller, resolved_id)
}

pub(crate) fn call_cjs_require_resolve_paths(
    caller: &mut Caller<'_, RuntimeState>,
    referrer: RuntimeModuleReferrer,
    args: Vec<i64>,
) -> i64 {
    let specifier = match require_specifier_to_string(caller, args.first().copied()) {
        Ok(specifier) => specifier,
        Err(exception) => return exception,
    };
    let loader = match module_loader(caller) {
        Ok(loader) => loader,
        Err(exception) => return exception,
    };
    let paths = match loader.resolve_paths_for_runtime(referrer, &specifier) {
        Ok(paths) => paths,
        Err(error) => return module_load_error_exception(caller, &specifier, error),
    };
    let Some(paths) = paths else {
        return value::encode_null();
    };
    paths_array(caller, paths)
}

pub(crate) fn call_cjs_require_cache_trap(
    caller: &mut Caller<'_, RuntimeState>,
    kind: CjsRequireCacheTrapKind,
    args: &[i64],
) -> i64 {
    match kind {
        CjsRequireCacheTrapKind::Get => {
            let Ok(Some(cache_key)) = require_cache_key_from_trap_args(caller, args) else {
                return value::encode_undefined();
            };
            require_cache_entry(caller, &cache_key)
                .map(|entry| entry.module_object)
                .unwrap_or_else(value::encode_undefined)
        }
        CjsRequireCacheTrapKind::Has => {
            let exists = require_cache_key_from_trap_args(caller, args)
                .ok()
                .flatten()
                .is_some_and(|cache_key| require_cache_entry(caller, &cache_key).is_some());
            value::encode_bool(exists)
        }
        CjsRequireCacheTrapKind::DeleteProperty => {
            let cache_key = match require_cache_key_from_trap_args(caller, args) {
                Ok(Some(cache_key)) => cache_key,
                Ok(None) => return value::encode_bool(true),
                Err(exception) => return exception,
            };
            let deleted = {
                let mut registry = caller
                    .data()
                    .module_registry
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                registry.delete_cache_entry_by_id(&cache_key)
            };
            value::encode_bool(deleted)
        }
        CjsRequireCacheTrapKind::OwnKeys => require_cache_keys_array(caller),
        CjsRequireCacheTrapKind::GetOwnPropertyDescriptor => {
            let Ok(Some(cache_key)) = require_cache_key_from_trap_args(caller, args) else {
                return value::encode_undefined();
            };
            require_cache_descriptor(caller, &cache_key)
        }
    }
}

fn require_cache_key_from_trap_args(
    caller: &mut Caller<'_, RuntimeState>,
    args: &[i64],
) -> std::result::Result<Option<String>, i64> {
    let Some(cache_key_val) = args.get(1).copied().or_else(|| args.first().copied()) else {
        return Ok(None);
    };
    if value::is_symbol(cache_key_val) {
        return Ok(None);
    }
    js_to_string(caller, cache_key_val).map(Some)
}

fn require_cache_entry(
    caller: &mut Caller<'_, RuntimeState>,
    cache_key: &str,
) -> Option<RuntimeRequireCacheEntry> {
    let registry = caller
        .data()
        .module_registry
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    registry.require_cache_entry_by_id(cache_key)
}

fn require_cache_keys_array(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    let keys = {
        let registry = caller
            .data()
            .module_registry
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        registry
            .require_cache_entries()
            .into_iter()
            .map(|entry| entry.id)
            .collect::<Vec<_>>()
    };
    let len = keys.len() as u32;
    let array = alloc_array(caller, len);
    let root_len = caller.data().push_host_temp_roots([array]);
    if let Some(ptr) = resolve_array_ptr(caller, array) {
        for (index, key) in keys.into_iter().enumerate() {
            let key_value = store_runtime_string(caller, key);
            write_array_elem(caller, ptr, index as u32, key_value);
        }
        write_array_length(caller, ptr, len);
    }
    caller.data().truncate_host_temp_roots(root_len);
    array
}

fn require_cache_descriptor(caller: &mut Caller<'_, RuntimeState>, cache_key: &str) -> i64 {
    let Some(entry) = require_cache_entry(caller, cache_key) else {
        return value::encode_undefined();
    };
    allocate_descriptor_object(
        caller,
        false,
        entry.module_object,
        true,
        true,
        true,
        value::encode_undefined(),
        value::encode_undefined(),
    )
    .unwrap_or_else(value::encode_undefined)
}

fn module_loader(
    caller: &mut Caller<'_, RuntimeState>,
) -> std::result::Result<Arc<dyn RuntimeModuleLoader>, i64> {
    caller.data().module_loader.clone().ok_or_else(|| {
        make_type_error_exception(caller, "Error: runtime module loader is not installed")
    })
}

fn require_specifier_to_string(
    caller: &mut Caller<'_, RuntimeState>,
    specifier: Option<i64>,
) -> std::result::Result<String, i64> {
    js_to_string(caller, specifier.unwrap_or_else(value::encode_undefined))
}

fn js_to_string(
    caller: &mut Caller<'_, RuntimeState>,
    raw_value: i64,
) -> std::result::Result<String, i64> {
    if value::is_exception(raw_value) {
        return Err(raw_value);
    }
    if value::is_symbol(raw_value) {
        return Err(make_type_error_exception(
            caller,
            "TypeError: Cannot convert a Symbol value to a string",
        ));
    }
    render_value(caller, raw_value)
        .map_err(|err| make_type_error_exception(caller, &err.to_string()))
}

fn module_referrer_from_value(
    caller: &mut Caller<'_, RuntimeState>,
    raw_value: i64,
) -> std::result::Result<RuntimeModuleReferrer, i64> {
    if value::is_undefined(raw_value) || value::is_null(raw_value) {
        return Ok(RuntimeModuleReferrer::None);
    }
    let filename = js_to_string(caller, raw_value)?;
    if filename.is_empty() {
        Ok(RuntimeModuleReferrer::None)
    } else {
        Ok(RuntimeModuleReferrer::Path(PathBuf::from(filename)))
    }
}

fn alloc_dynamic_import_promise(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    let promise = alloc_promise(caller, PromiseEntry::pending());
    let then_fn = create_promise_resolving_function(
        caller.data(),
        promise,
        Arc::new(Mutex::new(false)),
        PromiseResolvingKind::Fulfill,
    );
    let catch_fn = create_promise_resolving_function(
        caller.data(),
        promise,
        Arc::new(Mutex::new(false)),
        PromiseResolvingKind::Reject,
    );
    let _ = define_host_data_property_from_caller(caller, promise, "then", then_fn);
    let _ = define_host_data_property_from_caller(caller, promise, "catch", catch_fn);
    promise
}

fn rejection_reason(caller: &mut Caller<'_, RuntimeState>, value: i64) -> i64 {
    if value::is_exception(value) {
        exception_reason(caller, value)
    } else {
        value
    }
}

fn reject_dynamic_import_with_value(
    caller: &mut Caller<'_, RuntimeState>,
    promise: i64,
    value: i64,
) {
    let reason = rejection_reason(caller, value);
    settle_promise(caller.data(), promise, PromiseSettlement::Reject(reason));
}

fn module_load_error_exception(
    caller: &mut Caller<'_, RuntimeState>,
    specifier: &str,
    error: RuntimeModuleLoadError,
) -> i64 {
    let message = match error.code {
        RuntimeModuleLoadErrorCode::NotFound => {
            format!("Error: Cannot find module '{specifier}': {}", error.message)
        }
        _ => format!("Error: {}", error.message),
    };
    make_type_error_exception(caller, &message)
}

fn cached_require_result(
    caller: &mut Caller<'_, RuntimeState>,
    key: &RuntimeModuleKey,
) -> RuntimeModuleRequireResult {
    let registry = caller
        .data()
        .module_registry
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    registry.get_for_require(key)
}

fn cached_import_result(
    caller: &mut Caller<'_, RuntimeState>,
    key: &RuntimeModuleKey,
) -> RuntimeModuleImportResult {
    let registry = caller
        .data()
        .module_registry
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    registry.get_for_import(key)
}

fn loaded_module_exports(
    caller: &mut Caller<'_, RuntimeState>,
    module_object: i64,
    fallback_exports: i64,
) -> i64 {
    resolve_handle(caller, module_object)
        .and_then(|ptr| read_object_property_by_name(caller, ptr, "exports"))
        .unwrap_or(fallback_exports)
}

fn create_require_cache_proxy(caller: &mut Caller<'_, RuntimeState>) -> i64 {
    let Some(env) = WasmEnv::from_caller(caller) else {
        return value::encode_undefined();
    };
    let target = alloc_host_object(caller, &env, 0);
    let root_len = caller.data().push_host_temp_roots([target]);
    let handler = alloc_host_object(caller, &env, 5);
    caller.data().truncate_host_temp_roots(root_len);
    let root_len = caller.data().push_host_temp_roots([target, handler]);
    attach_require_cache_trap(caller, handler, "get", CjsRequireCacheTrapKind::Get);
    attach_require_cache_trap(caller, handler, "has", CjsRequireCacheTrapKind::Has);
    attach_require_cache_trap(
        caller,
        handler,
        "deleteProperty",
        CjsRequireCacheTrapKind::DeleteProperty,
    );
    attach_require_cache_trap(caller, handler, "ownKeys", CjsRequireCacheTrapKind::OwnKeys);
    attach_require_cache_trap(
        caller,
        handler,
        "getOwnPropertyDescriptor",
        CjsRequireCacheTrapKind::GetOwnPropertyDescriptor,
    );
    let proxy = {
        let mut table = caller
            .data()
            .proxy_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let handle = table.len() as u32;
        table.push(ProxyEntry {
            target,
            handler,
            revoked: false,
        });
        value::encode_proxy_handle(handle)
    };
    caller.data().truncate_host_temp_roots(root_len);
    proxy
}

fn attach_require_cache_trap(
    caller: &mut Caller<'_, RuntimeState>,
    handler: i64,
    name: &str,
    kind: CjsRequireCacheTrapKind,
) {
    let trap = create_native_callable(caller.data(), NativeCallable::CjsRequireCacheTrap { kind });
    let _ = define_host_data_property_from_caller(caller, handler, name, trap);
}

fn paths_array(caller: &mut Caller<'_, RuntimeState>, paths: Vec<PathBuf>) -> i64 {
    let len = paths.len() as u32;
    let array = alloc_array(caller, len);
    let root_len = caller.data().push_host_temp_roots([array]);
    if let Some(ptr) = resolve_array_ptr(caller, array) {
        for (index, path) in paths.into_iter().enumerate() {
            let path_value = store_runtime_string(caller, path.to_string_lossy().into_owned());
            write_array_elem(caller, ptr, index as u32, path_value);
        }
        write_array_length(caller, ptr, len);
    }
    caller.data().truncate_host_temp_roots(root_len);
    array
}
