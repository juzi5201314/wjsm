use std::fmt;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use wasmtime::{AsContext, Caller, Linker, Module, Ref, Val};

use crate::runtime_host_helpers::{
    alloc_heap_c_string_with_env, exception_reason, find_memory_c_string_with_env,
    make_type_error_exception, reflect_get_impl_with_receiver_async,
};
use crate::runtime_module_registry::RuntimeModuleRequireResult;
use crate::runtime_render::{render_value, store_runtime_string, store_runtime_string_in_state};
use crate::runtime_values::{resolve_handle, write_object_property_by_name_id};
use crate::{
    RuntimeState, WasmEnv, alloc_host_object, define_host_data_property_from_caller, value,
};

use crate::RuntimeModuleKey;

/// 运行时解析请求的来源模块。
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RuntimeModuleReferrer {
    /// 没有调用方模块，例如入口模块或宿主触发的加载。
    None,
    /// 调用方已经有规范化 registry key。
    Module(RuntimeModuleKey),
    /// 调用方只有文件路径，loader 负责按项目 root 规范化。
    Path(PathBuf),
}

/// 运行时解析语义：`import` 与 `require` 使用不同 package conditions。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RuntimeModuleResolutionKind {
    Import,
    Require,
    ImportMetaResolve,
}

/// loader 返回给 runtime 的模块格式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RuntimeModuleFormat {
    EsModule,
    CommonJs,
    Json,
    Builtin,
}

/// 已由外部 resolver 规范化的模块目标。
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RuntimeResolvedModule {
    pub key: RuntimeModuleKey,
    pub url: String,
    pub path: Option<PathBuf>,
    pub format: RuntimeModuleFormat,
}

impl RuntimeResolvedModule {
    /// 构造外部 loader 解析后交还 runtime 的模块目标。
    pub fn new(
        key: RuntimeModuleKey,
        url: impl Into<String>,
        path: Option<PathBuf>,
        format: RuntimeModuleFormat,
    ) -> Self {
        Self {
            key,
            url: url.into(),
            path,
            format,
        }
    }
}

/// 动态实例化所需的运行时上下文占位 DTO。
///
/// 当前 Task 只建立 trait 边界；后续 CLI loader 会在不让 runtime 依赖编译 crate 的前提下
/// 扩展这里的 plain fields，用于传递共享 env/memory/table 的宿主句柄。
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RuntimeInstantiationEnv {
    pub referrer: RuntimeModuleReferrer,
}

impl RuntimeInstantiationEnv {
    /// 构造传给外部 loader 的实例化上下文。
    pub fn new(referrer: RuntimeModuleReferrer) -> Self {
        Self { referrer }
    }
}

/// loader 实例化后交还给 registry 的 JS 值。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RuntimeInstantiatedModule {
    pub module_id: Option<u32>,
    pub module_object: i64,
    pub exports_object: i64,
    pub namespace_object: i64,
}

impl RuntimeInstantiatedModule {
    /// 构造外部 loader 实例化后交还 registry 的 JS 值。
    pub fn new(
        module_id: Option<u32>,
        module_object: i64,
        exports_object: i64,
        namespace_object: i64,
    ) -> Self {
        Self {
            module_id,
            module_object,
            exports_object,
            namespace_object,
        }
    }
}

/// runtime loader 的错误分类；JS Error value 由 host import 边界再包装。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RuntimeModuleLoadErrorCode {
    NotFound,
    Unsupported,
    InvalidModule,
    InstantiateFailed,
}

/// loader contract 使用的 plain error DTO。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeModuleLoadError {
    pub code: RuntimeModuleLoadErrorCode,
    pub message: String,
}

impl RuntimeModuleLoadError {
    pub fn new(code: RuntimeModuleLoadErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for RuntimeModuleLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for RuntimeModuleLoadError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeModulePlacement {
    pub table_base: u32,
    pub data_base: u32,
}

/// Runtime-owned import link descriptor for dynamically compiled modules.
///
/// The compiler/loader side owns which imports a generated module needs; the
/// runtime only links the requested names from the current caller exports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RuntimeModuleImportLink {
    pub module: &'static str,
    pub name: &'static str,
}

impl RuntimeModuleImportLink {
    pub const fn new(module: &'static str, name: &'static str) -> Self {
        Self { module, name }
    }

