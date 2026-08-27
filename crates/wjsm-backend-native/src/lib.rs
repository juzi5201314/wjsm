pub mod cache;
pub(crate) mod f64_analysis;
pub mod image;
pub(crate) mod lower;
pub(crate) mod template_meta;
pub use template_meta::{IcTemplateHint, ic_template_hints};
pub(crate) mod root_plan;
pub(crate) mod specialize;
pub(crate) mod unwind;

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use cranelift_codegen::settings::{self, Configurable};
use thiserror::Error;
use wjsm_artifact_format::PortableArtifact;
use wjsm_native_abi::NativeHostSymbol;

include!(concat!(env!("OUT_DIR"), "/native_codegen_hash.rs"));

pub const CRANELIFT_VERSION: &str = cranelift_codegen::VERSION;

/// Runtime 在 image load 时提供的窄 symbol 解析边界。
pub trait NativeSymbolResolver: Send + Sync {
    fn resolve(&self, symbol: NativeHostSymbol) -> Option<usize>;
}

#[derive(Clone, Debug)]
pub struct NativeObject {
    bytes: Arc<[u8]>,
    frame_bytes: Vec<u32>,
    function_count: u32,
    /// lowering 预计算的 IC 槽总数（32 字节/槽）；运行时据此分配 IC 缓冲。
    ic_slot_count: u32,
    /// lowering 预计算的类型反馈槽总数（48 字节/槽）；运行时据此分配反馈缓冲。
    feedback_slot_count: u32,
}

impl NativeObject {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn frame_bytes(&self) -> &[u32] {
        &self.frame_bytes
    }

    pub fn function_count(&self) -> u32 {
        self.function_count
    }

    pub fn ic_slot_count(&self) -> u32 {
        self.ic_slot_count
    }

    pub fn feedback_slot_count(&self) -> u32 {
        self.feedback_slot_count
    }

    /// 从已验证的预编译字段重建 object；不解析机器码。
    pub fn from_parts(
        bytes: impl Into<Arc<[u8]>>,
        frame_bytes: Vec<u32>,
        function_count: u32,
        ic_slot_count: u32,
        feedback_slot_count: u32,
    ) -> Result<Self, NativeCompileError> {
        let count = usize::try_from(function_count)
            .map_err(|_| NativeCompileError::Capacity("function count"))?;
        if frame_bytes.len() != count {
            return Err(NativeCompileError::CompilerInvariant(
                "native object frame_bytes length does not match function_count".into(),
            ));
        }
        Ok(Self {
            bytes: bytes.into(),
            frame_bytes,
            function_count,
            ic_slot_count,
            feedback_slot_count,
        })
    }
}

#[derive(Clone, Debug)]
pub struct NativeCompilationDiagnostics {
    pub object: NativeObject,
    pub clif: String,
    pub disassembly: String,
}

#[derive(Clone)]
pub struct NativeCompiler {
    isa: cranelift_codegen::isa::OwnedTargetIsa,
    settings_key: Arc<str>,
}

/// Cranelift `opt_level`：默认 `speed`（AOT 产出质量优先）。
/// `WJSM_OPT_LEVEL=none` 用 codegen 速度换生成码质量，供测试套件等只验证语义的场景使用。
/// 取值进入 [`NativeCompiler::settings_key`]，不同档位的 native cache 条目互不复用。
fn configured_opt_level() -> Result<&'static str, NativeCompileError> {
    match std::env::var("WJSM_OPT_LEVEL").as_deref() {
        Err(_) => Ok("speed"),
        Ok("speed") => Ok("speed"),
        Ok("none") => Ok("none"),
        Ok("speed_and_size") => Ok("speed_and_size"),
        Ok(other) => Err(NativeCompileError::UnsupportedTargetCapability(format!(
            "WJSM_OPT_LEVEL={other:?} 无效（可选 none / speed / speed_and_size）"
        ))),
    }
}

/// CLIF verifier 默认开启：本仓库自己产出 CLIF，verifier 是 lowering bug 的门禁。
/// `WJSM_VERIFY_CLIF=0` 关闭，换约 20% codegen 时间。verifier 不改变生成码，
/// 因此不进入 native cache 键。
fn clif_verifier_enabled() -> bool {
    !matches!(
        std::env::var("WJSM_VERIFY_CLIF").as_deref(),
        Ok("0") | Ok("false") | Ok("FALSE")
    )
}

