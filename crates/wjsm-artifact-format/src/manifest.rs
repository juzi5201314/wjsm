use std::sync::Arc;

use wjsm_ir::{ModuleId, Program};

/// Portable artifact 的模块类型。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u16)]
pub enum ModuleKind {
    Script = 0,
    EsModule = 1,
    CommonJs = 2,
    Builtin = 3,
}

impl ModuleKind {
    pub(crate) fn from_wire(tag: u16) -> Option<Self> {
        match tag {
            0 => Some(Self::Script),
            1 => Some(Self::EsModule),
            2 => Some(Self::CommonJs),
            3 => Some(Self::Builtin),
            _ => None,
        }
    }
}

/// 单个 portable 模块的 target-independent 描述。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManifestModule {
    pub id: ModuleId,
    pub logical_url: String,
    pub kind: ModuleKind,
    pub static_dependencies: Vec<ModuleId>,
    pub dynamic_dependencies: Vec<(String, ModuleId)>,
}

/// Portable 编译单元的模块图。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModuleManifest {
    pub entry: ModuleId,
    pub modules: Vec<ManifestModule>,
    pub resolution_conditions: Vec<String>,
}

impl ModuleManifest {
    pub fn single(logical_url: impl Into<String>, script_mode: bool) -> Self {
        Self {
            entry: ModuleId(0),
            modules: vec![ManifestModule {
                id: ModuleId(0),
                logical_url: logical_url.into(),
                kind: if script_mode {
                    ModuleKind::Script
                } else {
                    ModuleKind::EsModule
                },
                static_dependencies: Vec::new(),
                dynamic_dependencies: Vec::new(),
            }],
            resolution_conditions: Vec::new(),
        }
    }
}

/// 只影响 target-independent debug/source sections 的构建选项。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BuildOptions {
    pub include_source_map: bool,
    pub include_source_text: bool,
}

/// 经 lowering 产生、尚未编码的 portable 编译输入。
#[derive(Clone, Debug)]
pub struct ArtifactBuildInput {
    pub program: Arc<Program>,
    pub manifest: Arc<ModuleManifest>,
    pub options: BuildOptions,
    pub source_text: Option<Arc<str>>,
}

impl ArtifactBuildInput {
    pub fn new(program: Program, manifest: ModuleManifest, options: BuildOptions) -> Self {
        Self {
            program: Arc::new(program),
            manifest: Arc::new(manifest),
            options,
            source_text: None,
        }
    }
}