    pub const fn env(name: &'static str) -> Self {
        Self::new("env", name)
    }
}

/// 运行时实例化上下文；只暴露安全的布局预留与 WASM 实例化操作。
pub struct RuntimeModuleInstantiationContext<'a, 'b>
where
    'b: 'a,
{
    caller: &'a mut Caller<'b, RuntimeState>,
}

impl<'a, 'b> RuntimeModuleInstantiationContext<'a, 'b>
where
    'b: 'a,
{
    pub(crate) fn new(caller: &'a mut Caller<'b, RuntimeState>) -> Self {
        Self { caller }
    }

    pub fn reserve_runtime_module_ids(
        &mut self,
        count: u32,
    ) -> Result<u32, RuntimeModuleLoadError> {
        let mut registry = self
            .caller
            .data()
            .module_registry
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        registry
            .reserve_runtime_module_id_range(count)
            .ok_or_else(|| {
                RuntimeModuleLoadError::new(
                    RuntimeModuleLoadErrorCode::InstantiateFailed,
                    "runtime module id reservation overflows u32",
                )
            })
    }

    pub fn reserve_module_layout(
        &mut self,
        table_len: u32,
        data_len: u32,
    ) -> Result<RuntimeModulePlacement, RuntimeModuleLoadError> {
        let env = WasmEnv::from_caller(self.caller).ok_or_else(|| {
            RuntimeModuleLoadError::new(
                RuntimeModuleLoadErrorCode::InstantiateFailed,
                "runtime module loader cannot access current WASM environment",
            )
        })?;
        let table_base = u32::try_from(env.func_table.size(&mut *self.caller)).map_err(|_| {
            RuntimeModuleLoadError::new(
                RuntimeModuleLoadErrorCode::InstantiateFailed,
                "runtime function table is too large for wasm32 table indices",
            )
        })?;
        let table_end = table_base.checked_add(table_len).ok_or_else(|| {
            RuntimeModuleLoadError::new(
                RuntimeModuleLoadErrorCode::InstantiateFailed,
                "runtime module function table reservation overflows wasm32",
            )
        })?;
        grow_table_to(self.caller, &env, table_end)?;

        let heap_ptr = env
            .heap_ptr
            .get(&mut *self.caller)
            .i32()
            .unwrap_or(0)
            .max(0) as u32;
        let data_base = align_u32(heap_ptr, wjsm_ir::constants::HEAP_ALLOCATION_ALIGNMENT)?;
        let data_end = data_base.checked_add(data_len).ok_or_else(|| {
            RuntimeModuleLoadError::new(
                RuntimeModuleLoadErrorCode::InstantiateFailed,
                "runtime module data reservation overflows wasm32",
            )
        })?;
        if data_end > i32::MAX as u32 {
            return Err(RuntimeModuleLoadError::new(
                RuntimeModuleLoadErrorCode::InstantiateFailed,
                "runtime module data reservation exceeds wasm32 signed heap range",
            ));
        }
        grow_memory_to(self.caller, &env, data_end)?;
        set_global_i32(self.caller, &env.heap_ptr, data_end)?;
        if let Some(global) = env.alloc_ptr {
            set_global_i32(self.caller, &global, data_end)?;
        }
        if let Some(global) = env.alloc_end {
            set_global_i32(self.caller, &global, data_end)?;
        }

        Ok(RuntimeModulePlacement {
            table_base,
            data_base,
        })
    }

    pub async fn instantiate_compiled_module(
        &mut self,
        resolved: &RuntimeResolvedModule,
        wasm_bytes: &[u8],
        entry_module_id: Option<u32>,
    ) -> Result<RuntimeInstantiatedModule, RuntimeModuleLoadError> {
        self.instantiate_compiled_module_with_imports(
            resolved,
            wasm_bytes,
            entry_module_id,
            std::iter::empty(),
        )
        .await
    }

    pub async fn instantiate_compiled_module_with_imports<I>(
        &mut self,
        resolved: &RuntimeResolvedModule,
        wasm_bytes: &[u8],
        entry_module_id: Option<u32>,
        import_links: I,
    ) -> Result<RuntimeInstantiatedModule, RuntimeModuleLoadError>
    where
        I: IntoIterator<Item = RuntimeModuleImportLink>,
    {
        let engine = self.caller.as_context().engine().clone();
        let module = Module::new(&engine, wasm_bytes).map_err(|error| {
            RuntimeModuleLoadError::new(
                RuntimeModuleLoadErrorCode::InvalidModule,
                format!("runtime module WASM validation failed: {error:?}"),
            )
        })?;
        let mut linker = Linker::new(&engine);
        define_runtime_imports(&mut linker, self.caller, import_links)?;
        define_runtime_support_imports(&mut linker, self.caller)?;
        let instance = linker
            .instantiate_async(&mut *self.caller, &module)
            .await
            .map_err(|error| {
                RuntimeModuleLoadError::new(
                    RuntimeModuleLoadErrorCode::InstantiateFailed,
                    format!("runtime module instantiate failed: {error:?}"),
                )
            })?;
        let main = instance
            .get_typed_func::<(), i64>(&mut *self.caller, "main")
            .map_err(|error| {
                RuntimeModuleLoadError::new(
                    RuntimeModuleLoadErrorCode::InvalidModule,
                    format!("runtime module missing main export: {error:?}"),
                )
            })?;
        begin_runtime_commonjs_loading(self.caller, resolved, entry_module_id);
        let result = match main.call_async(&mut *self.caller, ()).await {
            Ok(result) => result,
            Err(error) => {
                let message = format!("runtime module main failed: {error:?}");
                finish_runtime_loading_errored_with_message(
                    self.caller,
                    resolved,
                    entry_module_id,
                    &message,
                );
                return Err(RuntimeModuleLoadError::new(
                    RuntimeModuleLoadErrorCode::InstantiateFailed,
                    message,
                ));
            }
        };
        if value::is_exception(result) {
            let reason = exception_reason(self.caller, result);
            finish_runtime_loading_errored(
                self.caller,
                resolved,
                entry_module_id,
                reason,
            );
            let message = render_value(self.caller, reason)
                .unwrap_or_else(|_| "runtime module threw an exception".to_string());
            return Err(RuntimeModuleLoadError::new(
                RuntimeModuleLoadErrorCode::InstantiateFailed,
                message,
            ));
        }

        let default_export = default_export_from_namespace(self.caller, entry_module_id).await;
        instantiated_from_registry(self.caller, resolved, entry_module_id, default_export)
    }

    pub fn instantiate_json_module(
        &mut self,
        resolved: &RuntimeResolvedModule,
        source: &str,
    ) -> Result<RuntimeInstantiatedModule, RuntimeModuleLoadError> {
        if resolved.format != RuntimeModuleFormat::Json {
            return Err(RuntimeModuleLoadError::new(
                RuntimeModuleLoadErrorCode::Unsupported,
                "runtime JSON instantiation requires a JSON module target",
            ));
        }
        let json_value = crate::runtime_json::parse_json_text(source).map_err(|error| {
            RuntimeModuleLoadError::new(
                RuntimeModuleLoadErrorCode::InvalidModule,
                format!("failed to parse JSON module {}: {error}", resolved.url),
            )
        })?;
        let env = WasmEnv::from_caller(self.caller).ok_or_else(|| {
            RuntimeModuleLoadError::new(
                RuntimeModuleLoadErrorCode::InstantiateFailed,
                "runtime JSON module loader cannot access current WASM environment",
            )
        })?;
        let exports_object =
            crate::runtime_json::build_wasm_value_with_env(self.caller, &env, &json_value);
        let root_len = self.caller.data().push_host_temp_roots([exports_object]);
        let module_object = alloc_host_object(self.caller, &env, 4);
        let _module_root_len = self.caller.data().push_host_temp_roots([module_object]);
        let module_id = resolved
            .path
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|| resolved.url.clone());
        let id_value = store_runtime_string(&*self.caller, module_id.clone());
        let filename_value = store_runtime_string(&*self.caller, module_id);
        let _ = define_host_data_property_from_caller(self.caller, module_object, "id", id_value);
        let _ = define_host_data_property_from_caller(
            self.caller,
            module_object,
            "filename",
            filename_value,
        );
        let _ = define_host_data_property_from_caller(
            self.caller,
            module_object,
            "exports",
            exports_object,
        );
        let _ = define_host_data_property_from_caller(
            self.caller,
            module_object,
            "loaded",
            value::encode_bool(true),
        );
        self.caller.data().truncate_host_temp_roots(root_len);
        Ok(RuntimeInstantiatedModule::new(
            None,
            module_object,
            exports_object,
            exports_object,
        ))
    }
}

