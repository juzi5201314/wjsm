use std::fmt;
use std::path::PathBuf;

use crate::Value;

/// 运行时模块缓存使用的规范化 key。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum RuntimeModuleKey {
    File(PathBuf),
    Json(PathBuf),
    Builtin(String),
    PrecompiledModuleId(u32),
    RuntimeModuleId(u32),
}

/// 运行时解析请求的来源模块。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeModuleReferrer {
    None,
    Module(RuntimeModuleKey),
    Path(PathBuf),
}

/// 运行时模块解析使用的 package conditions。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeModuleResolutionKind {
    Import,
    Require,
    ImportMetaResolve,
}

/// loader 返回给 runtime 的模块格式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeModuleFormat {
    EsModule,
    CommonJs,
    Json,
    Builtin,
}

/// 已由外部 resolver 规范化的模块目标。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeResolvedModule {
    pub key: RuntimeModuleKey,
    pub url: String,
    pub path: Option<PathBuf>,
    pub format: RuntimeModuleFormat,
}

impl RuntimeResolvedModule {
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

/// 动态实例化所需的后端无关环境。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeInstantiationEnv {
    pub referrer: RuntimeModuleReferrer,
}

impl RuntimeInstantiationEnv {
    pub fn new(referrer: RuntimeModuleReferrer) -> Self {
        Self { referrer }
    }
}

/// loader 实例化后交还给 registry 的 JS 值。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeInstantiatedModule {
    pub module_id: Option<u32>,
    pub module_object: Value,
    pub exports_object: Value,
    pub namespace_object: Value,
}

impl RuntimeInstantiatedModule {
    pub fn new(
        module_id: Option<u32>,
        module_object: Value,
        exports_object: Value,
        namespace_object: Value,
    ) -> Self {
        Self {
            module_id,
            module_object,
            exports_object,
            namespace_object,
        }
    }
}

/// runtime loader 的错误分类；JS Error value 由 builtins 边界包装。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

/// `require.cache` 上可观察的一条缓存记录。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeRequireCacheEntry {
    pub id: String,
    pub module_object: Value,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CjsRequireCacheTrapKind {
    Get,
    Has,
    DeleteProperty,
    OwnKeys,
    GetOwnPropertyDescriptor,
}

/// CJS `require()` 查询 registry 后的结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeModuleRequireResult {
    Missing,
    Exports(Value),
    LoadedModule {
        module_object: Value,
        exports_object: Value,
    },
    Errored(Value),
}

/// dynamic import 查询 registry 后的结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeModuleImportResult {
    Missing,
    Namespace(Value),
    Errored(Value),
}