/// 全局 compiler 缓存，避免重复初始化 CPU 密集的 ISA builder。
/// 测试套件中所有 compile 调用共享同一个 ISA 配置。
static CACHED_COMPILER: LazyLock<Result<NativeCompiler, NativeCompileError>> = LazyLock::new(
    || {
        if cfg!(not(target_pointer_width = "64")) {
            return Err(NativeCompileError::UnsupportedTargetCapability(
                "direct native backend requires a 64-bit host".into(),
            ));
        }
        if !cfg!(all(
            target_arch = "x86_64",
            any(target_os = "linux", target_os = "windows")
        )) {
            return Err(NativeCompileError::UnsupportedTargetCapability(format!(
                "unsupported native target {}-{}",
                std::env::consts::ARCH,
                std::env::consts::OS
            )));
        }

        let opt_level = configured_opt_level()?;
        let mut flag_builder = settings::builder();
        set_flag(&mut flag_builder, "opt_level", opt_level)?;
        set_flag(&mut flag_builder, "is_pic", "true")?;
        set_flag(&mut flag_builder, "unwind_info", "true")?;
        set_flag(
            &mut flag_builder,
            "enable_verifier",
            if clif_verifier_enabled() {
                "true"
            } else {
                "false"
            },
        )?;
        set_flag(&mut flag_builder, "enable_nan_canonicalization", "true")?;
        set_flag(&mut flag_builder, "use_colocated_libcalls", "false")?;
        set_flag(&mut flag_builder, "probestack_strategy", "inline")?;
        set_flag(&mut flag_builder, "probestack_size_log2", "12")?;
        let flags = settings::Flags::new(flag_builder);
        let isa_builder = cranelift_native::builder().map_err(|message| {
            NativeCompileError::UnsupportedTargetCapability(message.to_string())
        })?;
        let isa = isa_builder
            .finish(flags)
            .map_err(|error| NativeCompileError::Cranelift(error.to_string()))?;
        let unwind_policy = unwind::UnwindPolicy::for_triple(isa.triple())?;
        let target_os = match &isa.triple().operating_system {
            target_lexicon::OperatingSystem::Windows => "windows",
            target_lexicon::OperatingSystem::Linux => "linux",
            other => {
                return Err(NativeCompileError::UnsupportedTargetCapability(format!(
                    "unsupported native target OS {other}"
                )));
            }
        };
        let target_arch = match isa.triple().architecture {
            target_lexicon::Architecture::X86_64 => "x86_64",
            other => {
                return Err(NativeCompileError::UnsupportedTargetCapability(format!(
                    "unsupported native target architecture {other}"
                )));
            }
        };
        let settings_key = format!(
            "target={};arch={target_arch};os={target_os};cranelift={};pic={};unwind=1;unwind-object={};nan=canonical;opt={opt_level};probestack=inline:4096",
            isa.triple(),
            CRANELIFT_VERSION,
            1,
            unwind_policy.settings_name(),
        )
        .into();
        Ok(NativeCompiler { isa, settings_key })
    },
);

impl NativeCompiler {
    /// 返回全局缓存的 compiler 的 clone（isa 内部是 Arc，clone 成本低）。
    pub fn new() -> Result<Self, NativeCompileError> {
        CACHED_COMPILER.clone()
    }

    pub fn settings_key(&self) -> &str {
        &self.settings_key
    }

    pub fn compile(&self, artifact: &PortableArtifact) -> Result<NativeObject, NativeCompileError> {
        self.compile_program(artifact.program())
    }

    pub fn compile_program(
        &self,
        program: &wjsm_ir::Program,
    ) -> Result<NativeObject, NativeCompileError> {
        let slots = lower::slots_from_program(program)?;
        self.compile_program_with_slots(program, &slots)
    }

    pub fn compile_program_with_slots(
        &self,
        program: &wjsm_ir::Program,
        variable_slots: &HashMap<String, u32>,
    ) -> Result<NativeObject, NativeCompileError> {
        lower::compile_program(Arc::clone(&self.isa), program, variable_slots)
    }

    pub fn diagnostics(
        &self,
        artifact: &PortableArtifact,
    ) -> Result<NativeCompilationDiagnostics, NativeCompileError> {
        self.diagnostics_program(artifact.program())
    }