fn align_u32(value: u32, align: u32) -> Result<u32, RuntimeModuleLoadError> {
    value
        .checked_add(align - 1)
        .map(|value| value & !(align - 1))
        .ok_or_else(|| {
            RuntimeModuleLoadError::new(
                RuntimeModuleLoadErrorCode::InstantiateFailed,
                "runtime module layout alignment overflows wasm32",
            )
        })
}

fn grow_table_to(
    caller: &mut Caller<'_, RuntimeState>,
    env: &WasmEnv,
    table_end: u32,
) -> Result<(), RuntimeModuleLoadError> {
    let current = env.func_table.size(&mut *caller);
    let table_end = u64::from(table_end);
    if table_end <= current {
        return Ok(());
    }
    env.func_table
        .grow(&mut *caller, table_end - current, Ref::Func(None))
        .map(|_| ())
        .map_err(|error| {
            RuntimeModuleLoadError::new(
                RuntimeModuleLoadErrorCode::InstantiateFailed,
                format!("failed to grow runtime function table: {error:?}"),
            )
        })
}

fn grow_memory_to(
    caller: &mut Caller<'_, RuntimeState>,
    env: &WasmEnv,
    data_end: u32,
) -> Result<(), RuntimeModuleLoadError> {
    let needed = data_end as usize;
    let current = env.memory.data_size(&*caller);
    if needed <= current {
        return Ok(());
    }
    let page = 64 * 1024usize;
    let pages = needed
        .checked_sub(current)
        .and_then(|delta| delta.checked_add(page - 1))
        .map(|delta| delta / page)
        .ok_or_else(|| {
            RuntimeModuleLoadError::new(
                RuntimeModuleLoadErrorCode::InstantiateFailed,
                "runtime module memory growth overflows host usize",
            )
        })?;
    env.memory
        .grow(&mut *caller, pages as u64)
        .map(|_| ())
        .map_err(|error| {
            RuntimeModuleLoadError::new(
                RuntimeModuleLoadErrorCode::InstantiateFailed,
                format!("failed to grow runtime memory: {error:?}"),
            )
        })
}

