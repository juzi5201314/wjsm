// ExecContext 方法片段：modules
macro_rules! exec_ctx_modules {
    () => {
    fn register_module_namespace(&mut self, module_id: u32, namespace: Value) {
        let mut registry = self
            .caller
            .data()
            .module_registry
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        registry.register_static_namespace(module_id, namespace);
    }
    fn dynamic_import(&mut self, module_id: u32) -> Value {
        use std::sync::{Arc, Mutex};
        let promise = crate::alloc_promise(self.caller, crate::PromiseEntry::pending());
        let then_fn = crate::create_promise_resolving_function(
            self.caller.data(),
            promise,
            Arc::new(Mutex::new(false)),
            crate::PromiseResolvingKind::Fulfill,
        );
        let catch_fn = crate::create_promise_resolving_function(
            self.caller.data(),
            promise,
            Arc::new(Mutex::new(false)),
            crate::PromiseResolvingKind::Reject,
        );
        let _ = crate::define_host_data_property_from_caller(self.caller, promise, "then", then_fn);
        let _ =
            crate::define_host_data_property_from_caller(self.caller, promise, "catch", catch_fn);
        let namespace_obj = {
            let registry = self
                .caller
                .data()
                .module_registry
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            registry.get_namespace_by_module_id(module_id)
        };
        match namespace_obj {
            Some(ns_obj) => {
                crate::resolve_promise_from_caller(self.caller, promise, ns_obj);
            }
            None => {
                let error_msg = format!("Cannot find module with id {}", module_id);
                let error_val = crate::runtime_error_value(self.caller.data(), error_msg);
                crate::settle_promise(
                    self.caller.data(),
                    promise,
                    crate::PromiseSettlement::Reject(error_val),
                );
            }
        }
        promise
    }
    fn module_resolve(
        &mut self,
        referrer: wjsm_host::RuntimeModuleReferrer,
        specifier: &str,
        kind: wjsm_host::RuntimeModuleResolutionKind,
    ) -> Result<wjsm_host::RuntimeResolvedModule, wjsm_host::RuntimeModuleLoadError> {
        let loader = self.caller.data().module_loader.clone().ok_or_else(|| {
            wjsm_host::RuntimeModuleLoadError::new(
                wjsm_host::RuntimeModuleLoadErrorCode::Unsupported,
                "runtime module loader is not installed",
            )
        })?;
        loader.resolve_for_runtime(referrer, specifier, kind)
    }
    fn module_resolve_paths(
        &mut self,
        referrer: wjsm_host::RuntimeModuleReferrer,
        specifier: &str,
    ) -> Result<Option<Vec<std::path::PathBuf>>, wjsm_host::RuntimeModuleLoadError> {
        let loader = self.caller.data().module_loader.clone().ok_or_else(|| {
            wjsm_host::RuntimeModuleLoadError::new(
                wjsm_host::RuntimeModuleLoadErrorCode::Unsupported,
                "runtime module loader is not installed",
            )
        })?;
        loader.resolve_paths_for_runtime(referrer, specifier)
    }
    fn module_cached_require(
        &mut self,
        key: &wjsm_host::RuntimeModuleKey,
    ) -> wjsm_host::RuntimeModuleRequireResult {
        let registry = self
            .caller
            .data()
            .module_registry
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        registry.get_for_require(key)
    }
    fn module_cached_import(
        &mut self,
        key: &wjsm_host::RuntimeModuleKey,
    ) -> wjsm_host::RuntimeModuleImportResult {
        let registry = self
            .caller
            .data()
            .module_registry
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        registry.get_for_import(key)
    }
    fn module_instantiate_sync(
        &mut self,
        resolved: &wjsm_host::RuntimeResolvedModule,
        env: wjsm_host::RuntimeInstantiationEnv,
    ) -> Result<wjsm_host::RuntimeInstantiatedModule, wjsm_host::RuntimeModuleLoadError> {
        let loader = self.caller.data().module_loader.clone().ok_or_else(|| {
            wjsm_host::RuntimeModuleLoadError::new(
                wjsm_host::RuntimeModuleLoadErrorCode::Unsupported,
                "runtime module loader is not installed",
            )
        })?;
        loader.instantiate_runtime_module(resolved, env)
    }
    fn module_instantiate_async<'a>(
        &'a mut self,
        resolved: wjsm_host::RuntimeResolvedModule,
        env: wjsm_host::RuntimeInstantiationEnv,
    ) -> wjsm_host::ExecFuture<
        'a,
        Result<wjsm_host::RuntimeInstantiatedModule, wjsm_host::RuntimeModuleLoadError>,
    > {
        Box::pin(async move {
            let loader = self.caller.data().module_loader.clone().ok_or_else(|| {
                wjsm_host::RuntimeModuleLoadError::new(
                    wjsm_host::RuntimeModuleLoadErrorCode::Unsupported,
                    "runtime module loader is not installed",
                )
            })?;
            let context = crate::RuntimeModuleInstantiationContext::new(self.caller);
            loader
                .instantiate_runtime_module_with_context(&resolved, env, context)
                .await
        })
    }
    fn module_finish_loaded(
        &mut self,
        key: wjsm_host::RuntimeModuleKey,
        instantiated: wjsm_host::RuntimeInstantiatedModule,
    ) {
        let mut registry = self
            .caller
            .data()
            .module_registry
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        registry.finish_loaded(
            key,
            instantiated.module_id,
            instantiated.module_object,
            instantiated.exports_object,
            instantiated.namespace_object,
        );
    }
    fn module_finish_errored(
        &mut self,
        key: wjsm_host::RuntimeModuleKey,
        module_id: Option<u32>,
        reason: Value,
    ) {
        let mut registry = self
            .caller
            .data()
            .module_registry
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        registry.finish_errored(key, module_id, reason);
    }
    fn module_require_cache_entry(
        &mut self,
        cache_key: &str,
    ) -> Option<wjsm_host::RuntimeRequireCacheEntry> {
        let registry = self
            .caller
            .data()
            .module_registry
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        registry.require_cache_entry_by_id(cache_key)
    }
    fn module_require_cache_entries(&mut self) -> Vec<wjsm_host::RuntimeRequireCacheEntry> {
        let registry = self
            .caller
            .data()
            .module_registry
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        registry.require_cache_entries()
    }
    fn module_delete_require_cache_entry(&mut self, cache_key: &str) -> bool {
        let mut registry = self
            .caller
            .data()
            .module_registry
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        registry.delete_cache_entry_by_id(cache_key)
    }
    fn create_require_cache_proxy(&mut self) -> Value {
        crate::host_imports::create_require_cache_proxy(self.caller)
    }
    };
}