    pub fn diagnostics_program(
        &self,
        program: &wjsm_ir::Program,
    ) -> Result<NativeCompilationDiagnostics, NativeCompileError> {
        let slots = lower::slots_from_program(program)?;
        lower::compile_program_diagnostics(Arc::clone(&self.isa), program, &slots)
    }
    /// 使用运行时反馈 profile 编译进程内特化 overlay。
    pub fn compile_specialized_function(
        &self,
        program: &wjsm_ir::Program,
        variable_slots: &HashMap<String, u32>,
        function: wjsm_ir::FunctionId,
        argument_tags: &[wjsm_native_abi::NativeFeedbackTag],
        collect_diagnostics: bool,
    ) -> Result<NativeCompilationDiagnostics, specialize::SpecializationError> {
        let profile = specialize::SpecializationProfile {
            function,
            argument_tags: argument_tags.into(),
        };
        specialize::compile_specialized(
            Arc::clone(&self.isa),
            program,
            variable_slots,
            &profile,
            collect_diagnostics,
        )
    }

    /// 特化失败但不影响 JavaScript 语义时返回的内部诊断类型。
    pub fn specialized_diagnostics(
        &self,
        program: &wjsm_ir::Program,
        variable_slots: &HashMap<String, u32>,
        function: wjsm_ir::FunctionId,
        argument_tags: &[wjsm_native_abi::NativeFeedbackTag],
    ) -> Result<NativeCompilationDiagnostics, specialize::SpecializationError> {
        self.compile_specialized_function(program, variable_slots, function, argument_tags, true)
    }
}

fn set_flag(
    builder: &mut settings::Builder,
    name: &'static str,
    value: &'static str,
) -> Result<(), NativeCompileError> {
    builder
        .set(name, value)
        .map_err(|error| NativeCompileError::Cranelift(format!("invalid {name}={value}: {error}")))
}

#[derive(Clone, Debug, Error)]
pub enum NativeCompileError {
    #[error("unsupported native target capability: {0}")]
    UnsupportedTargetCapability(String),
    #[error("invalid semantic IR: {0}")]
    InvalidIr(String),
    #[error("native lowering failed for function {function:?}: {message}")]
    Lowering {
        function: wjsm_ir::FunctionId,
        message: String,
    },
    #[error("Cranelift compilation failed: {0}")]
    Cranelift(String),
    #[error("native object emission failed: {0}")]
    Object(String),
    #[error("native function {0:?} is missing unwind information")]
    MissingUnwindInfo(wjsm_ir::FunctionId),
    #[error(
        "native function {function:?} unwind variant mismatch: expected {expected}, got {actual}"
    )]
    UnwindVariantMismatch {
        function: wjsm_ir::FunctionId,
        expected: String,
        actual: String,
    },
    #[error("native compiler invariant failed: {0}")]
    CompilerInvariant(String),
    #[error("native compiler capacity exceeded for {0}")]
    Capacity(&'static str),
}

#[cfg(test)]
mod capability_tests {
    use super::{NativeCompileError, NativeCompiler, configured_opt_level};

    #[test]
    fn native_compiler_matches_declared_host_matrix() {
        let supported = cfg!(all(
            target_arch = "x86_64",
            any(target_os = "linux", target_os = "windows")
        ));
        if supported {
            let compiler = NativeCompiler::new().expect("declared native host must initialize");
            assert!(compiler.settings_key().contains("arch=x86_64"));
        } else {
            assert!(matches!(
                NativeCompiler::new(),
                Err(NativeCompileError::UnsupportedTargetCapability(_))
            ));
        }
    }

    /// opt_level 改变生成码，必须体现在 native cache 键里，否则不同档位会互相复用镜像。
    #[cfg(all(
        target_arch = "x86_64",
        any(target_os = "linux", target_os = "windows")
    ))]
    #[test]
    fn settings_key_tracks_configured_opt_level() {
        let compiler = NativeCompiler::new().expect("declared native host must initialize");
        let opt_level = configured_opt_level().expect("测试环境的 WJSM_OPT_LEVEL 必须合法");
        assert!(
            compiler
                .settings_key()
                .contains(&format!("opt={opt_level}")),
            "settings_key {:?} 未体现 opt_level {opt_level}",
            compiler.settings_key()
        );
    }
}

#[cfg(all(
    test,
    target_arch = "x86_64",
    any(target_os = "linux", target_os = "windows")
))]
mod tests {
    use object::{Object as _, ObjectSection as _};
    use std::collections::HashMap;
    use std::sync::Arc;
    use wjsm_artifact_format::{
        ArtifactBuildInput, BuildOptions, ModuleManifest, PortableArtifact,
    };
    use wjsm_ir::{
        BasicBlock, BasicBlockId, BinaryOp, Constant, Function, FunctionId, Instruction, Program,
        Terminator, ValueId,
    };
    use wjsm_native_abi::NativeFeedbackTag;