fn set_global_i32(
    caller: &mut Caller<'_, RuntimeState>,
    global: &wasmtime::Global,
    value: u32,
) -> Result<(), RuntimeModuleLoadError> {
    global
        .set(&mut *caller, Val::I32(value as i32))
        .map_err(|error| {
            RuntimeModuleLoadError::new(
                RuntimeModuleLoadErrorCode::InstantiateFailed,
                format!("failed to reserve runtime module heap data: {error:?}"),
            )
        })
}

fn define_runtime_imports<I>(
    linker: &mut Linker<RuntimeState>,
    caller: &mut Caller<'_, RuntimeState>,
    import_links: I,
) -> Result<(), RuntimeModuleLoadError>
where
    I: IntoIterator<Item = RuntimeModuleImportLink>,
{
    define_caller_export(linker, caller, "env", "memory")?;
    define_caller_export(linker, caller, "env", "__table")?;
    for global in wjsm_runtime_support::abi::ENV_GLOBALS {
        define_caller_export(linker, caller, "env", global.name)?;
    }
    for import_link in import_links {
        define_caller_export(linker, caller, import_link.module, import_link.name)?;
    }
    Ok(())
}

fn define_runtime_support_imports(
    linker: &mut Linker<RuntimeState>,
    caller: &mut Caller<'_, RuntimeState>,
) -> Result<(), RuntimeModuleLoadError> {
    let support_exports = caller
        .data()
        .support_exports
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    if support_exports.is_empty() {
        return Err(RuntimeModuleLoadError::new(
            RuntimeModuleLoadErrorCode::InstantiateFailed,
            "runtime support module exports are not available for runtime loading",
        ));
    }
    for (name, export) in support_exports {
        linker
            .define(&*caller, "wjsm_support", name, export)
            .map_err(|error| {
                RuntimeModuleLoadError::new(
                    RuntimeModuleLoadErrorCode::InstantiateFailed,
                    format!("failed to link runtime support import {name}: {error:?}"),
                )
            })?;
    }
    Ok(())
}

