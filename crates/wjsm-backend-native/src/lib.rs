pub mod cache;
pub(crate) mod call_graph;
pub(crate) mod env_layout;
pub(crate) mod f64_analysis;
pub(crate) mod fast_call;
pub mod image;
pub(crate) mod lower;
pub(crate) mod safepoint_free;
pub(crate) mod template_meta;
pub use env_layout::{ENV_LAYOUT_META_WORDS, bake_env_layout_meta_table};
pub use template_meta::{IcTemplateHint, ic_template_hints};
pub(crate) mod root_plan;
pub(crate) mod specialize;
pub(crate) mod unwind;
pub(crate) mod value_repr;

use std::collections::HashMap;
use std::collections::HashSet;
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
    /// lowering 预计算的类型反馈槽总数（80 字节/槽）；运行时据此分配反馈缓冲。
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
        // typed f64 热路径把原始机器浮点常驻寄存器，只在 boxed 边界
        // （`box_f64_result` / `use_value_boxed`）规范化 NaN，避免与 BOX_BASE
        // 撞车。Cranelift 全局规范化会在每个 fadd/fmul 后插入
        // vcmpunordpd+vpblendvb，把纯数值循环拉慢一个数量级。
        set_flag(&mut flag_builder, "enable_nan_canonicalization", "false")?;
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
            "target={};arch={target_arch};os={target_os};cranelift={};pic={};unwind=1;unwind-object={};nan=boxed-escape;resume=skip-typed-f64;roots=skip-bool-imm;poll=reg-spfree;opt={opt_level};probestack=inline:4096",
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
        extra_numbers: &HashSet<wjsm_ir::ValueId>,
        facts: Option<wjsm_optimize::SpeculativeFacts>,
        collect_diagnostics: bool,
    ) -> Result<NativeCompilationDiagnostics, specialize::SpecializationError> {
        let mut facts = facts.unwrap_or_else(|| wjsm_optimize::SpeculativeFacts {
            function,
            param_tags: argument_tags.iter().map(|tag| tag.code()).collect(),
            extra_number_values: extra_numbers.iter().copied().collect(),
            get_props: Vec::new(),
            set_props: Vec::new(),
            get_elems: Vec::new(),
            set_elems: Vec::new(),
            calls: Vec::new(),
            binaries: Vec::new(),
        });
        facts.function = function;
        if facts.extra_number_values.is_empty() {
            facts.extra_number_values = extra_numbers.iter().copied().collect();
        }
        let profile = specialize::SpecializationProfile {
            function,
            argument_tags: argument_tags.into(),
            extra_numbers: extra_numbers.clone(),
            slot_map: None,
            facts,
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
        self.compile_specialized_function(
            program,
            variable_slots,
            function,
            argument_tags,
            &HashSet::new(),
            None,
            true,
        )
    }
}

/// 反馈槽下标 → 源级 callsite 表达式渲染（`Call`/`ConstructCall` 携带的
/// `callsite`）。宿主按 `(image, slot)` 在拒绝路径渲染
/// `<expr> is not a function/constructor`（对齐 Node）。
pub fn callsites_by_feedback_slot(program: &wjsm_ir::Program) -> HashMap<u32, Box<str>> {
    lower::callsites_by_feedback_slot(program)
}

/// 反馈槽对应的 Binary/Compare/Unary SSA，用作 overlay 值类种子。
pub fn extra_numbers_at_feedback_site(
    program: &wjsm_ir::Program,
    function: wjsm_ir::FunctionId,
    site_index: u32,
) -> HashSet<wjsm_ir::ValueId> {
    specialize::extra_numbers_at_site(program, function, site_index)
}