    use super::*;

    fn arithmetic_artifact() -> PortableArtifact {
        let mut program = Program::new();
        let one = program.add_constant(Constant::Number(1.0));
        let two = program.add_constant(Constant::Number(2.0));
        let mut function = Function::new("main", BasicBlockId(0));
        let mut block = BasicBlock::new(BasicBlockId(0));
        block.push_instruction(Instruction::Const {
            dest: ValueId(0),
            constant: one,
        });
        block.push_instruction(Instruction::Const {
            dest: ValueId(1),
            constant: two,
        });
        block.push_instruction(Instruction::Binary {
            dest: ValueId(2),
            op: BinaryOp::Add,
            lhs: ValueId(0),
            rhs: ValueId(1),
        });
        block.set_terminator(Terminator::Return {
            value: Some(ValueId(2)),
        });
        function.push_block(block);
        program.push_function(function);
        PortableArtifact::from_input(&ArtifactBuildInput {
            program: Arc::new(program),
            manifest: Arc::new(ModuleManifest::single("input.js", true)),
            options: BuildOptions::default(),
            source_text: None,
        })
        .expect("artifact should encode")
    }

    /// 不可静态证明 f64 的二元运算（null 是 NaN-boxed 值），lowering 必须发射
    /// 守卫快路径（原生 fadd）与 miss 落 dispatcher 的完整分支。
    fn guarded_binary_artifact() -> PortableArtifact {
        let mut program = Program::new();
        let null = program.add_constant(Constant::Null);
        let mut function = Function::new("main", BasicBlockId(0));
        let mut block = BasicBlock::new(BasicBlockId(0));
        block.push_instruction(Instruction::Const {
            dest: ValueId(0),
            constant: null,
        });
        block.push_instruction(Instruction::Const {
            dest: ValueId(1),
            constant: null,
        });
        block.push_instruction(Instruction::Binary {
            dest: ValueId(2),
            op: BinaryOp::Add,
            lhs: ValueId(0),
            rhs: ValueId(1),
        });
        block.set_terminator(Terminator::Return {
            value: Some(ValueId(2)),
        });
        function.push_block(block);
        program.push_function(function);
        PortableArtifact::from_input(&ArtifactBuildInput {
            program: Arc::new(program),
            manifest: Arc::new(ModuleManifest::single("input.js", true)),
            options: BuildOptions::default(),
            source_text: None,
        })
        .expect("artifact should encode")
    }

    fn property_ic_artifact() -> PortableArtifact {
        let mut program = Program::new();
        let key = program.add_constant(Constant::String("value".into()));
        let stored = program.add_constant(Constant::Null);
        let mut function = Function::new("property_ic", BasicBlockId(0));
        let mut block = BasicBlock::new(BasicBlockId(0));
        block.push_instruction(Instruction::Const {
            dest: ValueId(0),
            constant: key,
        });
        block.push_instruction(Instruction::Const {
            dest: ValueId(1),
            constant: stored,
        });
        block.push_instruction(Instruction::NewObject {
            dest: ValueId(2),
            capacity: 1,
        });
        block.push_instruction(Instruction::SetProp {
            dest: ValueId(3),
            object: ValueId(2),
            key: ValueId(0),
            value: ValueId(1),
        });
        block.push_instruction(Instruction::GetProp {
            dest: ValueId(4),
            object: ValueId(2),
            key: ValueId(0),
        });
        block.set_terminator(Terminator::Return {
            value: Some(ValueId(4)),
        });
        function.push_block(block);
        program.push_function(function);
        PortableArtifact::from_input(&ArtifactBuildInput {
            program: Arc::new(program),
            manifest: Arc::new(ModuleManifest::single("input.js", true)),
            options: BuildOptions::default(),
            source_text: None,
        })
        .expect("property IC artifact should encode")
    }