fn define_caller_export(
    linker: &mut Linker<RuntimeState>,
    caller: &mut Caller<'_, RuntimeState>,
    module: &'static str,
    name: &'static str,
) -> Result<(), RuntimeModuleLoadError> {
    let export = caller.get_export(name).ok_or_else(|| {
        RuntimeModuleLoadError::new(
            RuntimeModuleLoadErrorCode::InstantiateFailed,
            format!("current runtime module does not export {name} for runtime loading"),
        )
    })?;
    linker
        .define(&*caller, module, name, export)
        .map(|_| ())
        .map_err(|error| {
            RuntimeModuleLoadError::new(
                RuntimeModuleLoadErrorCode::InstantiateFailed,
                format!("failed to link runtime import {module}.{name}: {error:?}"),
            )
        })
}

async fn default_export_from_namespace(
    caller: &mut Caller<'_, RuntimeState>,
    entry_module_id: Option<u32>,
) -> Option<i64> {
    let module_id = entry_module_id?;
    let namespace = {
        let registry = caller
            .data()
            .module_registry
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        registry.get_namespace_by_module_id(module_id)
    }?;
    let key = store_runtime_string_in_state(caller.data(), "default".to_string());
    let value = reflect_get_impl_with_receiver_async(caller, namespace, key, namespace).await;
    (!value::is_undefined(value)).then_some(value)
}

fn begin_runtime_commonjs_loading(
    caller: &mut Caller<'_, RuntimeState>,
    resolved: &RuntimeResolvedModule,
    module_id: Option<u32>,
) {
    if resolved.format != RuntimeModuleFormat::CommonJs {
        return;
    }
    let mut registry = caller
        .data()
        .module_registry
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    registry.begin_loading(
        resolved.key.clone(),
        module_id,
        value::encode_undefined(),
        value::encode_undefined(),
    );
}

fn finish_runtime_loading_errored(
    caller: &mut Caller<'_, RuntimeState>,
    resolved: &RuntimeResolvedModule,
    module_id: Option<u32>,
    error_value: i64,
) {
    if !matches!(
        resolved.format,
        RuntimeModuleFormat::CommonJs | RuntimeModuleFormat::EsModule | RuntimeModuleFormat::Builtin
    ) {
        return;
    }
    let mut registry = caller
        .data()
        .module_registry
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    registry.finish_errored(resolved.key.clone(), module_id, error_value);
}

fn finish_runtime_loading_errored_with_message(
    caller: &mut Caller<'_, RuntimeState>,
    resolved: &RuntimeResolvedModule,
    module_id: Option<u32>,
    message: &str,
) {
    if !matches!(
        resolved.format,
        RuntimeModuleFormat::CommonJs | RuntimeModuleFormat::EsModule | RuntimeModuleFormat::Builtin
    ) {
        return;
    }
    let exception = make_type_error_exception(caller, message);
    let reason = exception_reason(caller, exception);
    finish_runtime_loading_errored(caller, resolved, module_id, reason);
}

fn write_module_exports_property(
    caller: &mut Caller<'_, RuntimeState>,
    module_object: i64,
    exports_object: i64,
) {
    let Some(env) = WasmEnv::from_caller(caller) else {
        return;
    };
    let Some(name_id) = find_memory_c_string_with_env(caller, &env, "exports")
        .or_else(|| alloc_heap_c_string_with_env(caller, &env, "exports"))
    else {
        return;
    };
    let Some(module_ptr) = resolve_handle(caller, module_object) else {
        return;
    };
    write_object_property_by_name_id(
        caller,
        module_ptr,
        module_object,
        crate::property_key::encode_string_name_id(name_id),
        exports_object,
        wjsm_ir::constants::FLAG_CONFIGURABLE
            | wjsm_ir::constants::FLAG_ENUMERABLE
            | wjsm_ir::constants::FLAG_WRITABLE,
    );
}

