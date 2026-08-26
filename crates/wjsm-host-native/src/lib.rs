use std::cell::RefCell;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};
use std::marker::PhantomData;
use std::path::PathBuf;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use std::io::Write;
use thiserror::Error;
use wjsm_artifact_format::{
    ArtifactBuildInput, ArtifactLimits, BuildOptions, ModuleManifest, PortableArtifact,
};
use wjsm_backend_native::cache::{NativeCacheError, NativeImageRepository};
use wjsm_backend_native::image::CompiledImage;
use wjsm_backend_native::{NativeCompiler, NativeSymbolResolver};
use wjsm_gc::backoff::Backoff;
use wjsm_gc::heap_access::{object_payload_bytes, string_payload_bytes};
use wjsm_gc::{GcAlgorithmKind, HeapAccessV2Error, PROTO_NULL_SENTINEL, PropertyKey, StrView};
use wjsm_host::{RuntimeString, content_hash_latin1, content_hash_units};
use wjsm_ir::{Constant, Instruction, is_module_entry_ir_function, value};
use wjsm_native_abi::{
    MAX_NATIVE_ROOT_BITMAP_WORDS, NativeFeedbackSlot, NativeFeedbackTag, NativeHostSymbol,
    NativeRuntimeOp, NativeSlowEntry, NativeVmContext, PendingExceptionKind,
    encode_feedback_tag_signature, native_variable_names, native_variable_slots_for_segments,
};
mod dispatch;
mod gc;
mod inspector;
mod native_exec;
mod side_tables;
mod snapshot;
mod specialization;

pub use gc::NativeAllocationDiagnostics;
pub use inspector::InspectorConfig;
pub use native_exec::{
    PrecompiledNativeImages, compile_native_exec_images, compile_snapshot_entry,
    exec_payload_from_images, images_from_exec_payload,
};
pub use wjsm_module::ModuleSourceStore;

use dispatch::{
    native_host_operation, native_math_acos, native_math_acosh, native_math_asin,
    native_math_asinh, native_math_atan, native_math_atan2, native_math_atanh, native_math_cbrt,
    native_math_cos, native_math_cosh, native_math_exp, native_math_expm1, native_math_log,
    native_math_log1p, native_math_log2, native_math_log10, native_math_pow, native_math_sin,
    native_math_sinh, native_math_tan, native_math_tanh, native_string_add,
    native_string_builder_append, native_string_builder_append_number,
    native_string_builder_finish, native_zgc_load_barrier_assist, native_zgc_store_barrier,
};
use specialization::{
    CompilationRequest, SpecializationCoordinator, ValidatedFeedbackSlot, VariantKey,
};

const DEFAULT_CALL_ARENA_SLOTS: usize = 64 * 1024;
const FIRST_USER_SYMBOL_HANDLE: u32 = wjsm_ir::wk_symbol::UNSCOPABLES + 1;
const LATIN1_CHAR_COUNT: usize = 256;
const DEFAULT_MAX_HEAP_BYTES: u64 = 64 * 1024 * 1024;
const OUT_OF_MEMORY_MESSAGE: &str = "JavaScript heap out of memory";
const MAX_JS_CALL_DEPTH: u32 = 1024;
pub(crate) const ASSIGNED_PROPERTY_FLAGS: u32 = wjsm_ir::constants::FLAG_ENUMERABLE as u32
    | wjsm_ir::constants::FLAG_CONFIGURABLE as u32
    | wjsm_ir::constants::FLAG_WRITABLE as u32;
pub(crate) const BUILTIN_PROTOTYPE_PROPERTY_FLAGS: u32 =
    wjsm_ir::constants::FLAG_CONFIGURABLE as u32 | wjsm_ir::constants::FLAG_WRITABLE as u32;
pub(crate) const FUNCTION_PROTOTYPE_FLAGS: u32 = wjsm_ir::constants::FLAG_WRITABLE as u32;
pub(crate) const FUNCTION_METADATA_FLAGS: u32 = wjsm_ir::constants::FLAG_CONFIGURABLE as u32;

fn is_module_scope_var(name: &str) -> bool {
    let Some(rest) = name.strip_prefix('$') else {
        return false;
    };
    let Some((scope, _)) = rest.split_once('.') else {
        return false;
    };
    scope.bytes().all(|b| b.is_ascii_digit()) && scope != "0"
}

fn slot_table_len(slots: &HashMap<String, u32>) -> usize {
    slots.values().copied().max().map_or(0, |slot| {
        usize::try_from(slot).expect("槽号在 usize 内") + 1
    })
}

fn function_slots_for_program(
    program: &wjsm_ir::Program,
    variable_slots: &HashMap<String, u32>,
    shared_module_slots: &HashSet<&str>,
) -> Vec<Vec<usize>> {
    let frame_locals = program.frame_local_variable_names_by_function();
    program
        .functions()
        .iter()
        .zip(&frame_locals)
        .map(|(function, frame_locals)| {
            let mut slots = Vec::new();
            let captured_names = function
                .captured_names()
                .iter()
                .map(String::as_str)
                .collect::<HashSet<_>>();
            let uses_canonical_this = function.blocks().iter().any(|block| {
                block.instructions().iter().any(|instruction| {
                    matches!(
                        instruction,
                        Instruction::LoadVar { name, .. } | Instruction::StoreVar { name, .. }
                            if name == "$this"
                    )
                })
            });
            for (index, name) in function.params().iter().enumerate() {
                let storage_name = if index == 0
                    && (function.name().ends_with("$async")
                        || function.name().ends_with("$asyncgen")
                        || name == wjsm_ir::EVAL_SCOPE_ENV_PARAM)
                {
                    name.as_str()
                } else if index == 0 {
                    "$env"
                } else if index == 1
                    && uses_canonical_this
                    && !function.name().ends_with("$async")
                    && !function.name().ends_with("$asyncgen")
                {
                    "$this"
                } else {
                    name.as_str()
                };
                if frame_locals.contains(storage_name) {
                    continue;
                }
                if let Some(slot) = variable_slots.get(storage_name).copied() {
                    slots.push(usize::try_from(slot).expect("槽号在 usize 内"));
                }
            }
            for block in function.blocks() {
                for instruction in block.instructions() {
                    let name = match instruction {
                        Instruction::LoadVar { name, .. } | Instruction::StoreVar { name, .. } => {
                            name
                        }
                        _ => continue,
                    };
                    if frame_locals.contains(name.as_str()) {
                        continue;
                    }
                    if shared_module_slots.contains(name.as_str()) {
                        continue;
                    }
                    if !name.starts_with("$0.")
                        && !captured_names.contains(name.as_str())
                        && let Some(slot) = variable_slots.get(name.as_str()).copied()
                    {
                        slots.push(usize::try_from(slot).expect("槽号在 usize 内"));
                    }
                }
            }
            slots.sort_unstable();
            slots.dedup();
            slots
        })
        .collect()
}

/// install 期为每个 `ObjectTemplate` 常量烘焙 shape transition，供 InitObjectLiteral JIT 直读。
fn bake_object_template_meta_table(
    shapes: &wjsm_gc::ShapeTable,
    constants: &[Constant],
    string_constants: &[i64],
) -> Vec<u32> {
    use wjsm_gc::{PropertyKey, ShapeTable};
    use wjsm_ir::constants::{
        FLAG_CONFIGURABLE, FLAG_ENUMERABLE, FLAG_WRITABLE, OBJECT_TEMPLATE_MAX_PROPS,
        OBJECT_TEMPLATE_META_WORDS,
    };

    let flags = (FLAG_CONFIGURABLE | FLAG_ENUMERABLE | FLAG_WRITABLE) as u32;
    let mut meta = Vec::new();
    for constant in constants {
        let Constant::ObjectTemplate { keys } = constant else {
            continue;
        };
        let prop_count = keys.len().min(OBJECT_TEMPLATE_MAX_PROPS as usize);
        let mut shape_id = ShapeTable::empty_shape();
        let mut slot_count = 0_u32;
        let mut entry = vec![0_u32; OBJECT_TEMPLATE_META_WORDS as usize];
        for (index, key_raw) in keys.iter().take(prop_count).enumerate() {
            let key = if let Some(constant_idx) = value::template_key_name_ref(*key_raw) {
                let constant_idx = constant_idx as usize;
                debug_assert!(constant_idx < string_constants.len());
                let encoded = string_constants[constant_idx];
                if value::is_inline_string(encoded) {
                    PropertyKey::inline_string(encoded).expect("install 期 SSO 字符串常量")
                } else {
                    PropertyKey::from_name_id(value::decode_runtime_string_handle(encoded))
                }
            } else {
                PropertyKey::from_baked_raw(*key_raw)
            };
            let transition = shapes.transition_add(shape_id, key, flags);
            entry[4 + index] = transition.index;
            shape_id = transition.shape_id;
            slot_count = transition.slot_count;
        }
        entry[0] = shape_id;
        entry[1] = slot_count;
        entry[2] = std::cmp::max(4, prop_count as u32);
        entry[3] = prop_count as u32;
        meta.extend(entry);
    }
    meta
}

pub(crate) fn whole_program_slots(program: &wjsm_ir::Program) -> HashMap<String, u32> {
    native_variable_names(program)
        .into_iter()
        .enumerate()
        .map(|(index, name)| (name, u32::try_from(index).expect("整包变量槽数在 u32 内")))
        .collect()
}