    #[test]
    fn template_object_literal_lowering_uses_baked_meta_reads() {
        fn sso_key(text: &str) -> u64 {
            let encoded = wjsm_ir::value::encode_inline_ascii(text.as_bytes()).expect("sso key");
            wjsm_ir::value::inline_property_key_raw(encoded).expect("property key")
        }
        let mut program = Program::new();
        let template = program.add_constant(Constant::ObjectTemplate {
            keys: vec![sso_key("name"), sso_key("value"), sso_key("length")],
        });
        let key_name = program.add_constant(Constant::String("name".into()));
        let key_value = program.add_constant(Constant::String("value".into()));
        let key_length = program.add_constant(Constant::String("length".into()));
        let mut function = Function::new("template_object", BasicBlockId(0));
        let mut block = BasicBlock::new(BasicBlockId(0));
        for (dest, constant) in [
            (ValueId(0), 1_u32),
            (ValueId(1), 2_u32),
            (ValueId(2), 3_u32),
        ] {
            block.push_instruction(Instruction::Const {
                dest,
                constant: program.add_constant(Constant::Number(f64::from(constant))),
            });
        }
        block.push_instruction(Instruction::InitObjectLiteral {
            dest: ValueId(3),
            template,
            values: vec![ValueId(0), ValueId(1), ValueId(2)],
        });
        block.push_instruction(Instruction::Const {
            dest: ValueId(10),
            constant: key_name,
        });
        block.push_instruction(Instruction::GetProp {
            dest: ValueId(4),
            object: ValueId(3),
            key: ValueId(10),
        });
        block.push_instruction(Instruction::Const {
            dest: ValueId(11),
            constant: key_value,
        });
        block.push_instruction(Instruction::GetProp {
            dest: ValueId(5),
            object: ValueId(3),
            key: ValueId(11),
        });
        block.push_instruction(Instruction::Const {
            dest: ValueId(12),
            constant: key_length,
        });
        block.push_instruction(Instruction::GetProp {
            dest: ValueId(6),
            object: ValueId(3),
            key: ValueId(12),
        });
        block.push_instruction(Instruction::SetProp {
            dest: ValueId(13),
            object: ValueId(3),
            key: ValueId(10),
            value: ValueId(0),
        });
        block.set_terminator(Terminator::Return {
            value: Some(ValueId(4)),
        });
        function.push_block(block);
        program.push_function(function);
        let artifact = PortableArtifact::from_input(&ArtifactBuildInput {
            program: Arc::new(program),
            manifest: Arc::new(ModuleManifest::single("input.js", true)),
            options: BuildOptions::default(),
            source_text: None,
        })
        .expect("template artifact should encode");
        let compiler = NativeCompiler::new().expect("host ISA should be supported");
        let diagnostics = compiler
            .diagnostics(&artifact)
            .expect("template object diagnostics should compile");
        let hints = ic_template_hints(artifact.program());
        assert!(
            hints.iter().any(|hint| hint.template_meta_index.is_some()),
            "expected template-linked IC hints: {hints:?}"
        );
        assert!(
            hints
                .iter()
                .any(|hint| hint.trio_prop_indices == Some([0, 1, 2])),
            "expected shared trio mega-slot hint: {hints:?}"
        );
        assert_eq!(
            hints.len(),
            1,
            "name/value/length should share one IC slot: {hints:?}"
        );
        assert!(
            diagnostics.disassembly.contains("cmp") || diagnostics.clif.contains("icmp"),
            "expected shape guard in generated code"
        );
        for offset in ["+24", "+32", "+40"] {
            assert!(
                diagnostics.clif.contains(offset),
                "expected compile-time slot offset {offset} in clif:\n{}",
                diagnostics.clif
            );
        }
        assert!(
            diagnostics.clif.contains("store"),
            "expected template SetProp store in clif:\n{}",
            diagnostics.clif
        );
    }