fn instantiated_from_registry(
    caller: &mut Caller<'_, RuntimeState>,
    resolved: &RuntimeResolvedModule,
    entry_module_id: Option<u32>,
    default_export: Option<i64>,
) -> Result<RuntimeInstantiatedModule, RuntimeModuleLoadError> {
    if resolved.format == RuntimeModuleFormat::CommonJs {
        let loading_module = {
            let registry = caller
                .data()
                .module_registry
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            registry.loading_module(&resolved.key)
        };
        if let Some((module_object, exports_object)) = loading_module {
            if value::is_undefined(module_object) {
                return Err(RuntimeModuleLoadError::new(
                    RuntimeModuleLoadErrorCode::InstantiateFailed,
                    "runtime CommonJS module did not register itself",
                ));
            }
            let exports_object = default_export.unwrap_or(exports_object);
            if let Some(default_export) = default_export {
                write_module_exports_property(caller, module_object, default_export);
            }
            return Ok(RuntimeInstantiatedModule::new(
                entry_module_id,
                module_object,
                exports_object,
                exports_object,
            ));
        }
    }
    let require_result = {
        let registry = caller
            .data()
            .module_registry
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        registry.get_for_require(&resolved.key)
    };
    match require_result {
        RuntimeModuleRequireResult::LoadedModule {
            module_object,
            exports_object,
        } => {
            let exports_object = default_export.unwrap_or(exports_object);
            if let Some(default_export) = default_export {
                write_module_exports_property(caller, module_object, default_export);
            }
            return Ok(RuntimeInstantiatedModule::new(
                entry_module_id,
                module_object,
                exports_object,
                exports_object,
            ));
        }
        RuntimeModuleRequireResult::Exports(exports_object) => {
            let exports_object = default_export.unwrap_or(exports_object);
            return Ok(RuntimeInstantiatedModule::new(
                entry_module_id,
                value::encode_undefined(),
                exports_object,
                exports_object,
            ));
        }
        RuntimeModuleRequireResult::Errored(error_value) => {
            return Err(RuntimeModuleLoadError::new(
                RuntimeModuleLoadErrorCode::InstantiateFailed,
                format!("runtime CommonJS module failed with value {error_value}"),
            ));
        }
        RuntimeModuleRequireResult::Missing => {}
    }

    match resolved.format {
        RuntimeModuleFormat::CommonJs => Err(RuntimeModuleLoadError::new(
            RuntimeModuleLoadErrorCode::InstantiateFailed,
            "runtime CommonJS module did not register itself",
        )),
        RuntimeModuleFormat::EsModule | RuntimeModuleFormat::Builtin => {
            let module_id = entry_module_id.ok_or_else(|| {
                RuntimeModuleLoadError::new(
                    RuntimeModuleLoadErrorCode::InstantiateFailed,
                    "runtime ES module did not provide a module id",
                )
            })?;
            let namespace = {
                let registry = caller
                    .data()
                    .module_registry
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                registry.get_namespace_by_module_id(module_id)
            }
            .ok_or_else(|| {
                RuntimeModuleLoadError::new(
                    RuntimeModuleLoadErrorCode::InstantiateFailed,
                    "runtime ES module did not register a namespace object",
                )
            })?;
            let exports_object = if resolved.format == RuntimeModuleFormat::Builtin {
                default_export.unwrap_or_else(value::encode_undefined)
            } else {
                value::encode_undefined()
            };
            Ok(RuntimeInstantiatedModule::new(
                Some(module_id),
                value::encode_undefined(),
                exports_object,
                namespace,
            ))
        }
        RuntimeModuleFormat::Json => Err(RuntimeModuleLoadError::new(
            RuntimeModuleLoadErrorCode::Unsupported,
            "runtime loader cannot instantiate this module format",
        )),
    }
}
/// runtime crate 暴露的加载边界；实现方位于 CLI/编排层，而不是 runtime 内部。
pub trait RuntimeModuleLoader: Send + Sync {
    fn resolve_for_runtime(
        &self,
        referrer: RuntimeModuleReferrer,
        specifier: &str,
        kind: RuntimeModuleResolutionKind,
    ) -> Result<RuntimeResolvedModule, RuntimeModuleLoadError>;

    fn resolve_paths_for_runtime(
        &self,
        _referrer: RuntimeModuleReferrer,
        _specifier: &str,
    ) -> Result<Option<Vec<PathBuf>>, RuntimeModuleLoadError> {
        Ok(None)
    }

    fn instantiate_runtime_module(
        &self,
        resolved: &RuntimeResolvedModule,
        env: RuntimeInstantiationEnv,
    ) -> Result<RuntimeInstantiatedModule, RuntimeModuleLoadError>;

    fn instantiate_runtime_module_with_context<'a, 'b>(
        &'a self,
        resolved: &'a RuntimeResolvedModule,
        env: RuntimeInstantiationEnv,
        context: RuntimeModuleInstantiationContext<'a, 'b>,
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
        drop(context);
        Box::pin(async move { self.instantiate_runtime_module(resolved, env) })
    }
}