fn native_function_metadata(kind: NativeCallableKind) -> Option<(&'static str, u32)> {
    match kind {
        NativeCallableKind::Builtin(wjsm_ir::Builtin::ArraySlice, _) => Some(("slice", 2)),
        NativeCallableKind::Builtin(wjsm_ir::Builtin::FuncCall, _) => Some(("call", 1)),
        NativeCallableKind::Builtin(wjsm_ir::Builtin::PropertyIsEnumerable, _) => {
            Some(("propertyIsEnumerable", 1))
        }
        NativeCallableKind::Builtin(wjsm_ir::Builtin::ErrorConstructor, _) => Some(("Error", 1)),
        NativeCallableKind::Builtin(wjsm_ir::Builtin::EvalErrorConstructor, _) => {
            Some(("EvalError", 1))
        }
        NativeCallableKind::Builtin(wjsm_ir::Builtin::RangeErrorConstructor, _) => {
            Some(("RangeError", 1))
        }
        NativeCallableKind::Builtin(wjsm_ir::Builtin::ReferenceErrorConstructor, _) => {
            Some(("ReferenceError", 1))
        }
        NativeCallableKind::Builtin(wjsm_ir::Builtin::SyntaxErrorConstructor, _) => {
            Some(("SyntaxError", 1))
        }
        NativeCallableKind::Builtin(wjsm_ir::Builtin::TypeErrorConstructor, _) => {
            Some(("TypeError", 1))
        }
        NativeCallableKind::Builtin(wjsm_ir::Builtin::URIErrorConstructor, _) => {
            Some(("URIError", 1))
        }
        NativeCallableKind::ArrayToString | NativeCallableKind::RegExpToString => {
            Some(("toString", 0))
        }
        NativeCallableKind::ErrorToString => Some(("toString", 0)),
        NativeCallableKind::AggregateErrorConstructor => Some(("AggregateError", 2)),
        NativeCallableKind::ObjectConstructor => Some(("Object", 1)),
        NativeCallableKind::RealmArrayConstructor(_) => Some(("Array", 1)),
        NativeCallableKind::FunctionPrototype => Some(("", 0)),
        NativeCallableKind::TimerConstructor(true) => Some(("Immediate", 0)),
        NativeCallableKind::TimerConstructor(false) => Some(("Timeout", 0)),
        NativeCallableKind::ProcessHrtime => Some(("hrtime", 1)),
        NativeCallableKind::ProcessHrtimeBigInt => Some(("bigint", 0)),
        NativeCallableKind::ProcessUptime => Some(("uptime", 0)),
        NativeCallableKind::ProcessMemoryUsage => Some(("memoryUsage", 0)),
        NativeCallableKind::ProcessCpuUsage => Some(("cpuUsage", 1)),
        NativeCallableKind::Intl(kind) => dispatch::intl::metadata(kind),
        NativeCallableKind::DateMethod(method) => dispatch::date::method_metadata(method),
        _ => None,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeExecution {
    pub value: i64,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: i32,
    pub cache_entries: u64,
    pub cache_bytes: u64,
    pub cache_hit_count: u64,
    pub cache_miss_count: u64,
    pub cache_invalidated_count: u64,
}

/// 进程输出是捕获到 `NativeExecution` 还是立即写 OS 流。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputMode {
    Capture,
    Inherit,
}

#[derive(Clone, Debug)]
pub struct NativeRuntimeConfig {
    pub cache_dir: Option<PathBuf>,
    pub gc_algorithm: GcAlgorithmKind,
    pub max_heap_size: u64,
    /// 运行时类型反馈与热函数特化开关；`WJSM_DISABLE_SPECIALIZATION=1` 时关闭，
    /// 用于同 binary 的 generic AOT 对照。关闭时不启动反馈 worker、不发布 overlay，
    /// generic lowering、IC 与全部语义路径保持不变。
    pub specialization_enabled: bool,
    /// 分配诊断计数器开关；`WJSM_PERF_DIAGNOSTICS=1` 时启用，默认关闭。
    pub allocation_diagnostics_enabled: bool,
    pub output_mode: OutputMode,
    /// 子 agent 不得复用父进程 `SHARED_IMAGE_STATE` 里已跑过的 image：
    /// IC / 反馈槽指向父堆对象，packed worker 里 `require` 会变成 undefined。
    pub isolate_native_images: bool,
}

impl Default for NativeRuntimeConfig {
    fn default() -> Self {
        Self {
            cache_dir: None,
            gc_algorithm: GcAlgorithmKind::Zgc,
            max_heap_size: DEFAULT_MAX_HEAP_BYTES,
            specialization_enabled: true,
            allocation_diagnostics_enabled: false,
            output_mode: OutputMode::Capture,
            isolate_native_images: false,
        }
    }
}

impl NativeRuntimeConfig {
    pub fn from_environment(cache_dir: Option<PathBuf>) -> Result<Self, NativeRuntimeError> {
        let gc_algorithm = std::env::var("WJSM_TEST_GC")
            .or_else(|_| std::env::var("WJSM_GC"))
            .ok()
            .map(|name| name.parse::<GcAlgorithmKind>())
            .transpose()
            .map_err(NativeRuntimeError::Configuration)?
            .unwrap_or(GcAlgorithmKind::Zgc);
        let specialization_enabled =
            std::env::var("WJSM_DISABLE_SPECIALIZATION").ok().as_deref() != Some("1");
        let allocation_diagnostics_enabled =
            std::env::var("WJSM_PERF_DIAGNOSTICS").ok().as_deref() == Some("1");
        Ok(Self {
            cache_dir,
            gc_algorithm,
            max_heap_size: DEFAULT_MAX_HEAP_BYTES,
            specialization_enabled,
            allocation_diagnostics_enabled,
            output_mode: OutputMode::Capture,
            isolate_native_images: false,
        })
    }

    pub fn with_gc_algorithm(mut self, gc_algorithm: GcAlgorithmKind) -> Self {
        self.gc_algorithm = gc_algorithm;
        self
    }

    pub fn with_max_heap_size(mut self, max_heap_size: u64) -> Self {
        self.max_heap_size = max_heap_size;
        self
    }

    pub fn with_specialization_enabled(mut self, enabled: bool) -> Self {
        self.specialization_enabled = enabled;
        self
    }

    pub fn with_allocation_diagnostics_enabled(mut self, enabled: bool) -> Self {
        self.allocation_diagnostics_enabled = enabled;
        self
    }

    pub fn with_output_mode(mut self, output_mode: OutputMode) -> Self {
        self.output_mode = output_mode;
        self
    }

    pub(crate) fn child_config(&self) -> Self {
        Self {
            cache_dir: self.cache_dir.clone(),
            gc_algorithm: self.gc_algorithm,
            max_heap_size: self.max_heap_size,
            specialization_enabled: self.specialization_enabled,
            allocation_diagnostics_enabled: self.allocation_diagnostics_enabled,
            output_mode: self.output_mode,
            isolate_native_images: true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SourceCompileOptions {
    pub filename: Option<String>,
    pub script: bool,
    pub verify_ir: bool,
    pub include_source_map: bool,
}

impl Default for SourceCompileOptions {
    fn default() -> Self {
        Self {
            filename: None,
            script: false,
            verify_ir: true,
            include_source_map: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct RuntimeOptions {
    pub cache_dir: Option<PathBuf>,
    pub module_root: PathBuf,
    pub working_directory: PathBuf,
    pub env: Vec<(String, String)>,
    pub inherit_env: bool,
    pub gc_algorithm: GcAlgorithmKind,
    pub max_heap_size: u64,
    pub compile: SourceCompileOptions,
}

impl Default for RuntimeOptions {
    fn default() -> Self {
        Self {
            cache_dir: None,
            module_root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            working_directory: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            env: Vec::new(),
            inherit_env: true,
            gc_algorithm: GcAlgorithmKind::Zgc,
            max_heap_size: DEFAULT_MAX_HEAP_BYTES,
            compile: SourceCompileOptions::default(),
        }
    }
}

impl RuntimeOptions {
    pub fn with_gc_algorithm(mut self, gc_algorithm: GcAlgorithmKind) -> Self {
        self.gc_algorithm = gc_algorithm;
        self
    }

    pub fn with_max_heap_size(mut self, max_heap_size: u64) -> Self {
        self.max_heap_size = max_heap_size;
        self
    }
}

#[derive(Clone, Copy, Debug)]
pub enum RuntimeInput<'a> {
    Source(&'a str),
    Artifact(&'a [u8]),
}

pub fn compile_source(source: &str) -> Result<Vec<u8>, NativeRuntimeError> {
    compile_source_with_options(source, &SourceCompileOptions::default())
}

pub fn compile_source_with_options(
    source: &str,
    options: &SourceCompileOptions,
) -> Result<Vec<u8>, NativeRuntimeError> {
    let module = if options.script {
        wjsm_parser::parse_script_as_module(source)
    } else if let Some(filename) = &options.filename {
        wjsm_parser::parse_module_with_filename(source, filename)
    } else {
        wjsm_parser::parse_module(source)
    }
    .map_err(|error| NativeRuntimeError::SourceCompile(error.to_string()))?;
    let logical_url = options.filename.as_deref().unwrap_or(if options.script {
        "input.js"
    } else {
        "input.ts"
    });
    let program = wjsm_semantic::lower_module_with_debug_source(
        module,
        options.script,
        Some(Arc::from(source)),
        logical_url.to_string(),
        options.include_source_map,
    )
    .map_err(|error| NativeRuntimeError::SourceCompile(error.to_string()))?;
    if options.verify_ir {
        program
            .verify()
            .map_err(|error| NativeRuntimeError::SourceCompile(error.to_string()))?;
    }
    let artifact = PortableArtifact::from_input(&ArtifactBuildInput {
        program: Arc::new(program),
        manifest: Arc::new(ModuleManifest::single(logical_url, options.script)),
        options: BuildOptions {
            include_source_map: options.include_source_map,
            include_source_text: options.include_source_map,
        },
        source_text: options.include_source_map.then(|| Arc::from(source)),
    })
    .map_err(|error| NativeRuntimeError::SourceCompile(error.to_string()))?;
    Ok(artifact.bytes().to_vec())
}

pub fn execute_with_writer_with_options(
    input: RuntimeInput<'_>,
    mut writer: impl Write,
    options: RuntimeOptions,
) -> Result<NativeExecution, NativeRuntimeError> {
    let artifact = match input {
        RuntimeInput::Source(source) => {
            let bytes = compile_source_with_options(source, &options.compile)?;
            PortableArtifact::decode(Arc::from(bytes), &ArtifactLimits::default())
                .map_err(|error| NativeRuntimeError::Artifact(error.to_string()))?
        }
        RuntimeInput::Artifact(bytes) => {
            PortableArtifact::decode(Arc::from(bytes), &ArtifactLimits::default())
                .map_err(|error| NativeRuntimeError::Artifact(error.to_string()))?
        }
    };
    let environment_config = NativeRuntimeConfig::from_environment(None)?;
    let mut runtime = NativeRuntime::new_with_config(NativeRuntimeConfig {
        cache_dir: options.cache_dir,
        gc_algorithm: options.gc_algorithm,
        max_heap_size: options.max_heap_size,
        specialization_enabled: environment_config.specialization_enabled,
        allocation_diagnostics_enabled: environment_config.allocation_diagnostics_enabled,
        output_mode: OutputMode::Capture,
        isolate_native_images: false,
    })?;
    runtime.configure_environment(options.inherit_env, options.env)?;
    let execution = runtime.execute(&artifact, &options.module_root, &options.working_directory)?;
    writer.write_all(&execution.stdout)?;
    Ok(execution)
}

pub struct NativeHostRegistry;

impl NativeSymbolResolver for NativeHostRegistry {
    fn resolve(&self, symbol: NativeHostSymbol) -> Option<usize> {
        let pointer = match symbol {
            NativeHostSymbol::HostOperationDispatcher => native_host_operation as *const (),
            NativeHostSymbol::MathAcos => native_math_acos as *const (),
            NativeHostSymbol::MathAcosh => native_math_acosh as *const (),
            NativeHostSymbol::MathAsin => native_math_asin as *const (),
            NativeHostSymbol::MathAsinh => native_math_asinh as *const (),
            NativeHostSymbol::MathAtan => native_math_atan as *const (),
            NativeHostSymbol::MathAtanh => native_math_atanh as *const (),
            NativeHostSymbol::MathAtan2 => native_math_atan2 as *const (),
            NativeHostSymbol::MathCbrt => native_math_cbrt as *const (),
            NativeHostSymbol::MathCos => native_math_cos as *const (),
            NativeHostSymbol::MathCosh => native_math_cosh as *const (),
            NativeHostSymbol::MathExp => native_math_exp as *const (),
            NativeHostSymbol::MathExpm1 => native_math_expm1 as *const (),
            NativeHostSymbol::MathLog => native_math_log as *const (),
            NativeHostSymbol::MathLog1p => native_math_log1p as *const (),
            NativeHostSymbol::MathLog10 => native_math_log10 as *const (),
            NativeHostSymbol::MathLog2 => native_math_log2 as *const (),
            NativeHostSymbol::MathSin => native_math_sin as *const (),
            NativeHostSymbol::MathSinh => native_math_sinh as *const (),
            NativeHostSymbol::MathTan => native_math_tan as *const (),
            NativeHostSymbol::MathTanh => native_math_tanh as *const (),
            NativeHostSymbol::MathPow => native_math_pow as *const (),
            NativeHostSymbol::ZgcLoadBarrierAssist => native_zgc_load_barrier_assist as *const (),
            NativeHostSymbol::ZgcStoreBarrier => native_zgc_store_barrier as *const (),
            NativeHostSymbol::StringAdd => native_string_add as *const (),
            NativeHostSymbol::StringBuilderAppend => native_string_builder_append as *const (),
            NativeHostSymbol::StringBuilderAppendNumber => {
                native_string_builder_append_number as *const ()
            }
            NativeHostSymbol::StringBuilderFinish => native_string_builder_finish as *const (),
        };
        Some((pointer).addr())
    }
}

#[derive(Clone, Copy)]
struct NativeClosure {
    function_id: u32,
    environment: i64,
}

#[derive(Clone)]
struct NativeBoundFunction {
    target: i64,
    this_value: i64,
    arguments: Vec<i64>,
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum NativeIteratorKind {
    Keys,
    Values,
    Entries,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum NativeCallableKind {
    Builtin(wjsm_ir::Builtin, bool),
    Bound(u32),
    Fetch(dispatch::fetch::FetchCallable),
    DateMethod(dispatch::date::DateMethodKind),
    ObjectConstructor,
    ArrayConstructor,
    RealmArrayConstructor(u32),
    ArrayToString,
    ArrayIterator(NativeIteratorKind),
    ArgumentsStrictCallee,
    BufferConstructor,
    ErrorToString,
    AggregateErrorConstructor,
    BufferMethod(dispatch::node_buffer::BufferMethodKind),
    BufferStatic(dispatch::node_buffer::BufferStaticKind),
    BufferTranscode,
    CjsRequire(u32),
    CjsResolve(u32),
    NodeNet(dispatch::node_net::NodeNetMethod),
    NodeTls(dispatch::node_tls::NodeTlsMethod),
    NodeZlib(dispatch::node_zlib::NodeZlibMethod),
    NodeFs(dispatch::node_fs::NodeFsMethod),
    NodeCrypto(dispatch::node_crypto::NodeCryptoCallable),
    NodeDgram(dispatch::node_dgram::NodeDgramMethod),
    NodeAsyncHooks(dispatch::node_async_hooks::NodeAsyncHooksCallable),
    NodeOs(dispatch::node_os::NodeOsMethod),
    Idna(dispatch::idna::IdnaMethod),
    NodeVm(dispatch::node_vm::NodeVmCallable),
    NodeChildProcess(dispatch::node_child_process::NodeChildProcessCallable),
    NodePerfHooks(dispatch::node_perf_hooks::NodePerfHooksCallable),
    NodeWorkerThreads(dispatch::node_worker_threads::WorkerThreadsMethod),
    Test262Agent(dispatch::agent::Test262Method),
    CjsResolvePaths(u32),
    ImportMetaResolve(u32),
    CollectionNext(u32),
    IteratorNext(u32),
    PromiseResolve(u32),
    PromiseReject(u32),
    ProxyRevoke(u32),
    ProxyCall(u32),
    ProcessExit,
    ProcessWrite(bool),
    ProcessStreamEnd(bool),
    ProcessStreamReturnThis,
    ProcessHrtime,
    ProcessHrtimeBigInt,
    ProcessUptime,
    ProcessMemoryUsage,
    ProcessCpuUsage,
    StringConstructor,
    FunctionConstructor,
    FunctionPrototype,
    ProxyConstruct(u32),
    RegExpToString,
    RegExpIteratorNext(u32),
    ProcessCwd,
    ProcessOn,
    SetImmediate,
    TimerConstructor(bool),
    Gc,
    ProcessNextTick,
    Stream(dispatch::streams::StreamCallable),
    WebEncoding(dispatch::web_encoding::WebEncodingCallable),
    Intl(dispatch::intl::IntlCallable),
}

fn intrinsic_builtin(receiver: i64, key: &str) -> Option<wjsm_ir::Builtin> {
    use wjsm_ir::Builtin;

    let builtin = if value::is_bool(receiver) {
        match key {
            "toString" => Builtin::BooleanProtoToString,
            "valueOf" => Builtin::BooleanProtoValueOf,
            _ => return None,
        }
    } else if value::is_f64(receiver) {
        match key {
            "toExponential" => Builtin::NumberProtoToExponential,
            "toFixed" => Builtin::NumberProtoToFixed,
            "toPrecision" => Builtin::NumberProtoToPrecision,
            "toString" => Builtin::NumberProtoToString,
            "valueOf" => Builtin::NumberProtoValueOf,
            _ => return None,
        }
    } else if value::is_bigint(receiver) {
        match key {
            "toString" => Builtin::BigIntProtoToString,
            "valueOf" => Builtin::BigIntProtoValueOf,
            _ => return None,
        }
    } else if value::is_string(receiver) {
        match key {
            "at" => Builtin::StringAt,
            "charAt" => Builtin::StringCharAt,
            "charCodeAt" => Builtin::StringCharCodeAt,
            "codePointAt" => Builtin::StringCodePointAt,
            "concat" => Builtin::StringConcatVa,
            "endsWith" => Builtin::StringEndsWith,
            "includes" => Builtin::StringIncludes,
            "indexOf" => Builtin::StringIndexOf,
            "lastIndexOf" => Builtin::StringLastIndexOf,
            "matchAll" => Builtin::StringMatchAll,
            "padEnd" => Builtin::StringPadEnd,
            "padStart" => Builtin::StringPadStart,
            "repeat" => Builtin::StringRepeat,
            "replace" => Builtin::StringReplace,
            "replaceAll" => Builtin::StringReplaceAll,
            "search" => Builtin::StringSearch,
            "slice" => Builtin::StringSlice,
            "split" => Builtin::StringSplit,
            "startsWith" => Builtin::StringStartsWith,
            "substring" => Builtin::StringSubstring,
            "toString" => Builtin::StringToString,
            "trim" => Builtin::StringTrim,
            "trimEnd" => Builtin::StringTrimEnd,
            "trimStart" => Builtin::StringTrimStart,
            "valueOf" => Builtin::StringValueOf,
            _ => return None,
        }
    } else if value::is_array(receiver) {
        match key {
            "at" => Builtin::ArrayAt,
            "concat" => Builtin::ArrayConcatVa,
            "copyWithin" => Builtin::ArrayCopyWithin,
            "fill" => Builtin::ArrayFill,
            "flat" => Builtin::ArrayFlat,
            "every" => Builtin::ArrayEvery,
            "filter" => Builtin::ArrayFilter,
            "find" => Builtin::ArrayFind,
            "findIndex" => Builtin::ArrayFindIndex,
            "findLast" => Builtin::ArrayFindLast,
            "findLastIndex" => Builtin::ArrayFindLastIndex,
            "flatMap" => Builtin::ArrayFlatMap,
            "forEach" => Builtin::ArrayForEach,
            "includes" => Builtin::ArrayIncludes,
            "indexOf" => Builtin::ArrayIndexOf,
            "join" => Builtin::ArrayJoin,
            "map" => Builtin::ArrayMap,
            "lastIndexOf" => Builtin::ArrayLastIndexOf,
            "pop" => Builtin::ArrayPop,
            "push" => Builtin::ArrayPush,
            "reverse" => Builtin::ArrayReverse,
            "reduce" => Builtin::ArrayReduce,
            "reduceRight" => Builtin::ArrayReduceRight,
            "shift" => Builtin::ArrayShift,
            "slice" => Builtin::ArraySlice,
            "splice" => Builtin::ArraySpliceVa,
            "some" => Builtin::ArraySome,
            "sort" => Builtin::ArraySort,
            "values" => Builtin::IteratorFrom,
            "toReversed" => Builtin::ArrayToReversed,
            "toSpliced" => Builtin::ArrayToSplicedVa,
            "toSorted" => Builtin::ArrayToSorted,
            "unshift" => Builtin::ArrayUnshiftVa,
            "with" => Builtin::ArrayWith,
            "hasOwnProperty" => Builtin::HasOwnProperty,
            "propertyIsEnumerable" => Builtin::PropertyIsEnumerable,
            "toString" => Builtin::ObjectProtoToString,
            "valueOf" => Builtin::ObjectProtoValueOf,
            _ => return None,
        }
    } else if value::is_regexp(receiver) {
        match key {
            "exec" => Builtin::RegExpExec,
            "test" => Builtin::RegExpTest,
            "toString" => Builtin::ObjectProtoToString,
            _ => return None,
        }
    } else if value::is_callable(receiver) {
        match key {
            "bind" => Builtin::FuncBind,
            "apply" => Builtin::FuncApply,
            "call" => Builtin::FuncCall,
            "hasOwnProperty" => Builtin::HasOwnProperty,
            "propertyIsEnumerable" => Builtin::PropertyIsEnumerable,
            "toString" => Builtin::ObjectProtoToString,
            "valueOf" => Builtin::ObjectProtoValueOf,
            _ => return None,
        }
    } else if value::is_js_object(receiver) {
        match key {
            "hasOwnProperty" => Builtin::HasOwnProperty,
            "propertyIsEnumerable" => Builtin::PropertyIsEnumerable,
            "toString" => Builtin::ObjectProtoToString,
            "valueOf" => Builtin::ObjectProtoValueOf,
            _ => return None,
        }
    } else {
        return None;
    };
    Some(builtin)
}

fn reflect_builtin(name: &str) -> Option<wjsm_ir::Builtin> {
    Some(match name {
        "apply" => wjsm_ir::Builtin::ReflectApply,
        "construct" => wjsm_ir::Builtin::ReflectConstruct,
        "defineProperty" => wjsm_ir::Builtin::ReflectDefineProperty,
        "deleteProperty" => wjsm_ir::Builtin::ReflectDeleteProperty,
        "get" => wjsm_ir::Builtin::ReflectGet,
        "getOwnPropertyDescriptor" => wjsm_ir::Builtin::ReflectGetOwnPropertyDescriptor,
        "getPrototypeOf" => wjsm_ir::Builtin::ReflectGetPrototypeOf,
        "has" => wjsm_ir::Builtin::ReflectHas,
        "isExtensible" => wjsm_ir::Builtin::ReflectIsExtensible,
        "ownKeys" => wjsm_ir::Builtin::ReflectOwnKeys,
        "preventExtensions" => wjsm_ir::Builtin::ReflectPreventExtensions,
        "set" => wjsm_ir::Builtin::ReflectSet,
        "setPrototypeOf" => wjsm_ir::Builtin::ReflectSetPrototypeOf,
        _ => return None,
    })
}

fn static_builtin(owner: wjsm_ir::Builtin, key: &str) -> Option<wjsm_ir::Builtin> {
    use wjsm_ir::Builtin;

    Some(match (owner, key) {
        (Builtin::ObjectKeys, "keys") => Builtin::ObjectKeys,
        (Builtin::ObjectKeys, "values") => Builtin::ObjectValues,
        (Builtin::ObjectKeys, "entries") => Builtin::ObjectEntries,
        (Builtin::ObjectKeys, "assign") => Builtin::ObjectAssign,
        (Builtin::ObjectKeys, "create") => Builtin::ObjectCreate,
        (Builtin::ObjectKeys, "defineProperty") => Builtin::DefineProperty,
        (Builtin::ObjectKeys, "defineProperties") => Builtin::ObjectDefineProperties,
        (Builtin::ObjectKeys, "getOwnPropertyDescriptor") => Builtin::GetOwnPropDesc,
        (Builtin::ObjectKeys, "getOwnPropertyDescriptors") => {
            Builtin::ObjectGetOwnPropertyDescriptors
        }
        (Builtin::ObjectKeys, "getOwnPropertyNames") => Builtin::ObjectGetOwnPropertyNames,
        (Builtin::ObjectKeys, "getOwnPropertySymbols") => Builtin::ObjectGetOwnPropertySymbols,
        (Builtin::ObjectKeys, "getPrototypeOf") => Builtin::ObjectGetPrototypeOf,
        (Builtin::ObjectKeys, "setPrototypeOf") => Builtin::ObjectSetPrototypeOf,
        (Builtin::ObjectKeys, "hasOwn") => Builtin::ObjectHasOwn,
        (Builtin::ObjectKeys, "freeze") => Builtin::ObjectFreeze,
        (Builtin::ObjectKeys, "seal") => Builtin::ObjectSeal,
        (Builtin::ObjectKeys, "isFrozen") => Builtin::ObjectIsFrozen,
        (Builtin::ObjectKeys, "isSealed") => Builtin::ObjectIsSealed,
        (Builtin::ObjectKeys, "isExtensible") => Builtin::ObjectIsExtensible,
        (Builtin::ObjectKeys, "preventExtensions") => Builtin::ObjectPreventExtensions,
        (Builtin::ObjectKeys, "is") => Builtin::ObjectIs,
        (Builtin::ObjectKeys, "fromEntries") => Builtin::ObjectFromEntries,
        (Builtin::ObjectKeys, "groupBy") => Builtin::ObjectGroupBy,
        (Builtin::PromiseCreate, "resolve") => Builtin::PromiseResolveStatic,
        (Builtin::PromiseCreate, "reject") => Builtin::PromiseRejectStatic,
        (Builtin::PromiseCreate, "all") => Builtin::PromiseAll,
        (Builtin::PromiseCreate, "race") => Builtin::PromiseRace,
        (Builtin::PromiseCreate, "allSettled") => Builtin::PromiseAllSettled,
        (Builtin::PromiseCreate, "any") => Builtin::PromiseAny,
        (Builtin::PromiseCreate, "withResolvers") => Builtin::PromiseWithResolvers,
        (Builtin::ArrayIsArray, "isArray") => Builtin::ArrayIsArray,
        (Builtin::ArrayIsArray, "from") => Builtin::ArrayFrom,
        (Builtin::ArrayIsArray, "of") => Builtin::ArrayOf,
        (Builtin::StringFromCharCode, "fromCharCode") => Builtin::StringFromCharCode,
        (Builtin::StringFromCharCode, "fromCodePoint") => Builtin::StringFromCodePoint,
        (Builtin::NumberConstructor, "isNaN") => Builtin::NumberIsNaN,
        (Builtin::NumberConstructor, "isFinite") => Builtin::NumberIsFinite,
        (Builtin::NumberConstructor, "isInteger") => Builtin::NumberIsInteger,
        (Builtin::NumberConstructor, "isSafeInteger") => Builtin::NumberIsSafeInteger,
        (Builtin::NumberConstructor, "parseInt") => Builtin::NumberParseInt,
        (Builtin::NumberConstructor, "parseFloat") => Builtin::NumberParseFloat,
        (Builtin::SymbolCreate, "for") => Builtin::SymbolFor,
        (Builtin::SymbolCreate, "keyFor") => Builtin::SymbolKeyFor,
        (Builtin::ProxyCreate, "revocable") => Builtin::ProxyRevocable,
        (Builtin::ReflectGet, name) => reflect_builtin(name)?,
        (Builtin::JsonStringify, "stringify") => Builtin::JsonStringify,
        (Builtin::JsonStringify, "parse") => Builtin::JsonParse,
        (Builtin::DateConstructor, "now") => Builtin::DateNow,
        (Builtin::DateConstructor, "parse") => Builtin::DateParse,
        (Builtin::DateConstructor, "UTC") => Builtin::DateUTC,
        (Builtin::MapConstructor, "groupBy") => Builtin::MapGroupBy,
        (Builtin::MathAbs, "abs") => Builtin::MathAbs,
        (Builtin::MathAbs, "acos") => Builtin::MathAcos,
        (Builtin::MathAbs, "acosh") => Builtin::MathAcosh,
        (Builtin::MathAbs, "asin") => Builtin::MathAsin,
        (Builtin::MathAbs, "asinh") => Builtin::MathAsinh,
        (Builtin::MathAbs, "atan") => Builtin::MathAtan,
        (Builtin::MathAbs, "atanh") => Builtin::MathAtanh,
        (Builtin::MathAbs, "atan2") => Builtin::MathAtan2,
        (Builtin::MathAbs, "cbrt") => Builtin::MathCbrt,
        (Builtin::MathAbs, "ceil") => Builtin::MathCeil,
        (Builtin::MathAbs, "clz32") => Builtin::MathClz32,
        (Builtin::MathAbs, "cos") => Builtin::MathCos,
        (Builtin::MathAbs, "cosh") => Builtin::MathCosh,
        (Builtin::MathAbs, "exp") => Builtin::MathExp,
        (Builtin::MathAbs, "expm1") => Builtin::MathExpm1,
        (Builtin::MathAbs, "floor") => Builtin::MathFloor,
        (Builtin::MathAbs, "fround") => Builtin::MathFround,
        (Builtin::MathAbs, "hypot") => Builtin::MathHypot,
        (Builtin::MathAbs, "imul") => Builtin::MathImul,
        (Builtin::MathAbs, "log") => Builtin::MathLog,
        (Builtin::MathAbs, "log1p") => Builtin::MathLog1p,
        (Builtin::MathAbs, "log10") => Builtin::MathLog10,
        (Builtin::MathAbs, "log2") => Builtin::MathLog2,
        (Builtin::MathAbs, "max") => Builtin::MathMax,
        (Builtin::MathAbs, "min") => Builtin::MathMin,
        (Builtin::MathAbs, "pow") => Builtin::MathPow,
        (Builtin::MathAbs, "random") => Builtin::MathRandom,
        (Builtin::MathAbs, "round") => Builtin::MathRound,
        (Builtin::MathAbs, "sign") => Builtin::MathSign,
        (Builtin::MathAbs, "sin") => Builtin::MathSin,
        (Builtin::MathAbs, "sinh") => Builtin::MathSinh,
        (Builtin::MathAbs, "sqrt") => Builtin::MathSqrt,
        (Builtin::MathAbs, "tan") => Builtin::MathTan,
        (Builtin::MathAbs, "tanh") => Builtin::MathTanh,
        (Builtin::MathAbs, "trunc") => Builtin::MathTrunc,
        _ => return None,
    })
}

fn well_known_symbol_property(key: &str) -> Option<i64> {
    use wjsm_ir::wk_symbol;

    let handle = match key {
        "iterator" => wk_symbol::ITERATOR,
        "species" => wk_symbol::SPECIES,
        "toStringTag" => wk_symbol::TO_STRING_TAG,
        "asyncIterator" => wk_symbol::ASYNC_ITERATOR,
        "hasInstance" => wk_symbol::HAS_INSTANCE,
        "toPrimitive" => wk_symbol::TO_PRIMITIVE,
        "dispose" => wk_symbol::DISPOSE,
        "match" => wk_symbol::MATCH,
        "asyncDispose" => wk_symbol::ASYNC_DISPOSE,
        "isConcatSpreadable" => wk_symbol::IS_CONCAT_SPREADABLE,
        "matchAll" => wk_symbol::MATCH_ALL,
        "replace" => wk_symbol::REPLACE,
        "search" => wk_symbol::SEARCH,
        "split" => wk_symbol::SPLIT,
        "unscopables" => wk_symbol::UNSCOPABLES,
        _ => return None,
    };
    Some(value::encode_handle(value::TAG_SYMBOL, handle))
}
struct NativeActivation {
    active_len: u32,
    argument_count: u32,
    saved_variables: Vec<(usize, i64)>,
    environment: i64,
    caller_image_id: u64,
    new_target: i64,
    home_object: Option<wjsm_ir::HomeObject>,
    function: Option<NativeFunctionRef>,
    /// PrepareCall 选择特化 overlay 时 pin 住其 image，直到 FinishCall 弹出
    /// activation 才允许释放 RX mapping（LRU 淘汰只从选择表移除 Arc）。
    specialized_image: Option<Arc<CompiledImage>>,
}

struct NativeRegExp {
    pattern: String,
    flags: String,
    compiled: regress::Regex,
    last_index: usize,
}
#[derive(Debug, thiserror::Error)]
pub(crate) enum NativeRegExpError {
    #[error("Invalid regular expression flag: '{0}'")]
    InvalidFlag(char),
    #[error("Duplicate regular expression flag: '{0}'")]
    DuplicateFlag(char),
    #[error("Regular expression flags 'u' and 'v' are mutually exclusive")]
    ConflictingUnicodeFlags,
    #[error("Invalid regular expression: {0}")]
    InvalidPattern(#[source] regress::Error),
    #[error("Regular expression table capacity exceeded")]
    Capacity(#[source] std::num::TryFromIntError),
}

pub(crate) enum NativeConstantMaterializeError {
    InvalidRegExp(NativeRegExpError),
    InternalInvariant,
}

#[derive(Clone, Copy)]
struct NativeProxy {
    target: i64,
    handler: i64,
    revoked: bool,
}

#[derive(Clone, Copy)]
struct NativeFunctionRef {
    image_id: u64,
    function_index: u32,
    needs_prototype: bool,
    home_object: Option<wjsm_ir::HomeObject>,
    source_span: Option<wjsm_ir::SourceSpan>,
}

struct NativeProgramState {
    constants: Vec<Constant>,
    materialized_constants: Vec<Option<i64>>,
    /// 字符串常量的盒装值（NaN-boxed 运行时字符串句柄），install 期一次发布；
    /// 非字符串槽位为 `undefined`。生成代码经 vmctx `string_constants_base` 直读。
    string_constants: Vec<i64>,
    /// 对象模板 install 期烘焙元数据（每条 `OBJECT_TEMPLATE_META_WORDS` 个 u32）。
    object_template_meta: Vec<u32>,
    function_slots: Vec<Vec<usize>>,
    function_lengths: Vec<u32>,
    function_names: Vec<String>,
    function_source_spans: Vec<Option<wjsm_ir::SourceSpan>>,
    function_home_objects: Vec<Option<wjsm_ir::HomeObject>>,
    function_needs_prototype: Vec<bool>,
}

#[derive(Clone, Copy)]
enum NativePrivateSlot {
    Data(i64),
    Accessor { getter: i64, setter: i64 },
}

struct NativeAgentState {
    output: RefCell<Vec<u8>>,
    stderr: RefCell<Vec<u8>>,
    call_arena: Box<[i64]>,
    gc: gc::NativeGc,
    runtime_config: NativeRuntimeConfig,
    variables: Vec<i64>,
    shared_variable_slots: HashMap<String, usize>,
    isolated_variable_images: HashSet<u64>,
    isolated_variable_tables: HashMap<u64, Vec<i64>>,
    isolated_variable_active: Option<u64>,
    shared_variables_backup: Option<Vec<i64>>,
    builtin_image_id: Option<u64>,
    user_image_id: Option<u64>,
    user_function_count: Option<u32>,
    constants: Vec<Constant>,
    materialized_constants: Vec<Option<i64>>,
    /// 当前 image 的字符串常量盒装值（见 `NativeProgramState::string_constants`）。
    string_constants: Vec<i64>,
    /// 当前 image 的对象模板烘焙元数据（见 `NativeProgramState::object_template_meta`）。
    object_template_meta: Vec<u32>,
    function_slots: Vec<Vec<usize>>,
    function_needs_prototype: Vec<bool>,
    function_home_objects: Vec<Option<wjsm_ir::HomeObject>>,
    current_image_id: u64,
    /// 当前 image 反馈区的 `(基址, 字节长度)`，随 image 激活刷新。
    /// 每次宿主调用都要校验生成代码传入的反馈槽指针；走 `images` 哈希表会给
    /// 每次调用摊上一次 SipHash 查表。
    current_feedback_region: (usize, usize),
    /// install_program 批量发布期间的临时字符串根；提交到 program state 前也必须
    /// 覆盖中途分配触发的 GC。
    install_string_roots: Vec<i64>,
    /// 宿主方法执行期间（如高阶数组迭代器回调）跨 JS 调用的临时根。
    pub(crate) temporary_roots: Vec<i64>,
    programs: HashMap<u64, NativeProgramState>,
    retained_images: HashMap<u64, Arc<CompiledImage>>,
    program_snapshots: HashMap<u64, Arc<wjsm_ir::Program>>,
    variable_slot_snapshots: HashMap<u64, Arc<HashMap<String, u32>>>,
    ic_epochs: HashMap<u64, u64>,
    specialization: Option<SpecializationCoordinator>,
    function_lengths: Vec<u32>,
    function_names: Vec<String>,
    function_source_spans: Vec<Option<wjsm_ir::SourceSpan>>,
    images: HashMap<u64, Arc<CompiledImage>>,
    image_source_files: HashMap<u64, String>,
    functions: Vec<NativeFunctionRef>,
    function_ids: HashMap<(u64, u32), u32>,
    function_closures: HashMap<(u64, u32, i64), i64>,
    latest_function_closures: HashMap<(u64, u32), i64>,
    repository: NativeImageRepository,
    runtime_modules: dispatch::modules::NativeModuleState,
    scope_records: HashMap<u32, dispatch::modules::NativeScopeRecord>,
    string_ids: HashMap<(u32, u32), u32>,
    /// 码元值是密集的 `0..=255`，JIT 按值直接索引；固定数组避免热路径哈希与分配。
    latin1_char_strings: Box<[i64; LATIN1_CHAR_COUNT]>,
    activations: Vec<NativeActivation>,
    pending_stack_trace: Option<String>,
    maps: HashMap<u32, Vec<(i64, i64)>>,
    sets: HashMap<u32, Vec<i64>>,
    weak: dispatch::weak::NativeWeakState,
    collection_iterators: Vec<dispatch::collections::CollectionIterator>,
    array_iterators: HashMap<u32, NativeArrayIterator>,
    enumerators: HashMap<u32, dispatch::enumerator::NativeEnumerator>,
    regexp_iterators: Vec<dispatch::regexp::RegExpIterator>,
    array_buffers: HashMap<u32, dispatch::buffers::NativeArrayBuffer>,

    shared_array_buffers: HashMap<u32, dispatch::sab::NativeSharedArrayBuffer>,
    data_views: HashMap<u32, dispatch::buffers::NativeDataView>,
    typed_arrays: HashMap<u32, dispatch::typedarray::NativeTypedArray>,
    buffers: HashMap<u32, dispatch::node_buffer::NativeBuffer>,
    text_decoders: HashMap<u32, dispatch::web_encoding::TextDecoderSlot>,
    text_decoder_prototype: Option<i64>,
    node_buffer_bridge: Option<i64>,
    url_constructor: Option<i64>,
    url_search_params_constructor: Option<i64>,
    promises: HashMap<u32, dispatch::promise::NativePromise>,
    promise_combinators: Vec<dispatch::promise::NativePromiseCombinator>,
    continuations: HashMap<u32, dispatch::promise::NativeContinuation>,
    generators: HashMap<u32, dispatch::generator::NativeGenerator>,
    async_generators: HashMap<u32, dispatch::async_generator::NativeAsyncGenerator>,
    async_generator_prototype: Option<i64>,
    async_iterator_prototype: Option<i64>,
    async_from_sync_iterators: HashMap<u32, i64>,
    async_iterator_objects: HashSet<i64>,
    async_generator_resume_completions: HashMap<u32, f64>,
    promise_reactions: HashMap<u32, Vec<dispatch::promise::NativeScheduledReaction>>,
    /// 待报告的 unhandled rejection: (promise_handle, reason)
    /// GC 可能回收 promise 对象，但 reason 必须留存到事件循环结束时报告
    pending_unhandled_rejections: Vec<(u32, String)>,
    microtasks: VecDeque<dispatch::promise::NativeScheduledMicrotask>,
    iterator_next: HashMap<u32, u32>,
    array_properties: HashMap<(u32, PropertyKey), i64>,
    array_property_order: HashMap<u32, Vec<PropertyKey>>,
    array_accessors: HashMap<(u32, PropertyKey), (i64, i64, u32)>,
    array_property_flags: HashMap<(u32, PropertyKey), u32>,
    closures: Vec<Option<NativeClosure>>,
    closure_free: Vec<u32>,
    bound_functions: Vec<Option<NativeBoundFunction>>,
    bound_free: Vec<u32>,
    next_ticks: VecDeque<dispatch::promise::NativeScheduledMicrotask>,
    immediates: VecDeque<dispatch::promise::NativeScheduledMicrotask>,
    timers: BinaryHeap<dispatch::promise::NativeTimer>,
    timer_now_ms: u64,
    next_timer_sequence: u64,
    cancelled_timers: HashSet<u32>,
    exceptions: Vec<Option<i64>>,
    exception_free: Vec<u32>,
    out_of_memory_error: Option<i64>,
    out_of_memory_exception: Option<i64>,
    fatal_exception: Option<i64>,
    callable_prototypes: HashMap<i64, i64>,
    private_slots: HashMap<(i64, PropertyKey), NativePrivateSlot>,
    private_brands: HashMap<u32, i64>,
    callable_properties: HashMap<(i64, PropertyKey), i64>,
    callable_accessors: HashMap<(i64, PropertyKey), (i64, i64)>,
    callable_property_flags: HashMap<(i64, PropertyKey), u32>,
    non_extensible_objects: HashSet<u32>,
    environment: HashMap<String, String>,
    working_directory: PathBuf,
    process_arguments: Vec<String>,
    process_entry: Option<String>,
    process_started_at: Instant,
    requested_exit_code: Option<i32>,
    error_objects: HashSet<u32>,
    boxed_primitives: HashMap<u32, i64>,
    error_prototypes: HashMap<String, i64>,
    process_object: Option<i64>,
    process_env_object: Option<i64>,
    proxies: Vec<Option<NativeProxy>>,
    proxy_free: Vec<u32>,
    array_constructor: Option<i64>,
    global_object: Option<i64>,
    object_prototype: Option<i64>,
    array_prototype: Option<i64>,
    regexp_prototype: Option<i64>,
    symbol_prototype: Option<i64>,
    map_prototype: Option<i64>,
    set_prototype: Option<i64>,
    weak_map_prototype: Option<i64>,
    weak_set_prototype: Option<i64>,
    console_object: Option<i64>,
    intl: dispatch::intl::IntlState,
    native_callables: Vec<NativeCallableKind>,
    node_fs_bridge: Option<i64>,
    regexps: Vec<Option<NativeRegExp>>,
    regexp_free: Vec<u32>,
    node_crypto: dispatch::node_crypto::NodeCryptoState,
    node_async_hooks: dispatch::node_async_hooks::NodeAsyncHooksState,
    node_dgram: dispatch::node_dgram::NodeDgramState,
    node_net: dispatch::node_net::NodeNetState,
    node_tls: dispatch::node_tls::NodeTlsState,
    node_zlib: dispatch::node_zlib::NodeZlibState,
    node_os: dispatch::node_os::NodeOsState,
    idna: dispatch::idna::IdnaState,
    node_vm: dispatch::node_vm::NodeVmState,
    node_child_process: dispatch::node_child_process::NodeChildProcessState,
    node_perf_hooks: dispatch::node_perf_hooks::NodePerfHooksState,
    node_worker_threads: dispatch::node_worker_threads::NodeWorkerThreadsState,
    fetch: dispatch::fetch::NativeFetchState,
    streams: dispatch::streams::NativeStreamsState,
    /// test262 `$262.agent`：主 agent 侧缓存 `$262` 对象。
    agent_bridge: Option<i64>,
    /// test262 agent 线程内的 `$262.agent` 状态（仅 agent 线程 Some）。
    test262_agent: Option<dispatch::agent::Test262AgentState>,
    symbol_registry: HashMap<RuntimeString, u32>,
    symbol_descriptions: HashMap<u32, Option<RuntimeString>>,
    next_symbol_handle: u32,
    native_callable_ids: HashMap<NativeCallableKind, u32>,
    inspector: Option<inspector::InspectorRuntime>,
}

#[derive(Clone, Copy)]
pub(crate) enum NativeIteratorSource {
    Array(u32),
    ArrayLike(u32),
    String(i64),
    TypedArray(u32),
    Map(u32),
    Set(u32),
    Custom(i64),
}

#[derive(Clone, Copy)]
struct NativeArrayIterator {
    source: NativeIteratorSource,
    kind: NativeIteratorKind,
    index: u32,
    current: Option<i64>,
    done: bool,
}

impl NativeAgentState {
    fn new(config: NativeRuntimeConfig) -> Result<Self, NativeRuntimeError> {
        // 把 ICU4X compiled_data 留在 rustc 链接的 stub 里，避免 DCE 在 Intl API 落地前删掉。
        wjsm_intl_data::keep_compiled_data();
        let gc = gc::NativeGc::new(
            config.gc_algorithm,
            config.max_heap_size,
            config.allocation_diagnostics_enabled,
        )?;
        let compiler = NativeCompiler::new()?;
        let repository = if config.isolate_native_images {
            NativeImageRepository::new_exclusive(compiler.clone(), config.cache_dir.clone())
        } else {
            NativeImageRepository::new(compiler.clone(), config.cache_dir.clone())
        };
        let specialization = config
            .specialization_enabled
            .then(|| SpecializationCoordinator::new(compiler));
        let eval_callable = NativeCallableKind::Builtin(wjsm_ir::Builtin::EvalIndirect, false);
        Ok(Self {
            output: RefCell::new(Vec::new()),
            stderr: RefCell::new(Vec::new()),
            call_arena: vec![value::encode_undefined(); DEFAULT_CALL_ARENA_SLOTS]
                .into_boxed_slice(),
            runtime_config: config.clone(),
            gc,
            variables: Vec::new(),
            shared_variable_slots: HashMap::new(),
            isolated_variable_images: HashSet::new(),
            isolated_variable_tables: HashMap::new(),
            isolated_variable_active: None,
            shared_variables_backup: None,
            builtin_image_id: None,
            user_image_id: None,
            user_function_count: None,
            constants: Vec::new(),
            materialized_constants: Vec::new(),
            string_constants: Vec::new(),
            object_template_meta: Vec::new(),
            function_slots: Vec::new(),
            function_needs_prototype: Vec::new(),
            function_home_objects: Vec::new(),
            current_image_id: 0,
            current_feedback_region: (0, 0),
            install_string_roots: Vec::new(),
            temporary_roots: Vec::new(),
            programs: HashMap::new(),
            program_snapshots: HashMap::new(),
            variable_slot_snapshots: HashMap::new(),
            ic_epochs: HashMap::new(),
            specialization,
            images: HashMap::new(),
            function_lengths: Vec::new(),
            retained_images: HashMap::new(),
            function_names: Vec::new(),
            function_source_spans: Vec::new(),
            image_source_files: HashMap::new(),
            functions: Vec::new(),
            function_ids: HashMap::new(),
            pending_stack_trace: None,
            function_closures: HashMap::new(),
            latest_function_closures: HashMap::new(),
            repository,
            runtime_modules: dispatch::modules::NativeModuleState::default(),
            scope_records: HashMap::new(),
            string_ids: HashMap::new(),
            latin1_char_strings: Box::new([value::encode_undefined(); LATIN1_CHAR_COUNT]),
            activations: Vec::new(),
            maps: HashMap::new(),
            sets: HashMap::new(),
            weak: dispatch::weak::NativeWeakState::default(),
            array_accessors: HashMap::new(),
            array_property_flags: HashMap::new(),
            collection_iterators: Vec::new(),
            array_iterators: HashMap::new(),
            enumerators: HashMap::new(),
            buffers: HashMap::new(),
            text_decoders: HashMap::new(),
            text_decoder_prototype: None,
            node_buffer_bridge: None,
            url_constructor: None,
            url_search_params_constructor: None,
            regexp_iterators: Vec::new(),
            array_buffers: HashMap::new(),
            shared_array_buffers: HashMap::new(),
            data_views: HashMap::new(),
            typed_arrays: HashMap::new(),
            promises: HashMap::new(),
            continuations: HashMap::new(),
            generators: HashMap::new(),
            async_generators: HashMap::new(),
            async_generator_prototype: None,
            async_iterator_prototype: None,
            async_from_sync_iterators: HashMap::new(),
            async_iterator_objects: HashSet::new(),
            async_generator_resume_completions: HashMap::new(),
            promise_reactions: HashMap::new(),
            pending_unhandled_rejections: Vec::new(),
            promise_combinators: Vec::new(),
            microtasks: VecDeque::new(),
            iterator_next: HashMap::new(),
            array_properties: HashMap::new(),
            array_property_order: HashMap::new(),
            closures: Vec::new(),
            closure_free: Vec::new(),
            bound_functions: Vec::new(),
            bound_free: Vec::new(),
            exceptions: Vec::new(),
            exception_free: Vec::new(),
            out_of_memory_error: None,
            out_of_memory_exception: None,
            fatal_exception: None,
            callable_properties: HashMap::new(),
            callable_prototypes: HashMap::new(),
            private_slots: HashMap::new(),
            private_brands: HashMap::new(),
            callable_accessors: HashMap::new(),
            callable_property_flags: HashMap::new(),
            error_objects: HashSet::new(),
            boxed_primitives: HashMap::new(),
            error_prototypes: HashMap::new(),
            non_extensible_objects: HashSet::new(),
            next_ticks: VecDeque::new(),
            immediates: VecDeque::new(),
            timers: BinaryHeap::new(),
            timer_now_ms: 0,
            next_timer_sequence: 0,
            cancelled_timers: HashSet::new(),
            proxies: Vec::new(),
            proxy_free: Vec::new(),
            global_object: None,
            object_prototype: None,
            array_prototype: None,
            regexp_prototype: None,
            symbol_prototype: None,
            map_prototype: None,
            set_prototype: None,
            weak_map_prototype: None,
            weak_set_prototype: None,
            console_object: None,
            intl: dispatch::intl::IntlState::default(),
            array_constructor: None,
            node_net: dispatch::node_net::NodeNetState::default(),
            node_tls: dispatch::node_tls::NodeTlsState::default(),
            node_zlib: dispatch::node_zlib::NodeZlibState::default(),
            node_crypto: dispatch::node_crypto::NodeCryptoState::default(),
            node_async_hooks: dispatch::node_async_hooks::NodeAsyncHooksState::default(),
            node_dgram: dispatch::node_dgram::NodeDgramState::default(),
            node_os: dispatch::node_os::NodeOsState::default(),
            idna: dispatch::idna::IdnaState::default(),
            node_vm: dispatch::node_vm::NodeVmState::default(),
            node_child_process: dispatch::node_child_process::NodeChildProcessState::default(),
            node_perf_hooks: dispatch::node_perf_hooks::NodePerfHooksState::default(),
            node_worker_threads: dispatch::node_worker_threads::NodeWorkerThreadsState::main(),
            streams: dispatch::streams::NativeStreamsState::default(),
            fetch: dispatch::fetch::NativeFetchState::default(),
            agent_bridge: None,
            test262_agent: None,
            node_fs_bridge: None,
            native_callables: vec![eval_callable],
            environment: std::env::vars().collect(),
            working_directory: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            process_arguments: Vec::new(),
            process_entry: None,
            process_started_at: Instant::now(),
            requested_exit_code: None,
            process_object: None,
            process_env_object: None,
            regexps: Vec::new(),
            regexp_free: Vec::new(),
            symbol_registry: HashMap::new(),
            symbol_descriptions: HashMap::new(),
            next_symbol_handle: FIRST_USER_SYMBOL_HANDLE,
            inspector: None,
            native_callable_ids: HashMap::from([(eval_callable, 0)]),
        })
    }

    fn program_state(
        program: &wjsm_ir::Program,
        variable_slots: &HashMap<String, u32>,
        shared_module_slots: &HashSet<&str>,
    ) -> NativeProgramState {
        NativeProgramState {
            constants: program.constants().to_vec(),
            materialized_constants: vec![None; program.constants().len()],
            // install_program 随后按烘焙元数据填充字符串槽位。
            string_constants: Vec::new(),
            object_template_meta: Vec::new(),
            function_slots: function_slots_for_program(
                program,
                variable_slots,
                shared_module_slots,
            ),
            function_needs_prototype: program
                .functions()
                .iter()
                .map(wjsm_ir::Function::needs_prototype)
                .collect(),
            function_home_objects: program
                .functions()
                .iter()
                .map(|function| function.home_object)
                .collect(),
            function_lengths: program
                .functions()
                .iter()
                .map(|function| {
                    u32::try_from(function.params().len().saturating_sub(2)).unwrap_or(0)
                })
                .collect(),
            function_names: program
                .functions()
                .iter()
                .map(|function| function.name().to_owned())
                .collect(),
            function_source_spans: program
                .functions()
                .iter()
                .map(wjsm_ir::Function::source_span)
                .collect(),
        }
    }

    fn install_shared_variables(
        &mut self,
        builtin_slots: &HashMap<String, u32>,
        user_slots: &HashMap<String, u32>,
    ) {
        let len = slot_table_len(user_slots).max(slot_table_len(builtin_slots));
        self.variables = vec![value::encode_undefined(); len];
        self.shared_variable_slots = user_slots
            .iter()
            .map(|(name, slot)| {
                (
                    name.clone(),
                    usize::try_from(*slot).expect("槽号在 usize 内"),
                )
            })
            .collect();
        self.isolated_variable_images.clear();
        self.isolated_variable_tables.clear();
        self.isolated_variable_active = None;
        self.shared_variables_backup = None;
    }

    fn install_whole_program_variables(&mut self, program: &wjsm_ir::Program) {
        let names = native_variable_names(program);
        self.variables = vec![value::encode_undefined(); names.len()];
        self.shared_variable_slots = names
            .into_iter()
            .enumerate()
            .map(|(index, name)| (name, index))
            .collect();
        self.isolated_variable_images.clear();
        self.isolated_variable_tables.clear();
        self.isolated_variable_active = None;
        self.shared_variables_backup = None;
    }

    pub(crate) fn emit_output(&self, bytes: &[u8], stderr: bool) {
        if stderr {
            self.stderr.borrow_mut().extend_from_slice(bytes);
        } else {
            self.output.borrow_mut().extend_from_slice(bytes);
        }
        if self.runtime_config.output_mode != OutputMode::Inherit {
            return;
        }
        if stderr {
            let _ = std::io::stderr().write_all(bytes);
            let _ = std::io::stderr().flush();
        } else {
            let _ = std::io::stdout().write_all(bytes);
            let _ = std::io::stdout().flush();
        }
    }

    fn install_isolated_program(
        &mut self,
        ctx: &mut NativeVmContext,
        image: Arc<CompiledImage>,
        program: &wjsm_ir::Program,
    ) -> Result<(), NativeRuntimeError> {
        let slots = whole_program_slots(program);
        let image_id = image.image_id();
        self.install_program(ctx, image, program, &slots, &HashSet::new())?;
        self.isolated_variable_images.insert(image_id);
        self.isolated_variable_tables.insert(
            image_id,
            vec![value::encode_undefined(); slot_table_len(&slots)],
        );
        Ok(())
    }

    fn swap_isolated_variables(&mut self, image_id: u64) {
        if self.isolated_variable_active == Some(image_id) {
            return;
        }
        if let Some(previous) = self.isolated_variable_active.take() {
            self.isolated_variable_tables
                .insert(previous, std::mem::take(&mut self.variables));
            if let Some(shared) = self.shared_variables_backup.take() {
                self.variables = shared;
            }
        }
        if !self.isolated_variable_images.contains(&image_id) {
            return;
        }
        self.shared_variables_backup = Some(std::mem::take(&mut self.variables));
        self.variables = self
            .isolated_variable_tables
            .remove(&image_id)
            .unwrap_or_default();
        self.isolated_variable_active = Some(image_id);
    }

    fn reset_execution(&mut self) {
        self.variables.clear();
        self.shared_variable_slots.clear();
        self.isolated_variable_images.clear();
        self.isolated_variable_tables.clear();
        self.isolated_variable_active = None;
        self.shared_variables_backup = None;
        self.constants.clear();
        self.materialized_constants.clear();
        self.string_constants.clear();
        self.function_slots.clear();
        self.function_needs_prototype.clear();
        self.function_home_objects.clear();
        self.function_lengths.clear();
        self.function_names.clear();
        self.function_source_spans.clear();
        self.current_image_id = 0;
        self.current_feedback_region = (0, 0);
        self.install_string_roots.clear();
        self.builtin_image_id = None;
        self.user_image_id = None;
        self.user_function_count = None;
        self.retained_images.extend(self.images.drain());
        self.program_snapshots.clear();
        self.variable_slot_snapshots.clear();
        self.ic_epochs.clear();
        self.programs.clear();
        if let Some(coordinator) = self.specialization.as_mut() {
            coordinator.reset_runtime_state();
        }
        self.image_source_files.clear();
        self.process_object = None;
        self.process_env_object = None;
        self.process_entry = None;
        self.requested_exit_code = None;
        self.functions.clear();
        self.function_ids.clear();
        self.runtime_modules.clear();
        self.scope_records.clear();
        self.array_properties.clear();
        self.array_property_order.clear();
        self.string_ids.clear();
        self.latin1_char_strings.fill(value::encode_undefined());
        self.array_accessors.clear();
        self.array_property_flags.clear();
        self.activations.clear();
        self.pending_stack_trace = None;
        self.closures.clear();
        self.closure_free.clear();
        self.bound_functions.clear();
        self.bound_free.clear();
        self.exceptions.clear();
        self.exception_free.clear();
        self.out_of_memory_error = None;
        self.out_of_memory_exception = None;
        self.fatal_exception = None;
        self.object_prototype = None;
        self.array_prototype = None;
        self.regexp_prototype = None;
        self.symbol_prototype = None;
        self.map_prototype = None;
        self.set_prototype = None;
        self.weak_map_prototype = None;
        self.weak_set_prototype = None;
        self.array_constructor = None;
        self.global_object = None;
        self.console_object = None;
        self.intl = dispatch::intl::IntlState::default();
        self.maps.clear();
        self.sets.clear();
        self.weak.clear();
        self.collection_iterators.clear();
        self.promise_combinators.clear();
        self.array_iterators.clear();
        self.regexp_iterators.clear();
        self.array_buffers.clear();
        self.shared_array_buffers.clear();
        self.data_views.clear();
        self.typed_arrays.clear();
        self.node_fs_bridge = None;
        self.callable_properties.clear();
        self.callable_accessors.clear();
        self.callable_property_flags.clear();
        self.error_objects.clear();
        self.boxed_primitives.clear();
        self.error_prototypes.clear();
        self.non_extensible_objects.clear();
        self.node_worker_threads.reset_agent();
        self.node_child_process.reset_agent();
        // test262_agent 由 configure_test262_agent 注入，reset_execution 不清除，
        // 否则 agent 线程 execute 时会丢失 receiveBroadcast 注册。
        self.agent_bridge = None;
        self.buffers.clear();
        self.text_decoders.clear();
        self.text_decoder_prototype = None;
        self.node_buffer_bridge = None;
        self.url_constructor = None;
        self.url_search_params_constructor = None;
        self.node_net = dispatch::node_net::NodeNetState::default();
        self.node_tls = dispatch::node_tls::NodeTlsState::default();
        self.node_zlib = dispatch::node_zlib::NodeZlibState::default();
        self.node_crypto = dispatch::node_crypto::NodeCryptoState::default();
        self.node_async_hooks = dispatch::node_async_hooks::NodeAsyncHooksState::default();
        self.node_dgram = dispatch::node_dgram::NodeDgramState::default();
        self.node_os = dispatch::node_os::NodeOsState::default();
        self.idna = dispatch::idna::IdnaState::default();
        self.node_perf_hooks = dispatch::node_perf_hooks::NodePerfHooksState::default();
        self.streams = dispatch::streams::NativeStreamsState::default();
        self.fetch = dispatch::fetch::NativeFetchState::default();
        self.promises.clear();
        self.continuations.clear();
        self.generators.clear();
        self.async_generators.clear();
        self.async_generator_prototype = None;
        self.async_iterator_prototype = None;
        self.async_from_sync_iterators.clear();
        self.async_iterator_objects.clear();
        self.async_generator_resume_completions.clear();
        self.promise_reactions.clear();
        self.pending_unhandled_rejections.clear();
        self.microtasks.clear();
        self.iterator_next.clear();
        self.node_vm = dispatch::node_vm::NodeVmState::default();
        self.regexps.clear();
        self.regexp_free.clear();
        self.proxies.clear();
        self.proxy_free.clear();
        self.next_ticks.clear();
        self.immediates.clear();
        self.timers.clear();
        self.timer_now_ms = 0;
        self.process_started_at = Instant::now();
        self.next_timer_sequence = 0;
        self.cancelled_timers.clear();
        self.native_callables.clear();
        self.native_callable_ids.clear();
        let eval_callable = NativeCallableKind::Builtin(wjsm_ir::Builtin::EvalIndirect, false);
        self.native_callables.push(eval_callable);
        self.native_callable_ids.insert(eval_callable, 0);
        self.function_closures.clear();
        self.latest_function_closures.clear();
        self.callable_prototypes.clear();
        self.private_slots.clear();
        self.private_brands.clear();
        self.symbol_registry.clear();
        self.symbol_descriptions.clear();
        self.next_symbol_handle = FIRST_USER_SYMBOL_HANDLE;
        self.gc.reset_nlab();
    }

    fn take_program_state(&mut self) -> NativeProgramState {
        NativeProgramState {
            constants: std::mem::take(&mut self.constants),
            function_lengths: std::mem::take(&mut self.function_lengths),
            function_names: std::mem::take(&mut self.function_names),
            function_source_spans: std::mem::take(&mut self.function_source_spans),
            materialized_constants: std::mem::take(&mut self.materialized_constants),
            string_constants: std::mem::take(&mut self.string_constants),
            object_template_meta: std::mem::take(&mut self.object_template_meta),
            function_slots: std::mem::take(&mut self.function_slots),
            function_needs_prototype: std::mem::take(&mut self.function_needs_prototype),
            function_home_objects: std::mem::take(&mut self.function_home_objects),
        }
    }

    fn set_program_state(&mut self, state: NativeProgramState) {
        self.constants = state.constants;
        self.materialized_constants = state.materialized_constants;
        self.string_constants = state.string_constants;
        self.object_template_meta = state.object_template_meta;
        self.function_slots = state.function_slots;
        self.function_lengths = state.function_lengths;
        self.function_names = state.function_names;
        self.function_source_spans = state.function_source_spans;
        self.function_needs_prototype = state.function_needs_prototype;
        self.function_home_objects = state.function_home_objects;
    }

    fn install_program(
        &mut self,
        ctx: &mut NativeVmContext,
        mut image: Arc<CompiledImage>,
        program: &wjsm_ir::Program,
        variable_slots: &HashMap<String, u32>,
        shared_module_slots: &HashSet<&str>,
    ) -> Result<(), NativeRuntimeError> {
        let image_id = image.image_id();
        // 字符串常量 install 期一次发布（烘焙 hash + 载荷直发，运行时零哈希零转换）；
        // 分配耗尽按 2.6 的统一模式先收集再重试一次。
        let mut string_constants = Vec::with_capacity(program.constants().len());
        self.install_string_roots.clear();
        for constant in program.constants() {
            let encoded = match constant {
                Constant::String(text) => {
                    let owned;
                    let meta = match program.string_constant_meta(wjsm_ir::ConstantId(
                        u32::try_from(string_constants.len()).expect("常量下标在 u32 内"),
                    )) {
                        Some(meta) => meta,
                        // 元数据缺失（serde 兼容回退）时从文本重算；种子固定，同值。
                        None => {
                            owned = wjsm_ir::StringConstantMeta::from_text(text);
                            &owned
                        }
                    };
                    self.publish_baked_string(ctx, meta)?
                }
                _ => value::encode_undefined(),
            };
            string_constants.push(encoded);
            if encoded != value::encode_undefined() && !value::is_inline_string(encoded) {
                self.install_string_roots.push(encoded);
            }
        }
        self.install_string_roots.clear();
        let object_template_meta = bake_object_template_meta_table(
            self.gc.heap().shapes(),
            program.constants(),
            &string_constants,
        );
        let ic_hints = wjsm_backend_native::ic_template_hints(program);
        if let Some(image) = Arc::get_mut(&mut image) {
            image.prefill_template_ic_slots(&ic_hints, &object_template_meta);
        }
        self.images.insert(image_id, image);
        self.program_snapshots
            .insert(image_id, Arc::new(program.clone()));
        self.variable_slot_snapshots
            .insert(image_id, Arc::new(variable_slots.clone()));
        self.ic_epochs.insert(image_id, 0);
        let mut state = Self::program_state(program, variable_slots, shared_module_slots);
        state.string_constants = string_constants;
        state.object_template_meta = object_template_meta;
        self.programs.insert(image_id, state);
        if let Some(source_file) = program.source_file() {
            self.image_source_files
                .insert(image_id, source_file.to_owned());
        }
        Ok(())
    }

    /// 发布烘焙的字符串常量：去重命中复用既有句柄；否则按编译期元数据直发。
    /// 分配耗尽时全量收集并推进搬迁/回收 epoch 后重试一次（与
    /// `dispatch::runtime::intern_string_with_gc_retry` 同一模式）。
    fn publish_baked_string(
        &mut self,
        ctx: &mut NativeVmContext,
        meta: &wjsm_ir::StringConstantMeta,
    ) -> Result<i64, NativeRuntimeError> {
        if let Some(encoded) = self.try_publish_baked_string(meta) {
            return Ok(encoded);
        }
        self.collect_garbage(ctx)?;
        let _ = self.gc.heap().finish_relocation_epoch();
        for _ in 0..8 {
            let _ = self.gc.heap().advance_epoch_and_reclaim();
        }
        self.try_publish_baked_string(meta).ok_or_else(|| {
            NativeRuntimeError::Invariant("install 期字符串常量发布在收集重试后仍然失败".into())
        })
    }

    fn try_publish_baked_string(&mut self, meta: &wjsm_ir::StringConstantMeta) -> Option<i64> {
        let length = meta.unit_len();
        if meta.latin1 {
            if let Some(encoded) = value::encode_inline_ascii(&meta.payload) {
                return Some(encoded);
            }
            if let Some(encoded) = value::encode_inline_latin1(&meta.payload) {
                return Some(encoded);
            }
        }
        let key = (meta.hash, length);
        if let Some(encoded) = self.dedup_string_handle(&key) {
            return Some(encoded);
        }
        self.publish_flat_string(
            &key,
            length,
            &meta.payload,
            value::TAG_STRING,
            true,
            meta.latin1,
        )
    }

    fn activate_image(&mut self, ctx: &mut NativeVmContext, image_id: u64) -> Option<()> {
        if self.current_image_id != image_id {
            let next = self.programs.remove(&image_id)?;
            if self.current_image_id != 0 {
                let current = self.take_program_state();
                self.programs.insert(self.current_image_id, current);
            }
            self.set_program_state(next);
            self.current_image_id = image_id;
        }
        self.swap_isolated_variables(image_id);
        let image = self.images.get(&image_id)?;
        ctx.function_table = image.entries().as_ptr();
        ctx.function_table_len = u32::try_from(image.entries().len()).ok()?;
        ctx.current_image_id = image_id;
        // 与 function_table 同步：snapshot 恢复替换 heap 后句柄表基址会变，
        // 生成代码的属性快链依赖这些基址，必须在每次 image 激活时刷新。
        ctx.handle_table_base = self.gc.heap().handle_table_base();
        ctx.ic_slots_base = image.ic_slots().cast::<u8>().cast_mut();
        // 字符串常量数组与 ic_slots 同步：install 期已填充且不再变化，
        // 生成代码的函数入口直读替代旧的 MaterializeString 宿主往返。
        ctx.string_constants_base = if self.string_constants.is_empty() {
            std::ptr::null()
        } else {
            self.string_constants.as_ptr()
        };
        ctx.object_template_meta_base = if self.object_template_meta.is_empty() {
            std::ptr::null()
        } else {
            self.object_template_meta.as_ptr()
        };
        ctx.object_template_meta_count = u32::try_from(
            self.object_template_meta
                .len()
                .checked_div(wjsm_ir::constants::OBJECT_TEMPLATE_META_WORDS as usize)
                .unwrap_or(0),
        )
        .unwrap_or(0);
        ctx.feedback_slots_base = image.feedback_slots();
        self.current_feedback_region = (
            image.feedback_slots().addr(),
            usize::try_from(image.feedback_slot_count())
                .ok()?
                .saturating_mul(wjsm_ir::constants::FEEDBACK_SLOT_SIZE as usize),
        );
        ctx.proto_generation = self.gc.heap().shapes().proto_generation();
        // 对象地址的「逻辑 → 虚拟」偏移：snapshot 恢复后 virtual_base 可能改变，
        // 必须与 handle_table_base 同步刷新，属性快链才能把 entry 里的逻辑地址
        // 换算成真实映射地址。
        ctx.heap_object_delta = self.gc.heap().object_address_delta();
        Some(())
    }

    fn materialize_constant(
        &mut self,
        index: usize,
        operation: NativeRuntimeOp,
    ) -> Result<i64, NativeConstantMaterializeError> {
        if let Some(value) = self
            .materialized_constants
            .get(index)
            .copied()
            .ok_or(NativeConstantMaterializeError::InternalInvariant)?
        {
            return Ok(value);
        }
        let constant = self
            .constants
            .get(index)
            .cloned()
            .ok_or(NativeConstantMaterializeError::InternalInvariant)?;
        let encoded = match (operation, constant) {
            (NativeRuntimeOp::MaterializeBigInt, Constant::BigInt(text)) => self
                .intern_text(text, value::TAG_BIGINT)
                .ok_or(NativeConstantMaterializeError::InternalInvariant)?,
            (NativeRuntimeOp::MaterializeRegExp, Constant::RegExp { pattern, flags }) => self
                .create_regexp(pattern, flags)
                .map_err(NativeConstantMaterializeError::InvalidRegExp)?,
            _ => return Err(NativeConstantMaterializeError::InternalInvariant),
        };
        *self
            .materialized_constants
            .get_mut(index)
            .ok_or(NativeConstantMaterializeError::InternalInvariant)? = Some(encoded);
        Ok(encoded)
    }
    fn materialize_function(&mut self, function_index: u32) -> Option<i64> {
        if let (Some(builtin_id), Some(user_id), Some(user_count)) = (
            self.builtin_image_id,
            self.user_image_id,
            self.user_function_count,
        ) && self.current_image_id == user_id
            && function_index >= user_count
        {
            return self.materialize_function_in(builtin_id, function_index - user_count);
        }
        self.materialize_function_in(self.current_image_id, function_index)
    }

    fn materialize_function_in(&mut self, image_id: u64, function_index: u32) -> Option<i64> {
        let key = (image_id, function_index);
        if let Some(environment) = self.call_environment() {
            let closure_key = (image_id, function_index, environment);
            if let Some(closure) = self.function_closures.get(&closure_key).copied() {
                if self.closure_is_alive(closure) {
                    return Some(closure);
                }
                self.function_closures.remove(&closure_key);
            }
        }
        if let Some(closure) = self.latest_function_closures.get(&key).copied() {
            if self.closure_is_alive(closure) {
                return Some(closure);
            }
            self.latest_function_closures.remove(&key);
        }
        if let Some(function_id) = self.function_ids.get(&key).copied() {
            return Some(value::encode_function_idx(function_id));
        }
        let local_index = usize::try_from(function_index).ok()?;
        let (needs_prototype, home_object, source_span) = if image_id == self.current_image_id {
            (
                *self.function_needs_prototype.get(local_index)?,
                *self.function_home_objects.get(local_index)?,
                *self.function_source_spans.get(local_index)?,
            )
        } else {
            let program = self.programs.get(&image_id)?;
            (
                *program.function_needs_prototype.get(local_index)?,
                *program.function_home_objects.get(local_index)?,
                *program.function_source_spans.get(local_index)?,
            )
        };
        let function_id = u32::try_from(self.functions.len()).ok()?;
        let function = NativeFunctionRef {
            image_id,
            function_index,
            needs_prototype,
            home_object,
            source_span,
        };
        self.functions.push(function);
        self.function_ids.insert(key, function_id);
        Some(value::encode_function_idx(function_id))
    }

    fn create_regexp(&mut self, pattern: String, flags: String) -> Result<i64, NativeRegExpError> {
        let mut seen = HashSet::new();
        for flag in flags.chars() {
            if !matches!(flag, 'd' | 'g' | 'i' | 'm' | 's' | 'u' | 'v' | 'y') {
                return Err(NativeRegExpError::InvalidFlag(flag));
            }
            if !seen.insert(flag) {
                return Err(NativeRegExpError::DuplicateFlag(flag));
            }
        }
        if seen.contains(&'u') && seen.contains(&'v') {
            return Err(NativeRegExpError::ConflictingUnicodeFlags);
        }
        let engine_flags: String = flags
            .chars()
            .filter(|flag| matches!(flag, 'i' | 'm' | 's' | 'u' | 'v'))
            .collect();
        let compiled = regress::Regex::with_flags(&pattern, engine_flags.as_str())
            .map_err(NativeRegExpError::InvalidPattern)?;
        let handle = match self.regexp_free.pop() {
            Some(handle) => handle,
            None => u32::try_from(self.regexps.len()).map_err(NativeRegExpError::Capacity)?,
        };
        if (handle as usize) == self.regexps.len() {
            self.regexps.push(None);
        }
        self.regexps[handle as usize] = Some(NativeRegExp {
            pattern,
            flags,
            compiled,
            last_index: 0,
        });
        Ok(value::encode_regexp_handle(handle))
    }

    fn regexp(&self, encoded: i64) -> Option<&NativeRegExp> {
        value::is_regexp(encoded)
            .then(|| value::decode_regexp_handle(encoded))
            .and_then(|handle| usize::try_from(handle).ok())
            .and_then(|handle| self.regexps.get(handle))
            .and_then(|regexp| regexp.as_ref())
    }

    fn regexp_mut(&mut self, encoded: i64) -> Option<&mut NativeRegExp> {
        value::is_regexp(encoded)
            .then(|| value::decode_regexp_handle(encoded))
            .and_then(|handle| usize::try_from(handle).ok())
            .and_then(|handle| self.regexps.get_mut(handle))
            .and_then(|regexp| regexp.as_mut())
    }

    fn create_symbol(&mut self, description: Option<RuntimeString>) -> Option<i64> {
        let handle = self.next_symbol_handle;
        self.next_symbol_handle = handle.checked_add(1)?;
        self.symbol_descriptions.insert(handle, description);
        Some(value::encode_handle(value::TAG_SYMBOL, handle))
    }

    fn symbol_description(&self, encoded: i64) -> Option<RuntimeString> {
        let handle = value::is_symbol(encoded).then(|| value::decode_handle(encoded))?;
        if let Some(description) = self.symbol_descriptions.get(&handle) {
            return description.clone();
        }
        dispatch::well_known_description(handle).map(RuntimeString::from)
    }

    fn symbol_value(&self, receiver: i64) -> Option<i64> {
        if value::is_symbol(receiver) {
            return Some(receiver);
        }
        value::is_js_object(receiver)
            .then(|| value::decode_handle(receiver))
            .and_then(|handle| self.boxed_primitives.get(&handle))
            .copied()
            .filter(|primitive| value::is_symbol(*primitive))
    }

    fn text_matches(&self, encoded: i64, expected: &str) -> bool {
        self.with_string_bytes(encoded, |view| {
            view.len() == expected.encode_utf16().count()
                && expected
                    .encode_utf16()
                    .enumerate()
                    .all(|(index, unit)| view.unit(index) == Some(unit))
        })
        .unwrap_or(false)
    }

    fn property_value_by_name(&self, object: i64, name: &str) -> Option<i64> {
        let units = name.encode_utf16().collect::<Vec<_>>();
        let key = Self::encode_inline_ascii_units(&units)
            .and_then(PropertyKey::inline_string)
            .or_else(|| {
                let hash = content_hash_units(&units);
                let length = u32::try_from(units.len()).ok()?;
                self.string_ids
                    .get(&(hash, length))
                    .copied()
                    .map(PropertyKey::from_name_id)
            })?;
        self.gc
            .heap()
            .get_property(value::decode_handle(object), key)
            .ok()
            .flatten()
            .map(|stored| stored as i64)
    }

    fn compiled_entry(&self, function: NativeFunctionRef) -> Option<i64> {
        let image = self.images.get(&function.image_id)?;
        let entry = image
            .entries()
            .get(usize::try_from(function.function_index).ok()?)?;
        i64::try_from(entry.slow_entry as usize).ok()
    }
    fn bump_ic_epoch(&mut self, image_id: u64) {
        if let Some(epoch) = self.ic_epochs.get_mut(&image_id) {
            *epoch = epoch.saturating_add(1);
        }
    }

    pub(crate) fn with_string_bytes<R>(
        &self,
        encoded: i64,
        f: impl FnOnce(StrView<'_>) -> R,
    ) -> Option<R> {
        if value::is_inline_string(encoded) {
            let mut bytes = [0_u8; value::INLINE_STRING_MAX_LEN];
            return Some(f(StrView::Latin1(value::decode_inline_string(
                encoded, &mut bytes,
            )?)));
        }
        (value::is_runtime_string_handle(encoded) || value::is_bigint(encoded))
            .then(|| value::decode_handle(encoded))
            .and_then(|handle| self.gc.heap().with_string_bytes(handle, f).ok())
    }

    pub(crate) fn with_string_units<R>(
        &self,
        encoded: i64,
        f: impl FnOnce(&[u16]) -> R,
    ) -> Option<R> {
        if value::is_inline_string(encoded) {
            let mut bytes = [0_u8; value::INLINE_STRING_MAX_LEN];
            let bytes = value::decode_inline_string(encoded, &mut bytes)?;
            let mut units = [0_u16; value::INLINE_STRING_MAX_LEN];
            for (unit, byte) in units.iter_mut().zip(bytes.iter().copied()) {
                *unit = u16::from(byte);
            }
            return Some(f(&units[..bytes.len()]));
        }
        (value::is_runtime_string_handle(encoded) || value::is_bigint(encoded))
            .then(|| value::decode_handle(encoded))
            .and_then(|handle| self.gc.heap().with_string_units(handle, f).ok())
    }

    pub(crate) fn string_to_utf8(&self, encoded: i64) -> Option<String> {
        self.with_string_bytes(encoded, |view| view.to_utf8())
            .flatten()
    }

    pub(crate) fn string_to_utf8_lossy(&self, encoded: i64) -> Option<String> {
        self.with_string_bytes(encoded, |view| view.to_utf8_lossy())
    }

    pub(crate) fn string_len(&self, encoded: i64) -> Option<usize> {
        if value::is_inline_string(encoded) {
            return value::inline_string_len(encoded).map(usize::from);
        }
        (value::is_runtime_string_handle(encoded) || value::is_bigint(encoded))
            .then(|| value::decode_handle(encoded))
            .and_then(|handle| self.gc.heap().string_length(handle).ok())
            .map(|len| len as usize)
    }

    pub(crate) fn string_code_unit(&self, encoded: i64, index: usize) -> Option<u16> {
        self.with_string_units(encoded, |units| units.get(index).copied())
            .flatten()
    }

    pub(crate) fn string_owned(&self, encoded: i64) -> Option<RuntimeString> {
        self.with_string_units(encoded, |units| {
            RuntimeString::from_utf16_units(units.to_vec())
        })
    }

    pub(crate) fn string_is_builder(&self, encoded: i64) -> bool {
        value::is_runtime_string_handle(encoded)
            && self
                .gc
                .heap()
                .string_repr(value::decode_handle(encoded))
                .is_ok_and(|repr| repr == wjsm_ir::constants::STRING_REPR_BUILDER)
    }
    fn callable_function(&self, callable: i64) -> Option<NativeFunctionRef> {
        let function_id = if value::is_function(callable) {
            value::decode_function_idx(callable)
        } else if value::is_closure(callable) {
            self.closures
                .get(usize::try_from(value::decode_closure_idx(callable)).ok()?)
                .and_then(|closure| closure.as_ref())?
                .function_id
        } else {
            return None;
        };
        self.functions
            .get(usize::try_from(function_id).ok()?)
            .copied()
    }

    /// 闭包槽是否仍存活（GC 后 tombstone 的槽为 None）。
    fn closure_is_alive(&self, closure: i64) -> bool {
        value::is_closure(closure)
            && self
                .closures
                .get(usize::try_from(value::decode_closure_idx(closure)).unwrap_or(usize::MAX))
                .and_then(|closure| closure.as_ref())
                .is_some()
    }

    fn native_callable(&mut self, kind: NativeCallableKind) -> Option<i64> {
        if let Some(index) = self.native_callable_ids.get(&kind).copied() {
            return Some(value::encode_native_callable_idx(index));
        }
        let index = u32::try_from(self.native_callables.len()).ok()?;
        self.native_callables.push(kind);
        self.native_callable_ids.insert(kind, index);
        Some(value::encode_native_callable_idx(index))
    }

    fn prototype_chain_contains_value(&self, mut current: i64, target: i64) -> bool {
        let mut visited = HashSet::new();
        while visited.insert(current) {
            if current == target {
                return true;
            }
            if value::is_callable(current) {
                let Some(parent) = self.callable_prototypes.get(&current).copied() else {
                    return false;
                };
                current = parent;
                continue;
            }
            if value::is_regexp(current) {
                let Some(parent) = self.regexp_prototype else {
                    return false;
                };
                current = parent;
                continue;
            }
            let Some(handle) = (value::is_object(current) || value::is_array(current))
                .then(|| value::decode_handle(current))
            else {
                return false;
            };
            let Ok(parent) = self.gc.heap().prototype(handle) else {
                return false;
            };
            if parent == PROTO_NULL_SENTINEL {
                return false;
            }
            if parent & 0x8000_0000 != 0 {
                current = value::encode_proxy_handle(parent & 0x7fff_ffff);
            } else if self.gc.heap().object_type(parent).ok()
                == Some(u32::from(wjsm_ir::HEAP_TYPE_ARRAY))
            {
                current = value::encode_handle(value::TAG_ARRAY, parent);
            } else {
                current = value::encode_object_handle(parent);
            }
        }
        false
    }
    fn heap_chain_contains(&self, current: i64, target: i64) -> bool {
        let mut current = value::decode_handle(current);
        let target = value::decode_handle(target);
        let mut visited = HashSet::new();
        while visited.insert(current) {
            if current == target {
                return true;
            }
            let Ok(parent) = self.gc.heap().prototype(current) else {
                return false;
            };
            if parent == PROTO_NULL_SENTINEL {
                return false;
            }
            current = parent;
        }
        false
    }

    fn primitive_property(&mut self, receiver: i64, key: i64) -> Option<i64> {
        if value::is_regexp(receiver)
            && let Some(builtin) = dispatch::regexp::symbol_builtin(key)
        {
            return self.native_callable(NativeCallableKind::Builtin(builtin, true));
        }
        if value::is_symbol(key)
            && value::decode_handle(key) == wjsm_ir::wk_symbol::ASYNC_ITERATOR
            && let Some(callable) = dispatch::streams::async_iterator_property(self, receiver)
        {
            return self.native_callable(NativeCallableKind::Stream(callable));
        }
        if value::is_symbol(key)
            && value::decode_handle(key) == wjsm_ir::wk_symbol::ASYNC_ITERATOR
            && dispatch::async_generator::is_async_generator(self, receiver)
        {
            return self.native_callable(NativeCallableKind::Builtin(
                wjsm_ir::Builtin::ObjectProtoValueOf,
                true,
            ));
        }
        if value::is_symbol(key) && value::decode_handle(key) == wjsm_ir::wk_symbol::ITERATOR {
            let handle = value::decode_handle(receiver);
            let builtin = if dispatch::generator::is_generator(self, receiver)
                || value::is_js_object(receiver)
                    && (self.array_iterators.contains_key(&handle)
                        || self.iterator_next.contains_key(&handle))
            {
                wjsm_ir::Builtin::ObjectProtoValueOf
            } else if value::is_js_object(receiver) && self.maps.contains_key(&handle) {
                wjsm_ir::Builtin::MapSetEntries
            } else if value::is_js_object(receiver) && self.sets.contains_key(&handle) {
                wjsm_ir::Builtin::MapSetValues
            } else if value::is_js_object(receiver) && self.typed_arrays.contains_key(&handle) {
                wjsm_ir::Builtin::TypedArrayProtoValues
            } else if value::is_array(receiver)
                || value::is_string(receiver)
                || value::is_js_object(receiver)
                    && self
                        .gc
                        .heap()
                        .object_type(handle)
                        .is_ok_and(|kind| kind == u32::from(wjsm_ir::HEAP_TYPE_ARGUMENTS))
            {
                wjsm_ir::Builtin::IteratorFrom
            } else {
                return None;
            };
            return self.native_callable(NativeCallableKind::Builtin(builtin, true));
        }
        if value::is_symbol(key)
            && value::decode_handle(key) == wjsm_ir::wk_symbol::TO_STRING_TAG
            && (self.symbol_value(receiver).is_some() || self.symbol_prototype == Some(receiver))
        {
            return self.intern_text("Symbol".into(), value::TAG_STRING);
        }
        let key = self.string_owned(key)?.to_utf8()?;
        if let Some(symbol) = self.symbol_value(receiver) {
            return match key.as_str() {
                "description" => Some(
                    self.symbol_description(symbol)
                        .and_then(|description| {
                            self.intern_runtime_string(description, value::TAG_STRING)
                        })
                        .unwrap_or_else(value::encode_undefined),
                ),
                "toString" => self.native_callable(NativeCallableKind::Builtin(
                    wjsm_ir::Builtin::SymbolProtoToString,
                    true,
                )),
                "valueOf" => self.native_callable(NativeCallableKind::Builtin(
                    wjsm_ir::Builtin::SymbolProtoValueOf,
                    true,
                )),
                _ => None,
            };
        }
        if self.symbol_prototype == Some(receiver) {
            return match key.as_str() {
                "description" => Some(value::encode_undefined()),
                "toString" => self.native_callable(NativeCallableKind::Builtin(
                    wjsm_ir::Builtin::SymbolProtoToString,
                    true,
                )),
                "valueOf" => self.native_callable(NativeCallableKind::Builtin(
                    wjsm_ir::Builtin::SymbolProtoValueOf,
                    true,
                )),
                _ => None,
            };
        }
        if let Some(property) = dispatch::fetch::property(self, receiver, &key) {
            return match property {
                dispatch::fetch::FetchProperty::Callable(callable) => {
                    self.native_callable(NativeCallableKind::Fetch(callable))
                }
                dispatch::fetch::FetchProperty::Value(value) => Some(value),
            };
        }
        if let Some(property) = dispatch::streams::property(self, receiver, &key) {
            return match property {
                dispatch::streams::StreamProperty::Callable(callable) => {
                    self.native_callable(NativeCallableKind::Stream(callable))
                }
                dispatch::streams::StreamProperty::Value(value) => Some(value),
            };
        }
        if value::is_array(receiver) {
            let kind = match key.as_str() {
                "keys" => Some(NativeIteratorKind::Keys),
                "entries" => Some(NativeIteratorKind::Entries),
                _ => None,
            };
            if let Some(kind) = kind {
                return self.native_callable(NativeCallableKind::ArrayIterator(kind));
            }
        }
        if let Some(builtin) = dispatch::async_generator::method(self, receiver, &key) {
            return self.native_callable(NativeCallableKind::Builtin(builtin, true));
        }
        if let Some(builtin) = dispatch::generator::method(self, receiver, &key) {
            return self.native_callable(NativeCallableKind::Builtin(builtin, true));
        }
        if key == "next"
            && let Some(next) = self
                .iterator_next
                .get(&value::decode_handle(receiver))
                .copied()
        {
            return Some(value::encode_native_callable_idx(next));
        }
        if key == "next"
            && !self
                .iterator_next
                .contains_key(&value::decode_handle(receiver))
            && let Some(iterator) = self
                .array_iterators
                .get(&value::decode_handle(receiver))
                .map(|_| value::decode_handle(receiver))
        {
            return self.native_callable(NativeCallableKind::IteratorNext(iterator));
        }
        if let Some(method) = dispatch::date::method(self, receiver, &key) {
            return self.native_callable(NativeCallableKind::DateMethod(method));
        }
        if let Some(property) = dispatch::node_buffer::property(self, receiver, &key) {
            return match property {
                dispatch::node_buffer::BufferProperty::Method(method) => {
                    self.native_callable(NativeCallableKind::BufferMethod(method))
                }
                dispatch::node_buffer::BufferProperty::Value(value) => Some(value),
            };
        }
        if let Some(property) = dispatch::collections::property(self, receiver, &key) {
            return match property {
                dispatch::collections::CollectionProperty::Method(builtin) => {
                    self.native_callable(NativeCallableKind::Builtin(builtin, true))
                }
                dispatch::collections::CollectionProperty::Value(value) => Some(value),
            };
        }
        if let Some(builtin) = dispatch::weak::property(self, receiver, &key) {
            return self.native_callable(NativeCallableKind::Builtin(builtin, true));
        }
        if let Some(builtin) = dispatch::typedarray::typed_array_builtin(self, receiver, &key) {
            return self.native_callable(NativeCallableKind::Builtin(builtin, true));
        }
        if let Some(builtin) = dispatch::buffers::buffer_builtin(self, receiver, &key) {
            return self.native_callable(NativeCallableKind::Builtin(builtin, true));
        }
        if let Some(builtin) = dispatch::sab::sab_builtin(self, receiver, &key) {
            return self.native_callable(NativeCallableKind::Builtin(builtin, true));
        }
        if let Some(builtin) = dispatch::promise::promise_builtin(self, receiver, &key) {
            return self.native_callable(NativeCallableKind::Builtin(builtin, true));
        }
        if let Some(next) = dispatch::collections::iterator_property(self, receiver, &key) {
            return Some(next);
        }
        if value::is_native_callable(receiver)
            && let Some(property) = dispatch::modules::callable_property(self, receiver, &key)
        {
            return Some(property);
        }
        if value::is_native_callable(receiver)
            && self.native_callable_kind(receiver) == Some(NativeCallableKind::ObjectConstructor)
        {
            if key == "prototype" {
                return self.object_prototype;
            }
            let builtin = static_builtin(wjsm_ir::Builtin::ObjectKeys, &key)?;
            return self.native_callable(NativeCallableKind::Builtin(builtin, false));
        }
        if value::is_native_callable(receiver)
            && self.native_callable_kind(receiver) == Some(NativeCallableKind::BufferConstructor)
        {
            let kind = dispatch::node_buffer::constructor_property(&key)?;
            return self.native_callable(kind);
        }
        if value::is_native_callable(receiver) {
            let prototype = match self.native_callable_kind(receiver) {
                Some(NativeCallableKind::ArrayConstructor) => self.array_prototype,
                Some(NativeCallableKind::RealmArrayConstructor(context)) => {
                    dispatch::node_vm::array_prototype_for_handle(self, context)
                }
                _ => None,
            };
            if let Some(prototype) = prototype {
                return match key.as_str() {
                    "prototype" => Some(prototype),
                    "isArray" => self.native_callable(NativeCallableKind::Builtin(
                        wjsm_ir::Builtin::ArrayIsArray,
                        false,
                    )),
                    "from" => self.native_callable(NativeCallableKind::Builtin(
                        wjsm_ir::Builtin::ArrayFrom,
                        false,
                    )),
                    "of" => self.native_callable(NativeCallableKind::Builtin(
                        wjsm_ir::Builtin::ArrayOf,
                        false,
                    )),
                    _ => None,
                };
            }
        }
        if value::is_native_callable(receiver)
            && self.native_callable_kind(receiver) == Some(NativeCallableKind::StringConstructor)
        {
            let builtin = match key.as_str() {
                "fromCharCode" => wjsm_ir::Builtin::StringFromCharCode,
                "fromCodePoint" => wjsm_ir::Builtin::StringFromCodePoint,
                _ => return None,
            };
            return self.native_callable(NativeCallableKind::Builtin(builtin, false));
        }
        if value::is_native_callable(receiver)
            && let Some((owner, _)) = self.native_callable_builtin(receiver)
        {
            if owner == wjsm_ir::Builtin::ObjectKeys && key == "prototype" {
                return self.object_prototype;
            }
            if owner == wjsm_ir::Builtin::SymbolCreate
                && let Some(symbol) = well_known_symbol_property(&key)
            {
                return Some(symbol);
            }
            if let Some(builtin) = static_builtin(owner, &key) {
                return self.native_callable(NativeCallableKind::Builtin(builtin, false));
            }
        }
        if value::is_array(receiver) && key == "toString" {
            return self.native_callable(NativeCallableKind::ArrayToString);
        }
        if value::is_regexp(receiver) && key == "toString" {
            return self.native_callable(NativeCallableKind::RegExpToString);
        }
        if value::is_js_object(receiver)
            && self.error_objects.contains(&value::decode_handle(receiver))
            && key == "toString"
        {
            return self.native_callable(NativeCallableKind::ErrorToString);
        }
        if (value::is_object(receiver) || value::is_array(receiver))
            && matches!(
                key.as_str(),
                "hasOwnProperty" | "propertyIsEnumerable" | "toString" | "valueOf"
            )
            && self
                .object_prototype
                .is_none_or(|prototype| !self.heap_chain_contains(receiver, prototype))
        {
            return None;
        }
        if let Some(property) = dispatch::intl::primitive_locale_property(self, receiver, &key) {
            return Some(property);
        }
        let builtin = intrinsic_builtin(receiver, &key)?;
        self.native_callable(NativeCallableKind::Builtin(builtin, true))
    }
    fn ensure_console_object(&mut self) -> Option<i64> {
        if let Some(console) = self.console_object {
            return Some(console);
        }
        let console = self.allocate_object(6, false).ok()?;
        for (name, builtin) in [
            ("log", wjsm_ir::Builtin::ConsoleLog),
            ("info", wjsm_ir::Builtin::ConsoleInfo),
            ("debug", wjsm_ir::Builtin::ConsoleDebug),
            ("warn", wjsm_ir::Builtin::ConsoleWarn),
            ("error", wjsm_ir::Builtin::ConsoleError),
            ("trace", wjsm_ir::Builtin::ConsoleTrace),
        ] {
            let key = self.intern_property_string(name.into())?;
            let callable = self.native_callable(NativeCallableKind::Builtin(builtin, false))?;
            self.gc
                .heap()
                .set_property(value::decode_handle(console), key, callable as u64)
                .ok()?;
        }
        self.console_object = Some(console);
        Some(console)
    }

    fn ensure_process_object(&mut self) -> Option<i64> {
        if let Some(process) = self.process_object {
            return Some(process);
        }
        let capacity = u32::try_from(self.environment.len()).ok()?;
        let environment = self.allocate_object(capacity, false).ok()?;
        for (name, text) in self.environment.clone() {
            let key = self.intern_property_string(name.into())?;
            let stored = self.intern_text(text, value::TAG_STRING)?;
            self.gc
                .heap()
                .set_property(value::decode_handle(environment), key, stored as u64)
                .ok()?;
        }
        let exec_path = std::env::current_exe()
            .ok()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|| "wjsm".into());
        let mut argv_text = vec![exec_path.clone()];
        if let Some(entry) = self.process_entry.clone() {
            argv_text.push(entry);
        }
        argv_text.extend(self.process_arguments.clone());
        let mut argv_values = Vec::with_capacity(argv_text.len());
        for text in argv_text {
            argv_values.push(self.intern_text(text, value::TAG_STRING)?);
        }
        let argv = self.allocate_array_values(&argv_values).ok()?;
        let exec_argv = self.allocate_object(0, true).ok()?;
        let exec_path = self.intern_text(exec_path, value::TAG_STRING)?;

        let return_this = self.native_callable(NativeCallableKind::ProcessStreamReturnThis)?;
        let stdin = self.allocate_object(2, false).ok()?;
        for name in ["on", "resume"] {
            let key = self.intern_property_string(name.into())?;
            self.gc
                .heap()
                .set_property(value::decode_handle(stdin), key, return_this as u64)
                .ok()?;
        }

        let stdout = self.allocate_object(3, false).ok()?;
        let stderr = self.allocate_object(3, false).ok()?;
        for (stream, is_stderr) in [(stdout, false), (stderr, true)] {
            let write = self.native_callable(NativeCallableKind::ProcessWrite(is_stderr))?;
            let end = self.native_callable(NativeCallableKind::ProcessStreamEnd(is_stderr))?;
            for (name, callable) in [("write", write), ("end", end), ("on", return_this)] {
                let key = self.intern_property_string(name.into())?;
                self.gc
                    .heap()
                    .set_property(value::decode_handle(stream), key, callable as u64)
                    .ok()?;
            }
        }

        let versions = self.allocate_object(2, false).ok()?;
        let node_version = self.intern_text("22.0.0".into(), value::TAG_STRING)?;
        let wjsm_version = self.intern_text(env!("CARGO_PKG_VERSION").into(), value::TAG_STRING)?;
        for (name, stored) in [("node", node_version), ("wjsm", wjsm_version)] {
            let key = self.intern_property_string(name.into())?;
            self.gc
                .heap()
                .set_property(value::decode_handle(versions), key, stored as u64)
                .ok()?;
        }

        let process = self.allocate_object(24, false).ok()?;
        let platform = if cfg!(target_os = "windows") {
            "win32"
        } else if cfg!(target_os = "macos") {
            "darwin"
        } else {
            std::env::consts::OS
        };
        let platform = self.intern_text(platform.into(), value::TAG_STRING)?;
        let version = self.intern_text("v22.0.0".into(), value::TAG_STRING)?;
        let cwd = self.native_callable(NativeCallableKind::ProcessCwd)?;
        let on = self.native_callable(NativeCallableKind::ProcessOn)?;
        let exit = self.native_callable(NativeCallableKind::ProcessExit)?;
        let next_tick = self.native_callable(NativeCallableKind::ProcessNextTick)?;
        let hrtime = self.native_callable(NativeCallableKind::ProcessHrtime)?;
        let hrtime_bigint = self.native_callable(NativeCallableKind::ProcessHrtimeBigInt)?;
        let bigint_key = self.intern_property_string("bigint".into())?;
        self.callable_properties
            .insert((hrtime, bigint_key), hrtime_bigint);
        let uptime = self.native_callable(NativeCallableKind::ProcessUptime)?;
        let memory_usage = self.native_callable(NativeCallableKind::ProcessMemoryUsage)?;
        let cpu_usage = self.native_callable(NativeCallableKind::ProcessCpuUsage)?;
        let process_send = self.native_callable(NativeCallableKind::NodeChildProcess(
            dispatch::node_child_process::NodeChildProcessCallable::ProcessSend,
        ))?;
        let process_disconnect = self.native_callable(NativeCallableKind::NodeChildProcess(
            dispatch::node_child_process::NodeChildProcessCallable::ProcessDisconnect,
        ))?;
        let process_connected = value::encode_bool(self.node_child_process.process_connected());
        let packed = value::encode_bool(self.runtime_modules.store.is_snapshot());
        let pid = value::encode_f64(f64::from(std::process::id()));
        #[cfg(unix)]
        let ppid = {
            // SAFETY: getppid 无参数、无前置条件，仅查询当前进程的父进程 id。
            value::encode_f64(f64::from(unsafe { libc::getppid() }))
        };
        #[cfg(not(unix))]
        let ppid = value::encode_f64(0.0);
        for (name, stored) in [
            ("env", environment),
            ("platform", platform),
            ("argv", argv),
            ("execArgv", exec_argv),
            ("execPath", exec_path),
            ("stdin", stdin),
            ("stdout", stdout),
            ("stderr", stderr),
            ("cwd", cwd),
            ("exit", exit),
            ("on", on),
            ("nextTick", next_tick),
            ("pid", pid),
            ("ppid", ppid),
            ("version", version),
            ("versions", versions),
            ("hrtime", hrtime),
            ("uptime", uptime),
            ("memoryUsage", memory_usage),
            ("cpuUsage", cpu_usage),
            ("send", process_send),
            ("disconnect", process_disconnect),
            ("connected", process_connected),
            ("__wjsm_packed", packed),
        ] {
            let key = self.intern_property_string(name.into())?;
            self.gc
                .heap()
                .set_property(value::decode_handle(process), key, stored as u64)
                .ok()?;
        }
        self.process_env_object = Some(environment);
        self.process_object = Some(process);
        Some(process)
    }

    fn ensure_intrinsic_prototypes(&mut self) -> Result<(), HeapAccessV2Error> {
        let object_prototype = if let Some(prototype) = self.object_prototype {
            prototype
        } else {
            let prototype = self.allocate_object_with_prototype(0, false, PROTO_NULL_SENTINEL)?;
            self.object_prototype = Some(prototype);
            prototype
        };
        if self.array_prototype.is_none() {
            let prototype = self.allocate_object_with_prototype(
                0,
                true,
                value::decode_handle(object_prototype),
            )?;
            dispatch::intl::install_array_to_locale_string(self, prototype)
                .map_err(|()| HeapAccessV2Error::AddressOverflow)?;
            self.array_prototype = Some(prototype);
            let constructor = self
                .native_callable(NativeCallableKind::ArrayConstructor)
                .ok_or(HeapAccessV2Error::AddressOverflow)?;
            let prototype_key = self
                .intern_property_string("prototype".into())
                .ok_or(HeapAccessV2Error::AddressOverflow)?;
            self.callable_properties
                .insert((constructor, prototype_key), prototype);
            self.callable_property_flags
                .insert((constructor, prototype_key), FUNCTION_PROTOTYPE_FLAGS);
            self.install_prototype_constructor(prototype, constructor)
                .ok_or(HeapAccessV2Error::AddressOverflow)?;
            self.array_constructor = Some(constructor);
        }
        if self.regexp_prototype.is_none() {
            let prototype = self.allocate_object_with_prototype(
                0,
                false,
                value::decode_handle(object_prototype),
            )?;
            self.regexp_prototype = Some(prototype);
        }
        Ok(())
    }

    fn ensure_object_constructor(&mut self) -> Option<i64> {
        self.ensure_intrinsic_prototypes().ok()?;
        let constructor = self.native_callable(NativeCallableKind::ObjectConstructor)?;
        let prototype_key = self.intern_property_string("prototype".into())?;
        self.callable_properties
            .insert((constructor, prototype_key), self.object_prototype?);
        self.callable_property_flags
            .insert((constructor, prototype_key), FUNCTION_PROTOTYPE_FLAGS);
        self.install_prototype_constructor(self.object_prototype?, constructor)?;
        Some(constructor)
    }

    fn ensure_regexp_constructor(&mut self) -> Option<i64> {
        self.ensure_intrinsic_prototypes().ok()?;
        let constructor = self.native_callable(NativeCallableKind::Builtin(
            wjsm_ir::Builtin::RegExpCreate,
            false,
        ))?;
        let prototype_key = self.intern_property_string("prototype".into())?;
        self.callable_properties
            .insert((constructor, prototype_key), self.regexp_prototype?);
        self.callable_property_flags
            .insert((constructor, prototype_key), FUNCTION_PROTOTYPE_FLAGS);
        self.install_prototype_constructor(self.regexp_prototype?, constructor)?;
        Some(constructor)
    }

    fn ensure_array_constructor(&mut self) -> Option<i64> {
        if let Some(constructor) = self.array_constructor {
            return Some(constructor);
        }
        self.ensure_intrinsic_prototypes().ok()?;
        let prototype = self.array_prototype?;
        let constructor = self.native_callable(NativeCallableKind::ArrayConstructor)?;
        let prototype_key = self.intern_property_string("prototype".into())?;
        self.callable_properties
            .insert((constructor, prototype_key), prototype);
        self.callable_property_flags
            .insert((constructor, prototype_key), FUNCTION_PROTOTYPE_FLAGS);
        self.array_prototype = Some(prototype);
        self.array_constructor = Some(constructor);
        self.install_prototype_constructor(prototype, constructor)?;
        Some(constructor)
    }

    fn ensure_error_prototype(&mut self, name: &str) -> Option<i64> {
        if let Some(prototype) = self.error_prototypes.get(name).copied() {
            return Some(prototype);
        }
        let constructor_kind = match name {
            "AggregateError" => NativeCallableKind::AggregateErrorConstructor,
            "DataCloneError" => {
                NativeCallableKind::Builtin(wjsm_ir::Builtin::ErrorConstructor, false)
            }
            "Error" => NativeCallableKind::Builtin(wjsm_ir::Builtin::ErrorConstructor, false),
            "EvalError" => {
                NativeCallableKind::Builtin(wjsm_ir::Builtin::EvalErrorConstructor, false)
            }
            "RangeError" => {
                NativeCallableKind::Builtin(wjsm_ir::Builtin::RangeErrorConstructor, false)
            }
            "ReferenceError" => {
                NativeCallableKind::Builtin(wjsm_ir::Builtin::ReferenceErrorConstructor, false)
            }
            "SyntaxError" => {
                NativeCallableKind::Builtin(wjsm_ir::Builtin::SyntaxErrorConstructor, false)
            }
            "TypeError" => {
                NativeCallableKind::Builtin(wjsm_ir::Builtin::TypeErrorConstructor, false)
            }
            "URIError" => NativeCallableKind::Builtin(wjsm_ir::Builtin::URIErrorConstructor, false),
            _ => return None,
        };
        let parent = if name == "Error" {
            None
        } else {
            Some(self.ensure_error_prototype("Error")?)
        };
        let prototype = self.allocate_object(2, false).ok()?;
        if let Some(parent) = parent {
            self.gc
                .heap()
                .set_prototype(
                    value::decode_handle(prototype),
                    value::decode_handle(parent),
                )
                .ok()?;
        }
        let name_value = self.intern_text(name.into(), value::TAG_STRING)?;
        let message = self.intern_text(String::new(), value::TAG_STRING)?;
        for (key, stored) in [("name", name_value), ("message", message)] {
            let key = self.intern_property_string(key.into())?;
            self.gc
                .heap()
                .set_property(value::decode_handle(prototype), key, stored as u64)
                .ok()?;
        }
        let constructor = self.native_callable(constructor_kind)?;
        let prototype_key = self.intern_property_string("prototype".into())?;
        self.callable_properties
            .insert((constructor, prototype_key), prototype);
        self.callable_property_flags
            .insert((constructor, prototype_key), FUNCTION_PROTOTYPE_FLAGS);
        self.error_prototypes.insert(name.to_owned(), prototype);
        self.install_prototype_constructor(prototype, constructor)?;
        Some(prototype)
    }

    fn install_prototype_constructor(&mut self, prototype: i64, constructor: i64) -> Option<()> {
        let key = self.intern_property_string("constructor".into())?;
        let flags =
            wjsm_ir::constants::FLAG_WRITABLE as u32 | wjsm_ir::constants::FLAG_CONFIGURABLE as u32;
        if value::is_array(prototype) {
            let handle = value::decode_handle(prototype);
            self.note_array_property(handle, key);
            self.array_properties.insert((handle, key), constructor);
            self.array_property_flags.insert((handle, key), flags);
            return Some(());
        }
        self.gc
            .heap()
            .define_data_property(
                value::decode_handle(prototype),
                key,
                constructor as u64,
                flags,
            )
            .ok()
    }
    fn global_property(&mut self, receiver: i64, key: i64) -> Option<i64> {
        let is_realm_global =
            self.global_object == Some(receiver) || dispatch::node_vm::is_context(self, receiver);
        if !is_realm_global {
            return None;
        }
        let name = self.string_owned(key)?.to_utf8()?;
        if matches!(name.as_str(), "globalThis" | "global") {
            return Some(receiver);
        }

        if name == "__wjsm_inspector_url" {
            let url = self
                .inspector
                .as_ref()
                .map(|inspector| inspector.url().to_owned())?;
            return self.intern_text(url, value::TAG_STRING);
        }
        if name == "console" {
            return self.ensure_console_object();
        }
        if name == "Intl" {
            return dispatch::intl::ensure_intl_object(self);
        }
        if name == "process" {
            return self.ensure_process_object();
        }
        if name == "Buffer" {
            return self.native_callable(NativeCallableKind::BufferConstructor);
        }
        if name == "__wjsm_node_net" {
            return dispatch::node_net::ensure_bridge(self);
        }
        if name == "__wjsm_node_tls" {
            return dispatch::node_tls::ensure_bridge(self);
        }
        if name == "__wjsm_node_zlib" {
            return dispatch::node_zlib::ensure_bridge(self);
        }
        if name == "__wjsm_node_async_hooks" {
            return dispatch::node_async_hooks::ensure_bridge(self);
        }
        if name == "gc" {
            return self.native_callable(NativeCallableKind::Gc);
        }
        if name == "setImmediate" {
            return self.native_callable(NativeCallableKind::SetImmediate);
        }
        if name == "clearImmediate" {
            return self.native_callable(NativeCallableKind::Builtin(
                wjsm_ir::Builtin::ClearTimeout,
                false,
            ));
        }
        if name == "__wjsm_node_crypto" {
            return dispatch::node_crypto::ensure_bridge(self);
        }
        if name == "__wjsm_node_dgram" {
            return dispatch::node_dgram::ensure_bridge(self);
        }
        if name == "__wjsm_node_fs" {
            return dispatch::node_fs::ensure_bridge(self);
        }
        if name == "__wjsm_node_buffer" {
            return dispatch::node_buffer::ensure_bridge(self);
        }
        if name == "__wjsm_idna" {
            return dispatch::idna::ensure_bridge(self);
        }
        if name == "__wjsm_node_os" {
            return dispatch::node_os::ensure_bridge(self);
        }
        if name == "__wjsm_node_perf_hooks" {
            return dispatch::node_perf_hooks::ensure_bridge(self);
        }
        if name == "performance" {
            return dispatch::node_perf_hooks::ensure_performance(self);
        }
        if name == "__wjsm_node_worker_threads" {
            return dispatch::node_worker_threads::ensure_bridge(self);
        }
        if name == "__wjsm_node_child_process" {
            return dispatch::node_child_process::ensure_bridge(self);
        }
        if name == "$262" {
            return dispatch::agent::ensure_bridge(self);
        }
        if name == "__wjsm_node_vm" {
            return dispatch::node_vm::ensure_bridge(self);
        }
        if name == "Object" {
            return self.ensure_object_constructor();
        }
        if name == "JSON" {
            return self.native_callable(NativeCallableKind::Builtin(
                wjsm_ir::Builtin::JsonStringify,
                false,
            ));
        }
        if name == "Math" {
            return self.native_callable(NativeCallableKind::Builtin(
                wjsm_ir::Builtin::MathAbs,
                false,
            ));
        }
        if name == "Function" {
            return self.native_callable(NativeCallableKind::FunctionConstructor);
        }
        if name == "eval" {
            return self.native_callable(NativeCallableKind::Builtin(
                wjsm_ir::Builtin::EvalIndirect,
                false,
            ));
        }
        if name == "String" {
            return self.native_callable(NativeCallableKind::StringConstructor);
        }
        if name == "Array"
            && let Some(constructor) =
                dispatch::node_vm::array_constructor_for_context(self, receiver)
        {
            return Some(constructor);
        }
        if let Some(callable) = match name.as_str() {
            "TextDecoder" => {
                Some(dispatch::web_encoding::WebEncodingCallable::TextDecoderConstructor)
            }
            "TextEncoder" => {
                Some(dispatch::web_encoding::WebEncodingCallable::TextEncoderConstructor)
            }
            "atob" => Some(dispatch::web_encoding::WebEncodingCallable::Atob),
            "btoa" => Some(dispatch::web_encoding::WebEncodingCallable::Btoa),
            _ => None,
        } {
            return self.native_callable(NativeCallableKind::WebEncoding(callable));
        }
        let standalone = match name.as_str() {
            "setTimeout" => Some(wjsm_ir::Builtin::SetTimeout),
            "clearTimeout" => Some(wjsm_ir::Builtin::ClearTimeout),
            "setInterval" => Some(wjsm_ir::Builtin::SetInterval),
            "clearInterval" => Some(wjsm_ir::Builtin::ClearInterval),
            "fetch" => Some(wjsm_ir::Builtin::Fetch),
            "parseInt" => Some(wjsm_ir::Builtin::NumberParseInt),
            "parseFloat" => Some(wjsm_ir::Builtin::NumberParseFloat),
            "isNaN" => Some(wjsm_ir::Builtin::GlobalIsNaN),
            "isFinite" => Some(wjsm_ir::Builtin::GlobalIsFinite),
            "structuredClone" => Some(wjsm_ir::Builtin::StructuredClone),
            _ => None,
        };
        if let Some(builtin) = standalone {
            return self.native_callable(NativeCallableKind::Builtin(builtin, false));
        }
        let builtin = match name.as_str() {
            "AggregateError" => {
                return self.native_callable(NativeCallableKind::AggregateErrorConstructor);
            }
            "Array" => return self.ensure_array_constructor(),
            "ArrayBuffer" => wjsm_ir::Builtin::ArrayBufferConstructor,
            "BigInt" => wjsm_ir::Builtin::BigIntFromLiteral,
            "BigInt64Array" => wjsm_ir::Builtin::BigInt64ArrayConstructor,
            "BigUint64Array" => wjsm_ir::Builtin::BigUint64ArrayConstructor,
            "Boolean" => wjsm_ir::Builtin::BooleanConstructor,
            "Date" => wjsm_ir::Builtin::DateConstructor,
            "DataView" => wjsm_ir::Builtin::DataViewConstructor,
            "Error" => wjsm_ir::Builtin::ErrorConstructor,
            "EvalError" => wjsm_ir::Builtin::EvalErrorConstructor,
            "Headers" => wjsm_ir::Builtin::HeadersConstructor,
            "Float32Array" => wjsm_ir::Builtin::Float32ArrayConstructor,
            "Float64Array" => wjsm_ir::Builtin::Float64ArrayConstructor,
            "Int16Array" => wjsm_ir::Builtin::Int16ArrayConstructor,
            "Int32Array" => wjsm_ir::Builtin::Int32ArrayConstructor,
            "Int8Array" => wjsm_ir::Builtin::Int8ArrayConstructor,
            "Map" => wjsm_ir::Builtin::MapConstructor,
            "Number" => wjsm_ir::Builtin::NumberConstructor,
            "Promise" => wjsm_ir::Builtin::PromiseCreate,
            "Proxy" => wjsm_ir::Builtin::ProxyCreate,
            "WeakMap" => wjsm_ir::Builtin::WeakMapConstructor,
            "WeakSet" => wjsm_ir::Builtin::WeakSetConstructor,
            "WeakRef" => wjsm_ir::Builtin::WeakRefConstructor,
            "FinalizationRegistry" => wjsm_ir::Builtin::FinalizationRegistryConstructor,
            "Request" => wjsm_ir::Builtin::RequestConstructor,
            "Response" => wjsm_ir::Builtin::ResponseConstructor,
            "queueMicrotask" => wjsm_ir::Builtin::QueueMicrotask,
            "RangeError" => wjsm_ir::Builtin::RangeErrorConstructor,
            "ReferenceError" => wjsm_ir::Builtin::ReferenceErrorConstructor,
            "Reflect" => wjsm_ir::Builtin::ReflectGet,
            "RegExp" => return self.ensure_regexp_constructor(),
            "Set" => wjsm_ir::Builtin::SetConstructor,
            "Symbol" => wjsm_ir::Builtin::SymbolCreate,
            "SyntaxError" => wjsm_ir::Builtin::SyntaxErrorConstructor,
            "TypeError" => wjsm_ir::Builtin::TypeErrorConstructor,
            "Uint16Array" => wjsm_ir::Builtin::Uint16ArrayConstructor,
            "Uint32Array" => wjsm_ir::Builtin::Uint32ArrayConstructor,
            "Uint8Array" => wjsm_ir::Builtin::Uint8ArrayConstructor,
            "Uint8ClampedArray" => wjsm_ir::Builtin::Uint8ClampedArrayConstructor,
            "URIError" => wjsm_ir::Builtin::URIErrorConstructor,
            _ => return None,
        };
        self.native_callable(NativeCallableKind::Builtin(builtin, false))
    }

    fn native_callable_builtin(&self, callee: i64) -> Option<(wjsm_ir::Builtin, bool)> {
        if !value::is_native_callable(callee) {
            return None;
        }
        match self
            .native_callables
            .get(usize::try_from(value::decode_native_callable_idx(callee)).ok()?)?
        {
            NativeCallableKind::Builtin(builtin, with_receiver) => Some((*builtin, *with_receiver)),
            NativeCallableKind::DateMethod(_)
            | NativeCallableKind::ArrayConstructor
            | NativeCallableKind::ObjectConstructor
            | NativeCallableKind::ArrayToString
            | NativeCallableKind::RealmArrayConstructor(_)
            | NativeCallableKind::ArrayIterator(_)
            | NativeCallableKind::ArgumentsStrictCallee
            | NativeCallableKind::BufferConstructor
            | NativeCallableKind::BufferMethod(_)
            | NativeCallableKind::BufferStatic(_)
            | NativeCallableKind::BufferTranscode
            | NativeCallableKind::CjsRequire(_)
            | NativeCallableKind::CjsResolve(_)
            | NativeCallableKind::CjsResolvePaths(_)
            | NativeCallableKind::ImportMetaResolve(_)
            | NativeCallableKind::CollectionNext(_)
            | NativeCallableKind::IteratorNext(_)
            | NativeCallableKind::FunctionConstructor
            | NativeCallableKind::FunctionPrototype
            | NativeCallableKind::NodeNet(_)
            | NativeCallableKind::NodeTls(_)
            | NativeCallableKind::NodeCrypto(_)
            | NativeCallableKind::NodeZlib(_)
            | NativeCallableKind::NodeDgram(_)
            | NativeCallableKind::NodeAsyncHooks(_)
            | NativeCallableKind::NodeFs(_)
            | NativeCallableKind::NodeOs(_)
            | NativeCallableKind::Idna(_)
            | NativeCallableKind::NodeVm(_)
            | NativeCallableKind::NodeChildProcess(_)
            | NativeCallableKind::ErrorToString
            | NativeCallableKind::AggregateErrorConstructor
            | NativeCallableKind::NodePerfHooks(_)
            | NativeCallableKind::NodeWorkerThreads(_)
            | NativeCallableKind::Test262Agent(_)
            | NativeCallableKind::PromiseResolve(_)
            | NativeCallableKind::PromiseReject(_)
            | NativeCallableKind::ProxyRevoke(_)
            | NativeCallableKind::ProxyCall(_)
            | NativeCallableKind::ProxyConstruct(_)
            | NativeCallableKind::ProcessExit
            | NativeCallableKind::ProcessWrite(_)
            | NativeCallableKind::ProcessStreamEnd(_)
            | NativeCallableKind::ProcessStreamReturnThis
            | NativeCallableKind::ProcessHrtime
            | NativeCallableKind::ProcessHrtimeBigInt
            | NativeCallableKind::ProcessUptime
            | NativeCallableKind::ProcessMemoryUsage
            | NativeCallableKind::ProcessCpuUsage
            | NativeCallableKind::StringConstructor
            | NativeCallableKind::RegExpToString
            | NativeCallableKind::RegExpIteratorNext(_)
            | NativeCallableKind::ProcessCwd
            | NativeCallableKind::Stream(_)
            | NativeCallableKind::WebEncoding(_)
            | NativeCallableKind::Fetch(_)
            | NativeCallableKind::ProcessNextTick
            | NativeCallableKind::ProcessOn
            | NativeCallableKind::Gc
            | NativeCallableKind::SetImmediate
            | NativeCallableKind::TimerConstructor(_)
            | NativeCallableKind::Bound(_)
            | NativeCallableKind::Intl(_) => None,
        }
    }

    fn native_callable_kind(&self, callee: i64) -> Option<NativeCallableKind> {
        if !value::is_native_callable(callee) {
            return None;
        }
        self.native_callables
            .get(usize::try_from(value::decode_native_callable_idx(callee)).ok()?)
            .copied()
    }

    fn is_callable_value(&self, value: i64) -> bool {
        if !value::is_proxy(value) {
            return value::is_callable(value);
        }
        self.proxies
            .get(value::decode_proxy_handle(value) as usize)
            .and_then(|proxy| proxy.as_ref())
            .is_some_and(|proxy| !proxy.revoked && self.is_callable_value(proxy.target))
    }

    fn validated_feedback_slot(&self, pointer: *mut u8) -> Option<ValidatedFeedbackSlot> {
        if pointer.is_null() {
            return None;
        }
        let (base_address, byte_len) = self.current_feedback_region;
        if base_address == 0 || byte_len == 0 {
            return None;
        }
        let slot_size = wjsm_ir::constants::FEEDBACK_SLOT_SIZE as usize;
        let pointer_address = pointer.addr();
        let offset = pointer_address.checked_sub(base_address)?;
        if offset >= byte_len || offset % slot_size != 0 {
            return None;
        }
        let site_index = u32::try_from(offset / slot_size).ok()?;
        Some(ValidatedFeedbackSlot::new(
            pointer.cast(),
            self.current_image_id,
            site_index,
        ))
    }

    /// 直接算出反馈签名。
    ///
    /// tag 序列的唯一消费者是 [`encode_feedback_tag_signature`]，先物化成
    /// `Box<[_]>` 会让每一次带反馈的宿主调用都做一次堆分配与释放；上界
    /// [`wjsm_ir::constants::FEEDBACK_MAX_TAGS`] 是编译期常量，栈数组即可。
    fn feedback_tag_signature(arguments: &[i64]) -> Option<u64> {
        const MAX_TAGS: usize = wjsm_ir::constants::FEEDBACK_MAX_TAGS as usize;
        if arguments.len() > MAX_TAGS {
            return None;
        }
        let mut tags = [NativeFeedbackTag::Undefined; MAX_TAGS];
        for (slot, argument) in tags.iter_mut().zip(arguments) {
            let tag = NativeFeedbackTag::of(*argument);
            if matches!(
                tag,
                NativeFeedbackTag::Exception
                    | NativeFeedbackTag::Iterator
                    | NativeFeedbackTag::Enumerator
                    | NativeFeedbackTag::ScopeRecord
                    | NativeFeedbackTag::ArrayHole
                    | NativeFeedbackTag::Other
            ) {
                return None;
            }
            *slot = tag;
        }
        Some(encode_feedback_tag_signature(&tags[..arguments.len()]))
    }

    fn load_feedback_slot(slot: ValidatedFeedbackSlot) -> NativeFeedbackSlot {
        // SAFETY: `ValidatedFeedbackSlot` 只能由当前 image 反馈区的范围与槽边界校验创建；
        // 使用 unaligned 访问，不依赖 `Box<[u8]>` 的静态对齐类型。
        unsafe { slot.slot().read_unaligned() }
    }

    fn store_feedback_slot(slot: ValidatedFeedbackSlot, value: NativeFeedbackSlot) {
        // SAFETY: 与 `load_feedback_slot` 相同，且 owner thread 是反馈区唯一写入者。
        unsafe { slot.slot().write_unaligned(value) };
    }

    fn record_value_feedback(
        &mut self,
        feedback: ValidatedFeedbackSlot,
        operation: u32,
        arguments: &[i64],
    ) {
        let Some(signature) = Self::feedback_tag_signature(arguments) else {
            let mut slot = Self::load_feedback_slot(feedback);
            slot.state = wjsm_ir::constants::FEEDBACK_STATE_DISABLED;
            Self::store_feedback_slot(feedback, slot);
            return;
        };
        let mut slot = Self::load_feedback_slot(feedback);
        let same = slot.last_target_image_id == 0 && slot.last_tag_signature == signature;
        slot.last_target_image_id = 0;
        slot.last_target_function = 0;
        slot.last_tag_signature = signature;
        slot.consecutive_count = if same {
            slot.consecutive_count.saturating_add(1)
        } else {
            1
        };
        slot.total_count = slot.total_count.saturating_add(1);
        slot.caller_function = self
            .activations
            .last()
            .and_then(|activation| activation.function)
            .map_or(0, |function| function.function_index);
        slot.site_index = feedback.site_index;
        slot.operation = operation;
        slot.state = wjsm_ir::constants::FEEDBACK_STATE_RECORDING;
        Self::store_feedback_slot(feedback, slot);
    }

    fn drain_specialization_results(&mut self) {
        // 绝大多数调用没有待收敛结果；先用一次原子读短路，避免搬动整个协调器。
        if !self
            .specialization
            .as_ref()
            .is_some_and(SpecializationCoordinator::has_results)
        {
            return;
        }
        let Some(mut coordinator) = self.specialization.take() else {
            return;
        };
        for result in coordinator.drain_results() {
            let request = result.request;
            let Some(object) = result.object else {
                continue;
            };
            let Some(program) = self.program_snapshots.get(&request.key.target_image_id) else {
                continue;
            };
            let Some(variable_slots) = self
                .variable_slot_snapshots
                .get(&request.key.target_image_id)
            else {
                continue;
            };
            if !Arc::ptr_eq(program, &request.program)
                || !Arc::ptr_eq(variable_slots, &request.variable_slots)
                || !self.images.contains_key(&request.key.caller_image_id)
                || !self.images.contains_key(&request.key.target_image_id)
                || self
                    .ic_epochs
                    .get(&request.key.target_image_id)
                    .copied()
                    .unwrap_or(0)
                    != request.ic_epoch
                || u64::from(self.gc.heap().shapes().proto_generation()) != request.proto_generation
            {
                continue;
            }
            let image_id = coordinator.next_image_id();
            let symbol = format!("wjsm_function_{}", request.key.target_function);
            let Ok(image) = CompiledImage::load_single_entry(
                &object,
                image_id,
                request.key.target_function,
                &symbol,
                &NativeHostRegistry,
            ) else {
                continue;
            };
            coordinator.publish(
                request.key,
                image,
                request.ic_epoch,
                request.proto_generation,
            );
        }
        self.specialization = Some(coordinator);
    }

    fn select_specialized_entry(
        &mut self,
        feedback: ValidatedFeedbackSlot,
        function: NativeFunctionRef,
        arguments: &[i64],
        generic_entry: i64,
    ) -> Option<i64> {
        let Some(tag_signature) = Self::feedback_tag_signature(arguments) else {
            let mut slot = Self::load_feedback_slot(feedback);
            slot.state = wjsm_ir::constants::FEEDBACK_STATE_DISABLED;
            Self::store_feedback_slot(feedback, slot);
            return Some(generic_entry);
        };
        let key = VariantKey {
            caller_image_id: feedback.caller_image_id,
            site_index: feedback.site_index,
            target_image_id: function.image_id,
            target_function: function.function_index,
            tag_signature,
        };
        let mut slot = Self::load_feedback_slot(feedback);
        let same = slot.last_target_image_id == function.image_id
            && slot.last_target_function == function.function_index
            && slot.last_tag_signature == tag_signature;
        slot.last_target_image_id = function.image_id;
        slot.last_target_function = function.function_index;
        slot.last_tag_signature = tag_signature;
        slot.consecutive_count = if same {
            slot.consecutive_count.saturating_add(1)
        } else {
            1
        };
        slot.total_count = slot.total_count.saturating_add(1);
        slot.caller_function = self
            .activations
            .iter()
            .rev()
            .nth(1)
            .and_then(|activation| activation.function)
            .map_or(0, |caller| caller.function_index);
        slot.site_index = feedback.site_index;
        slot.operation = NativeRuntimeOp::PrepareCall.id();
        slot.state = wjsm_ir::constants::FEEDBACK_STATE_RECORDING;
        Self::store_feedback_slot(feedback, slot);

        let ic_epoch = self.ic_epochs.get(&function.image_id).copied().unwrap_or(0);
        let proto_generation = u64::from(self.gc.heap().shapes().proto_generation());
        if let Some(image) = self
            .specialization
            .as_mut()
            .and_then(|coordinator| coordinator.select(key, ic_epoch, proto_generation))
        {
            let entry = image.entries().first()?;
            self.activations.last_mut()?.specialized_image = Some(Arc::clone(&image));
            return i64::try_from(entry.slow_entry as usize).ok();
        }
        if slot.consecutive_count >= wjsm_ir::constants::FEEDBACK_STABLE_THRESHOLD {
            let program = Arc::clone(self.program_snapshots.get(&function.image_id)?);
            let variable_slots = Arc::clone(self.variable_slot_snapshots.get(&function.image_id)?);
            self.specialization.as_mut()?.enqueue(CompilationRequest {
                key,
                program,
                variable_slots,
                // 冷路径才物化 tag 数组：签名已校验过全部 tag 合法且数量在上界内。
                argument_tags: arguments
                    .iter()
                    .copied()
                    .map(NativeFeedbackTag::of)
                    .collect(),
                ic_epoch,
                proto_generation,
            });
        }
        Some(generic_entry)
    }

    fn prepare_call(
        &mut self,
        ctx: &mut NativeVmContext,
        args: &[i64],
        construct: bool,
        feedback_slot: Option<ValidatedFeedbackSlot>,
    ) -> Option<i64> {
        let (&callee, arguments) = args.split_first()?;
        let (function, environment, entry) = if value::is_function(callee) {
            let function = self.callable_function(callee)?;
            let entry = self.compiled_entry(function)?;
            (Some(function), value::encode_undefined(), entry)
        } else if value::is_closure(callee) {
            let closure = *self
                .closures
                .get(usize::try_from(value::decode_closure_idx(callee)).ok()?)
                .and_then(|closure| closure.as_ref())?;
            let function = self.callable_function(callee)?;
            let entry = self.compiled_entry(function)?;
            (Some(function), closure.environment, entry)
        } else if value::is_proxy(callee) {
            let proxy = self
                .proxies
                .get(usize::try_from(value::decode_proxy_handle(callee)).ok()?)
                .and_then(|proxy| proxy.as_ref())?;
            if !self.is_callable_value(proxy.target) {
                return None;
            }
            let proxy_id = value::decode_proxy_handle(callee);
            let kind = if construct {
                NativeCallableKind::ProxyConstruct(proxy_id)
            } else {
                NativeCallableKind::ProxyCall(proxy_id)
            };
            let callable = self.native_callable(kind)?;
            (
                None,
                callable,
                i64::try_from(native_callable_call as *const () as usize).ok()?,
            )
        } else if value::is_native_callable(callee) {
            let kind = self
                .native_callables
                .get(usize::try_from(value::decode_native_callable_idx(callee)).ok()?)
                .copied()?;
            if construct
                && match kind {
                    NativeCallableKind::Intl(kind) => !dispatch::intl::is_constructor(kind),
                    NativeCallableKind::DateMethod(_) => true,
                    _ => false,
                }
            {
                return None;
            }
            (
                None,
                callee,
                i64::try_from(native_callable_call as *const () as usize).ok()?,
            )
        } else {
            return None;
        };
        if ctx.js_call_depth >= MAX_JS_CALL_DEPTH {
            return None;
        }
        let active_len = ctx.call_arena_active_len;
        let argument_count = u32::try_from(arguments.len()).ok()?;
        let end = active_len.checked_add(argument_count)?;
        if end > ctx.call_arena_capacity {
            ctx.pending_exception_kind = PendingExceptionKind::CallArenaOverflow;
            return None;
        }
        let base = usize::try_from(active_len).ok()?;
        self.call_arena
            .get_mut(base..base + arguments.len())?
            .copy_from_slice(arguments);
        let caller_image_id = self.current_image_id;
        if let Some(function) = function {
            self.activate_image(ctx, function.image_id)?;
        }
        let saved_variables = if let Some(function) = function {
            self.function_slots
                .get(usize::try_from(function.function_index).ok()?)?
                .iter()
                .map(|slot| Some((*slot, *self.variables.get(*slot)?)))
                .collect::<Option<Vec<_>>>()?
        } else {
            Vec::new()
        };
        self.activations.push(NativeActivation {
            active_len,
            argument_count,
            saved_variables,
            environment,
            caller_image_id,
            new_target: if construct {
                callee
            } else {
                value::encode_undefined()
            },
            home_object: function.and_then(|function| function.home_object),
            function,
            specialized_image: None,
        });
        ctx.call_arena_active_len = end;
        ctx.js_call_depth += 1;
        if let (Some(slot), Some(function)) = (feedback_slot, function) {
            // 反馈记录与 overlay 选择只影响返回的 entry 地址；激活/变量保存协议
            // 已经完成，overlay 入口失配时 wrapper 自行回落 base entry。
            return self.select_specialized_entry(slot, function, arguments, entry);
        }
        Some(entry)
    }
    fn prepare_super_call(
        &mut self,
        ctx: &mut NativeVmContext,
        args: &[i64],
        forward_args: bool,
        feedback_slot: Option<ValidatedFeedbackSlot>,
    ) -> Option<i64> {
        let activation = self.activations.last()?;
        let new_target = activation.new_target;
        let prepared = if forward_args {
            let start = usize::try_from(activation.active_len).ok()?;
            let count = usize::try_from(activation.argument_count).ok()?;
            let end = start.checked_add(count)?;
            let mut prepared = Vec::with_capacity(count + 1);
            prepared.push(*args.first()?);
            prepared.extend_from_slice(self.call_arena.get(start..end)?);
            prepared
        } else {
            args.to_vec()
        };
        let entry = self.prepare_call(ctx, &prepared, true, feedback_slot)?;
        self.activations.last_mut()?.new_target = new_target;
        Some(entry)
    }

    fn push_entry_activation(&mut self, ctx: &mut NativeVmContext, caller_image_id: u64) {
        self.activations.push(NativeActivation {
            active_len: ctx.call_arena_active_len,
            argument_count: 0,
            saved_variables: Vec::new(),
            environment: value::encode_undefined(),
            caller_image_id,
            new_target: value::encode_undefined(),
            home_object: None,
            function: None,
            specialized_image: None,
        });
        ctx.js_call_depth += 1;
    }

    fn prepare_entry_call(
        &mut self,
        ctx: &mut NativeVmContext,
        caller_image_id: u64,
    ) -> Option<()> {
        if ctx.js_call_depth >= MAX_JS_CALL_DEPTH {
            return None;
        }
        self.push_entry_activation(ctx, caller_image_id);
        Some(())
    }
    fn prepare_rejected_call(
        &mut self,
        ctx: &mut NativeVmContext,
        callee: i64,
        construct: bool,
    ) -> i64 {
        if ctx.js_call_depth >= MAX_JS_CALL_DEPTH {
            self.pending_stack_trace = Some(self.native_stack_trace());
            ctx.pending_exception_kind = PendingExceptionKind::StackOverflow;
        }
        self.push_entry_activation(ctx, self.current_image_id);
        self.activations
            .last_mut()
            .expect("entry activation exists")
            .environment = callee;
        let rejected = if construct {
            dispatch::native_rejected_construct as *const ()
        } else {
            dispatch::native_rejected_call as *const ()
        };
        i64::try_from(rejected as usize).expect("native rejected call address fits i64")
    }

    fn finish_call(&mut self, ctx: &mut NativeVmContext) -> Option<i64> {
        let activation = self.activations.pop()?;
        for (slot, value) in activation.saved_variables {
            self.variables[slot] = value;
        }
        self.activate_image(ctx, activation.caller_image_id)?;
        ctx.call_arena_active_len = activation.active_len;
        ctx.js_call_depth = ctx.js_call_depth.checked_sub(1)?;
        Some(value::encode_undefined())
    }

    fn native_stack_trace(&self) -> String {
        let mut frames = Vec::<(NativeFunctionRef, usize)>::new();
        for function in self
            .activations
            .iter()
            .rev()
            .filter_map(|activation| activation.function)
        {
            if let Some((previous, repeated)) = frames.last_mut()
                && previous.image_id == function.image_id
                && previous.function_index == function.function_index
            {
                *repeated += 1;
            } else {
                frames.push((function, 1));
            }
        }

        let mut trace = String::new();
        for (function, repeated) in frames {
            let index =
                usize::try_from(function.function_index).expect("function index fits usize");
            let name = if function.image_id == self.current_image_id {
                self.function_names.get(index)
            } else {
                self.programs
                    .get(&function.image_id)
                    .and_then(|program| program.function_names.get(index))
            };
            let (Some(name), Some(source), Some(span)) = (
                name,
                self.image_source_files.get(&function.image_id),
                function.source_span,
            ) else {
                continue;
            };
            trace.push_str("\n at ");
            trace.push_str(name);
            trace.push_str(" (");
            trace.push_str(source);
            trace.push(':');
            trace.push_str(&span.line.to_string());
            trace.push(':');
            trace.push_str(&span.col.to_string());
            trace.push(')');
            if repeated > 1 {
                trace.push_str(" (repeated)");
            }
        }
        trace
    }

    fn invoke_callable(
        &mut self,
        ctx: &mut NativeVmContext,
        callee: i64,
        this_value: i64,
        arguments: &[i64],
    ) -> Option<i64> {
        let environment = if value::is_closure(callee) {
            self.closures
                .get(usize::try_from(value::decode_closure_idx(callee)).ok()?)
                .and_then(|closure| closure.as_ref())
                .map(|closure| closure.environment)?
        } else {
            self.call_environment()
                .unwrap_or_else(value::encode_undefined)
        };
        self.invoke_callable_with_environment_and_new_target(
            ctx,
            callee,
            environment,
            this_value,
            arguments,
            value::encode_undefined(),
        )
    }

    fn invoke_constructor(
        &mut self,
        ctx: &mut NativeVmContext,
        callee: i64,
        new_target: i64,
        this_value: i64,
        arguments: &[i64],
    ) -> Option<i64> {
        let environment = if value::is_closure(callee) {
            self.closures
                .get(usize::try_from(value::decode_closure_idx(callee)).ok()?)
                .and_then(|closure| closure.as_ref())
                .map(|closure| closure.environment)?
        } else {
            self.call_environment()
                .unwrap_or_else(value::encode_undefined)
        };
        self.invoke_callable_with_environment_and_new_target(
            ctx,
            callee,
            environment,
            this_value,
            arguments,
            new_target,
        )
    }

    fn invoke_callable_with_environment(
        &mut self,
        ctx: &mut NativeVmContext,
        callee: i64,
        environment: i64,
        this_value: i64,
        arguments: &[i64],
    ) -> Option<i64> {
        self.invoke_callable_with_environment_and_new_target(
            ctx,
            callee,
            environment,
            this_value,
            arguments,
            value::encode_undefined(),
        )
    }

    fn invoke_callable_with_environment_and_new_target(
        &mut self,
        ctx: &mut NativeVmContext,
        callee: i64,
        environment: i64,
        this_value: i64,
        arguments: &[i64],
        new_target: i64,
    ) -> Option<i64> {
        let mut prepared = Vec::with_capacity(arguments.len() + 1);
        prepared.push(callee);
        prepared.extend_from_slice(arguments);
        let entry = self.prepare_call(ctx, &prepared, false, None)?;
        self.activations.last_mut()?.new_target = new_target;
        let args_count = u32::try_from(arguments.len()).ok()?;
        let args_base = ctx.call_arena_active_len.checked_sub(args_count)?;
        let native_entry = value::is_native_callable(callee) || value::is_proxy(callee);
        if !native_entry && let Some(activation) = self.activations.last_mut() {
            activation.environment = environment;
        }
        let call_environment = if native_entry {
            self.activations.last()?.environment
        } else {
            environment
        };
        let entry = unsafe {
            std::mem::transmute::<*const (), NativeSlowEntry>(
                usize::try_from(entry).ok()? as *const ()
            )
        };
        let result = unsafe { entry(ctx, call_environment, this_value, args_base, args_count) };
        self.finish_call(ctx)?;
        Some(result)
    }

    fn call_environment(&self) -> Option<i64> {
        self.activations
            .last()
            .map(|activation| activation.environment)
    }

    fn create_closure(&mut self, args: &[i64]) -> Option<i64> {
        let [function, environment] = args else {
            return None;
        };
        let function_id = if value::is_function(*function) {
            value::decode_function_idx(*function)
        } else if value::is_closure(*function) {
            self.closures
                .get(usize::try_from(value::decode_closure_idx(*function)).ok()?)
                .and_then(|closure| closure.as_ref())?
                .function_id
        } else {
            return None;
        };
        let function_ref = *self.functions.get(function_id as usize)?;
        // 优先复用空闲槽；无空闲才扩表。扩表时先填 None 占位，下标稳定。
        let index = match self.closure_free.pop() {
            Some(index) => index,
            None => {
                let index = u32::try_from(self.closures.len()).ok()?;
                self.closures.push(None);
                index
            }
        };
        self.closures[index as usize] = Some(NativeClosure {
            function_id,
            environment: *environment,
        });
        let closure = value::encode_closure_idx(index);
        self.gc.record_host_write(closure, None, Some(closure));
        self.gc.record_host_write(closure, None, Some(*environment));
        self.function_closures.insert(
            (
                function_ref.image_id,
                function_ref.function_index,
                *environment,
            ),
            closure,
        );
        self.latest_function_closures.insert(
            (function_ref.image_id, function_ref.function_index),
            closure,
        );
        Some(closure)
    }

    fn callable_matches_local_function(&self, callable: i64, function_index: u32) -> bool {
        self.callable_function(callable).is_some_and(|function| {
            function.image_id == self.current_image_id && function.function_index == function_index
        })
    }
    fn callable_property(&mut self, callable: i64, key: PropertyKey) -> Option<i64> {
        let callable = value::strip_gc_color(callable);
        if let Some(value) = self.callable_properties.get(&(callable, key)).copied() {
            return Some(value);
        }
        if self.text_matches(key.to_value(), "name") {
            let name = if let Some(function) = self.callable_function(callable) {
                let index = usize::try_from(function.function_index).ok()?;
                (if function.image_id == self.current_image_id {
                    self.function_names.get(index)?
                } else {
                    self.programs
                        .get(&function.image_id)?
                        .function_names
                        .get(index)?
                })
                .clone()
            } else {
                native_function_metadata(self.native_callable_kind(callable)?)
                    .map(|(name, _)| name.to_owned())?
            };
            let stored = self.intern_text(name, value::TAG_STRING)?;
            self.callable_properties.insert((callable, key), stored);
            self.callable_property_flags
                .insert((callable, key), FUNCTION_METADATA_FLAGS);
            return Some(stored);
        }
        if self.text_matches(key.to_value(), "length") {
            let length = if let Some(function) = self.callable_function(callable) {
                if function.image_id == self.current_image_id {
                    self.function_lengths
                        .get(usize::try_from(function.function_index).ok()?)
                } else {
                    self.programs
                        .get(&function.image_id)?
                        .function_lengths
                        .get(usize::try_from(function.function_index).ok()?)
                }
                .copied()?
            } else {
                native_function_metadata(self.native_callable_kind(callable)?)
                    .map(|(_, length)| length)?
            };
            let stored = value::encode_f64(f64::from(length));
            self.callable_properties.insert((callable, key), stored);
            self.callable_property_flags
                .insert((callable, key), FUNCTION_METADATA_FLAGS);
            return Some(stored);
        }
        if self.native_callable_kind(callable) == Some(NativeCallableKind::FunctionConstructor)
            && self.text_matches(key.to_value(), "prototype")
        {
            let prototype = self.native_callable(NativeCallableKind::FunctionPrototype)?;
            let constructor_key = self.intern_property_string("constructor".into())?;
            self.callable_properties
                .insert((prototype, constructor_key), callable);
            self.callable_property_flags.insert(
                (prototype, constructor_key),
                wjsm_ir::constants::FLAG_WRITABLE as u32
                    | wjsm_ir::constants::FLAG_CONFIGURABLE as u32,
            );
            self.callable_properties.insert((callable, key), prototype);
            self.callable_property_flags
                .insert((callable, key), FUNCTION_PROTOTYPE_FLAGS);
            return Some(prototype);
        }
        if self
            .native_callable_builtin(callable)
            .is_some_and(|(builtin, _)| {
                builtin == wjsm_ir::Builtin::SymbolCreate
                    && self.text_matches(key.to_value(), "prototype")
            })
        {
            let prototype = if let Some(prototype) = self.symbol_prototype {
                prototype
            } else {
                let prototype = self.allocate_object(1, false).ok()?;
                let constructor_key = self.intern_property_string("constructor".into())?;
                self.gc
                    .heap()
                    .set_property(
                        value::decode_handle(prototype),
                        constructor_key,
                        callable as u64,
                    )
                    .ok()?;
                self.symbol_prototype = Some(prototype);
                prototype
            };
            self.callable_properties.insert((callable, key), prototype);
            self.callable_property_flags
                .insert((callable, key), FUNCTION_PROTOTYPE_FLAGS);
            return Some(prototype);
        }
        if let Some((builtin, false)) = self.native_callable_builtin(callable)
            && matches!(
                builtin,
                wjsm_ir::Builtin::MapConstructor
                    | wjsm_ir::Builtin::SetConstructor
                    | wjsm_ir::Builtin::WeakMapConstructor
                    | wjsm_ir::Builtin::WeakSetConstructor
            )
            && self.text_matches(key.to_value(), "prototype")
        {
            let prototype = self.ensure_collection_prototype(callable, builtin)?;
            self.callable_properties.insert((callable, key), prototype);
            self.callable_property_flags
                .insert((callable, key), FUNCTION_PROTOTYPE_FLAGS);
            return Some(prototype);
        }

        if self
            .native_callable_builtin(callable)
            .is_some_and(|(builtin, _)| {
                matches!(
                    builtin,
                    wjsm_ir::Builtin::ObjectKeys | wjsm_ir::Builtin::PromiseCreate
                ) && self.text_matches(key.to_value(), "prototype")
            })
        {
            let prototype = self.allocate_object(1, false).ok()?;
            let constructor_key = self.intern_property_string("constructor".into())?;
            self.gc
                .heap()
                .set_property(
                    value::decode_handle(prototype),
                    constructor_key,
                    callable as u64,
                )
                .ok()?;
            self.callable_properties.insert((callable, key), prototype);
            self.callable_property_flags
                .insert((callable, key), FUNCTION_PROTOTYPE_FLAGS);
            return Some(prototype);
        }
        if let Some(NativeCallableKind::Intl(kind)) = self.native_callable_kind(callable)
            && dispatch::intl::is_constructor(kind)
            && self.text_matches(key.to_value(), "prototype")
        {
            let prototype = dispatch::intl::ensure_constructor_prototype(self, callable, kind)?;
            self.callable_properties.insert((callable, key), prototype);
            // 内置构造器 `prototype`：{ writable: false, enumerable: false, configurable: false }
            self.callable_property_flags.insert((callable, key), 0);
            return Some(prototype);
        }
        if self.native_callable_kind(callable)
            == Some(NativeCallableKind::WebEncoding(
                dispatch::web_encoding::WebEncodingCallable::TextDecoderConstructor,
            ))
            && self.text_matches(key.to_value(), "prototype")
        {
            let prototype = dispatch::web_encoding::ensure_text_decoder_prototype(self)?;
            self.callable_properties.insert((callable, key), prototype);
            self.callable_property_flags.insert((callable, key), 0);
            return Some(prototype);
        }
        if self.native_callable_kind(callable) == Some(NativeCallableKind::StringConstructor)
            && self.text_matches(key.to_value(), "prototype")
        {
            return dispatch::intl::ensure_string_prototype(self);
        }
        if self
            .native_callable_builtin(callable)
            .is_some_and(|(builtin, _)| {
                builtin == wjsm_ir::Builtin::NumberConstructor
                    && self.text_matches(key.to_value(), "prototype")
            })
        {
            return dispatch::intl::ensure_number_prototype(self);
        }
        if self
            .native_callable_builtin(callable)
            .is_some_and(|(builtin, _)| {
                builtin == wjsm_ir::Builtin::BigIntFromLiteral
                    && self.text_matches(key.to_value(), "prototype")
            })
        {
            return dispatch::intl::ensure_bigint_prototype(self);
        }
        if self
            .native_callable_builtin(callable)
            .is_some_and(|(builtin, _)| {
                builtin == wjsm_ir::Builtin::DateConstructor
                    && self.text_matches(key.to_value(), "prototype")
            })
        {
            let prototype = self.allocate_object(1, false).ok()?;
            let constructor_key = self.intern_property_string("constructor".into())?;
            self.gc
                .heap()
                .set_property(
                    value::decode_handle(prototype),
                    constructor_key,
                    callable as u64,
                )
                .ok()?;
            dispatch::date::install_prototype_methods(self, prototype).ok()?;
            self.callable_properties.insert((callable, key), prototype);
            self.callable_property_flags
                .insert((callable, key), FUNCTION_PROTOTYPE_FLAGS);
            return Some(prototype);
        }
        let function = self.callable_function(callable)?;
        if !self.text_matches(key.to_value(), "prototype") || !function.needs_prototype {
            return None;
        }
        let prototype = self.allocate_object(1, false).ok()?;
        let constructor_key = self.intern_property_string("constructor".into())?;
        self.gc
            .heap()
            .set_property(
                value::decode_handle(prototype),
                constructor_key,
                callable as u64,
            )
            .ok()?;
        self.callable_properties.insert((callable, key), prototype);
        self.callable_property_flags
            .insert((callable, key), FUNCTION_PROTOTYPE_FLAGS);
        Some(prototype)
    }

    fn ensure_collection_prototype(
        &mut self,
        constructor: i64,
        builtin: wjsm_ir::Builtin,
    ) -> Option<i64> {
        let cached = match builtin {
            wjsm_ir::Builtin::MapConstructor => self.map_prototype,
            wjsm_ir::Builtin::SetConstructor => self.set_prototype,
            wjsm_ir::Builtin::WeakMapConstructor => self.weak_map_prototype,
            wjsm_ir::Builtin::WeakSetConstructor => self.weak_set_prototype,
            _ => return None,
        };
        if let Some(prototype) = cached {
            return Some(prototype);
        }
        self.ensure_intrinsic_prototypes().ok()?;
        let prototype = self.allocate_object(10, false).ok()?;
        let constructor_key = self.intern_property_string("constructor".into())?;
        self.gc
            .heap()
            .set_property(
                value::decode_handle(prototype),
                constructor_key,
                constructor as u64,
            )
            .ok()?;
        self.gc
            .heap()
            .update_property_flags(
                value::decode_handle(prototype),
                constructor_key,
                BUILTIN_PROTOTYPE_PROPERTY_FLAGS,
            )
            .ok()?;
        match builtin {
            wjsm_ir::Builtin::MapConstructor => {
                dispatch::collections::install_prototype_methods(self, prototype, false).ok()?;
                self.map_prototype = Some(prototype);
            }
            wjsm_ir::Builtin::SetConstructor => {
                dispatch::collections::install_prototype_methods(self, prototype, true).ok()?;
                self.set_prototype = Some(prototype);
            }
            wjsm_ir::Builtin::WeakMapConstructor => {
                dispatch::weak::install_prototype_methods(self, prototype, false).ok()?;
                self.weak_map_prototype = Some(prototype);
            }
            wjsm_ir::Builtin::WeakSetConstructor => {
                dispatch::weak::install_prototype_methods(self, prototype, true).ok()?;
                self.weak_set_prototype = Some(prototype);
            }
            _ => return None,
        }
        Some(prototype)
    }

    fn set_collection_prototype(
        &mut self,
        object: i64,
        builtin: wjsm_ir::Builtin,
    ) -> Result<(), ()> {
        let constructor = self
            .native_callable(NativeCallableKind::Builtin(builtin, false))
            .ok_or(())?;
        let prototype_key = self.intern_property_string("prototype".into()).ok_or(())?;
        let prototype = self
            .callable_property(constructor, prototype_key)
            .filter(|prototype| value::is_object(*prototype))
            .map(value::decode_handle)
            .ok_or(())?;
        self.gc
            .heap()
            .set_prototype(value::decode_handle(object), prototype)
            .map_err(|_| ())
    }

    fn note_array_property(&mut self, handle: u32, key: PropertyKey) {
        let order = self.array_property_order.entry(handle).or_default();
        if !order.contains(&key) {
            order.push(key);
        }
    }

    fn forget_array_property(&mut self, handle: u32, key: PropertyKey) {
        if let Some(order) = self.array_property_order.get_mut(&handle) {
            order.retain(|candidate| *candidate != key);
            if order.is_empty() {
                self.array_property_order.remove(&handle);
            }
        }
    }

    fn prepare_out_of_memory_error(&mut self) -> Result<(), NativeRuntimeError> {
        let error = dispatch::modules::frozen_named_error_object(
            self,
            "RangeError",
            OUT_OF_MEMORY_MESSAGE.into(),
        )
        .ok_or_else(|| {
            NativeRuntimeError::Invariant("failed to prepare heap exhaustion error".into())
        })?;
        let exception = self.create_exception(error).ok_or_else(|| {
            NativeRuntimeError::Invariant("failed to prepare heap exhaustion exception".into())
        })?;
        self.out_of_memory_error = Some(error);
        self.out_of_memory_exception = Some(exception);
        Ok(())
    }

    fn out_of_memory_exception(&self) -> Option<i64> {
        self.out_of_memory_exception
    }

    fn create_exception(&mut self, value: i64) -> Option<i64> {
        let index = match self.exception_free.pop() {
            Some(index) => index,
            None => {
                let index = u32::try_from(self.exceptions.len()).ok()?;
                self.exceptions.push(None);
                index
            }
        };
        self.exceptions[index as usize] = Some(value);
        let exception = value::encode_handle(value::TAG_EXCEPTION, index);
        self.gc.record_host_write(exception, None, Some(exception));
        self.gc.record_host_write(exception, None, Some(value));
        Some(exception)
    }

    fn exception_value(&self, exception: i64) -> Option<i64> {
        value::is_exception(exception)
            .then(|| value::decode_handle(exception))
            .and_then(|index| self.exceptions.get(index as usize))
            .and_then(|stored| *stored)
    }

    fn load_argument(&self, args: &[i64]) -> Option<i64> {
        let [base, len, index] = args else {
            return None;
        };
        let base = usize::try_from(*base).ok()?;
        let len = usize::try_from(*len).ok()?;
        let index = usize::try_from(*index).ok()?;
        if index >= len {
            return Some(value::encode_undefined());
        }
        self.call_arena.get(base.checked_add(index)?).copied()
    }
    fn collect_rest_arguments(&self, ctx: &NativeVmContext, args: &[i64]) -> Option<i64> {
        let [skip] = args else {
            return None;
        };
        let skip = u32::try_from(*skip).ok()?;
        let activation = self.activations.last()?;
        let arena_len = ctx
            .call_arena_active_len
            .saturating_sub(activation.active_len);
        let length = arena_len
            .min(activation.argument_count)
            .saturating_sub(skip);
        let array = self.allocate_object(length, true).ok()?;
        let handle = value::decode_handle(array);
        let base = usize::try_from(activation.active_len.checked_add(skip)?).ok()?;
        for index in 0..usize::try_from(length).ok()? {
            self.gc
                .heap()
                .push_element(handle, *self.call_arena.get(base + index)? as u64)
                .ok()?;
        }
        Some(array)
    }

    pub(crate) fn rebuild_string_ids(&mut self) -> Result<(), NativeRuntimeError> {
        self.string_ids.clear();
        let (entries, _) = self.gc.heap().capture_handles()?;
        for entry in entries {
            let handle = entry.handle.get();
            if self.gc.heap().object_type(handle).ok() != Some(u32::from(wjsm_ir::HEAP_TYPE_STRING))
                || self
                    .gc
                    .heap()
                    .string_flags(handle)
                    .ok()
                    .is_none_or(|flags| flags & wjsm_ir::constants::STRING_FLAG_INTERNED == 0)
            {
                continue;
            }
            let hash = self.gc.heap().string_content_hash(handle)?;
            let length = self.gc.heap().string_length(handle)?;
            self.string_ids.insert((hash, length), handle);
        }
        Ok(())
    }

    fn rebuild_latin1_char_strings(&mut self) -> Result<(), NativeRuntimeError> {
        for unit in 0_u16..=u16::from(u8::MAX) {
            let encoded = if unit <= u16::from(0x7f_u8) {
                value::encode_inline_ascii(&[unit as u8]).expect("ASCII SSO")
            } else {
                self.publish_string_bytes(
                    &[u8::try_from(unit).expect("Latin-1 码元不超过 u8")],
                    value::TAG_STRING,
                    true,
                )
                .ok_or_else(|| {
                    NativeRuntimeError::Invariant(
                        "failed to materialize Latin-1 character cache".into(),
                    )
                })?
            };
            self.latin1_char_strings[usize::from(unit)] = encoded;
        }
        Ok(())
    }
    pub(crate) fn property_name_handles(&self) -> Vec<i64> {
        let mut names = self.gc.heap().property_name_ids();
        names.extend(
            self.array_properties
                .keys()
                .filter_map(|(_, key)| key.name_id()),
        );
        names.extend(
            self.array_accessors
                .keys()
                .filter_map(|(_, key)| key.name_id()),
        );
        names.extend(
            self.array_property_flags
                .keys()
                .filter_map(|(_, key)| key.name_id()),
        );
        names.extend(
            self.array_property_order
                .values()
                .flatten()
                .filter_map(|key| key.name_id()),
        );
        names.extend(
            self.callable_properties
                .keys()
                .filter_map(|(_, key)| key.name_id()),
        );
        names.extend(
            self.callable_accessors
                .keys()
                .filter_map(|(_, key)| key.name_id()),
        );
        names.extend(
            self.callable_property_flags
                .keys()
                .filter_map(|(_, key)| key.name_id()),
        );
        self.string_ids
            .values()
            .copied()
            .filter(|handle| names.contains(handle))
            .map(value::encode_runtime_string_handle)
            .collect()
    }

    pub(crate) fn prune_string_ids(&mut self, retired: &[u32]) {
        self.string_ids
            .retain(|_, handle| retired.binary_search(handle).is_err());
    }

    pub(crate) fn prune_unmarked_string_ids(&mut self) {
        self.string_ids.retain(|_, handle| {
            self.gc
                .heap()
                .handle_generation(*handle)
                .is_some_and(|generation| {
                    self.gc
                        .heap()
                        .is_marked_handle(*handle, generation)
                        .unwrap_or(false)
                })
        });
    }

    fn encode_inline_ascii_units(units: &[u16]) -> Option<i64> {
        if units.len() > value::INLINE_STRING_MAX_LEN {
            return None;
        }
        let mut bytes = [0_u8; value::INLINE_STRING_MAX_LEN];
        for (byte, unit) in bytes.iter_mut().zip(units.iter().copied()) {
            if unit > u16::from(u8::MAX) {
                return None;
            }
            *byte = unit as u8;
        }
        let slice = &bytes[..units.len()];
        value::encode_inline_ascii(slice)
            .or_else(|| value::encode_inline_latin1(slice))
    }

    fn intern_text(&mut self, text: String, tag: u64) -> Option<i64> {
        if tag == value::TAG_STRING
            && let Some(encoded) = value::encode_inline_ascii(text.as_bytes())
        {
            self.gc.record_inline_string();
            return Some(encoded);
        }
        let units = text.encode_utf16().collect::<Vec<_>>();
        self.publish_string_units(&units, tag, true)
    }

    /// 去重命中时复用既有句柄；键为（内容哈希, UTF-16 长度），同一内容无论
    /// Latin-1 还是 UTF-16 载荷都得到同一键，表示选择不影响句柄唯一性。
    fn dedup_string_handle(&self, key: &(u32, u32)) -> Option<i64> {
        self.string_ids
            .get(key)
            .copied()
            .map(value::encode_runtime_string_handle)
    }

    fn publish_string_units(&mut self, units: &[u16], tag: u64, interned: bool) -> Option<i64> {
        if tag == value::TAG_STRING
            && let Some(encoded) = Self::encode_inline_ascii_units(units)
        {
            self.gc.record_inline_string();
            return Some(encoded);
        }
        let length = u32::try_from(units.len()).ok()?;
        let key = (content_hash_units(units), length);
        if interned
            && tag == value::TAG_STRING
            && let Some(encoded) = self.dedup_string_handle(&key)
        {
            return Some(encoded);
        }
        // 全 Latin-1 内容选单字节载荷：ASCII 负载的内存与拷贝带宽减半。
        let latin1 = units.iter().all(|unit| *unit <= u16::from(u8::MAX));
        let bytes = if latin1 {
            units.iter().map(|&unit| unit as u8).collect::<Vec<_>>()
        } else {
            let mut bytes = Vec::with_capacity(units.len().checked_mul(2)?);
            for unit in units {
                bytes.extend_from_slice(&unit.to_le_bytes());
            }
            bytes
        };
        self.publish_flat_string(&key, length, &bytes, tag, interned, latin1)
    }

    fn publish_builder_units(&mut self, units: &[u16], tag: u64) -> Option<i64> {
        let length = u32::try_from(units.len()).ok()?;
        let payload_len = units.len().checked_mul(2)?;
        let capacity = u32::try_from(payload_len.max(16 * 1024).checked_add(7)? & !7).ok()?;
        let mut bytes = Vec::with_capacity(payload_len);
        for unit in units {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        let address = self
            .gc
            .allocate(string_payload_bytes(capacity).ok()?)
            .ok()?;
        let handle = self.gc.heap().allocate_handle().ok()?;
        self.gc
            .heap()
            .publish_string(
                handle,
                address,
                PROTO_NULL_SENTINEL,
                wjsm_ir::constants::STRING_REPR_BUILDER,
                0,
                length,
                capacity,
            )
            .ok()?;
        self.gc
            .heap()
            .write_string_payload(handle, 0, &bytes)
            .ok()?;
        self.gc.mark_black_allocation(handle).ok()?;
        Some(if tag == value::TAG_STRING {
            value::encode_runtime_string_handle(handle)
        } else {
            value::encode_handle(tag, handle)
        })
    }

    fn publish_string_bytes(&mut self, bytes: &[u8], tag: u64, interned: bool) -> Option<i64> {
        let length = u32::try_from(bytes.len()).ok()?;
        let key = (content_hash_latin1(bytes), length);
        if interned
            && tag == value::TAG_STRING
            && let Some(encoded) = self.dedup_string_handle(&key)
        {
            return Some(encoded);
        }
        self.publish_flat_string(&key, length, bytes, tag, interned, true)
    }

    /// 以 `latin1` 指定的载荷表示发布扁平字符串：分配、写头、写 payload、
    /// 黑标记，需要去重时惰性物化内容哈希并登记 `string_ids`。
    fn publish_flat_string(
        &mut self,
        key: &(u32, u32),
        length: u32,
        bytes: &[u8],
        tag: u64,
        interned: bool,
        latin1: bool,
    ) -> Option<i64> {
        let capacity = u32::try_from(bytes.len().checked_add(7)? & !7).ok()?;
        let address = self
            .gc
            .allocate(string_payload_bytes(capacity).ok()?)
            .ok()?;
        let handle = self.gc.heap().allocate_handle().ok()?;
        let flags = if interned {
            wjsm_ir::constants::STRING_FLAG_INTERNED
        } else {
            0
        };
        self.gc
            .heap()
            .publish_string(
                handle,
                address,
                PROTO_NULL_SENTINEL,
                if latin1 {
                    wjsm_ir::constants::STRING_REPR_LATIN1_FLAT
                } else {
                    wjsm_ir::constants::STRING_REPR_UTF16_FLAT
                },
                flags,
                length,
                capacity,
            )
            .ok()?;
        self.gc.heap().write_string_payload(handle, 0, bytes).ok()?;
        self.gc.mark_black_allocation(handle).ok()?;
        if interned && tag == value::TAG_STRING {
            // key.0 即调用方按同函数算好的内容哈希，直写堆头即可，无需再从
            // 载荷重算一遍（旧路径的 string_content_hash 是重复计算）。
            self.gc.heap().write_string_hash(handle, key.0).ok()?;
            self.string_ids.insert(*key, handle);
        }
        Some(if tag == value::TAG_STRING {
            value::encode_runtime_string_handle(handle)
        } else {
            value::encode_handle(tag, handle)
        })
    }

    /// 发布 Cons 字符串（O(1) 绳索拼接节点，无数据拷贝）。
    pub(crate) fn publish_cons_string(
        &mut self,
        left: u32,
        right: u32,
        length: u32,
        tag: u64,
    ) -> Option<i64> {
        let capacity = wjsm_ir::constants::HEAP_STRING_CONS_PAYLOAD_SIZE;
        let address = self
            .gc
            .allocate(string_payload_bytes(capacity).ok()?)
            .ok()?;
        let handle = self.gc.heap().allocate_handle().ok()?;
        self.gc
            .heap()
            .publish_string(
                handle,
                address,
                PROTO_NULL_SENTINEL,
                wjsm_ir::constants::STRING_REPR_CONS,
                0,
                length,
                capacity,
            )
            .ok()?;
        self.gc.heap().set_cons_children(handle, left, right).ok()?;
        self.gc.mark_black_allocation(handle).ok()?;
        Some(if tag == value::TAG_STRING {
            value::encode_runtime_string_handle(handle)
        } else {
            value::encode_handle(tag, handle)
        })
    }

    /// 发布 Slice 字符串（O(1) 零拷贝切片视图）。
    pub(crate) fn publish_slice_string(
        &mut self,
        base: u32,
        start: u32,
        end: u32,
        length: u32,
        tag: u64,
    ) -> Option<i64> {
        let capacity = wjsm_ir::constants::HEAP_STRING_SLICE_PAYLOAD_SIZE;
        let address = self
            .gc
            .allocate(string_payload_bytes(capacity).ok()?)
            .ok()?;
        let handle = self.gc.heap().allocate_handle().ok()?;
        self.gc
            .heap()
            .publish_string(
                handle,
                address,
                PROTO_NULL_SENTINEL,
                wjsm_ir::constants::STRING_REPR_SLICE,
                0,
                length,
                capacity,
            )
            .ok()?;
        self.gc
            .heap()
            .set_slice_parts(handle, base, start, end)
            .ok()?;
        self.gc.mark_black_allocation(handle).ok()?;
        Some(if tag == value::TAG_STRING {
            value::encode_runtime_string_handle(handle)
        } else {
            value::encode_handle(tag, handle)
        })
    }

    pub(crate) fn publish_cons_string_with_gc_retry(
        &mut self,
        ctx: &mut NativeVmContext,
        left: u32,
        right: u32,
        length: u32,
        tag: u64,
    ) -> Option<i64> {
        if self.gc.take_pacing_poll_request() {
            let _ = self.poll_gc(ctx);
        }
        if let Some(encoded) = self.publish_cons_string(left, right, length, tag) {
            return Some(encoded);
        }
        if self.collect_garbage(ctx).is_ok() {
            let _ = self.gc.heap().finish_relocation_epoch();
            let _ = self.gc.heap().advance_epoch_and_reclaim();
            return self.publish_cons_string(left, right, length, tag);
        }
        None
    }

    pub(crate) fn publish_slice_string_with_gc_retry(
        &mut self,
        ctx: &mut NativeVmContext,
        base: u32,
        start: u32,
        end: u32,
        length: u32,
        tag: u64,
    ) -> Option<i64> {
        if self.gc.take_pacing_poll_request() {
            let _ = self.poll_gc(ctx);
        }
        if let Some(encoded) = self.publish_slice_string(base, start, end, length, tag) {
            return Some(encoded);
        }
        if self.collect_garbage(ctx).is_ok() {
            let _ = self.gc.heap().finish_relocation_epoch();
            let _ = self.gc.heap().advance_epoch_and_reclaim();
            return self.publish_slice_string(base, start, end, length, tag);
        }
        None
    }

    /// 属性名必须内容唯一：同一名字在任何路径下都要解析到同一 handle。
    pub(crate) fn intern_property_string(&mut self, text: RuntimeString) -> Option<PropertyKey> {
        let units = text.as_flat_slice();
        if let Some(encoded) = Self::encode_inline_ascii_units(units) {
            self.gc.record_inline_property_key();
            return PropertyKey::inline_string(encoded);
        }
        let encoded = self.publish_string_units(units, value::TAG_STRING, true)?;
        let handle = value::decode_runtime_string_handle(encoded);
        self.gc.record_host_write(encoded, None, Some(encoded));
        Some(PropertyKey::from_name_id(handle))
    }

    fn intern_utf16_slice(&mut self, units: &[u16], tag: u64) -> Option<i64> {
        self.publish_string_units(units, tag, tag == value::TAG_STRING)
    }

    fn intern_runtime_string(&mut self, text: RuntimeString, tag: u64) -> Option<i64> {
        let is_builder = text.is_builder();
        let dedup = tag == value::TAG_STRING && text.utf16_len() <= 64 && text.is_flat();
        let units = text.as_flat_slice();
        let encoded = if !is_builder
            && tag == value::TAG_STRING
            && let Some(encoded) = Self::encode_inline_ascii_units(units)
        {
            self.gc.record_inline_string();
            encoded
        } else if is_builder {
            self.publish_builder_units(units, tag)?
        } else {
            self.publish_string_units(units, tag, dedup)?
        };
        if value::is_handle_backed_reference(encoded) {
            self.gc.record_host_write(encoded, None, Some(encoded));
        }
        Some(encoded)
    }

    fn allocate_object(&self, capacity: u32, array: bool) -> Result<i64, HeapAccessV2Error> {
        let prototype = if array {
            self.array_prototype
        } else {
            self.object_prototype
        }
        .map_or(PROTO_NULL_SENTINEL, value::decode_handle);
        self.allocate_object_with_prototype(capacity, array, prototype)
    }

    fn allocate_object_with_gc_retry(
        &mut self,
        ctx: &mut NativeVmContext,
        capacity: u32,
        array: bool,
    ) -> Result<i64, NativeRuntimeError> {
        self.try_native_object_allocation(ctx, capacity, array)
    }

    fn try_native_object_allocation(
        &mut self,
        ctx: &mut NativeVmContext,
        capacity: u32,
        array: bool,
    ) -> Result<i64, NativeRuntimeError> {
        let capacity = if array {
            capacity
        } else {
            capacity.max(wjsm_ir::constants::HEAP_OBJECT_INITIAL_VALUE_CAPACITY)
        };
        if self.gc.take_pacing_poll_request() {
            self.poll_gc(ctx)?;
        }
        if let Some(value) = self.try_native_object_allocation_fast(ctx, capacity, array)? {
            return Ok(value);
        }
        self.gc.flush_native_tlab(ctx)?;
        let prototype = if array {
            self.array_prototype
        } else {
            self.object_prototype
        }
        .map_or(PROTO_NULL_SENTINEL, value::decode_handle);
        self.gc.allocation_diagnostics_slow_allocation();
        match self.allocate_object_with_prototype(capacity, array, prototype) {
            Ok(value) => Ok(value),
            Err(
                error @ (HeapAccessV2Error::HeapExhausted { .. }
                | HeapAccessV2Error::Allocator(wjsm_gc::AllocatorError::OutOfPages {
                    ..
                })),
            ) => {
                self.collect_garbage(ctx)?;
                let _ = self.gc.heap().finish_relocation_epoch();
                let _ = self.gc.heap().advance_epoch_and_reclaim();
                self.allocate_object_with_prototype(capacity, array, prototype)
                    .map_err(|retry| match retry {
                        HeapAccessV2Error::HeapExhausted { .. }
                        | HeapAccessV2Error::Allocator(wjsm_gc::AllocatorError::OutOfPages {
                            ..
                        }) => error.into(),
                        retry => retry.into(),
                    })
            }
            Err(error) => Err(error.into()),
        }
    }

    fn try_native_object_allocation_fast(
        &mut self,
        ctx: &mut NativeVmContext,
        capacity: u32,
        array: bool,
    ) -> Result<Option<i64>, NativeRuntimeError> {
        if ctx.allocation_fast_flags & wjsm_native_abi::NATIVE_ALLOCATION_FAST_HOST == 0
            || !self.gc.host_fast_allocation_allowed()
        {
            return Ok(None);
        }
        let bytes = object_payload_bytes(capacity)?;
        if bytes > ctx.allocation_small_limit
            || ctx
                .bump_ptr
                .checked_add(bytes)
                .is_none_or(|end| end > ctx.bump_limit)
            || ctx.bump_handle_cursor >= ctx.bump_handle_limit
        {
            return Ok(None);
        }
        let object = ctx.bump_ptr;
        let handle = ctx.bump_handle_cursor;
        let prototype = if array {
            ctx.array_prototype_handle
        } else {
            ctx.object_prototype_handle
        };
        self.gc
            .publish_native_tlab_object(ctx, handle, object, prototype, array, capacity)?;
        ctx.bump_ptr = ctx.bump_ptr.checked_add(bytes).ok_or_else(|| {
            NativeRuntimeError::Invariant("native TLAB object cursor overflow".into())
        })?;
        ctx.bump_handle_cursor += 1;
        self.gc.commit_native_tlab_cursor(ctx);
        Ok(Some(value::encode_handle(
            if array {
                value::TAG_ARRAY
            } else {
                value::TAG_OBJECT
            },
            handle,
        )))
    }

    fn allocate_object_with_prototype(
        &self,
        capacity: u32,
        array: bool,
        prototype: u32,
    ) -> Result<i64, HeapAccessV2Error> {
        let bytes = object_payload_bytes(capacity)?;
        let address = self.reserve_object_space(bytes)?;
        let handle = self.gc.heap().allocate_handle()?;
        if array {
            self.gc
                .heap()
                .publish_array(handle, address, u32::MAX, capacity)?;
        } else {
            self.gc
                .heap()
                .publish_object(handle, address, u32::MAX, capacity)?;
        }
        self.gc.mark_black_allocation(handle)?;
        self.gc.heap().set_prototype(handle, prototype)?;
        Ok(value::encode_handle(
            if array {
                value::TAG_ARRAY
            } else {
                value::TAG_OBJECT
            },
            handle,
        ))
    }

    fn reserve_object_space(&self, bytes: u64) -> Result<u64, HeapAccessV2Error> {
        self.gc.allocate(bytes)
    }

    fn collect_garbage_if_needed(
        &mut self,
        ctx: &mut NativeVmContext,
    ) -> Result<bool, NativeRuntimeError> {
        if !self.gc.take_safepoint_poll_request() {
            return Ok(false);
        }
        self.poll_gc(ctx)
    }

    fn has_pending_external_events(&self) -> bool {
        self.node_child_process.has_pending()
            || dispatch::node_dgram::has_pending(self)
            || dispatch::node_tls::has_pending(self)
            || self.node_worker_threads.has_pending()
            || self.node_worker_threads.cluster.has_wait_timeouts()
            || dispatch::fetch::has_pending(self)
    }

    /// 在 cluster 级 SAB 表中分配 backing，返回 backing_id。
    fn allocate_sab_backing(&self, byte_length: usize, max_byte_length: Option<usize>) -> u32 {
        self.node_worker_threads
            .cluster
            .allocate_sab(byte_length, max_byte_length)
    }

    /// 从已有 bytes 分配 SAB backing（slice 结果）。
    fn allocate_sab_backing_from_bytes(&self, bytes: Vec<u8>) -> u32 {
        let length = bytes.len();
        self.node_worker_threads
            .cluster
            .allocate_sab_bytes(bytes, length, None)
    }

    /// 把 JS handle 映射到 cluster backing。
    fn insert_shared_array_buffer(&mut self, handle: u32, backing_id: u32) {
        if let Some(backing) = self.cluster_backing(backing_id) {
            self.shared_array_buffers.insert(
                handle,
                dispatch::sab::NativeSharedArrayBuffer {
                    backing_id,
                    backing,
                },
            );
        }
    }

    /// 按 backing_id 取得 cluster backing 引用。
    fn cluster_backing(&self, backing_id: u32) -> Option<dispatch::sab::SABBacking> {
        self.node_worker_threads.cluster.sab(backing_id)
    }

    fn poll_external_events(&mut self, ctx: &mut NativeVmContext) -> i64 {
        let child_result = dispatch::node_child_process::poll(ctx, self);
        if value::is_exception(child_result) || self.node_child_process.has_pending() {
            return child_result;
        }
        let worker_result = dispatch::node_worker_threads::poll(ctx, self);
        if value::is_exception(worker_result) || self.node_worker_threads.has_pending() {
            return worker_result;
        }
        for (backing_id, byte_offset, promise) in
            self.node_worker_threads.cluster.pop_wait_timeouts()
        {
            let _ = (backing_id, byte_offset);
            let result = self.intern_text("timed-out".into(), value::TAG_STRING);
            if let Some(result) = result {
                dispatch::promise::settle_promise(self, promise, result, false);
            }
        }
        let tls_result = dispatch::node_tls::poll_pending(ctx, self);
        if value::is_exception(tls_result) {
            return tls_result;
        }
        let fetch_result = dispatch::fetch::poll(ctx, self);
        if value::is_exception(fetch_result) {
            return fetch_result;
        }
        if !dispatch::node_dgram::has_pending(self) {
            return value::encode_undefined();
        }
        dispatch::node_dgram::poll_pending(ctx, self)
    }

    fn allocate_array_values(&self, values: &[i64]) -> Result<i64, HeapAccessV2Error> {
        let capacity =
            u32::try_from(values.len()).map_err(|_| HeapAccessV2Error::AddressOverflow)?;
        let array = self.allocate_object(capacity, true)?;
        let handle = value::decode_handle(array);
        for value in values {
            self.gc.heap().push_element(handle, *value as u64)?;
        }
        Ok(array)
    }

    pub(crate) fn allocate_array_values_with_gc_retry(
        &mut self,
        ctx: &mut NativeVmContext,
        values: &[i64],
    ) -> Result<i64, NativeRuntimeError> {
        let initial_temp_roots = self.temporary_roots.len();
        self.temporary_roots.extend(values.iter().copied());

        let capacity = match u32::try_from(values.len()) {
            Ok(cap) => cap,
            Err(_) => {
                self.temporary_roots.truncate(initial_temp_roots);
                return Err(NativeRuntimeError::Invariant(
                    "array length overflow".into(),
                ));
            }
        };
        let array = match self.allocate_object_with_gc_retry(ctx, capacity, true) {
            Ok(arr) => arr,
            Err(e) => {
                self.temporary_roots.truncate(initial_temp_roots);
                return Err(e);
            }
        };
        let handle = value::decode_handle(array);
        self.temporary_roots.push(array);

        for value in values {
            if self.gc.heap().push_element(handle, *value as u64).is_err() {
                if self.collect_garbage(ctx).is_ok() {
                    let _ = self.gc.heap().finish_relocation_epoch();
                    let _ = self.gc.heap().advance_epoch_and_reclaim();
                    if self.gc.heap().push_element(handle, *value as u64).is_ok() {
                        continue;
                    }
                }
                self.temporary_roots.truncate(initial_temp_roots);
                return Err(NativeRuntimeError::Invariant("push_element failed".into()));
            }
        }
        self.temporary_roots.truncate(initial_temp_roots);
        Ok(array)
    }
    pub(crate) fn collect_garbage(
        &mut self,
        ctx: &mut NativeVmContext,
    ) -> Result<wjsm_gc::RuntimeGcReport, NativeRuntimeError> {
        self.gc.flush_native_tlab(ctx)?;
        let frame_roots = native_root_values(ctx)?;
        let graph = dispatch::weak::snapshot_gc_graph(ctx, self, frame_roots, 0);
        let report = self.gc.collect_full(ctx, graph)?;
        dispatch::weak::finish_gc_cycle(self, &report);
        Ok(report)
    }

    fn poll_gc(&mut self, ctx: &mut NativeVmContext) -> Result<bool, NativeRuntimeError> {
        let action = self.gc.safepoint_action();
        let snapshot = if let wjsm_gc::GcSafepointAction::PublishRoots { epoch } = action {
            self.gc.flush_native_tlab(ctx)?;
            let frame_roots = native_root_values(ctx)?;
            Some(dispatch::weak::snapshot_gc_graph(
                ctx,
                self,
                frame_roots,
                epoch,
            ))
        } else {
            None
        };
        let report = self.gc.at_safepoint(ctx, action, snapshot)?;
        if let Some(report) = report {
            dispatch::weak::finish_gc_cycle(self, &report);
            return Ok(true);
        }
        Ok(!matches!(action, wjsm_gc::GcSafepointAction::Idle))
    }

    fn cleanup_retired_handles(&mut self, retired: &[u32]) {
        if retired.is_empty() {
            return;
        }
        let is_live = |handle: &u32| retired.binary_search(handle).is_err();
        self.maps.retain(|handle, _| is_live(handle));
        self.sets.retain(|handle, _| is_live(handle));
        self.weak
            .retain_live_owners(|handle| retired.binary_search(&handle).is_err());
        self.array_buffers.retain(|handle, _| is_live(handle));
        self.shared_array_buffers
            .retain(|handle, _| is_live(handle));
        self.data_views.retain(|handle, _| is_live(handle));
        self.typed_arrays.retain(|handle, _| is_live(handle));
        self.buffers.retain(|handle, _| is_live(handle));
        self.text_decoders.retain(|handle, _| is_live(handle));
        self.promises.retain(|handle, _| is_live(handle));
        self.continuations.retain(|handle, _| is_live(handle));
        self.generators.retain(|handle, _| is_live(handle));
        self.async_generators.retain(|handle, _| is_live(handle));
        self.array_iterators.retain(|handle, _| is_live(handle));
        self.enumerators.retain(|handle, _| is_live(handle));
        self.iterator_next.retain(|handle, _| is_live(handle));
        self.array_property_order
            .retain(|handle, _| is_live(handle));
        self.error_objects.retain(is_live);
        self.boxed_primitives.retain(|handle, _| is_live(handle));
        self.non_extensible_objects.retain(is_live);
        self.array_properties
            .retain(|(handle, _), _| is_live(handle));
        self.array_accessors
            .retain(|(handle, _), _| is_live(handle));
        self.array_property_flags
            .retain(|(handle, _), _| is_live(handle));
        self.scope_records.retain(|handle, _| is_live(handle));
        self.async_from_sync_iterators
            .retain(|handle, _| is_live(handle));
        self.async_generator_resume_completions
            .retain(|handle, _| is_live(handle));
        self.promise_reactions.retain(|handle, _| is_live(handle));
        self.intl.slots.retain(|handle, _| is_live(handle));
    }
    fn drain_gc_cycle(&mut self, ctx: &mut NativeVmContext) -> Result<(), NativeRuntimeError> {
        let mut backoff = Backoff::new();
        while self.gc.cycle_active() {
            self.poll_gc(ctx)?;
            backoff.wait();
        }
        Ok(())
    }
}

fn native_root_values(ctx: &NativeVmContext) -> Result<Vec<i64>, NativeRuntimeError> {
    let mut roots = Vec::new();
    let mut frame = ctx.root_frame_head.load(Ordering::Acquire);
    let mut depth = 0_u32;
    while !frame.is_null() {
        depth = depth.checked_add(1).ok_or_else(|| {
            NativeRuntimeError::Invariant("native root frame depth overflow".into())
        })?;
        if depth > MAX_JS_CALL_DEPTH.saturating_add(1) {
            return Err(NativeRuntimeError::Invariant(
                "native root frame chain exceeds JavaScript call depth".into(),
            ));
        }
        // SAFETY: 仅在 owner thread 的同步 host call 内扫描。`frame` 来自 pinned vmctx
        // 的 Acquire head；每个节点由仍在栈上的 generated function 持有，image 仍注册，
        // 扫描期间不执行 JS、分配或移动 frame，读取值立即复制进 owned Vec。
        let current = unsafe { &*frame };
        let word_count = usize::try_from(current.bitmap_word_count).map_err(|_| {
            NativeRuntimeError::Invariant("native root bitmap word count exceeds usize".into())
        })?;
        if word_count > MAX_NATIVE_ROOT_BITMAP_WORDS {
            return Err(NativeRuntimeError::Invariant(
                "native root bitmap exceeds ABI slot range".into(),
            ));
        }
        if word_count != 0 && (current.bitmap_words.is_null() || current.slots.is_null()) {
            return Err(NativeRuntimeError::Invariant(
                "native root frame payload is null".into(),
            ));
        }
        for word_index in 0..word_count {
            // SAFETY: `word_count` 已按 ABI 上限验证；generated frame 的 bitmap 指向
            // 当前 loaded image 的只读数据，slots 指向同一活动 stack frame。
            let mut bits = unsafe { current.bitmap_words.add(word_index).read() };
            while bits != 0 {
                let bit = bits.trailing_zeros() as usize;
                let slot = word_index * u64::BITS as usize + bit;
                // SAFETY: 置位 bitmap 只由 compiler 为已分配 root slot 生成。
                roots.push(unsafe { current.slots.add(slot).read() });
                bits &= bits - 1;
            }
        }
        frame = current.previous;
    }
    Ok(roots)
}

fn process_write(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    arguments: &[i64],
    stderr: bool,
) -> i64 {
    let input = arguments
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let text = match dispatch::to_string_coerced(ctx, state, input) {
        Ok(text) => text,
        Err(exception) => return exception,
    };
    state.emit_output(text.as_bytes(), stderr);
    value::encode_bool(true)
}

fn process_numeric_object(state: &mut NativeAgentState, fields: &[(&str, f64)]) -> Option<i64> {
    let object = state
        .allocate_object(u32::try_from(fields.len()).ok()?, false)
        .ok()?;
    for (name, number) in fields {
        let key = state.intern_property_string((*name).into())?;
        state
            .gc
            .heap()
            .set_property(
                value::decode_handle(object),
                key,
                value::encode_f64(*number) as u64,
            )
            .ok()?;
    }
    Some(object)
}

fn box_or_return_string(
    state: &mut NativeAgentState,
    this_value: i64,
    text: String,
) -> Option<i64> {
    let primitive = state.intern_text(text, value::TAG_STRING)?;
    let constructing = state
        .activations
        .last()
        .is_some_and(|activation| !value::is_undefined(activation.new_target));
    if constructing && value::is_js_object(this_value) {
        state
            .boxed_primitives
            .insert(value::decode_handle(this_value), primitive);
        return Some(this_value);
    }
    Some(primitive)
}

unsafe extern "C" fn native_callable_call(
    ctx: *mut NativeVmContext,
    callee: i64,
    this_value: i64,
    args_base: u32,
    args_count: u32,
) -> i64 {
    // SAFETY: 入口只由已验证的 generated code 调用，vmctx 在 runtime 生命周期内 pinned。
    let Some(ctx) = (unsafe { ctx.as_mut() }) else {
        return value::encode_handle(value::TAG_EXCEPTION, 0);
    };
    let Some(end) = args_base.checked_add(args_count) else {
        ctx.pending_exception_kind = PendingExceptionKind::InternalInvariant;
        return value::encode_handle(value::TAG_EXCEPTION, 0);
    };
    if end > ctx.call_arena_active_len {
        ctx.pending_exception_kind = PendingExceptionKind::InternalInvariant;
        return value::encode_handle(value::TAG_EXCEPTION, 0);
    }
    // SAFETY: heap_state 指向 NativeRuntime 独占的 boxed state，不跨同步调用保留引用。
    let Some(state) = (unsafe { ctx.heap_state.cast::<NativeAgentState>().as_mut() }) else {
        ctx.pending_exception_kind = PendingExceptionKind::InternalInvariant;
        return value::encode_handle(value::TAG_EXCEPTION, 0);
    };
    let Some(kind) = state.native_callable_kind(callee) else {
        ctx.pending_exception_kind = PendingExceptionKind::InternalInvariant;
        return value::encode_handle(value::TAG_EXCEPTION, 0);
    };
    let (Ok(base), Ok(end)) = (usize::try_from(args_base), usize::try_from(end)) else {
        ctx.pending_exception_kind = PendingExceptionKind::InternalInvariant;
        return value::encode_handle(value::TAG_EXCEPTION, 0);
    };
    if let NativeCallableKind::ProxyCall(proxy) = kind {
        let Some(arguments) = state.call_arena.get(base..end).map(<[i64]>::to_vec) else {
            ctx.pending_exception_kind = PendingExceptionKind::InternalInvariant;
            return value::encode_handle(value::TAG_EXCEPTION, 0);
        };
        return dispatch::proxy::call(
            ctx,
            state,
            value::encode_proxy_handle(proxy),
            this_value,
            &arguments,
        );
    }
    if let NativeCallableKind::ProxyConstruct(proxy) = kind {
        let Some(arguments) = state.call_arena.get(base..end).map(<[i64]>::to_vec) else {
            ctx.pending_exception_kind = PendingExceptionKind::InternalInvariant;
            return value::encode_handle(value::TAG_EXCEPTION, 0);
        };
        return dispatch::proxy::construct(
            ctx,
            state,
            value::encode_proxy_handle(proxy),
            &arguments,
            value::encode_proxy_handle(proxy),
        );
    }
    let Some(arguments) = state.call_arena.get(base..end).map(<[i64]>::to_vec) else {
        ctx.pending_exception_kind = PendingExceptionKind::InternalInvariant;
        return value::encode_handle(value::TAG_EXCEPTION, 0);
    };
    match kind {
        NativeCallableKind::Bound(bound) => {
            let Some(bound) = state
                .bound_functions
                .get(bound as usize)
                .and_then(|bound| bound.as_ref())
                .cloned()
            else {
                return dispatch::fail_dispatch(ctx);
            };
            let mut combined = Vec::with_capacity(bound.arguments.len() + arguments.len());
            combined.extend_from_slice(&bound.arguments);
            combined.extend_from_slice(&arguments);
            state
                .invoke_callable(ctx, bound.target, bound.this_value, &combined)
                .unwrap_or_else(|| dispatch::fail_dispatch(ctx))
        }
        NativeCallableKind::Fetch(callable) => {
            dispatch::fetch::call(ctx, state, callable, &arguments)
        }
        NativeCallableKind::WebEncoding(callable) => {
            dispatch::web_encoding::call(ctx, state, callable, this_value, &arguments)
        }
        NativeCallableKind::Intl(callable) => {
            dispatch::intl::call(ctx, state, callable, this_value, &arguments)
        }
        NativeCallableKind::ArrayConstructor => dispatch::construct_array(ctx, state, &arguments),
        NativeCallableKind::ObjectConstructor => dispatch::construct_object(ctx, state, &arguments),
        NativeCallableKind::RealmArrayConstructor(context) => {
            let previous = state.array_prototype;
            if let Some(prototype) = dispatch::node_vm::array_prototype_for_handle(state, context) {
                state.array_prototype = Some(prototype);
            }
            let result = dispatch::construct_array(ctx, state, &arguments);
            state.array_prototype = previous;
            result
        }
        NativeCallableKind::ArrayToString => dispatch::array_to_string(ctx, state, this_value),
        NativeCallableKind::ArrayIterator(kind) => {
            dispatch::array_iterator(ctx, state, this_value, kind)
        }
        NativeCallableKind::CjsRequire(_)
        | NativeCallableKind::CjsResolve(_)
        | NativeCallableKind::CjsResolvePaths(_)
        | NativeCallableKind::ImportMetaResolve(_) => {
            dispatch::modules::invoke_module_callable(ctx, state, kind, &arguments)
                .unwrap_or_else(|| value::encode_handle(value::TAG_EXCEPTION, 0))
        }
        NativeCallableKind::BufferConstructor => {
            dispatch::node_buffer::call_constructor(ctx, state, &arguments)
        }
        NativeCallableKind::DateMethod(method) => {
            dispatch::date::call_method(ctx, state, this_value, method, &arguments)
        }
        NativeCallableKind::ErrorToString => dispatch::error_to_string(ctx, state, this_value),
        NativeCallableKind::BufferMethod(method) => {
            dispatch::node_buffer::call_method(ctx, state, this_value, method, &arguments)
        }
        NativeCallableKind::NodeFs(method) => {
            dispatch::node_fs::call(ctx, state, method, &arguments)
        }
        NativeCallableKind::BufferStatic(kind) => {
            dispatch::node_buffer::call_static(ctx, state, kind, &arguments)
        }
        NativeCallableKind::BufferTranscode => {
            dispatch::node_buffer::transcode(ctx, state, &arguments)
        }
        NativeCallableKind::NodeAsyncHooks(callable) => {
            dispatch::node_async_hooks::call(ctx, state, callable, this_value, &arguments)
        }
        NativeCallableKind::NodeCrypto(callable) => {
            dispatch::node_crypto::call(ctx, state, callable, &arguments)
        }
        NativeCallableKind::NodeDgram(method) => {
            dispatch::node_dgram::call(ctx, state, method, &arguments)
        }
        NativeCallableKind::NodeNet(method) => {
            dispatch::node_net::call(ctx, state, method, &arguments)
        }
        NativeCallableKind::NodeTls(method) => {
            dispatch::node_tls::call(ctx, state, method, &arguments)
        }
        NativeCallableKind::NodeZlib(method) => {
            dispatch::node_zlib::call(ctx, state, method, &arguments)
        }
        NativeCallableKind::NodeOs(method) => dispatch::node_os::call(ctx, state, method),
        NativeCallableKind::Idna(method) => dispatch::idna::call(ctx, state, method, &arguments),
        NativeCallableKind::NodeVm(callable) => {
            dispatch::node_vm::call(ctx, state, callable, &arguments)
        }
        NativeCallableKind::NodeChildProcess(callable) => {
            dispatch::node_child_process::call(ctx, state, callable, &arguments)
        }
        NativeCallableKind::NodeWorkerThreads(method) => {
            dispatch::node_worker_threads::call(ctx, state, method, &arguments)
        }
        NativeCallableKind::Test262Agent(method) => {
            dispatch::agent::call(ctx, state, method, &arguments)
        }
        NativeCallableKind::ProxyCall(_) | NativeCallableKind::ProxyConstruct(_) => {
            unreachable!("proxy callables handled before dispatch")
        }
        NativeCallableKind::ProcessExit => {
            let code = arguments
                .first()
                .and_then(|code| dispatch::number_value(state, *code))
                .filter(|code| code.is_finite())
                .map_or(0, |code| code as i32);
            state.requested_exit_code = Some(code);
            state
                .create_exception(value::encode_undefined())
                .unwrap_or_else(|| dispatch::fail_dispatch(ctx))
        }
        NativeCallableKind::ProcessWrite(stderr) => process_write(ctx, state, &arguments, stderr),
        NativeCallableKind::ProcessStreamEnd(stderr) => {
            if arguments
                .first()
                .is_some_and(|input| !value::is_undefined(*input))
            {
                let result = process_write(ctx, state, &arguments, stderr);
                if value::is_exception(result) {
                    return result;
                }
            }
            this_value
        }
        NativeCallableKind::ProcessStreamReturnThis => this_value,
        NativeCallableKind::ProxyRevoke(proxy) => {
            let Some(proxy) = usize::try_from(proxy)
                .ok()
                .and_then(|proxy| state.proxies.get_mut(proxy))
                .and_then(|proxy| proxy.as_mut())
            else {
                ctx.pending_exception_kind = PendingExceptionKind::InternalInvariant;
                return value::encode_handle(value::TAG_EXCEPTION, 0);
            };
            proxy.revoked = true;
            value::encode_undefined()
        }
        NativeCallableKind::CollectionNext(iterator_id) => {
            dispatch::collections::next(ctx, state, iterator_id)
        }
        NativeCallableKind::IteratorNext(iterator) => {
            dispatch::iterator_next_result(ctx, state, iterator)
        }
        NativeCallableKind::ArgumentsStrictCallee => {
            dispatch::arguments::strict_callee_error(ctx, state)
        }
        NativeCallableKind::RegExpToString => {
            let Some((pattern, flags)) = dispatch::regexp::clone_parts(state, this_value) else {
                return dispatch::fail_dispatch(ctx);
            };
            let source = if pattern.is_empty() { "(?:)" } else { &pattern };
            state
                .intern_text(format!("/{source}/{flags}"), value::TAG_STRING)
                .unwrap_or_else(|| dispatch::fail_dispatch(ctx))
        }
        NativeCallableKind::RegExpIteratorNext(iterator_id) => {
            dispatch::regexp::next_match_all(ctx, state, iterator_id)
        }
        NativeCallableKind::PromiseResolve(promise) => dispatch::promise::settle_resolver(
            ctx,
            state,
            promise,
            arguments.first().copied(),
            false,
        ),
        NativeCallableKind::PromiseReject(promise) => dispatch::promise::settle_resolver(
            ctx,
            state,
            promise,
            arguments.first().copied(),
            true,
        ),
        NativeCallableKind::AggregateErrorConstructor => {
            dispatch::promise::construct_aggregate_error(ctx, state, &arguments)
        }
        NativeCallableKind::NodePerfHooks(callable) => {
            dispatch::node_perf_hooks::call(ctx, state, callable, &arguments)
        }
        NativeCallableKind::FunctionConstructor => {
            let global = dispatch::node_vm::current_context(state);
            if !dispatch::node_vm::strings_enabled(state, global) {
                return dispatch::modules::named_error_object(
                    state,
                    "EvalError",
                    "Code generation from strings disallowed for this context".into(),
                )
                .and_then(|error| state.create_exception(error))
                .unwrap_or_else(|| dispatch::fail_dispatch(ctx));
            }
            let Some((body, parameters)) = arguments.split_last() else {
                return dispatch::node_vm::compile_dynamic_function(ctx, state, "", &[], global);
            };
            let Some(body) = state.string_owned(*body).and_then(|text| text.to_utf8()) else {
                return dispatch::fail_dispatch(ctx);
            };
            let mut parameter_names = Vec::with_capacity(parameters.len());
            for parameter in parameters {
                let Some(parameter) = state
                    .string_owned(*parameter)
                    .and_then(|text| text.to_utf8())
                else {
                    return dispatch::fail_dispatch(ctx);
                };
                parameter_names.push(parameter);
            }
            dispatch::node_vm::compile_dynamic_function(ctx, state, &body, &parameter_names, global)
        }
        NativeCallableKind::StringConstructor => {
            let Some(argument) = arguments.first().copied() else {
                return box_or_return_string(state, this_value, String::new())
                    .unwrap_or_else(|| dispatch::fail_dispatch(ctx));
            };
            let text = if value::is_symbol(argument) {
                dispatch::render_value(state, argument)
            } else {
                match dispatch::to_string_coerced(ctx, state, argument) {
                    Ok(text) => text,
                    Err(exception) => return exception,
                }
            };
            box_or_return_string(state, this_value, text)
                .unwrap_or_else(|| dispatch::fail_dispatch(ctx))
        }
        NativeCallableKind::ProcessCwd => state
            .intern_text(
                state.working_directory.to_string_lossy().into_owned(),
                value::TAG_STRING,
            )
            .unwrap_or_else(|| dispatch::fail_dispatch(ctx)),
        NativeCallableKind::ProcessHrtime => {
            let elapsed = state.process_started_at.elapsed().as_nanos();
            let seconds = (elapsed / 1_000_000_000) as f64;
            let nanoseconds = (elapsed % 1_000_000_000) as f64;
            state
                .allocate_array_values(&[
                    value::encode_f64(seconds),
                    value::encode_f64(nanoseconds),
                ])
                .unwrap_or_else(|_| dispatch::fail_dispatch(ctx))
        }
        NativeCallableKind::ProcessHrtimeBigInt => dispatch::store_bigint(
            state,
            num_bigint::BigInt::from(state.process_started_at.elapsed().as_nanos()),
        )
        .unwrap_or_else(|| dispatch::fail_dispatch(ctx)),
        NativeCallableKind::ProcessUptime => {
            value::encode_f64(state.process_started_at.elapsed().as_secs_f64())
        }
        NativeCallableKind::ProcessMemoryUsage => {
            let used = state.gc.heap().used_bytes() as f64;
            let total = state.gc.heap().heap_limit_bytes() as f64;
            process_numeric_object(
                state,
                &[
                    ("rss", used),
                    ("heapTotal", total),
                    ("heapUsed", used),
                    ("external", 0.0),
                    ("arrayBuffers", 0.0),
                ],
            )
            .unwrap_or_else(|| dispatch::fail_dispatch(ctx))
        }
        NativeCallableKind::ProcessCpuUsage => {
            process_numeric_object(state, &[("user", 0.0), ("system", 0.0)])
                .unwrap_or_else(|| dispatch::fail_dispatch(ctx))
        }
        NativeCallableKind::Stream(callable) => {
            dispatch::streams::call(ctx, state, callable, &arguments)
        }
        NativeCallableKind::ProcessOn => {
            dispatch::node_child_process::process_on(ctx, state, this_value, &arguments)
        }
        NativeCallableKind::Gc => {
            let result = dispatch::node_async_hooks::collect_auto_resources(ctx, state);
            if let Err(error) = state.collect_garbage(ctx) {
                ctx.pending_exception_kind = PendingExceptionKind::InternalInvariant;
                state
                    .stderr
                    .borrow_mut()
                    .extend_from_slice(error.to_string().as_bytes());
                return dispatch::fail_dispatch(ctx);
            }
            if result.is_none() {
                dispatch::node_perf_hooks::emit_gc_entry(ctx, state);
            }
            result.unwrap_or_else(value::encode_undefined)
        }
        NativeCallableKind::FunctionPrototype => value::encode_undefined(),
        NativeCallableKind::TimerConstructor(_) => this_value,
        NativeCallableKind::SetImmediate => {
            let Some(callback) = arguments
                .first()
                .copied()
                .filter(|callback| value::is_callable(*callback))
            else {
                return dispatch::fail_dispatch(ctx);
            };
            dispatch::promise::enqueue_immediate(ctx, state, callback, arguments[1..].to_vec())
        }
        NativeCallableKind::ProcessNextTick => {
            let Some(callback) = arguments
                .first()
                .copied()
                .filter(|callback| value::is_callable(*callback))
            else {
                return dispatch::fail_dispatch(ctx);
            };
            let scheduled =
                dispatch::promise::enqueue_next_tick(ctx, state, callback, arguments[1..].to_vec());
            if value::is_exception(scheduled) {
                scheduled
            } else {
                value::encode_undefined()
            }
        }
        NativeCallableKind::Builtin(
            builtin @ (wjsm_ir::Builtin::ErrorConstructor
            | wjsm_ir::Builtin::EvalErrorConstructor
            | wjsm_ir::Builtin::RangeErrorConstructor
            | wjsm_ir::Builtin::ReferenceErrorConstructor
            | wjsm_ir::Builtin::SyntaxErrorConstructor
            | wjsm_ir::Builtin::TypeErrorConstructor
            | wjsm_ir::Builtin::URIErrorConstructor),
            _,
        ) => dispatch::error_constructor(ctx, state, builtin, this_value, &arguments),
        NativeCallableKind::Builtin(wjsm_ir::Builtin::PromiseCreate, _) => {
            dispatch::promise::construct(ctx, state, this_value, &arguments)
        }
        NativeCallableKind::Builtin(builtin, with_receiver) => {
            let mut call_args = Vec::with_capacity(arguments.len() + usize::from(with_receiver));
            if with_receiver {
                call_args.push(this_value);
            }
            call_args.extend_from_slice(&arguments);
            let result = dispatch::dispatch_builtin(ctx, state, builtin, &call_args);
            if builtin == wjsm_ir::Builtin::NumberConstructor
                && !value::is_exception(result)
                && value::is_f64(result)
                && state
                    .activations
                    .last()
                    .is_some_and(|activation| !value::is_undefined(activation.new_target))
                && value::is_js_object(this_value)
            {
                state
                    .boxed_primitives
                    .insert(value::decode_handle(this_value), result);
                return this_value;
            }
            result
        }
    }
}

pub struct NativeRuntime {
    state: Box<NativeAgentState>,
    vmctx: Pin<Box<NativeVmContext>>,
    owner_thread: std::thread::ThreadId,
    not_send_sync: PhantomData<Rc<()>>,
}

/// builtin / user 两段 image 及其槽表，供 split install 一次装入。
struct SplitProgramImages {
    builtin_program: wjsm_ir::Program,
    user_program: wjsm_ir::Program,
    builtin_slots: HashMap<String, u32>,
    user_slots: HashMap<String, u32>,
    builtin_image: Arc<CompiledImage>,
    user_image: Arc<CompiledImage>,
}

impl NativeRuntime {
    pub fn new(cache_dir: Option<PathBuf>) -> Result<Self, NativeRuntimeError> {
        Self::new_with_config(NativeRuntimeConfig::from_environment(cache_dir)?)
    }

    pub fn new_with_config(config: NativeRuntimeConfig) -> Result<Self, NativeRuntimeError> {
        Self::new_with_config_and_inspector(config, None)
    }

    pub fn new_with_inspector(
        cache_dir: Option<PathBuf>,
        inspector: Option<InspectorConfig>,
    ) -> Result<Self, NativeRuntimeError> {
        Self::new_with_config_and_inspector(
            NativeRuntimeConfig::from_environment(cache_dir)?,
            inspector,
        )
    }

    pub fn new_with_config_and_inspector(
        config: NativeRuntimeConfig,
        inspector: Option<InspectorConfig>,
    ) -> Result<Self, NativeRuntimeError> {
        let mut state = Box::new(NativeAgentState::new(config)?);
        state.restore_startup_snapshot(snapshot::STARTUP_SNAPSHOT_BYTES)?;
        state.inspector = inspector
            .map(inspector::InspectorRuntime::start)
            .transpose()?;
        let mut vmctx = Box::pin(NativeVmContext::default());
        let stack_marker = 0_u8;
        let stack_pointer = std::ptr::from_ref(&stack_marker).addr();
        let context = Pin::as_mut(&mut vmctx).get_mut();
        context.heap_state = std::ptr::from_mut(state.as_mut()).cast();
        context.call_arena_slots = state.call_arena.as_mut_ptr();
        // 反馈开关位：生成代码的守卫快路径据此决定是否内联更新反馈槽。
        context.flags = if state.runtime_config.specialization_enabled {
            wjsm_native_abi::NATIVE_FLAGS_FEEDBACK_ENABLED
        } else {
            0
        };
        context.call_arena_capacity = u32::try_from(state.call_arena.len())
            .map_err(|_| NativeRuntimeError::Invariant("call arena exceeds u32".into()))?;
        context.stack_low = stack_pointer.saturating_sub(8 * 1024 * 1024);
        context.stack_high = stack_pointer.saturating_add(1024 * 1024);
        context.stack_budget_bytes = wjsm_native_abi::COOPERATIVE_POLL_BUDGET;
        // 句柄表基址：generated code 属性快链用；snapshot 恢复替换 heap 后由
        // `activate_image` 重新同步（每次 execute 必经）。
        context.handle_table_base = state.gc.heap().handle_table_base();
        context.latin1_char_strings = state.latin1_char_strings.as_ptr();
        let object_prototype = state.object_prototype.map(value::decode_handle);
        let array_prototype = state.array_prototype.map(value::decode_handle);
        state
            .gc
            .bind_context(context, object_prototype, array_prototype)?;
        Ok(Self {
            state,
            vmctx,
            owner_thread: std::thread::current().id(),
            not_send_sync: PhantomData,
        })
    }

    pub fn inspector_url(&self) -> Option<&str> {
        self.state
            .inspector
            .as_ref()
            .map(inspector::InspectorRuntime::url)
    }

    fn configure_worker(&mut self, context: dispatch::node_worker_threads::WorkerAgentContext) {
        self.state.node_worker_threads =
            dispatch::node_worker_threads::NodeWorkerThreadsState::worker(context);
    }

    pub fn configure_environment(
        &mut self,
        inherit_env: bool,
        env: impl IntoIterator<Item = (String, String)>,
    ) -> Result<(), NativeRuntimeError> {
        self.assert_owner_thread()?;
        self.state.environment.clear();
        if inherit_env {
            self.state.environment.extend(std::env::vars());
        }
        self.state.environment.extend(env);
        self.state.process_object = None;
        self.state.process_env_object = None;
        Ok(())
    }

    pub fn configure_process_arguments(
        &mut self,
        arguments: impl IntoIterator<Item = String>,
    ) -> Result<(), NativeRuntimeError> {
        self.assert_owner_thread()?;
        self.state.process_arguments.clear();
        self.state.process_arguments.extend(arguments);
        self.state.process_object = None;
        Ok(())
    }

    pub fn execute(
        &mut self,
        artifact: &PortableArtifact,
        module_root: &std::path::Path,
        working_directory: &std::path::Path,
    ) -> Result<NativeExecution, NativeRuntimeError> {
        self.execute_with_store(
            artifact,
            ModuleSourceStore::disk(module_root),
            working_directory,
        )
    }

    pub fn execute_with_store(
        &mut self,
        artifact: &PortableArtifact,
        store: ModuleSourceStore,
        working_directory: &std::path::Path,
    ) -> Result<NativeExecution, NativeRuntimeError> {
        self.begin_execute(artifact, &store, working_directory)?;
        let (entry, image_id) = self.install_compiled_images(artifact, &store)?;
        self.finish_execute(artifact, entry, image_id)
    }

    /// 用打包期预编译 object 执行，跳过启动 codegen。
    pub fn execute_precompiled(
        &mut self,
        artifact: &PortableArtifact,
        images: &PrecompiledNativeImages,
        store: ModuleSourceStore,
        working_directory: &std::path::Path,
    ) -> Result<NativeExecution, NativeRuntimeError> {
        self.begin_execute(artifact, &store, working_directory)?;
        let (entry, image_id) = self.install_precompiled_images(artifact, &store, images)?;
        self.finish_execute(artifact, entry, image_id)
    }

    fn begin_execute(
        &mut self,
        artifact: &PortableArtifact,
        store: &ModuleSourceStore,
        working_directory: &std::path::Path,
    ) -> Result<(), NativeRuntimeError> {
        self.assert_owner_thread()?;
        self.state.output.borrow_mut().clear();
        self.state.stderr.borrow_mut().clear();
        self.state.reset_execution();
        self.state
            .restore_startup_snapshot(snapshot::STARTUP_SNAPSHOT_BYTES)?;
        let object_prototype = self.state.object_prototype.map(value::decode_handle);
        let array_prototype = self.state.array_prototype.map(value::decode_handle);
        self.state.gc.bind_context(
            Pin::as_mut(&mut self.vmctx).get_mut(),
            object_prototype,
            array_prototype,
        )?;
        self.state.process_entry = process_entry_for_store(artifact, store)?;
        self.state.working_directory = working_directory
            .canonicalize()
            .unwrap_or_else(|_| working_directory.to_path_buf());
        Ok(())
    }

    fn install_compiled_images(
        &mut self,
        artifact: &PortableArtifact,
        store: &ModuleSourceStore,
    ) -> Result<(NativeSlowEntry, u64), NativeRuntimeError> {
        match artifact.program().split_builtin_segment() {
            Some((builtin_program, user_program)) => {
                let (builtin_slots, user_slots) =
                    native_variable_slots_for_segments(&builtin_program, &user_program);
                let builtin_image = self.state.repository.prepare_program_with_slots(
                    &builtin_program,
                    &builtin_slots,
                    &NativeHostRegistry,
                )?;
                let user_image = self.state.repository.prepare_program_with_slots(
                    &user_program,
                    &user_slots,
                    &NativeHostRegistry,
                )?;
                self.install_split_images(
                    artifact,
                    store,
                    SplitProgramImages {
                        builtin_program,
                        user_program,
                        builtin_slots,
                        user_slots,
                        builtin_image,
                        user_image,
                    },
                )
            }
            None => {
                let image = self
                    .state
                    .repository
                    .prepare(artifact, &NativeHostRegistry)?;
                self.install_whole_image(artifact, store, image)
            }
        }
    }

    fn install_precompiled_images(
        &mut self,
        artifact: &PortableArtifact,
        store: &ModuleSourceStore,
        images: &PrecompiledNativeImages,
    ) -> Result<(NativeSlowEntry, u64), NativeRuntimeError> {
        native_exec::validate_images_match_program(artifact.program(), images)?;
        match (artifact.program().split_builtin_segment(), images) {
            (
                Some((builtin_program, user_program)),
                PrecompiledNativeImages::Split { builtin, user },
            ) => {
                let (builtin_slots, user_slots) =
                    native_variable_slots_for_segments(&builtin_program, &user_program);
                let builtin_image = self.state.repository.load_precompiled(
                    &builtin_program,
                    builtin,
                    &NativeHostRegistry,
                )?;
                let user_image = self.state.repository.load_precompiled(
                    &user_program,
                    user,
                    &NativeHostRegistry,
                )?;
                self.install_split_images(
                    artifact,
                    store,
                    SplitProgramImages {
                        builtin_program,
                        user_program,
                        builtin_slots,
                        user_slots,
                        builtin_image,
                        user_image,
                    },
                )
            }
            (None, PrecompiledNativeImages::Whole(object)) => {
                let image = self.state.repository.load_precompiled(
                    artifact.program(),
                    object,
                    &NativeHostRegistry,
                )?;
                self.install_whole_image(artifact, store, image)
            }
            _ => Err(NativeRuntimeError::Invariant(
                "precompiled images do not match program layout".into(),
            )),
        }
    }

    fn install_split_images(
        &mut self,
        artifact: &PortableArtifact,
        store: &ModuleSourceStore,
        images: SplitProgramImages,
    ) -> Result<(NativeSlowEntry, u64), NativeRuntimeError> {
        let SplitProgramImages {
            builtin_program,
            user_program,
            builtin_slots,
            user_slots,
            builtin_image,
            user_image,
        } = images;
        self.state
            .install_shared_variables(&builtin_slots, &user_slots);
        let builtin_image_id = builtin_image.image_id();
        let user_image_id = user_image.image_id();
        let shared_module_slots = builtin_slots
            .keys()
            .map(String::as_str)
            .filter(|name| is_module_scope_var(name))
            .collect::<HashSet<_>>();
        let context = Pin::as_mut(&mut self.vmctx).get_mut();
        self.state.install_program(
            context,
            builtin_image,
            &builtin_program,
            &builtin_slots,
            &shared_module_slots,
        )?;
        self.state.install_program(
            context,
            user_image.clone(),
            &user_program,
            &user_slots,
            &shared_module_slots,
        )?;
        self.state.builtin_image_id = Some(builtin_image_id);
        self.state.user_image_id = Some(user_image_id);
        self.state.user_function_count = u32::try_from(user_program.functions().len()).ok();
        let entry_index = user_program
            .functions()
            .iter()
            .position(|function| is_module_entry_ir_function(function.name()))
            .unwrap_or(0);
        let entry = user_image
            .entries()
            .get(entry_index)
            .ok_or_else(|| NativeRuntimeError::Invariant("entry function is missing".into()))?
            .slow_entry;
        dispatch::modules::configure(
            &mut self.state,
            store.clone(),
            user_image_id,
            artifact.manifest(),
        )
        .map_err(NativeRuntimeError::Invariant)?;
        Ok((entry, user_image_id))
    }

    fn install_whole_image(
        &mut self,
        artifact: &PortableArtifact,
        store: &ModuleSourceStore,
        image: Arc<CompiledImage>,
    ) -> Result<(NativeSlowEntry, u64), NativeRuntimeError> {
        let slots = whole_program_slots(artifact.program());
        self.state
            .install_whole_program_variables(artifact.program());
        let entry_index = artifact
            .program()
            .functions()
            .iter()
            .position(|function| is_module_entry_ir_function(function.name()))
            .unwrap_or(0);
        let entry = image
            .entries()
            .get(entry_index)
            .ok_or_else(|| NativeRuntimeError::Invariant("entry function is missing".into()))?
            .slow_entry;
        let image_id = image.image_id();
        dispatch::modules::configure(
            &mut self.state,
            store.clone(),
            image_id,
            artifact.manifest(),
        )
        .map_err(NativeRuntimeError::Invariant)?;
        let context = Pin::as_mut(&mut self.vmctx).get_mut();
        self.state
            .install_program(context, image, artifact.program(), &slots, &HashSet::new())?;
        Ok((entry, image_id))
    }

    fn finish_execute(
        &mut self,
        artifact: &PortableArtifact,
        entry: NativeSlowEntry,
        image_id: u64,
    ) -> Result<NativeExecution, NativeRuntimeError> {
        if let Some(inspector) = self.state.inspector.as_mut() {
            inspector.register_script(artifact);
        }
        let context = Pin::as_mut(&mut self.vmctx).get_mut();
        self.state
            .activate_image(context, image_id)
            .ok_or_else(|| NativeRuntimeError::Invariant("entry image is missing".into()))?;
        self.state
            .prepare_entry_call(context, image_id)
            .ok_or_else(|| NativeRuntimeError::Range("Maximum call stack size exceeded".into()))?;
        // SAFETY: typed entry 由 state 中仍存活的 RX image 拥有；vmctx 已 pinned，
        // 当前调用位于 owner thread，零实参不访问 call arena。
        let mut value = unsafe { entry(context, 0, value::encode_undefined(), 0, 0) };
        if value::is_exception(value) {
            inspector::pause_for_exception(context, &mut self.state, value, true);
        }
        self.state.drain_gc_cycle(context)?;
        if context.pending_exception_kind == PendingExceptionKind::None
            && self.state.requested_exit_code.is_none()
        {
            let drained = dispatch::promise::drain_event_loop(context, &mut self.state);
            if value::is_exception(drained) {
                value = drained;
            }
        }
        self.state.gc.flush_native_tlab(context)?;
        self.state
            .finish_call(context)
            .ok_or_else(|| NativeRuntimeError::Invariant("entry activation is missing".into()))?;
        dispatch::node_child_process::shutdown(&mut self.state);
        if context.pending_exception_kind != PendingExceptionKind::None {
            let kind = context.pending_exception_kind;
            context.pending_exception_kind = PendingExceptionKind::None;
            return Err(NativeRuntimeError::Pending(kind));
        }
        if self.state.requested_exit_code.is_some() {
            value = value::encode_undefined();
        }
        if value::is_exception(value) {
            if self.state.fatal_exception.take().is_some() {
                return Err(NativeRuntimeError::FatalJavaScript(
                    dispatch::modules::named_exception_text(&mut self.state, value),
                ));
            }
            let text = dispatch::modules::exception_text(&mut self.state, value);
            return Err(NativeRuntimeError::JavaScript(text));
        }
        let stats = self.state.repository.stats();
        Ok(NativeExecution {
            value,
            stdout: self.state.output.borrow().clone(),
            stderr: self.state.stderr.borrow().clone(),
            exit_code: self.state.requested_exit_code.unwrap_or(0),
            cache_entries: stats.entries,
            cache_bytes: stats.bytes,
            cache_hit_count: stats.hits,
            cache_miss_count: stats.misses,
            cache_invalidated_count: stats.invalidated,
        })
    }

    pub fn gc_telemetry(&self) -> wjsm_gc::GcTelemetrySnapshot {
        self.state.gc.telemetry_snapshot()
    }
    pub fn reset_gc_telemetry(&self) {
        self.state.gc.reset_telemetry();
    }
    pub fn allocation_diagnostics(&self) -> NativeAllocationDiagnostics {
        self.state.gc.allocation_diagnostics()
    }

    pub fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(self.state.output.get_mut())
    }

    pub fn take_stderr(&mut self) -> Vec<u8> {
        std::mem::take(self.state.stderr.get_mut())
    }

    #[cfg(test)]
    fn collect_garbage_now(&mut self) -> Result<wjsm_gc::RuntimeGcReport, NativeRuntimeError> {
        self.assert_owner_thread()?;
        let ctx = Pin::as_mut(&mut self.vmctx).get_mut();
        self.state.collect_garbage(ctx)
    }

    #[cfg(test)]
    fn host_side_table_stats(&self) -> crate::host_table_reclaim::HostSideTableStats {
        crate::host_table_reclaim::HostSideTableStats {
            live_closures: self.state.closures.iter().filter(|c| c.is_some()).count(),
            function_closures: self.state.function_closures.len(),
            latest_function_closures: self.state.latest_function_closures.len(),
            live_strings: self.state.string_ids.len(),
            string_ids: self.state.string_ids.len(),
            scope_records: self.state.scope_records.len(),
        }
    }

    fn assert_owner_thread(&self) -> Result<(), NativeRuntimeError> {
        if self.owner_thread == std::thread::current().id() {
            Ok(())
        } else {
            Err(NativeRuntimeError::Invariant(
                "NativeRuntime used from a non-owner thread".into(),
            ))
        }
    }
}

/// packed 用虚拟入口；`wjsm run` 仍优先 IR `source_file`（主机路径）。
fn process_entry_for_store(
    artifact: &PortableArtifact,
    store: &ModuleSourceStore,
) -> Result<Option<String>, NativeRuntimeError> {
    let logical = artifact
        .manifest()
        .modules
        .iter()
        .find(|module| module.id == artifact.manifest().entry)
        .filter(|module| !module.logical_url.starts_with("node:"))
        .map(|module| module.logical_url.as_str());
    if store.is_snapshot() {
        return Ok(logical.map(|url| format!("{}/{}", wjsm_module::SNAPSHOT_VIRTUAL_ROOT, url)));
    }
    if let Some(source) = artifact.program().source_file() {
        return Ok(Some(source.to_owned()));
    }
    logical
        .map(|url| wjsm_module::logical_url_path(&store.root(), url))
        .transpose()
        .map_err(|error| NativeRuntimeError::Invariant(error.to_string()))
        .map(|path| path.map(|path| path.to_string_lossy().into_owned()))
}

#[derive(Debug, Error)]
pub enum NativeRuntimeError {
    #[error("portable artifact error: {0}")]
    Artifact(String),
    #[error(transparent)]
    Compile(#[from] wjsm_backend_native::NativeCompileError),
    #[error(transparent)]
    Cache(#[from] NativeCacheError),
    #[error(transparent)]
    Heap(#[from] HeapAccessV2Error),
    #[error(transparent)]
    Gc(#[from] gc::NativeGcError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("native runtime invariant failed: {0}")]
    Invariant(String),
    #[error("RangeError: {0}")]
    Range(String),
    #[error("native pending exception: {0:?}")]
    Pending(PendingExceptionKind),
    #[error("{0}")]
    JavaScript(String),
    #[error("{0}")]
    FatalJavaScript(String),
    #[error("{0}")]
    Configuration(String),
    #[error("source compilation failed: {0}")]
    SourceCompile(String),
}

#[cfg(test)]
mod host_table_reclaim;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use wjsm_artifact_format::{
        ArtifactBuildInput, BuildOptions, ModuleManifest, PortableArtifact,
    };

    use super::*;

    #[test]
    fn default_gc_algorithm_is_zgc() {
        assert_eq!(
            NativeRuntimeConfig::default().gc_algorithm,
            GcAlgorithmKind::Zgc,
            "未显式指定时运行时必须默认使用 zgc"
        );
        assert_eq!(
            RuntimeOptions::default().gc_algorithm,
            GcAlgorithmKind::Zgc,
            "in-process 执行入口同样默认 zgc"
        );
    }
    #[test]
    fn allocation_diagnostics_follow_runtime_switch() {
        let disabled = gc::NativeGc::new(GcAlgorithmKind::Zgc, DEFAULT_MAX_HEAP_BYTES, false)
            .expect("disabled diagnostics heap should initialize");
        disabled
            .allocate(64)
            .expect("disabled diagnostics allocation should succeed");
        assert_eq!(
            disabled.allocation_diagnostics(),
            gc::NativeAllocationDiagnostics::default()
        );

        let enabled = gc::NativeGc::new(GcAlgorithmKind::Zgc, DEFAULT_MAX_HEAP_BYTES, true)
            .expect("enabled diagnostics heap should initialize");
        enabled
            .allocate(64)
            .expect("enabled diagnostics allocation should succeed");
        let diagnostics = enabled.allocation_diagnostics();
        assert_eq!(diagnostics.slow_allocations, 1);
        assert_eq!(diagnostics.tlab_refills, 1);
        assert_eq!(diagnostics.tlab_fast_allocations, 0);
    }

    #[test]
    fn child_runtime_inherits_cache_directory() {
        let cache_dir = std::path::PathBuf::from("/tmp/wjsm-native-cache");
        let parent = NativeRuntimeConfig {
            cache_dir: Some(cache_dir.clone()),
            ..NativeRuntimeConfig::default()
        };

        let child = parent.child_config();

        assert_eq!(child.cache_dir, Some(cache_dir));
        assert_eq!(child.gc_algorithm, parent.gc_algorithm);
        assert_eq!(child.max_heap_size, parent.max_heap_size);
        assert_eq!(child.specialization_enabled, parent.specialization_enabled);
        assert!(child.isolate_native_images);
    }
    fn artifact(source: &str) -> PortableArtifact {
        let source: Arc<str> = source.into();
        let ast = wjsm_parser::parse_module(&source).expect("source should parse");
        let program = wjsm_semantic::lower_module_with_source(
            ast,
            true,
            Some(Arc::clone(&source)),
            "input.js",
        )
        .expect("source should lower");
        PortableArtifact::from_input(&ArtifactBuildInput {
            program: Arc::new(program),
            manifest: Arc::new(ModuleManifest::single("input.js", true)),
            options: BuildOptions::default(),
            source_text: None,
        })
        .expect("artifact should encode")
    }

    fn execute_source(source: &str) -> NativeExecution {
        let artifact = artifact(source);
        let mut runtime = NativeRuntime::new(None).expect("native runtime should initialize");
        runtime
            .execute(
                &artifact,
                std::path::Path::new("."),
                std::path::Path::new("."),
            )
            .expect("source should execute")
    }

    /// 执行后返回（完成值句柄, 运行时），供白盒断言堆内字符串表示。
    fn execute_source_with_runtime(source: &str) -> (NativeExecution, NativeRuntime) {
        let artifact = artifact(source);
        let mut runtime = NativeRuntime::new(None).expect("native runtime should initialize");
        let execution = runtime
            .execute(
                &artifact,
                std::path::Path::new("."),
                std::path::Path::new("."),
            )
            .expect("source should execute");
        (execution, runtime)
    }

    /// 断言执行后堆内存在内容恰为 `text` 的字符串句柄且其表示为预期值。
    fn assert_string_repr(state: &NativeAgentState, text: &str, expect_latin1: bool) {
        let units = text.encode_utf16().collect::<Vec<u16>>();
        let handles = state
            .gc
            .heap()
            .capture_handles()
            .expect("capture_handles 应成功")
            .0;
        let matches = handles.into_iter().filter(|entry| {
            let handle = entry.handle.get();
            state
                .with_string_units(value::encode_runtime_string_handle(handle), |found| {
                    found == units.as_slice()
                })
                .unwrap_or(false)
        });
        let mut seen = 0;
        for entry in matches {
            let repr = state
                .gc
                .heap()
                .string_repr(entry.handle.get())
                .expect("字符串 repr 读取失败");
            assert_eq!(
                repr == wjsm_ir::constants::STRING_REPR_LATIN1_FLAT,
                expect_latin1,
                "内容 {text:?} 的句柄 {} 表示不符预期（repr={repr}）",
                entry.handle.get()
            );
            seen += 1;
        }
        assert!(seen > 0, "堆内未找到内容为 {text:?} 的字符串");
    }

    #[test]
    fn latin1_construction_paths_pick_single_byte_payload() {
        // 全 Latin-1 内容（字面量物化、builder finish）必须是单字节载荷，
        // 含 UTF-16 码元的拼接结果保持双字节。push 进存活数组强制物化并防止
        // 编译期常量折叠把用例消除。
        let (_, runtime) = execute_source_with_runtime(
            r#"const keep = [];
               let acc = "";
               for (let i = 0; i < 4; i++) acc += "seg" + i;
               keep.push("ascii-literal", acc, "\u20ac" + "ascii");
               console.log(keep.length);"#,
        );
        assert_string_repr(&runtime.state, "ascii-literal", true);
        assert_string_repr(&runtime.state, "seg0seg1seg2seg3", true);
        assert_string_repr(&runtime.state, "\u{20ac}ascii", false);
    }

    #[test]
    fn execute_precompiled_matches_compile_path() {
        let artifact = artifact("console.log(1 + 2)");
        let images =
            compile_native_exec_images(&artifact).expect("precompiled images should compile");
        let mut runtime = NativeRuntime::new_with_config(
            NativeRuntimeConfig::default().with_specialization_enabled(false),
        )
        .expect("native runtime should initialize");
        let execution = runtime
            .execute_precompiled(
                &artifact,
                &images,
                ModuleSourceStore::disk(std::path::Path::new(".")),
                std::path::Path::new("."),
            )
            .expect("precompiled source should execute");
        assert_eq!(execution.stdout, b"3\n");
        assert_eq!(execution.exit_code, 0);
    }

    #[test]
    fn large_array_churn_collects_before_cooperative_poll() {
        let artifact = artifact(
            r#"
                let holder;
                let sink = 0;
                for (let i = 0; i < 12; i++) {
                    const tmp = new Array(1 << 16);
                    tmp[0] = i;
                    holder = tmp;
                    sink = holder[0];
                }
                console.log("done", sink, holder.length);
            "#,
        );
        for algorithm in [
            GcAlgorithmKind::MarkSweep,
            GcAlgorithmKind::G1,
            GcAlgorithmKind::Zgc,
        ] {
            let config = NativeRuntimeConfig::default()
                .with_gc_algorithm(algorithm)
                .with_max_heap_size(16 * 1024 * 1024);
            let mut runtime = NativeRuntime::new_with_config(config)
                .unwrap_or_else(|error| panic!("{algorithm:?} runtime should initialize: {error}"));
            let execution = runtime
                .execute(
                    &artifact,
                    std::path::Path::new("."),
                    std::path::Path::new("."),
                )
                .unwrap_or_else(|error| {
                    panic!("{algorithm:?} bounded live set should complete: {error:?}")
                });

            assert_eq!(execution.stdout, b"done 11 65536\n", "{algorithm:?}");
            assert!(
                runtime.gc_telemetry().cycles > 0,
                "{algorithm:?} should collect before the 128-backedge poll budget"
            );
        }
    }

    #[test]
    fn inline_ascii_and_property_keys_survive_zgc() {
        let artifact = artifact(
            r#"
                const s = "abcdef";
                const o = {};
                o.name = 1;
                gc();
                console.log(s.length, s === "abcdef", s[5], o.name, Object.keys(o)[0]);
            "#,
        );
        let config = NativeRuntimeConfig::default()
            .with_gc_algorithm(GcAlgorithmKind::Zgc)
            .with_allocation_diagnostics_enabled(true);
        let mut runtime =
            NativeRuntime::new_with_config(config).expect("ZGC runtime should initialize");
        let before = runtime.host_side_table_stats().string_ids;
        let execution = runtime.execute(
            &artifact,
            std::path::Path::new("."),
            std::path::Path::new("."),
        );
        let execution = execution.unwrap_or_else(|error| {
            panic!(
                "ZGC SSO execution failed: {error:?}; stderr={:?}",
                runtime.take_stderr()
            )
        });
        assert_eq!(execution.stdout, b"6 true f 1 name\n");
        assert_eq!(runtime.host_side_table_stats().string_ids, before);
        let diagnostics = runtime.allocation_diagnostics();
        assert!(
            diagnostics.inline_string_constructions > 0,
            "{diagnostics:?}"
        );
        assert!(diagnostics.inline_property_keys > 0, "{diagnostics:?}");
    }

    #[test]
    fn inline_ascii_operations_stay_inline_under_zgc() {
        let artifact = artifact(
            r#"
                const s = "abcdef";
                console.log(
                    typeof s,
                    s.length,
                    s[0],
                    s[5],
                    s.charAt(1),
                    s.charCodeAt(2),
                    s === "abcdef",
                    s.slice(0, 3),
                    s.at(-1)
                );
            "#,
        );
        let config = NativeRuntimeConfig::default()
            .with_gc_algorithm(GcAlgorithmKind::Zgc)
            .with_allocation_diagnostics_enabled(true);
        let mut runtime =
            NativeRuntime::new_with_config(config).expect("ZGC runtime should initialize");
        let before = runtime.host_side_table_stats().string_ids;
        let execution = runtime
            .execute(
                &artifact,
                std::path::Path::new("."),
                std::path::Path::new("."),
            )
            .expect("ZGC SSO operations should execute");
        assert_eq!(execution.stdout, b"string 6 a f b 99 true abc f\n");
        assert_eq!(runtime.host_side_table_stats().string_ids, before);
        assert!(
            runtime.allocation_diagnostics().inline_string_constructions > 0,
            "{:?}",
            runtime.allocation_diagnostics()
        );
    }

    #[test]
    fn array_literal_push_avoids_excessive_tlab_flushes() {
        let artifact = artifact(
            r#"
                let total = 0;
                for (let i = 0; i < 32; i++) {
                    const array = [1, 2, 3, 4, 5, 6];
                    total += array[0] + array[5] + array.length;
                }
                console.log(total);
            "#,
        );
        let config = NativeRuntimeConfig::default()
            .with_gc_algorithm(GcAlgorithmKind::Zgc)
            .with_allocation_diagnostics_enabled(true);
        let mut runtime =
            NativeRuntime::new_with_config(config).expect("runtime should initialize");
        let execution = runtime
            .execute(
                &artifact,
                std::path::Path::new("."),
                std::path::Path::new("."),
            )
            .expect("array literal workload should execute");
        assert_eq!(execution.stdout, b"416\n");
        let diagnostics = runtime.allocation_diagnostics();
        assert!(
            diagnostics.tlab_fast_allocations >= 32,
            "{diagnostics:?}"
        );
        assert!(
            diagnostics.tlab_flushes <= 2,
            "packed array push should not flush TLAB per element: {diagnostics:?}"
        );
    }

    #[test]
    fn tlab_object_template_literal_fast_path() {
        let artifact = artifact(
            r#"
                let total = 0;
                for (let i = 0; i < 64; i++) {
                    const object = { name: i, value: i * 2, length: i + 1 };
                    total += object.name + object.value + object.length;
                }
                console.log(total);
            "#,
        );
        let config = NativeRuntimeConfig::default()
            .with_gc_algorithm(GcAlgorithmKind::Zgc)
            .with_allocation_diagnostics_enabled(true);
        let mut runtime =
            NativeRuntime::new_with_config(config).expect("runtime should initialize");
        let execution = runtime
            .execute(
                &artifact,
                std::path::Path::new("."),
                std::path::Path::new("."),
            )
            .expect("tlab object template workload should execute");
        assert_eq!(execution.stdout, b"8128\n");
        let diagnostics = runtime.allocation_diagnostics();
        assert!(
            diagnostics.tlab_fast_allocations >= 64,
            "{diagnostics:?}"
        );
    }

    #[test]
    fn native_tlab_fast_allocations_are_observable() {
        let artifact = artifact(
            r#"
                let sink = 0;
                for (let i = 0; i < 64; i++) {
                    const object = {};
                    object.name = i;
                    const array = [i, i];
                    sink += object.name + array[0] + array[1];
                }
                console.log(sink);
            "#,
        );
        let config = NativeRuntimeConfig::default()
            .with_gc_algorithm(GcAlgorithmKind::Zgc)
            .with_allocation_diagnostics_enabled(true);
        let mut runtime =
            NativeRuntime::new_with_config(config).expect("runtime should initialize");
        let execution = runtime.execute(
            &artifact,
            std::path::Path::new("."),
            std::path::Path::new("."),
        );
        let execution = execution.unwrap_or_else(|error| {
            panic!(
                "native TLAB workload should execute: {error:?}; stderr={:?}",
                runtime.take_stderr()
            )
        });
        assert_eq!(execution.stdout, b"6048\n");
        assert_ne!(
            runtime.vmctx.allocation_fast_flags & wjsm_native_abi::NATIVE_ALLOCATION_FAST_HOST,
            0,
            "allocation flags: {}",
            runtime.vmctx.allocation_fast_flags
        );
        assert_ne!(
            runtime.vmctx.allocation_fast_flags & wjsm_native_abi::NATIVE_ALLOCATION_FAST_OBJECT,
            0,
            "object fast path must be enabled under idle ZGC"
        );
        assert_ne!(
            runtime.vmctx.allocation_fast_flags & wjsm_native_abi::NATIVE_ALLOCATION_FAST_ARRAY,
            0,
            "array fast path must be enabled under idle ZGC"
        );
        assert!(runtime.vmctx.allocation_small_limit > 0);
        assert!(runtime.vmctx.bump_ptr < runtime.vmctx.bump_limit);
        assert!(runtime.vmctx.bump_handle_cursor < runtime.vmctx.bump_handle_limit);
        let diagnostics = runtime.allocation_diagnostics();
        assert!(
            diagnostics.tlab_fast_allocations >= 128,
            "{diagnostics:?}, small_limit={} cursor={}/{} ptr={}/{} flags={}",
            runtime.vmctx.allocation_small_limit,
            runtime.vmctx.bump_handle_cursor,
            runtime.vmctx.bump_handle_limit,
            runtime.vmctx.bump_ptr,
            runtime.vmctx.bump_limit,
            runtime.vmctx.allocation_fast_flags
        );
        assert!(diagnostics.tlab_fast_bytes > 0, "{diagnostics:?}");
        assert!(diagnostics.tlab_refills > 0, "{diagnostics:?}");
    }
    #[test]
    fn unbounded_string_accumulation_survives_gc_pressure() {
        // 单个 builder 无界增长 + 循环内小字符串拼接：zgc 年代耗尽时 mutator 必须
        // 推进 GC 后重试发布，而非误报 InternalInvariant（回归：binary_add 裸 intern）。
        let artifact = artifact(
            r#"
                let s = "";
                for (let i = 0; i < 5000; i++) {
                    s += "x" + i;
                }
                console.log(s.length);
            "#,
        );
        for algorithm in [
            GcAlgorithmKind::MarkSweep,
            GcAlgorithmKind::G1,
            GcAlgorithmKind::Zgc,
        ] {
            let config = NativeRuntimeConfig::default()
                .with_gc_algorithm(algorithm)
                .with_max_heap_size(16 * 1024 * 1024);
            let mut runtime = NativeRuntime::new_with_config(config)
                .unwrap_or_else(|error| panic!("{algorithm:?} runtime should initialize: {error}"));
            let execution = runtime
                .execute(
                    &artifact,
                    std::path::Path::new("."),
                    std::path::Path::new("."),
                )
                .unwrap_or_else(|error| {
                    panic!("{algorithm:?} accumulation should complete: {error:?}")
                });
            assert_eq!(execution.stdout, b"23890\n", "{algorithm:?}");
            assert_eq!(execution.exit_code, 0, "{algorithm:?}");
        }
    }

    fn assert_live_set_exhaustion_is_observable_range_error(algorithm: GcAlgorithmKind) {
        let oom_artifact = artifact(
            r#"
                const retained = new Array(1 << 20);
                function exhaust(label, mutate) {
                    try {
                        const impossible = new Array(1 << 20);
                        console.log("unexpected", impossible.length);
                    } catch (e) {
                        console.log(
                            label,
                            e instanceof RangeError,
                            e.name,
                            e.message,
                            Object.isFrozen(e),
                        );
                        if (mutate) {
                            Reflect.set(e, "name", "Error");
                            Reflect.set(e, "message", "polluted");
                            Reflect.deleteProperty(e, "message");
                            Reflect.setPrototypeOf(e, null);
                        }
                    }
                }
                exhaust("first", true);
                exhaust("second", false);
            "#,
        );
        let lifecycle_artifact = artifact(
            r#"
                try {
                    new Array(1 << 22);
                } catch (e) {
                    console.log(e instanceof RangeError, e.name, e.message);
                }
            "#,
        );
        let expected = b"first true RangeError JavaScript heap out of memory true\n\
second true RangeError JavaScript heap out of memory true\n";
        let config = NativeRuntimeConfig::default()
            .with_gc_algorithm(algorithm)
            .with_max_heap_size(16 * 1024 * 1024);
        let mut runtime = NativeRuntime::new_with_config(config)
            .unwrap_or_else(|error| panic!("{algorithm:?} runtime should initialize: {error}"));
        let execution = runtime
            .execute(
                &oom_artifact,
                std::path::Path::new("."),
                std::path::Path::new("."),
            )
            .unwrap_or_else(|error| {
                panic!("{algorithm:?} should catch OOM as RangeError: {error:?}")
            });
        let oom_error = runtime
            .state
            .out_of_memory_error
            .expect("OOM error should stay rooted");
        assert_eq!(
            runtime
                .state
                .exceptions
                .iter()
                .filter(|entry| **entry == Some(oom_error))
                .count(),
            1,
            "{algorithm:?} should keep one entry for the dedicated OOM error",
        );
        assert_eq!(execution.stdout, expected, "{algorithm:?}");

        let lifecycle_execution = runtime
            .execute(
                &lifecycle_artifact,
                std::path::Path::new("."),
                std::path::Path::new("."),
            )
            .unwrap_or_else(|error| {
                panic!("{algorithm:?} should rebuild the OOM error after reset: {error:?}")
            });
        assert_eq!(
            lifecycle_execution.stdout, b"true RangeError JavaScript heap out of memory\n",
            "{algorithm:?} reset lifecycle",
        );
        let oom_error = runtime
            .state
            .out_of_memory_error
            .expect("reset should rebuild the OOM error");
        assert_eq!(
            runtime
                .state
                .exceptions
                .iter()
                .filter(|entry| **entry == Some(oom_error))
                .count(),
            1,
            "{algorithm:?} reset should rebuild one entry for the OOM error",
        );
    }

    #[test]
    fn live_set_exhaustion_is_observable_range_error_mark_sweep() {
        assert_live_set_exhaustion_is_observable_range_error(GcAlgorithmKind::MarkSweep);
    }

    #[test]
    fn live_set_exhaustion_is_observable_range_error_g1() {
        assert_live_set_exhaustion_is_observable_range_error(GcAlgorithmKind::G1);
    }

    #[test]
    fn live_set_exhaustion_is_observable_range_error_zgc() {
        assert_live_set_exhaustion_is_observable_range_error(GcAlgorithmKind::Zgc);
    }

    fn execute_source_with_specialization(source: &str, enabled: bool) -> NativeExecution {
        let artifact = artifact(source);
        let config = NativeRuntimeConfig::default().with_specialization_enabled(enabled);
        let mut runtime =
            NativeRuntime::new_with_config(config).expect("native runtime should initialize");
        runtime
            .execute(
                &artifact,
                std::path::Path::new("."),
                std::path::Path::new("."),
            )
            .expect("source should execute")
    }

    #[test]
    fn specialization_toggle_preserves_drift_output() {
        let source = r#"
            function addOne(value) { return value + 1; }
            function invoke(value) { return addOne(value); }
            let sum = 0;
            for (let i = 0; i < 150; i++) sum += invoke(i);
            console.log(sum, invoke("x"));
            try { invoke(1n); } catch (error) { console.log(error.name); }
            console.log(invoke({ valueOf() { return 9; } }));
        "#;
        let enabled = execute_source_with_specialization(source, true);
        let disabled = execute_source_with_specialization(source, false);
        assert_eq!(enabled.stdout, disabled.stdout);
        assert_eq!(enabled.stderr, disabled.stderr);
        assert_eq!(enabled.exit_code, disabled.exit_code);
        assert_eq!(enabled.value, disabled.value);
    }

    fn empty_fn(name: &str) -> wjsm_ir::Function {
        let mut function = wjsm_ir::Function::new(name, wjsm_ir::BasicBlockId(0));
        let mut block = wjsm_ir::BasicBlock::new(wjsm_ir::BasicBlockId(0));
        block.set_terminator(wjsm_ir::Terminator::Return { value: None });
        function.push_block(block);
        function
    }

    fn dual_image_program(user_marker: f64) -> PortableArtifact {
        let mut program = wjsm_ir::Program::new();
        let forty_two = program.add_constant(wjsm_ir::Constant::Number(42.0));
        let marker = program.add_constant(wjsm_ir::Constant::Number(user_marker));
        let builtin_ref =
            program.add_constant(wjsm_ir::Constant::FunctionRef(wjsm_ir::FunctionId(1)));
        let undefined = program.add_constant(wjsm_ir::Constant::Undefined);

        program.push_function(empty_fn("builtin_helper"));

        let mut builtin_main = wjsm_ir::Function::new("$builtin_main", wjsm_ir::BasicBlockId(0));
        let mut builtin_block = wjsm_ir::BasicBlock::new(wjsm_ir::BasicBlockId(0));
        builtin_block.push_instruction(wjsm_ir::Instruction::Const {
            dest: wjsm_ir::ValueId(0),
            constant: forty_two,
        });
        builtin_block.push_instruction(wjsm_ir::Instruction::StoreVar {
            name: "$1.answer".into(),
            value: wjsm_ir::ValueId(0),
        });
        builtin_block.set_terminator(wjsm_ir::Terminator::Return { value: None });
        builtin_main.push_block(builtin_block);
        program.push_function(builtin_main);

        let mut module_main = wjsm_ir::Function::new("$module_main", wjsm_ir::BasicBlockId(0));
        let mut user_block = wjsm_ir::BasicBlock::new(wjsm_ir::BasicBlockId(0));
        user_block.push_instruction(wjsm_ir::Instruction::Const {
            dest: wjsm_ir::ValueId(0),
            constant: builtin_ref,
        });
        user_block.push_instruction(wjsm_ir::Instruction::Const {
            dest: wjsm_ir::ValueId(1),
            constant: undefined,
        });
        user_block.push_instruction(wjsm_ir::Instruction::Call {
            dest: Some(wjsm_ir::ValueId(2)),
            callee: wjsm_ir::ValueId(0),
            this_val: wjsm_ir::ValueId(1),
            args: Vec::new(),
        });
        user_block.push_instruction(wjsm_ir::Instruction::LoadVar {
            dest: wjsm_ir::ValueId(3),
            name: "$1.answer".into(),
        });
        user_block.push_instruction(wjsm_ir::Instruction::Const {
            dest: wjsm_ir::ValueId(4),
            constant: marker,
        });
        user_block.set_terminator(wjsm_ir::Terminator::Return {
            value: Some(wjsm_ir::ValueId(3)),
        });
        module_main.push_block(user_block);
        program.push_function(module_main);

        PortableArtifact::from_input(&ArtifactBuildInput {
            program: Arc::new(program),
            manifest: Arc::new(ModuleManifest::single("input.js", true)),
            options: BuildOptions::default(),
            source_text: None,
        })
        .expect("dual-image artifact should encode")
    }

    #[test]
    fn shared_variables_survive_builtin_call_and_reuse_image() {
        let first = dual_image_program(1.0);
        let second = dual_image_program(2.0);
        let mut runtime = NativeRuntime::new(None).expect("native runtime should initialize");
        let first_execution = runtime
            .execute(&first, std::path::Path::new("."), std::path::Path::new("."))
            .expect("first dual-image program should execute");
        assert_eq!(wjsm_ir::value::decode_f64(first_execution.value), 42.0);
        let second_execution = runtime
            .execute(
                &second,
                std::path::Path::new("."),
                std::path::Path::new("."),
            )
            .expect("second dual-image program should execute");
        assert_eq!(wjsm_ir::value::decode_f64(second_execution.value), 42.0);
        assert!(
            runtime.state.repository.stats().hits >= 1,
            "同一 builtin 段第二次 execute 必须命中 native image cache"
        );
    }

    #[test]
    fn executes_console_arithmetic_from_source() {
        let execution = execute_source("console.log(1 + 2)");
        assert_eq!(execution.stdout, b"3\n");
    }

    #[test]
    fn startup_snapshot_isolates_consecutive_realms() {
        let first =
            artifact("globalThis.leaked = { value: 42 }; console.log(globalThis.leaked.value)");
        let second = artifact("console.log('leaked' in globalThis, globalThis.leaked)");
        let mut runtime = NativeRuntime::new(None).expect("native runtime should initialize");

        let first_execution = runtime
            .execute(&first, std::path::Path::new("."), std::path::Path::new("."))
            .expect("first realm should execute");
        let second_execution = runtime
            .execute(
                &second,
                std::path::Path::new("."),
                std::path::Path::new("."),
            )
            .expect("second realm should execute");

        assert_eq!(first_execution.stdout, b"42\n");
        assert_eq!(second_execution.stdout, b"false undefined\n");
    }

    #[test]
    fn executes_promise_and_queue_microtask_order() {
        let execution = execute_source(
            "console.log(1); Promise.resolve().then(() => console.log(3)); queueMicrotask(() => console.log(2)); console.log(0);",
        );
        assert_eq!(execution.stdout, b"1\n0\n3\n2\n");
    }

    #[test]
    fn executes_promise_reaction_chain() {
        let execution = execute_source(
            "Promise.resolve(1).then(value => value + 1).then(value => console.log(value)); console.log(0);",
        );
        assert_eq!(execution.stdout, b"0\n2\n");
    }

    #[test]
    fn executes_queue_microtask_global_as_function() {
        let execution = execute_source(
            "console.log(typeof queueMicrotask); queueMicrotask(() => console.log('queued')); console.log('sync');",
        );
        assert_eq!(execution.stdout, b"function\nsync\nqueued\n");
    }
    #[test]
    fn executes_async_await_after_sync_entry() {
        let execution = execute_source(
            "async function run() { await Promise.resolve('hello'); console.log('async'); } run(); console.log('sync');",
        );
        assert_eq!(execution.stdout, b"sync\nasync\n");
    }
    #[test]
    fn executes_async_await_rejection_catch() {
        let execution = execute_source(
            "async function run() { try { await Promise.reject('boom'); } catch (error) { console.log(error); } } run(); console.log('sync');",
        );
        assert_eq!(execution.stdout, b"sync\nboom\n");
    }
    #[test]
    fn executes_variable_string_and_dynamic_operations() {
        let execution = execute_source(
            "let message = 'hello'; message = message + ' native'; console.log(message, !0, 5 % 2)",
        );
        assert_eq!(execution.stdout, b"hello native true 1\n");
    }
    #[test]
    fn executes_object_and_array_operations() {
        let execution = execute_source(
            "let object = { answer: 1 }; object.answer = 4; let array = [2, 3]; console.log(object.answer, array[1])",
        );
        assert_eq!(execution.stdout, b"4 3\n");
    }
    #[test]
    fn executes_dynamic_function_calls() {
        let execution = execute_source(
            "function add(a, b) { return a + b; } let f = add; console.log(f(2, 3))",
        );
        assert_eq!(execution.stdout, b"5\n");
    }

    #[test]
    fn executes_recursive_function_calls() {
        let execution = execute_source(
            "function factorial(n) { if (n === 0) return 1; return n * factorial(n - 1); } console.log(factorial(5))",
        );
        assert_eq!(execution.stdout, b"120\n");
    }
    #[test]
    fn executes_loop_control_flow() {
        let execution =
            execute_source("let sum = 0; for (let i = 0; i < 5; i++) sum += i; console.log(sum)");
        assert_eq!(execution.stdout, b"10\n");
    }
    #[test]
    fn executes_abstract_equality_fixture() {
        let execution = execute_source(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/happy/abstract_eq.js"
        )));
        assert_eq!(
            execution.stdout,
            b"true\ntrue\nfalse\ntrue\ntrue\ntrue\ntrue\nfalse\ntrue\ntrue\ntrue\ntrue\ntrue\nfalse\nfalse\ntrue\ntrue\ntrue\nfalse\nfalse\n"
        );
    }

    #[test]
    fn executes_switch_fixture() {
        let execution = execute_source(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/happy/switch_nonliteral.js"
        )));
        assert_eq!(
            execution.stdout,
            b"function call match\nfunction call match 2\nstring match\ncomputed match\n"
        );
    }

    #[test]
    fn executes_nested_closure_fixture() {
        let execution = execute_source(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/happy/closure_nested_capture.js"
        )));
        assert_eq!(execution.stdout, b"1\n5\n2\n");
    }
    #[test]
    fn executes_cross_call_try_catch() {
        let execution = execute_source(
            "function fail() { throw 42; } try { fail(); } catch (error) { console.log(error); }",
        );
        assert_eq!(execution.stdout, b"42\n");
    }
    #[test]
    fn executes_dynamic_constructor_and_typeof() {
        let execution = execute_source(
            "function Point(x) { this.x = x; } const Constructor = Point; const point = new Constructor(7); console.log(point.x, typeof point, typeof Constructor);",
        );
        assert_eq!(execution.stdout, b"7 object function\n");
    }
    #[test]
    fn executes_array_length_reads_and_writes() {
        let execution = execute_source(
            "const values = [1, 2, 3]; console.log(values.length); values.length = 1; console.log(values.length, values[1]);",
        );
        assert_eq!(execution.stdout, b"3\n1 undefined\n");
    }
    #[test]
    fn executes_class_prototype_setup() {
        let execution = execute_source(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/happy/class_prototype_constructor.js"
        )));
        assert_eq!(execution.stdout, b"true\nobject\n");
    }
    #[test]
    fn executes_function_prototype_fixture() {
        let execution = execute_source(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/happy/func_prototype.js"
        )));
        assert_eq!(
            execution.stdout,
            b"object\ntrue\n42\ntrue\ntrue\ntrue\nhello\nundefined\nobject\ntrue\n"
        );
    }
    #[test]
    fn executes_rest_arguments() {
        let execution = execute_source(
            "function collect(first, ...rest) { console.log(first, rest.length, rest[0], rest[1]); } collect(1, 2, 3);",
        );
        assert_eq!(execution.stdout, b"1 2 2 3\n");
    }
    #[test]
    fn executes_object_spread() {
        let execution = execute_source(
            "const source = { a: 1, b: 2 }; const target = { ...source, b: 3 }; console.log(target.a, target.b);",
        );
        assert_eq!(execution.stdout, b"1 3\n");
    }
    #[test]
    fn executes_optional_property_access() {
        let execution = execute_source(
            "const missing = null; const object = { x: 2 }; const array = [3]; console.log(missing?.x, object?.x, array?.[0]);",
        );
        assert_eq!(execution.stdout, b"undefined 2 3\n");
    }
    #[test]
    fn executes_optional_calls() {
        let execution = execute_source(
            "const missing = null; console.log(missing?.(1)); const increment = (value) => value + 1; console.log(increment?.(2));",
        );
        assert_eq!(execution.stdout, b"undefined\n3\n");
    }
    #[test]
    fn executes_object_collection_builtins() {
        let execution = execute_source(
            "const prototype = { inherited: 1 }; const object = Object.create(prototype); object.a = 2; object.b = 3; const keys = Object.keys(object); const values = Object.values(object); const entries = Object.entries(object); const assigned = Object.assign({}, object, { b: 4 }); const names = Object.getOwnPropertyNames(assigned); console.log(Object.getPrototypeOf(object) === prototype, keys.length, keys[0], keys[1], values[0], values[1], entries[0][0], entries[0][1]); console.log(assigned.a, assigned.b, names.length, Object.is(NaN, NaN), Object.is(0, -0));",
        );
        assert_eq!(execution.stdout, b"true 2 a b 2 3 a 2\n2 4 2 true false\n");
    }
    #[test]
    fn executes_math_builtins() {
        let execution = execute_source(
            "console.log(Math.abs(-3), Math.floor(1.9), Math.round(-1.5), Math.hypot(3, 4)); console.log(Math.imul(0xffffffff, 5), Math.clz32(1), Math.max(...[1, 5, 2])); console.log(Object.is(Math.max(-0, 0), 0), Object.is(Math.min(-0, 0), -0), Math.sign(-2)); console.log(0xffffffff | 0, 0xffffffff >>> 0, ~0xffffffff);",
        );
        assert_eq!(
            execution.stdout,
            b"3 1 -1 5\n-5 31 5\ntrue true -1\n-1 4294967295 0\n"
        );
    }
    #[test]
    fn executes_number_and_boolean_builtins() {
        let execution = execute_source(
            "console.log(Number('12.5'), Number(), Number(null), Number(true)); console.log(Number.isNaN(NaN), Number.isFinite(Infinity), Number.isInteger(2), Number.isSafeInteger(9007199254740991)); console.log((255).toString(16), (1.25).toFixed(2), Boolean(0), Boolean('x')); console.log(true.toString(), false.valueOf());",
        );
        assert_eq!(
            execution.stdout,
            b"12.5 0 0 1\ntrue false true true\nff 1.25 false true\ntrue false\n"
        );
    }
    #[test]
    fn executes_string_builtins_with_utf16_indices() {
        let execution = execute_source(
            "const text = 'A😀B'; console.log(text.length, text.at(-1), text.charCodeAt(1), text.codePointAt(1)); console.log('hello'.includes('ell'), 'hello'.indexOf('l'), 'hello'.lastIndexOf('l'), 'hello'.startsWith('he'), 'hello'.endsWith('lo')); console.log('x'.padStart(3, '0'), 'x'.padEnd(3, '0'), 'ab'.repeat(3), 'abcdef'.slice(-3), 'abcdef'.substring(4, 1)); console.log(' AbC '.trim().toLowerCase(), String.fromCharCode(65, 0xD83D, 0xDE00), String.fromCodePoint(0x1F600)); console.log(String.fromCodePoint(0xD800).charCodeAt(0));",
        );
        assert_eq!(
            execution.stdout,
            "4 B 55357 128512\ntrue 2 3 true true\n00x x00 ababab def bcd\nabc A😀 😀\n55296\n"
                .as_bytes()
        );
    }
    #[test]
    fn executes_array_value_builtins() {
        let execution = execute_source(
            "const values = [1, 2, 3]; console.log(values.concat([4, 5]).join(','), values.slice(1).join(','), values.at(-1)); values.fill(0, 1, 2); console.log(values.join(','), values.reverse().join(',')); console.log(values.shift(), values.join(','), values.unshift(7, 8), values.join(',')); console.log([1, , 3].includes(undefined), [1, , 3].indexOf(undefined), [1, , 3].join('-')); console.log([1, 2, 3].toReversed().join(','), [1, 2, 3].with(-1, 9).join(','), Array.of(4, 5).join(','), Array.isArray(values));",
        );
        assert_eq!(
            execution.stdout,
            b"1,2,3,4,5 2,3 3\n1,0,3 3,0,1\n3 0,1 4 7,8,0,1\ntrue -1 1--3\n3,2,1 1,2,9 4,5 true\n"
        );
    }
    #[test]
    fn executes_function_call_and_apply() {
        let execution = execute_source(
            "function add(left, right) { return this.base + left + right; } const target = add; console.log(target.call({ base: 1 }, 2, 3), target.apply({ base: 4 }, [5, 6]));",
        );
        assert_eq!(execution.stdout, b"6 15\n");
    }
    #[test]
    fn executes_array_callback_builtins() {
        let execution = execute_source(
            "const values = [1, 2, 3, 4]; console.log(values.map((value) => value * 2).join(','), values.filter((value) => value % 2 === 0).join(','), values.reduce((sum, value) => sum + value, 0)); console.log(values.find((value) => value > 2), values.findIndex((value) => value > 2), values.some((value) => value === 4), values.every((value) => value > 0)); console.log([3, 1, 2].sort((left, right) => left - right).join(','), [3, 1, 2].toSorted((left, right) => right - left).join(','), values.flatMap((value) => [value, value]).join(','));",
        );
        assert_eq!(
            execution.stdout,
            b"2,4,6,8 2,4 10\n3 2 true true\n1,2,3 3,2,1 1,1,2,2,3,3,4,4\n"
        );
    }
    #[test]
    fn executes_object_static_builtins() {
        let execution = execute_source(
            "const prototype = { inherited: 9 }; const object = Object.create(prototype); Object.assign(object, { a: 1, b: 2 }); const entries = Object.entries(object); console.log(Object.keys(object).join(','), Object.values(object).join(','), entries[0].join('-'), Object.getPrototypeOf(object) === prototype, Object.hasOwn(object, 'a'), object.inherited); Object.defineProperty(object, 'hidden', { value: 3, enumerable: false, writable: false, configurable: false }); const descriptor = Object.getOwnPropertyDescriptor(object, 'hidden'); console.log(Object.keys(object).join(','), Object.getOwnPropertyNames(object).join(','), descriptor.value, descriptor.enumerable, Object.is(NaN, NaN), Object.is(0, -0)); Object.preventExtensions(object); console.log(Object.isExtensible(object), Object.isSealed(Object.seal({ x: 1 })), Object.isFrozen(Object.freeze({ y: 2 })));",
        );
        assert_eq!(
            execution.stdout,
            b"a,b 1,2 a-1 true true 9\na,b a,b,hidden 3 false true false\nfalse true true\n"
        );
    }
    #[test]
    fn executes_json_builtins() {
        let execution = execute_source(
            "const parsed = JSON.parse('{\"a\":[1,true,null],\"text\":\"😀\"}'); console.log(parsed.a[1], parsed.text, JSON.stringify(parsed)); console.log(JSON.stringify([undefined, NaN, Infinity]));",
        );
        assert_eq!(
            execution.stdout,
            "true 😀 {\"a\":[1,true,null],\"text\":\"😀\"}\n[null,null,null]\n".as_bytes()
        );
    }
    #[test]
    fn executes_bigint_builtins() {
        let execution = execute_source(
            "console.log(10n + 3n, 10n - 3n, 10n * 3n, 10n / 3n, 10n % 3n, 2n ** 8n); console.log(5n & 3n, 5n | 2n, 5n ^ 1n, 1n << 5n, 32n >> 2n, ~0n); console.log(10n === BigInt('10'), typeof 10n);",
        );
        assert_eq!(
            execution.stdout,
            b"13 7 30 3 1 256\n1 7 4 32 8 -1\ntrue bigint\n"
        );
    }
    #[test]
    fn executes_symbol_builtins() {
        let execution = execute_source(
            "const first = Symbol('x'); const second = Symbol('x'); const shared = Symbol.for('key'); console.log(typeof first, first, first === second); console.log(shared === Symbol.for('key'), Symbol.keyFor(shared));",
        );
        assert_eq!(execution.stdout, b"symbol Symbol(x) false\ntrue key\n");
    }
    #[test]
    fn executes_regexp_builtins() {
        let execution = execute_source(
            "const regexp = /a(.)/gi; const match = regexp.exec('Aba'); console.log(match[0], match[1], match.index, match.input, regexp.lastIndex); console.log(regexp.test('Aba'), regexp.lastIndex, regexp.source, regexp.flags, regexp.global, regexp.ignoreCase); const dynamic = RegExp('b+', 'g'); console.log(dynamic.test('abbb'), dynamic.lastIndex);",
        );
        assert_eq!(
            execution.stdout,
            b"Ab b 0 Aba 2\nfalse 0 a(.) gi true true\ntrue 4\n"
        );
    }
    #[test]
    fn executes_string_regexp_builtins() {
        let execution = execute_source(
            "const text = 'ab ac'; console.log(text.match(/a./g).join(','), text.search(/ac/), text.replace(/a(.)/g, 'x$1'), text.replaceAll(' ', '-'), 'a,b,c'.split(',').join('|')); console.log(text.replace(/a(.)/g, (match, capture, index) => capture + index));",
        );
        assert_eq!(execution.stdout, b"ab,ac 3 xb xc ab-ac a|b|c\nb0 c3\n");
    }
    #[test]
    fn executes_string_match_all_builtin() {
        let execution = execute_source(
            "const iterator = 'a1a2'.matchAll(/a([0-9])/g); const first = iterator.next(); const second = iterator.next(); const end = iterator.next(); console.log(first.value[0], first.value[1], first.value.index, second.value[0], end.done);",
        );
        assert_eq!(execution.stdout, b"a1 1 0 a2 true\n");
    }
    #[test]
    fn executes_proxy_reflect_builtins() {
        let execution = execute_source(
            "const target = { a: 1 }; const proxy = new Proxy(target, { set(object, key, value) { object[key] = value * 2; return true; }, has(object, key) { return key === 'virtual' || Object.hasOwn(object, key); }, deleteProperty(object, key) { return Reflect.deleteProperty(object, key); } }); proxy.b = 3; console.log(target.b, Reflect.set(proxy, 'c', 4), target.c, Reflect.has(proxy, 'virtual'), Reflect.has(proxy, 'a')); console.log(Reflect.deleteProperty(proxy, 'a'), Object.hasOwn(target, 'a')); const revocable = Proxy.revocable({ x: 9 }, {}); console.log(revocable.proxy.x); revocable.revoke();",
        );
        assert_eq!(execution.stdout, b"6 true 8 true true\ntrue false\n9\n");
    }
    #[test]
    fn executes_proxy_reflect_extended_builtins() {
        let execution = execute_source(
            "const proto = { marker: 1 }; const target = { a: 1, b: 2 }; const proxy = new Proxy(target, { ownKeys() { return ['b', 'a']; }, getOwnPropertyDescriptor(object, key) { return { value: object[key], enumerable: key === 'b', configurable: true, writable: true }; }, getPrototypeOf() { return proto; }, setPrototypeOf() { return true; } }); console.log(Reflect.ownKeys(proxy).join(','), Object.keys(proxy).join(','), Reflect.getPrototypeOf(proxy) === proto, Reflect.setPrototypeOf(proxy, proto)); function add(value) { return this.base + value; } const callable = new Proxy(add, { apply(target, thisValue, args) { return target.call(thisValue, args[0] * 2); } }); console.log(Reflect.apply(callable, { base: 1 }, [3]), callable.call({ base: 2 }, 4)); function Constructor(value) { this.value = value; } const constructable = new Proxy(Constructor, { construct(target, args) { return { value: args[0] * 2 }; } }); console.log(new constructable(3).value, Reflect.construct(constructable, [4]).value);",
        );
        assert_eq!(execution.stdout, b"b,a b true true\n7 10\n6 8\n");
    }
    #[test]
    fn executes_map_set_builtins() {
        let execution = execute_source(
            "const map = new Map([['a', 1], ['b', 2]]); map.set('c', 3); const map_iterator = map.entries(); const first = map_iterator.next(); const second = map_iterator.next(); const set = new Set([1, 2]); const set_iterator = set.values(); console.log(first.value[0], first.value[1], first.done, second.value[0], set_iterator.next().value, set_iterator.next().done);",
        );
        assert_eq!(execution.stdout, b"a 1 false b 1 false\n");
    }
    #[test]
    fn executes_array_buffer_data_view_builtins() {
        let execution = execute_source(
            "const buffer = new ArrayBuffer(8); const view = new DataView(buffer); view.setInt16(0, 258, true); view.setFloat32(2, 1.5, false); console.log(buffer.byteLength, view.byteLength, view.byteOffset, view.getInt16(0, true), view.getUint8(0), view.getUint8(1), view.getFloat32(2, false)); console.log(buffer.slice(0, 2).byteLength);",
        );
        assert_eq!(execution.stdout, b"8 8 0 258 2 1 1.5\n2\n");
    }
    #[test]
    fn executes_typed_array_buffer_views() {
        let execution = execute_source(
            "const buffer = new ArrayBuffer(4); const values = new Uint16Array(buffer); values[0] = 258; const view = new DataView(buffer); console.log(values.length, values.byteLength, values.byteOffset, view.getUint16(0, true), view.getUint8(0), view.getUint8(1));",
        );
        assert_eq!(execution.stdout, b"2 4 0 258 2 1\n");
    }
    #[test]
    fn executes_typed_array_builtins() {
        let execution = execute_source(
            "const bytes = new Uint8Array([1, 258, -1]); console.log(bytes.length, bytes[0], bytes[1], bytes[2], bytes.byteLength, bytes.join('-')); bytes[1] = 260; bytes.set([7, 8], 1); const iterator = bytes.values(); const first = iterator.next(); const second = iterator.next(); console.log(bytes[1], bytes[2], bytes.slice(1, 3).join(','), bytes.at(-1), first.value, first.done, second.value, second.done);",
        );
        assert_eq!(
            execution.stdout,
            b"3 1 2 255 3 1-2-255\n7 8 7,8 8 1 false 7 false\n",
        );
    }
    #[test]
    fn allocation_pressure_collects_with_each_runtime_algorithm() {
        let artifact = artifact(
            "let checksum = 0; for (let i = 0; i < 12000; i += 1) { globalThis.current = { i }; checksum += globalThis.current.i; } console.log(checksum);",
        );
        for algorithm in [
            GcAlgorithmKind::MarkSweep,
            GcAlgorithmKind::G1,
            GcAlgorithmKind::Zgc,
        ] {
            let config = NativeRuntimeConfig::default()
                .with_gc_algorithm(algorithm)
                .with_max_heap_size(4 * 1024 * 1024);
            let mut runtime =
                NativeRuntime::new_with_config(config).expect("native runtime should initialize");
            let execution = runtime
                .execute(
                    &artifact,
                    std::path::Path::new("."),
                    std::path::Path::new("."),
                )
                .unwrap_or_else(|error| {
                    panic!("{algorithm:?} allocation pressure should finish: {error:?}")
                });
            let telemetry = runtime.gc_telemetry();
            assert_eq!(execution.stdout, b"71994000\n");
            assert!(telemetry.cycles > 0, "{algorithm:?} should collect");
            assert_eq!(telemetry.collector, algorithm.as_str());
        }
    }

    #[test]
    fn recursive_function_keeps_own_frame_locals() {
        let execution = execute_source(
            "function fib(n) { if (n < 2) return n; return fib(n - 1) + fib(n - 2); } console.log(fib(10));",
        );
        assert_eq!(execution.stdout, b"55\n");
    }

    #[test]
    fn loop_locals_do_not_leak_across_recursive_calls() {
        let execution = execute_source(
            "function walk(n) { let total = 0; for (let i = 0; i < n; i++) { if (n > 1 && i === 0) total += walk(n - 1); total += i; } return total; } console.log(walk(4));",
        );
        assert_eq!(execution.stdout, b"10\n");
    }

    #[test]
    fn eval_still_reads_and_writes_visible_locals() {
        let execution = execute_source(
            "function f() { let x = 1; eval('x = x + 2'); return x; } console.log(f());",
        );
        assert_eq!(execution.stdout, b"3\n");
    }

    #[test]
    fn mixed_type_local_still_uses_dynamic_add() {
        let execution = execute_source(
            "function f(flag) { let x = 1; if (flag) x = 'a'; return x + 1; } console.log(f(false), f(true));",
        );
        assert_eq!(execution.stdout, b"2 a1\n");
    }

    #[test]
    fn recursive_object_locals_remain_reachable() {
        let execution = execute_source(
            "function walk(n) { const o = { n }; if (n === 0) return o.n; return walk(n - 1) + o.n; } console.log(walk(5));",
        );
        assert_eq!(execution.stdout, b"15\n");
    }

    #[test]
    fn loop_locals_are_selected_as_frame_locals() {
        let source: Arc<str> = "
function work() {
  let s = 0.0;
  for (let i = 0; i < 3; i++) s += i;
  return s;
}
work();
"
        .into();
        let ast = wjsm_parser::parse_module(&source).expect("source should parse");
        let program = wjsm_semantic::lower_module_with_source(
            ast,
            true,
            Some(Arc::clone(&source)),
            "input.js",
        )
        .expect("source should lower");
        let work = program
            .functions()
            .iter()
            .find(|function| function.name() == "work")
            .expect("work function exists");
        let names: Vec<_> = program
            .frame_local_variable_names(work)
            .into_iter()
            .collect();
        assert!(
            names.contains(&"$1.s") && names.contains(&"$2.i"),
            "work frame locals were {names:?}"
        );
        let compiler = NativeCompiler::new().expect("native compiler should initialize");
        let compiled = match program.split_builtin_segment() {
            Some((_, user_program)) => compiler
                .diagnostics_program(&user_program)
                .expect("user segment should compile"),
            None => compiler
                .diagnostics_program(&program)
                .expect("loop program should compile"),
        };
        let work_clif = compiled
            .clif
            .split(";; function")
            .find(|chunk| chunk.contains(": work"))
            .expect("work CLIF should exist");
        let has_host_slots = work_clif.contains("0x0001_0300")
            || work_clif.contains("0x0001_0301")
            || work_clif.contains("66304")
            || work_clif.contains("66305");
        assert!(
            !has_host_slots && work_clif.contains("fadd"),
            "work CLIF should keep loop locals in SSA and emit fadd:\n{work_clif}"
        );
    }
}