    #[test]
    fn template_origins_follow_module_binding_across_functions() {
        fn sso_key(text: &str) -> u64 {
            let encoded = wjsm_ir::value::encode_inline_ascii(text.as_bytes()).expect("sso key");
            wjsm_ir::value::inline_property_key_raw(encoded).expect("property key")
        }
        let mut program = Program::new();
        let template = program.add_constant(Constant::ObjectTemplate {
            keys: vec![sso_key("name"), sso_key("value"), sso_key("length")],
        });
        let key_name = program.add_constant(Constant::String("name".into()));
        let one = program.add_constant(Constant::Number(1.0));
        let mut init = Function::new("init", BasicBlockId(0));
        let mut init_block = BasicBlock::new(BasicBlockId(0));
        init_block.push_instruction(Instruction::Const {
            dest: ValueId(0),
            constant: one,
        });
        init_block.push_instruction(Instruction::InitObjectLiteral {
            dest: ValueId(1),
            template,
            values: vec![ValueId(0), ValueId(0), ValueId(0)],
        });
        init_block.push_instruction(Instruction::StoreVar {
            name: "$0.RECORD".into(),
            value: ValueId(1),
        });
        init_block.set_terminator(Terminator::Return { value: None });
        init.push_block(init_block);
        program.push_function(init);

        let mut work = Function::new("work", BasicBlockId(0));
        let mut work_block = BasicBlock::new(BasicBlockId(0));
        work_block.push_instruction(Instruction::LoadVar {
            dest: ValueId(0),
            name: "$0.RECORD".into(),
        });
        work_block.push_instruction(Instruction::Const {
            dest: ValueId(1),
            constant: key_name,
        });
        work_block.push_instruction(Instruction::GetProp {
            dest: ValueId(2),
            object: ValueId(0),
            key: ValueId(1),
        });
        work_block.push_instruction(Instruction::SetProp {
            dest: ValueId(3),
            object: ValueId(0),
            key: ValueId(1),
            value: ValueId(2),
        });
        work_block.set_terminator(Terminator::Return {
            value: Some(ValueId(2)),
        });
        work.push_block(work_block);
        program.push_function(work);

        let origins = crate::template_meta::build_template_origin_maps(&program);
        assert!(
            origins
                .get(1)
                .is_some_and(|map| map.contains_key(&ValueId(0))),
            "work() LoadVar RECORD should keep template origin: {origins:?}"
        );
        let artifact = PortableArtifact::from_input(&ArtifactBuildInput {
            program: Arc::new(program),
            manifest: Arc::new(ModuleManifest::single("input.js", true)),
            options: BuildOptions::default(),
            source_text: None,
        })
        .expect("cross-function template artifact should encode");
        let compiler = NativeCompiler::new().expect("host ISA should be supported");
        let diagnostics = compiler
            .diagnostics(&artifact)
            .expect("cross-function template diagnostics should compile");
        assert!(
            diagnostics.clif.contains("+24"),
            "expected compile-time name slot offset in work():\n{}",
            diagnostics.clif
        );
    }

    #[test]
    fn property_ic_lowering_guards_epoch_and_imports_barriers() {
        let compiler = NativeCompiler::new().expect("host ISA should be supported");
        let diagnostics = compiler
            .diagnostics(&property_ic_artifact())
            .expect("property IC diagnostics should compile");
        assert!(diagnostics.clif.contains("atomic_load"));
        assert!(diagnostics.clif.contains("atomic_store"));

        let parsed = object::File::parse(diagnostics.object.bytes()).expect("object should parse");
        assert!(
            parsed
                .symbol_by_name("wjsm_native_zgc_load_barrier_assist")
                .is_some()
        );
        assert!(
            parsed
                .symbol_by_name("wjsm_native_zgc_store_barrier")
                .is_some()
        );
    }

    #[test]
    fn compiles_arithmetic_to_native_object() {
        let compiler = NativeCompiler::new().expect("host ISA should be supported");
        let object = compiler
            .compile(&arithmetic_artifact())
            .expect("arithmetic should compile");
        assert_eq!(object.function_count(), 1);
        assert_eq!(object.frame_bytes().len(), 1);
        let parsed = object::File::parse(object.bytes()).expect("object should parse");
        assert_eq!(parsed.architecture(), object::Architecture::X86_64);
        assert!(parsed.symbol_by_name("wjsm_function_0").is_some());
    }

    #[test]
    fn f64_add_skips_nan_canonicalization() {
        let compiler = NativeCompiler::new().expect("host ISA should be supported");
        let diagnostics = compiler
            .diagnostics(&arithmetic_artifact())
            .expect("arithmetic diagnostics should compile");
        assert!(
            diagnostics.clif.contains("fadd"),
            "expected native fadd:\n{}",
            diagnostics.clif
        );
        assert!(
            !diagnostics.clif.contains("uno"),
            "f64 Add/Sub should bitcast without unordered NaN canonicalize:\n{}",
            diagnostics.clif
        );
    }

    #[test]
    fn diagnostics_report_clif_and_machine_disassembly() {
        let compiler = NativeCompiler::new().expect("host ISA should be supported");
        let diagnostics = compiler
            .diagnostics(&arithmetic_artifact())
            .expect("arithmetic diagnostics should compile");
        assert!(diagnostics.clif.contains("function"));
        assert!(diagnostics.disassembly.contains("function 0: main"));
        assert!(!diagnostics.disassembly.trim().is_empty());
    }