pub fn feedback_instruction_at(
    program: &wjsm_ir::Program,
    function: wjsm_ir::FunctionId,
    site_index: u32,
) -> Option<(wjsm_ir::BasicBlockId, u32, wjsm_ir::Instruction)> {
    specialize::feedback_instruction_at(program, function, site_index)
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
            assert!(
                compiler.settings_key().contains("roots=skip-bool-imm"),
                "布尔立即数不当 GC 根必须进 cache 键:\n{}",
                compiler.settings_key()
            );
            assert!(
                compiler.settings_key().contains("poll=reg-spfree"),
                "safepoint-free 寄存器预算必须进 cache 键:\n{}",
                compiler.settings_key()
            );
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

    /// `let i = 0; while (i < 4) { i = i + step; } return i;` 的帧局部形状。
    ///
    /// `step` 决定 `$1.i` 是纯数值归纳变量还是混合类型局部。
    fn numeric_loop_artifact(step: Constant) -> PortableArtifact {
        let mut program = Program::new();
        let zero = program.add_constant(Constant::Number(0.0));
        let limit = program.add_constant(Constant::Number(4.0));
        let step = program.add_constant(step);
        let mut function = Function::new("loop_sum", BasicBlockId(0));

        let mut entry = BasicBlock::new(BasicBlockId(0));
        entry.push_instruction(Instruction::Const {
            dest: ValueId(0),
            constant: zero,
        });
        entry.push_instruction(Instruction::StoreVar {
            name: "$1.i".into(),
            value: ValueId(0),
        });
        entry.set_terminator(Terminator::Jump {
            target: BasicBlockId(1),
        });

        let mut header = BasicBlock::new(BasicBlockId(1));
        header.push_instruction(Instruction::LoadVar {
            dest: ValueId(1),
            name: "$1.i".into(),
        });
        header.push_instruction(Instruction::Const {
            dest: ValueId(2),
            constant: limit,
        });
        header.push_instruction(Instruction::Compare {
            dest: ValueId(3),
            op: wjsm_ir::CompareOp::Lt,
            lhs: ValueId(1),
            rhs: ValueId(2),
        });
        header.set_terminator(Terminator::Branch {
            condition: ValueId(3),
            true_block: BasicBlockId(2),
            false_block: BasicBlockId(3),
        });

        let mut body = BasicBlock::new(BasicBlockId(2));
        body.push_instruction(Instruction::LoadVar {
            dest: ValueId(4),
            name: "$1.i".into(),
        });
        body.push_instruction(Instruction::Const {
            dest: ValueId(5),
            constant: step,
        });
        body.push_instruction(Instruction::Binary {
            dest: ValueId(6),
            op: BinaryOp::Add,
            lhs: ValueId(4),
            rhs: ValueId(5),
        });
        body.push_instruction(Instruction::StoreVar {
            name: "$1.i".into(),
            value: ValueId(6),
        });
        body.set_terminator(Terminator::Jump {
            target: BasicBlockId(1),
        });

        let mut exit = BasicBlock::new(BasicBlockId(3));
        exit.push_instruction(Instruction::LoadVar {
            dest: ValueId(7),
            name: "$1.i".into(),
        });
        exit.set_terminator(Terminator::Return {
            value: Some(ValueId(7)),
        });

        for block in [entry, header, body, exit] {
            function.push_block(block);
        }
        program.push_function(function);
        PortableArtifact::from_input(&ArtifactBuildInput {
            program: Arc::new(program),
            manifest: Arc::new(ModuleManifest::single("input.js", true)),
            options: BuildOptions::default(),
            source_text: None,
        })
        .expect("artifact should encode")
    }

    /// 一个输入已证明 f64、另一个输入是 `null` 的 φ 合流。
    fn mixed_phi_artifact() -> PortableArtifact {
        let mut program = Program::new();
        let flag = program.add_constant(Constant::Bool(true));
        let number = program.add_constant(Constant::Number(1.5));
        let null = program.add_constant(Constant::Null);
        let mut function = Function::new("mixed_phi", BasicBlockId(0));

        let mut entry = BasicBlock::new(BasicBlockId(0));
        entry.push_instruction(Instruction::Const {
            dest: ValueId(0),
            constant: flag,
        });
        entry.set_terminator(Terminator::Branch {
            condition: ValueId(0),
            true_block: BasicBlockId(1),
            false_block: BasicBlockId(2),
        });

        let mut number_arm = BasicBlock::new(BasicBlockId(1));
        number_arm.push_instruction(Instruction::Const {
            dest: ValueId(1),
            constant: number,
        });
        number_arm.set_terminator(Terminator::Jump {
            target: BasicBlockId(3),
        });

        let mut null_arm = BasicBlock::new(BasicBlockId(2));
        null_arm.push_instruction(Instruction::Const {
            dest: ValueId(2),
            constant: null,
        });
        null_arm.set_terminator(Terminator::Jump {
            target: BasicBlockId(3),
        });

        let mut merge = BasicBlock::new(BasicBlockId(3));
        merge.push_instruction(Instruction::Phi {
            dest: ValueId(3),
            sources: vec![
                wjsm_ir::PhiSource {
                    predecessor: BasicBlockId(1),
                    value: ValueId(1),
                },
                wjsm_ir::PhiSource {
                    predecessor: BasicBlockId(2),
                    value: ValueId(2),
                },
            ],
        });
        merge.set_terminator(Terminator::Return {
            value: Some(ValueId(3)),
        });

        for block in [entry, number_arm, null_arm, merge] {
            function.push_block(block);
        }
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
            strict: false,
        });
        block.push_instruction(Instruction::GetProp {
            dest: ValueId(4),
            object: ValueId(2),
            key: ValueId(0),
            latch: None,
            latch_template: None,
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
            latch: None,
            latch_template: None,
        });
        block.push_instruction(Instruction::Const {
            dest: ValueId(11),
            constant: key_value,
        });
        block.push_instruction(Instruction::GetProp {
            dest: ValueId(5),
            object: ValueId(3),
            key: ValueId(11),
            latch: None,
            latch_template: None,
        });
        block.push_instruction(Instruction::Const {
            dest: ValueId(12),
            constant: key_length,
        });
        block.push_instruction(Instruction::GetProp {
            dest: ValueId(6),
            object: ValueId(3),
            key: ValueId(12),
            latch: None,
            latch_template: None,
        });
        block.push_instruction(Instruction::SetProp {
            dest: ValueId(13),
            object: ValueId(3),
            key: ValueId(10),
            value: ValueId(0),
            strict: false,
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
            latch: None,
            latch_template: None,
        });
        work_block.push_instruction(Instruction::SetProp {
            dest: ValueId(3),
            object: ValueId(0),
            key: ValueId(1),
            value: ValueId(2),
            strict: false,
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
    fn safepoint_free_numeric_main_omits_root_frame_link() {
        let compiler = NativeCompiler::new().expect("host ISA should be supported");
        let diagnostics = compiler
            .diagnostics(&arithmetic_artifact())
            .expect("arithmetic diagnostics should compile");
        assert!(
            !diagnostics.clif.contains("root_frame_head"),
            "safepoint-free main should not touch root_frame_head:\n{}",
            diagnostics.clif
        );
        assert!(
            !diagnostics.clif.contains("atomic_rmw"),
            "safepoint-free main should not link/unlink root frames:\n{}",
            diagnostics.clif
        );
    }

    /// Compare dest 是 tagged bool 立即数，不是堆句柄：数值循环不得因此挂 root frame。
    #[test]
    fn safepoint_free_numeric_loop_omits_root_frame_link() {
        let compiler = NativeCompiler::new().expect("host ISA should be supported");
        let diagnostics = compiler
            .diagnostics(&numeric_loop_artifact(Constant::Number(1.0)))
            .expect("numeric loop diagnostics should compile");
        assert!(
            !diagnostics.clif.contains("root_frame_head"),
            "f64 循环的 Compare 布尔不应挂 root frame:\n{}",
            diagnostics.clif
        );
        assert!(
            !diagnostics.clif.contains("atomic_rmw"),
            "f64 循环不应在回边发布根:\n{}",
            diagnostics.clif
        );
    }

    /// safepoint-free 数值循环把 poll 预算留在寄存器：快路径 `isub` 块不得 store vmctx。
    #[test]
    fn safepoint_free_loop_keeps_poll_budget_in_register() {
        let compiler = NativeCompiler::new().expect("host ISA should be supported");
        let diagnostics = compiler
            .diagnostics(&numeric_loop_artifact(Constant::Number(1.0)))
            .expect("numeric loop diagnostics should compile");
        let body = clif_section(&diagnostics.clif, ";; function 0: loop_sum");
        let step = i64::try_from(wjsm_native_abi::COOPERATIVE_POLL_LOOP_BACKEDGE_STEP_BYTES)
            .expect("loop poll step");
        let step_imm = format!("iconst.i64 {step}");
        let fast = clif_block_containing(body, "isub").expect("回边快路径应发出 isub");
        assert!(
            fast.contains(&step_imm) || body.contains(&step_imm),
            "快路径应扣减回边步长 {step}:\n{body}"
        );
        assert!(
            !fast.contains("store"),
            "寄存器预算快路径不得 store stack_budget_bytes:\n{fast}"
        );
        assert!(
            body.contains("store"),
            "函数出口仍须把预算写回 vmctx:\n{body}"
        );
    }

    #[test]
    fn guarded_binary_still_links_root_frame() {
        let compiler = NativeCompiler::new().expect("host ISA should be supported");
        let diagnostics = compiler
            .diagnostics(&guarded_binary_artifact())
            .expect("guarded binary diagnostics should compile");
        assert!(
            diagnostics.clif.contains("atomic_rmw"),
            "may-GC function should still link/unlink NativeRootFrame:\n{}",
            diagnostics.clif
        );
    }

    /// CLIF 只在控制类型不可推断时打印 `.f64` 后缀，比对 `fcmp` 形状前先归一。
    fn normalize_fcmp(clif: &str) -> String {
        clif.replace("fcmp.f64", "fcmp")
    }

    /// 已证明 f64 的常量与加法全程留在浮点表示里：既不 iconst 编码常量，
    /// 也不为了喂给 `fadd` 而拆包。
    #[test]
    fn f64_add_stays_unboxed_until_escape() {
        let compiler = NativeCompiler::new().expect("host ISA should be supported");
        let diagnostics = compiler
            .diagnostics(&arithmetic_artifact())
            .expect("arithmetic diagnostics should compile");
        assert!(
            diagnostics.clif.contains("f64const"),
            "number 常量应直接物化成浮点常量:\n{}",
            diagnostics.clif
        );
        assert!(
            diagnostics.clif.contains("fadd"),
            "expected native fadd:\n{}",
            diagnostics.clif
        );
        // 热路径用 f64const 直喂 fadd。generic 入口的 resume 分发会把 boxed
        // live 载入后再 bitcast 成 f64 块参数，那不是给 fadd 拆编码常量。
        assert_eq!(
            normalize_fcmp(&diagnostics.clif)
                .matches("fcmp uno")
                .count(),
            1,
            "只有 return 这一个逃逸点需要规范化 NaN:\n{}",
            diagnostics.clif
        );
    }

    /// 逃逸点必须把原始机器 NaN 换成规范 NaN：硬件默认 QNaN 的位模式与
    /// `value::BOX_BASE` 相同，直接外泄会被误判成句柄。
    #[test]
    fn f64_escape_boxes_canonical_nan() {
        let compiler = NativeCompiler::new().expect("host ISA should be supported");
        let diagnostics = compiler
            .diagnostics(&arithmetic_artifact())
            .expect("arithmetic diagnostics should compile");
        assert!(
            normalize_fcmp(&diagnostics.clif).contains("fcmp uno"),
            "逃逸点应测 unordered:\n{}",
            diagnostics.clif
        );
        assert!(
            diagnostics.clif.contains("0x7ff8_0000_0000_0000"),
            "逃逸点应选出规范 NaN:\n{}",
            diagnostics.clif
        );
    }

    /// 循环回边 cooperative poll 使用较小步长，避免无分配紧循环频繁进 dispatcher。
    #[test]
    fn loop_backedge_cooperative_poll_uses_small_step() {
        use wjsm_native_abi::COOPERATIVE_POLL_LOOP_BACKEDGE_STEP_BYTES;
        let compiler = NativeCompiler::new().expect("host ISA should be supported");
        let diagnostics = compiler
            .diagnostics(&numeric_loop_artifact(Constant::Number(1.0)))
            .expect("numeric loop diagnostics should compile");
        let step =
            i64::try_from(COOPERATIVE_POLL_LOOP_BACKEDGE_STEP_BYTES).expect("loop poll step");
        assert!(
            diagnostics.clif.contains(&format!("iconst.i64 {step}")),
            "loop back-edge poll should deduct {step} bytes:\n{}",
            diagnostics.clif
        );
        assert!(
            !diagnostics.clif.contains("iconst.i64 0x0001_0000"),
            "loop back-edge should not use allocation poll step:\n{}",
            diagnostics.clif
        );
    }

    /// 循环携带的归纳变量整轮迭代常驻浮点寄存器：循环头的块参数是 `f64`，
    /// 回边上没有打标/拆包，只有 `return` 这一个逃逸点转换一次。
    #[test]
    fn loop_carried_f64_stays_in_float_registers() {
        let compiler = NativeCompiler::new().expect("host ISA should be supported");
        let diagnostics = compiler
            .diagnostics(&numeric_loop_artifact(Constant::Number(1.0)))
            .expect("numeric loop diagnostics should compile");
        assert!(
            diagnostics.clif.contains(": f64"),
            "循环头应以 f64 块参数携带归纳变量:\n{}",
            diagnostics.clif
        );
        assert!(
            diagnostics.clif.contains("fadd"),
            "自增应发原生 fadd:\n{}",
            diagnostics.clif
        );
        assert!(
            normalize_fcmp(&diagnostics.clif).contains("fcmp lt"),
            "已证明 f64 的关系比较应发原生 fcmp:\n{}",
            diagnostics.clif
        );
        // resume landing pad 恢复循环 live 时允许 bitcast.f64；循环体自增仍是 fadd。
        assert_eq!(
            normalize_fcmp(&diagnostics.clif)
                .matches("fcmp uno")
                .count(),
            1,
            "只有 return 这一个逃逸点需要规范化 NaN:\n{}",
            diagnostics.clif
        );
    }

    /// Cranelift 不得在 typed f64 循环体里逐条规范化 NaN：那会在每个
    /// fadd/fmul 后插入 vcmpunordpd+vpblendvb。NaN-Box 只在 boxed 逃逸点处理。
    #[test]
    fn typed_f64_loop_machine_code_skips_nan_canonicalize() {
        let compiler = NativeCompiler::new().expect("host ISA should be supported");
        let diagnostics = compiler
            .diagnostics(&numeric_loop_artifact(Constant::Number(1.0)))
            .expect("numeric loop diagnostics should compile");
        let body = clif_section(&diagnostics.disassembly, ";; function 0: loop_sum");
        assert!(
            !body.is_empty(),
            "应能截出 loop_sum 反汇编:\n{}",
            diagnostics.disassembly
        );
        assert!(
            !body.contains("vcmpunordpd") && !body.contains("vpblendvb"),
            "typed f64 循环体不应被 Cranelift 逐条规范化 NaN:\n{body}"
        );
        assert!(
            body.contains("addsd") || body.contains("vaddsd"),
            "循环自增应保留原生 addsd:\n{body}"
        );
    }

    /// 已证明 f64 的算术不再按指令切开 resume pad：`fadd` 必须与回边
    /// cooperative poll 同块。逐指令 pad 会在 fadd 后立刻 `jump`，把 poll 拆走。
    #[test]
    fn typed_f64_loop_arith_shares_block_with_backedge() {
        let compiler = NativeCompiler::new().expect("host ISA should be supported");
        let diagnostics = compiler
            .diagnostics(&numeric_loop_artifact(Constant::Number(1.0)))
            .expect("numeric loop diagnostics should compile");
        let body = clif_section(&diagnostics.clif, ";; function 0: loop_sum");
        let block = clif_block_containing(body, "fadd").expect("loop_sum 应发出 fadd");
        assert!(
            block.contains("brif"),
            "fadd 应与回边 poll 同块，而不是被 resume pad 切开:\n{block}"
        );
    }

    /// 同一帧局部混入非 number 写入时，加法必须走动态二元 dispatcher；
    /// 入口 `undefined` 初值与 `null` 都不是合法的 double 位模式。
    /// resume 分发仍可能为其它已证明 number 的 SSA 发出 f64 块参数。
    #[test]
    fn mixed_type_local_stays_boxed() {
        let compiler = NativeCompiler::new().expect("host ISA should be supported");
        let diagnostics = compiler
            .diagnostics(&numeric_loop_artifact(Constant::Null))
            .expect("mixed loop diagnostics should compile");
        // `$1.i` 混入 null 后加法必须走动态二元（宿主 dispatcher），不能只靠
        // 原生 fadd。resume 仍可能为循环里的 number 常量比较发出 f64 块参数。
        assert!(
            diagnostics.clif.contains("0x0001_0000"),
            "混合类型加法应调用动态二元 dispatcher:\n{}",
            diagnostics.clif
        );
    }

    /// φ 只在部分输入已证明 f64 时保持 boxed：typed 那条边打标，另一条边直传。
    #[test]
    fn mixed_phi_boxes_only_f64_edge() {
        let compiler = NativeCompiler::new().expect("host ISA should be supported");
        let diagnostics = compiler
            .diagnostics(&mixed_phi_artifact())
            .expect("mixed phi diagnostics should compile");
        assert!(
            diagnostics.clif.contains("f64const"),
            "已证明 f64 的那条边仍以浮点常量物化:\n{}",
            diagnostics.clif
        );
        assert_eq!(
            normalize_fcmp(&diagnostics.clif)
                .matches("fcmp uno")
                .count(),
            1,
            "只有 f64 那条边需要打标:\n{}",
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
            diagnostics.clif.contains("fadd")
                || diagnostics.clif.contains("sadd_overflow")
                || diagnostics.clif.contains("iadd"),
            "typed body should use native number add, got:\n{}",
            diagnostics.clif
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

    fn clif_section<'a>(clif: &'a str, marker: &str) -> &'a str {
        let Some(start) = clif.find(marker) else {
            return "";
        };
        let rest = &clif[start..];
        let skip = marker.len();
        let next = rest[skip..]
            .find("\n;; function")
            .or_else(|| rest[skip..].find("\n;; trampoline"));
        match next {
            Some(rel) => &rest[..skip + rel],
            None => rest,
        }
    }

    /// 截出包含 `needle` 的 CLIF 基本块（从 `blockN:` 到下一个 `block`）。
    fn clif_block_containing<'a>(clif: &'a str, needle: &str) -> Option<&'a str> {
        let at = clif.find(needle)?;
        let prefix = &clif[..at];
        let start = prefix.rfind("\nblock").map(|idx| idx + 1).unwrap_or(0);
        let rest = &clif[at..];
        let end = rest
            .find("\nblock")
            .map(|rel| at + rel)
            .unwrap_or(clif.len());
        Some(&clif[start..end])
    }

    fn direct_call_artifact(js_params: usize, call_args: usize, callee: &str) -> PortableArtifact {
        let mut program = Program::new();
        let number = program.add_constant(Constant::Number(1.0));
        let function_ref = program.add_constant(Constant::FunctionRef(FunctionId(1)));

        let mut caller = Function::new("main", BasicBlockId(0));
        let mut caller_block = BasicBlock::new(BasicBlockId(0));
        caller_block.push_instruction(Instruction::Const {
            dest: ValueId(0),
            constant: number,
        });
        caller_block.push_instruction(Instruction::Const {
            dest: ValueId(1),
            constant: function_ref,
        });
        let mut args = Vec::new();
        for index in 0..call_args {
            let dest = ValueId(2 + u32::try_from(index).expect("arg index fits u32"));
            caller_block.push_instruction(Instruction::Const {
                dest,
                constant: number,
            });
            args.push(dest);
        }
        let result = ValueId(2 + u32::try_from(call_args).expect("result id fits u32"));
        caller_block.push_instruction(Instruction::Call {
            dest: Some(result),
            callee: ValueId(1),
            this_val: ValueId(0),
            args,
            callsite: None,
        });
        caller_block.set_terminator(Terminator::Return {
            value: Some(result),
        });
        caller.push_block(caller_block);
        program.push_function(caller);

        let mut callee_fn = Function::new(callee, BasicBlockId(0));
        let mut params = vec!["$env".into(), "$this".into()];
        for index in 0..js_params {
            params.push(format!("$1.p{index}"));
        }
        callee_fn.set_params(params);
        callee_fn.set_direct_callable(true);
        let mut callee_block = BasicBlock::new(BasicBlockId(0));
        if js_params == 0 {
            callee_block.push_instruction(Instruction::Const {
                dest: ValueId(0),
                constant: number,
            });
            callee_block.set_terminator(Terminator::Return {
                value: Some(ValueId(0)),
            });
        } else {
            callee_block.push_instruction(Instruction::LoadVar {
                dest: ValueId(0),
                name: "$1.p0".into(),
            });
            callee_block.set_terminator(Terminator::Return {
                value: Some(ValueId(0)),
            });
        }
        callee_fn.push_block(callee_block);
        program.push_function(callee_fn);

        PortableArtifact::from_input(&ArtifactBuildInput {
            program: Arc::new(program),
            manifest: Arc::new(ModuleManifest::single("input.js", true)),
            options: BuildOptions::default(),
            source_text: None,
        })
        .expect("direct-call artifact should encode")
    }

    #[test]
    fn fast_direct_call_uses_register_signature() {
        let compiler = NativeCompiler::new().expect("host ISA should be supported");
        let diagnostics = compiler
            .diagnostics(&direct_call_artifact(1, 1, "add"))
            .expect("fast direct call should compile");
        let body = clif_section(&diagnostics.clif, ";; function 1: add");
        let sig_line = body
            .lines()
            .find(|line| line.contains("function u0:"))
            .unwrap_or(body);
        assert!(
            sig_line.contains("(i64, i64, i64, i64) -> i64"),
            "fast body should take ctx/env/this/arg0 as i64:\n{body}"
        );
        assert!(
            !sig_line.contains("i32"),
            "fast body signature must not use slow i32 arena indices:\n{sig_line}"
        );
        let trampoline = clif_section(&diagnostics.clif, ";; trampoline 1: add");
        assert!(
            trampoline.contains("i32, i32"),
            "slow trampoline should keep NativeSlowEntry:\n{trampoline}"
        );
        let parsed = object::File::parse(diagnostics.object.bytes()).expect("object should parse");
        assert!(parsed.symbol_by_name("wjsm_function_1").is_some());
    }

    #[test]
    fn wide_direct_call_keeps_call_arena_signature() {
        let compiler = NativeCompiler::new().expect("host ISA should be supported");
        let diagnostics = compiler
            .diagnostics(&direct_call_artifact(5, 5, "wide"))
            .expect("wide direct call should compile");
        let body = clif_section(&diagnostics.clif, ";; function 1: wide");
        assert!(
            body.contains("i32, i32"),
            "arity>4 must stay on NativeSlowEntry:\n{body}"
        );
        assert!(
            !diagnostics.clif.contains(";; trampoline 1: wide"),
            "arity>4 must not emit a fast trampoline:\n{}",
            diagnostics.clif
        );
    }
}
