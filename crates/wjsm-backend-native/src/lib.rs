pub mod cache;
pub mod image;
mod lower;
mod root_plan;
mod unwind;

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
static CACHED_COMPILER: LazyLock<Result<NativeCompiler, NativeCompileError>> =
    LazyLock::new(|| {
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
    });


impl NativeCompiler {
    /// 返回全局缓存的 compiler 的 clone（isa 内部是 Arc，clone 成本低）。
    pub fn new() -> Result<Self, NativeCompileError> {
        CACHED_COMPILER.clone()
    }

    pub fn settings_key(&self) -> &str {
        &self.settings_key
    }

    pub fn compile(&self, artifact: &PortableArtifact) -> Result<NativeObject, NativeCompileError> {
        lower::compile_program(Arc::clone(&self.isa), artifact.program())
    }

    pub fn diagnostics(
        &self,
        artifact: &PortableArtifact,
    ) -> Result<NativeCompilationDiagnostics, NativeCompileError> {
        lower::compile_program_diagnostics(Arc::clone(&self.isa), artifact.program())
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
            compiler.settings_key().contains(&format!("opt={opt_level}")),
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
    use std::sync::Arc;

    use object::{Object as _, ObjectSection as _};
    use wjsm_artifact_format::{
        ArtifactBuildInput, BuildOptions, ModuleManifest, PortableArtifact,
    };
    use wjsm_ir::{
        BasicBlock, BasicBlockId, BinaryOp, Constant, Function, Instruction, Program, Terminator,
        ValueId,
    };

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
    fn diagnostics_report_clif_and_machine_disassembly() {
        let compiler = NativeCompiler::new().expect("host ISA should be supported");
        let diagnostics = compiler
            .diagnostics(&arithmetic_artifact())
            .expect("arithmetic diagnostics should compile");
        assert!(diagnostics.clif.contains("function"));
        assert!(diagnostics.disassembly.contains("function 0: main"));
        assert!(!diagnostics.disassembly.trim().is_empty());
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
        let cache_dir =
            std::env::temp_dir().join(format!("wjsm_native_cache_test_{}", std::process::id()));
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
        let _ = std::fs::remove_dir_all(cache_dir);
    }
}