    #[test]
    fn guarded_binary_emits_native_fadd_with_dispatcher_fallback() {
        let compiler = NativeCompiler::new().expect("host ISA should be supported");
        let diagnostics = compiler
            .diagnostics(&guarded_binary_artifact())
            .expect("guarded binary diagnostics should compile");
        // 快路径：原生 fadd。
        assert!(diagnostics.clif.contains("fadd"), "CLIF 应包含原生 fadd");
        // 慢路径：miss 时落 DYNAMIC_BINARY_BASE + BinaryAdd.tag。
        assert!(
            diagnostics.clif.contains("0x0001_0000"),
            "CLIF 应包含 BinaryAdd dispatcher 操作码"
        );
    }

    #[test]
    fn specialized_overlay_emits_guard_body_and_generic_fallback() {
        let mut program = Program::new();
        let one = program.add_constant(Constant::Number(1.0));
        let mut function = Function::new("add1", BasicBlockId(0));
        function.set_params(vec!["$env".into(), "$this".into(), "x".into()]);
        let mut block = BasicBlock::new(BasicBlockId(0));
        block.push_instruction(Instruction::LoadVar {
            dest: ValueId(0),
            name: "x".into(),
        });
        block.push_instruction(Instruction::Const {
            dest: ValueId(1),
            constant: one,
        });
        block.push_instruction(Instruction::Binary {
            dest: ValueId(2),
            op: BinaryOp::Add,
            lhs: ValueId(0),
            rhs: ValueId(1),
        });
        block.set_terminator(Terminator::Return {
            value: Some(ValueId(2)),
        });
        function.push_block(block);
        program.push_function(function);

        let compiler = NativeCompiler::new().expect("host ISA should be supported");
        let diagnostics = compiler
            .specialized_diagnostics(
                &program,
                &HashMap::new(),
                FunctionId(0),
                &[NativeFeedbackTag::Number],
            )
            .expect("number profile should produce a specialized overlay");
        assert!(
            diagnostics.clif.contains("brif"),
            "wrapper should guard tags"
        );
        assert!(
            diagnostics.clif.contains("call_indirect"),
            "wrapper should fall back to the base slow entry"
        );
        assert!(
            diagnostics.clif.contains("fadd"),
            "typed body should use native fadd"
        );
        assert!(
            diagnostics.clif.contains("specialized wrapper"),
            "diagnostics should include the specialized wrapper"
        );
        assert_eq!(diagnostics.object.function_count(), 2);
    }

    /// 编译 arithmetic object 后，各平台必须产出对应 unwind section；
    /// 用 cfg 保证每个 runner 只执行本平台断言。
    #[test]
    fn native_object_contains_platform_unwind_sections() {
        let compiler = NativeCompiler::new().expect("host ISA should be supported");
        let object = compiler
            .compile(&arithmetic_artifact())
            .expect("arithmetic should compile");
        let parsed = object::File::parse(object.bytes()).expect("object should parse");
        let names: Vec<String> = parsed
            .sections()
            .map(|section| section.name().unwrap_or("<invalid>").to_owned())
            .collect();
        #[cfg(target_os = "linux")]
        {
            assert!(
                names.iter().any(|name| name == ".eh_frame"),
                "Linux object must contain .eh_frame, got {names:?}"
            );
        }
        #[cfg(target_os = "windows")]
        {
            assert!(
                names.iter().any(|name| name == ".pdata"),
                "Windows object must contain .pdata, got {names:?}"
            );
            assert!(
                names.iter().any(|name| name == ".xdata"),
                "Windows object must contain .xdata, got {names:?}"
            );
        }
    }

    struct NoSymbols;

    impl NativeSymbolResolver for NoSymbols {
        fn resolve(&self, _symbol: NativeHostSymbol) -> Option<usize> {
            None
        }
    }

    #[test]
    fn loads_and_executes_arithmetic_image() {
        let compiler = NativeCompiler::new().expect("host ISA should be supported");
        let object = compiler
            .compile(&arithmetic_artifact())
            .expect("arithmetic should compile");
        let image = image::CompiledImage::load(&object, 7, &NoSymbols)
            .expect("arithmetic image should load");
        let mut context = wjsm_native_abi::NativeVmContext::default();
        let entry = image.entries()[0].slow_entry;
        // SAFETY: entry 由 loader 从统一 slow-entry signature 的已验证 RX symbol 构造；vmctx
        // 在整个调用期间有效，当前函数不读取 call arena。
        let result = unsafe { entry(&mut context, 0, 0, 0, 0) };
        assert_eq!(wjsm_ir::value::decode_f64(result), 3.0);
    }

    #[test]
    fn native_cache_hits_and_invalidates_corruption() {
        let cache_dir = std::env::temp_dir()
            .join("wjsm-test-cache")
            .join("backend")
            .join(format!("native-cache-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&cache_dir);
        let artifact = arithmetic_artifact();
        let first = cache::NativeImageRepository::new(
            NativeCompiler::new().expect("host ISA should be supported"),
            Some(cache_dir.clone()),
        );
        let image = first
            .prepare(&artifact, &NoSymbols)
            .expect("cache miss should compile");
        assert_eq!(first.stats().misses, 1);
        drop(image);

        let second = cache::NativeImageRepository::new(
            NativeCompiler::new().expect("host ISA should be supported"),
            Some(cache_dir.clone()),
        );
        let image = second
            .prepare(&artifact, &NoSymbols)
            .expect("cache hit should load");
        assert_eq!(second.stats().hits, 1);
        drop(image);

        let cache_path = std::fs::read_dir(&cache_dir)
            .expect("cache directory should exist")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| path.extension().and_then(|extension| extension.to_str()) == Some("wnat"))
            .expect("cache entry should exist");
        let mut bytes = std::fs::read(&cache_path).expect("cache entry should be readable");
        let last = bytes.last_mut().expect("cache entry should be non-empty");
        *last ^= 1;
        std::fs::write(&cache_path, bytes).expect("cache entry should be corruptible");

        let third = cache::NativeImageRepository::new(
            NativeCompiler::new().expect("host ISA should be supported"),
            Some(cache_dir.clone()),
        );
        third
            .prepare(&artifact, &NoSymbols)
            .expect("corrupt cache should be recompiled");
        assert_eq!(third.stats().invalidated, 1);
        assert_eq!(third.stats().misses, 1);
        let _ = std::fs::remove_dir_all(&cache_dir);
    }

    #[test]
    fn native_cache_hits_same_builtin_program_across_users() {
        let cache_dir = std::env::temp_dir()
            .join("wjsm-test-cache")
            .join("backend")
            .join(format!("native-cache-segment-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&cache_dir);
        let builtin = {
            let mut program = Program::new();
            let mut function = Function::new("$builtin_main", BasicBlockId(0));
            let mut block = BasicBlock::new(BasicBlockId(0));
            block.set_terminator(Terminator::Return { value: None });
            function.push_block(block);
            program.push_function(function);
            program
        };
        let user_a = {
            let mut program = Program::new();
            let mut function = Function::new("$module_main", BasicBlockId(0));
            let mut block = BasicBlock::new(BasicBlockId(0));
            block.set_terminator(Terminator::Return { value: None });
            function.push_block(block);
            program.push_function(function);
            program
        };
        let user_b = {
            let mut program = Program::new();
            let one = program.add_constant(Constant::Number(1.0));
            let mut function = Function::new("$module_main", BasicBlockId(0));
            let mut block = BasicBlock::new(BasicBlockId(0));
            block.push_instruction(Instruction::Const {
                dest: ValueId(0),
                constant: one,
            });
            block.set_terminator(Terminator::Return {
                value: Some(ValueId(0)),
            });
            function.push_block(block);
            program.push_function(function);
            program
        };
        let empty_slots = std::collections::HashMap::new();
        let segmented = cache::NativeImageRepository::new(
            NativeCompiler::new().expect("host ISA should be supported"),
            Some(cache_dir.clone()),
        );
        segmented
            .prepare_program_with_slots(&builtin, &empty_slots, &NoSymbols)
            .expect("builtin 段首次 prepare 应 miss");
        segmented
            .prepare_program_with_slots(&user_a, &empty_slots, &NoSymbols)
            .expect("用户段 A 应独立编译");
        let before = segmented.stats();
        segmented
            .prepare_program_with_slots(&builtin, &empty_slots, &NoSymbols)
            .expect("同一 builtin 段第二次 prepare 应 hit");
        segmented
            .prepare_program_with_slots(&user_b, &empty_slots, &NoSymbols)
            .expect("用户段 B 应独立编译");
        let after = segmented.stats();
        assert!(
            after.hits > before.hits,
            "builtin 段必须按 Program digest 复用"
        );
        let _ = std::fs::remove_dir_all(cache_dir);
    }
}
