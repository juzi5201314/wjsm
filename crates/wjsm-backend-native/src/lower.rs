use std::collections::{BTreeSet, HashMap, HashSet};
use std::mem::{offset_of, size_of};

use anyhow::{Context, Result, anyhow, bail};
use cranelift_codegen::ir::{
    self, AbiParam, AtomicRmwOp, Function, InstBuilder, MemFlagsData, Signature, StackSlot,
    StackSlotData, StackSlotKind, UserFuncName, types,
};
use cranelift_codegen::isa::CallConv;
use cranelift_codegen::isa::unwind::UnwindInfo;
use cranelift_control::ControlPlane;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_module::{
    DataDescription, DataId, FuncId, Linkage, Module, ModuleDeclarations, ModuleReloc,
};
use cranelift_object::{ObjectBuilder, ObjectModule};
use wjsm_ir::{
    BasicBlockId, BinaryOp, Builtin, CompareOp, Constant, ConstantId, EVAL_SCOPE_ENV_PARAM,
    FunctionId, Instruction, Program, Terminator, UnaryOp, ValueId, constants, value,
};
use wjsm_native_abi::{
    COOPERATIVE_POLL_STEP_BYTES, NATIVE_BARRIER_MARKING_MASK, NativeBarrierState, NativeHostSymbol,
    NativeRootFrame, NativeRuntimeOp, NativeSignature, NativeVmContext, native_variable_names,
};

use rayon::prelude::*;

use crate::f64_analysis::infer_f64_values;
use crate::root_plan::RootPlan;
use crate::unwind::{UnwindPolicy, UnwindRecord, validate_unwind_info, write_object_unwind};
use crate::{NativeCompileError, NativeObject};

const HOST_OPERATION_SYMBOL: NativeHostSymbol = NativeHostSymbol::HostOperationDispatcher;
const DYNAMIC_BINARY_BASE: u32 = 0x1_0000;
const DYNAMIC_UNARY_BASE: u32 = 0x1_0100;
const DYNAMIC_COMPARE_BASE: u32 = 0x1_0200;
/// 共享 host 参数区的下限尺寸；无 host 调用的函数也保留一个合法槽。
const ARENA_MIN_BYTES: u32 = 8;

/// 一个 generated function 栈帧上的固定资源：GC root frame 与 host 调用参数区。
///
/// 三个 base 指针在入口块一次性物化；入口块支配其余所有块，因此它们可以在任意块里
/// 直接以 `store base + 常量 offset` 使用，无需每次重算 `stack_addr`。
struct FrameLowering {
    bitmap_by_root_count: Vec<ir::GlobalValue>,
    capacity: usize,
    published_roots: usize,
    bitmap_published: bool,
    /// 块内各 root 槽当前持有的 ValueId；跨块必须清空（前驱可能发布了不同内容）。
    published_slots: Vec<Option<ValueId>>,
    /// 入口块一次性物化的基址，被所有块支配后可跨块复用。
    frame_base: ir::Value,
    roots_base: ir::Value,
    /// 全函数共用的 host 调用参数区：参数在写入前已全部物化，写完立即被同一条
    /// `call` 消费，调用返回后即为死数据，因此各调用点不会重叠使用。
    arena_slot: StackSlot,
    arena_base: ir::Value,
    arena_bytes: u32,
    /// 提升到 SSA 的 boxed 局部占用 root 槽的尾部，跨 safepoint 常驻。
    pinned_local_count: usize,
}

impl FrameLowering {
    /// 必须在入口块内调用：本方法物化的基址值被其余所有块支配，供跨块复用。
    fn new(
        builder: &mut FunctionBuilder<'_>,
        bitmaps: &[DeclaredData],
        capacity: usize,
        ctx: ir::Value,
    ) -> Result<Self> {
        let frame_bytes = u32::try_from(size_of::<NativeRootFrame>())
            .context("native root frame size exceeds u32")?;
        let root_bytes = capacity
            .checked_mul(size_of::<i64>())
            .and_then(|bytes| u32::try_from(bytes).ok())
            .context("native root slots exceed u32")?;
        let frame_slot = builder.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            frame_bytes,
            3,
        ));
        let roots_slot = builder.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            root_bytes,
            3,
        ));
        // 参数区尺寸在 lower 过程中按最大 arity 增长，[`Self::finish`] 写回最终值。
        let arena_slot = builder.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            ARENA_MIN_BYTES,
            3,
        ));
        let bitmap_by_root_count = bitmaps
            .iter()
            .map(|bitmap| bitmap.import(builder.func))
            .collect();
        let pointer_type = builder.func.dfg.value_type(ctx);
        let frame_base = builder.ins().stack_addr(pointer_type, frame_slot, 0);
        let roots_base = builder.ins().stack_addr(pointer_type, roots_slot, 0);
        let arena_base = builder.ins().stack_addr(pointer_type, arena_slot, 0);
        Ok(Self {
            bitmap_by_root_count,
            capacity,
            published_roots: 0,
            bitmap_published: false,
            published_slots: Vec::new(),
            frame_base,
            roots_base,
            arena_slot,
            arena_base,
            arena_bytes: ARENA_MIN_BYTES,
            pinned_local_count: 0,
        })
    }

    /// 为一次 host 调用预留 `bytes` 字节参数区，返回共享参数区基址。
    fn reserve_arena(&mut self, bytes: u32) -> ir::Value {
        self.arena_bytes = self.arena_bytes.max(bytes);
        self.arena_base
    }

    /// lower 结束、`finalize` 之前写回参数区实际尺寸。
    fn finish(&self, builder: &mut FunctionBuilder<'_>) {
        builder.func.sized_stack_slots[self.arena_slot].size = self.arena_bytes;
    }

    fn link(&self, builder: &mut FunctionBuilder<'_>, ctx: ir::Value) -> Result<()> {
        let pointer_type = builder.func.dfg.value_type(ctx);
        let previous = builder.ins().load(
            pointer_type,
            MemFlagsData::trusted(),
            ctx,
            vmctx_offset(offset_of!(NativeVmContext, root_frame_head))?,
        );
        builder.ins().store(
            MemFlagsData::trusted(),
            previous,
            self.frame_base,
            frame_offset(offset_of!(NativeRootFrame, previous))?,
        );
        builder.ins().store(
            MemFlagsData::trusted(),
            self.roots_base,
            self.frame_base,
            frame_offset(offset_of!(NativeRootFrame, slots))?,
        );
        let empty_bitmap = builder
            .ins()
            .symbol_value(pointer_type, self.bitmap_by_root_count[0]);
        builder.ins().store(
            MemFlagsData::trusted(),
            empty_bitmap,
            self.frame_base,
            frame_offset(offset_of!(NativeRootFrame, bitmap_words))?,
        );
        let zero = builder.ins().iconst(types::I32, 0);
        builder.ins().store(
            MemFlagsData::trusted(),
            zero,
            self.frame_base,
            frame_offset(offset_of!(NativeRootFrame, bitmap_word_count))?,
        );
        let head = builder.ins().iadd_imm_s(
            ctx,
            i64::from(vmctx_offset(offset_of!(NativeVmContext, root_frame_head))?),
        );
        builder.ins().atomic_rmw(
            pointer_type,
            MemFlagsData::trusted(),
            AtomicRmwOp::Xchg,
            head,
            self.frame_base,
        );
        Ok(())
    }

    fn publish(
        &mut self,
        builder: &mut FunctionBuilder<'_>,
        variables: &HashMap<ValueId, Variable>,
        roots: &[ValueId],
        temporaries: &[ir::Value],
    ) -> Result<()> {
        let live_count = roots.len() + temporaries.len();
        let root_count = live_count
            .checked_add(self.pinned_local_count)
            .context("native root count overflow")?;
        let pointer_type = builder.func.dfg.value_type(self.roots_base);
        if root_count > self.capacity {
            bail!("native root plan exceeds frame capacity");
        }
        if self.published_slots.len() < live_count {
            self.published_slots.resize(live_count, None);
        }
        let local_base = self.pinned_local_count;
        for (index, root) in roots.iter().enumerate() {
            // IR 是 SSA：块内直线执行时同一 ValueId 的运行时值不会变，槽里已经是它就无需重写。
            if self.published_slots[index] == Some(*root) {
                continue;
            }
            let value = use_value(builder, variables, *root)?;
            builder.ins().store(
                MemFlagsData::trusted(),
                value,
                self.roots_base,
                slot_offset(local_base + index, "native root spill")?,
            );
            self.published_slots[index] = Some(*root);
        }
        for (index, temporary) in temporaries.iter().enumerate() {
            let slot = roots.len() + index;
            builder.ins().store(
                MemFlagsData::trusted(),
                *temporary,
                self.roots_base,
                slot_offset(local_base + slot, "native temporary root spill")?,
            );
            self.published_slots[slot] = None;
        }
        if root_count != self.published_roots || !self.bitmap_published {
            let bitmap = builder
                .ins()
                .symbol_value(pointer_type, self.bitmap_by_root_count[root_count]);
            builder.ins().store(
                MemFlagsData::trusted(),
                bitmap,
                self.frame_base,
                frame_offset(offset_of!(NativeRootFrame, bitmap_words))?,
            );
            let bitmap_word_count = root_count.div_ceil(u64::BITS as usize);
            let bitmap_word_count = u32::try_from(bitmap_word_count)
                .context("native root bitmap word count exceeds u32")?;
            let bitmap_word_count = builder
                .ins()
                .iconst(types::I32, i64::from(bitmap_word_count));
            builder.ins().store(
                MemFlagsData::trusted(),
                bitmap_word_count,
                self.frame_base,
                frame_offset(offset_of!(NativeRootFrame, bitmap_word_count))?,
            );
            self.bitmap_published = true;
        }
        self.published_roots = root_count;
        Ok(())
    }

    fn pin_frame_locals(
        &mut self,
        builder: &mut FunctionBuilder<'_>,
        values: &[ir::Value],
    ) -> Result<()> {
        self.pinned_local_count = values.len();
        for (index, value) in values.iter().enumerate() {
            builder.ins().store(
                MemFlagsData::trusted(),
                *value,
                self.roots_base,
                slot_offset(index, "native frame-local root")?,
            );
        }
        Ok(())
    }

    fn update_pinned_local(
        &self,
        builder: &mut FunctionBuilder<'_>,
        index: usize,
        value: ir::Value,
    ) -> Result<()> {
        builder.ins().store(
            MemFlagsData::trusted(),
            value,
            self.roots_base,
            slot_offset(index, "native frame-local root")?,
        );
        Ok(())
    }

    /// 进入新块：root 槽内容与 bitmap 状态只在块内直线执行时可复用，
    /// 跨块（前驱不唯一 / 前驱发布内容不同）必须全部重新发布。
    fn enter_block(&mut self) {
        self.bitmap_published = false;
        self.published_slots.clear();
    }

    fn unlink(&self, builder: &mut FunctionBuilder<'_>, ctx: ir::Value) -> Result<()> {
        let pointer_type = builder.func.dfg.value_type(ctx);
        let previous = builder.ins().load(
            pointer_type,
            MemFlagsData::trusted(),
            self.frame_base,
            frame_offset(offset_of!(NativeRootFrame, previous))?,
        );
        let head = builder.ins().iadd_imm_s(
            ctx,
            i64::from(vmctx_offset(offset_of!(NativeVmContext, root_frame_head))?),
        );
        builder.ins().atomic_rmw(
            pointer_type,
            MemFlagsData::trusted(),
            AtomicRmwOp::Xchg,
            head,
            previous,
        );
        Ok(())
    }
}

fn slot_offset(index: usize, context: &'static str) -> Result<i32> {
    index
        .checked_mul(size_of::<i64>())
        .and_then(|offset| i32::try_from(offset).ok())
        .with_context(|| format!("{context} offset exceeds i32"))
}
fn vmctx_offset(offset: usize) -> Result<i32> {
    i32::try_from(offset).context("native vmctx field offset exceeds i32")
}

fn barrier_state_offset(offset: usize) -> i32 {
    i32::try_from(offset).expect("native barrier state field offset fits i32")
}

fn increment_barrier_counter(builder: &mut FunctionBuilder<'_>, barrier: ir::Value, offset: usize) {
    let offset = barrier_state_offset(offset);
    let current = builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), barrier, offset);
    let next = builder.ins().iadd_imm_u(current, 1);
    builder
        .ins()
        .store(MemFlagsData::trusted(), next, barrier, offset);
}

fn frame_offset(offset: usize) -> Result<i32> {
    i32::try_from(offset).context("native root frame field offset exceeds i32")
}

pub(crate) fn declare_root_bitmaps(
    module: &mut ObjectModule,
    max_roots: usize,
) -> Result<Vec<DataId>, NativeCompileError> {
    (0..=max_roots)
        .map(|root_count| {
            let data_id = module
                .declare_data(
                    &format!("wjsm_root_bitmap_{root_count}"),
                    Linkage::Local,
                    false,
                    false,
                )
                .map_err(|error| NativeCompileError::Cranelift(error.to_string()))?;
            let word_count = root_count.div_ceil(u64::BITS as usize).max(1);
            let mut words = vec![u64::MAX; word_count];
            if root_count == 0 {
                words[0] = 0;
            } else if let Some(last) = words.last_mut() {
                let tail_bits = root_count % u64::BITS as usize;
                if tail_bits != 0 {
                    *last = (1_u64 << tail_bits) - 1;
                }
            }
            let mut bytes = Vec::with_capacity(word_count * size_of::<u64>());
            for word in words {
                bytes.extend_from_slice(&word.to_le_bytes());
            }
            let mut description = DataDescription::new();
            description.define(bytes.into_boxed_slice());
            description.set_align(8);
            module
                .define_data(data_id, &description)
                .map_err(|error| NativeCompileError::Cranelift(error.to_string()))?;
            Ok(data_id)
        })
        .collect()
}

pub(crate) fn root_frame_capacity(
    function: &wjsm_ir::Function,
    plan: &RootPlan,
    boxed_locals: usize,
) -> usize {
    let entry_roots = function.params().len().min(2);
    let temporary_roots = function
        .blocks()
        .iter()
        .flat_map(|block| block.instructions())
        .map(|instruction| match instruction {
            Instruction::GeneratorSuspend { .. } => 1,
            Instruction::Call { .. }
            | Instruction::SuperCall { .. }
            | Instruction::ConstructCall { .. }
            | Instruction::OptionalCall { .. } => 1,
            // IC accessor 命中时会把刚 load 出的 getter 作为临时 root 发布后再
            // 调用宿主 invoke_callable；保守起见所有 GetProp/OptionalGetProp 都
            // 预留一个临时槽（多预留不影响正确性）。
            Instruction::GetProp { .. } | Instruction::OptionalGetProp { .. } => 1,
            _ => 0,
        })
        .max()
        .unwrap_or(0);
    // pinned boxed local 与 live SSA roots 同时占用槽位，必须相加而不是取 max。
    entry_roots.max(plan.max_roots() + temporary_roots) + boxed_locals
}

pub(crate) fn slots_from_program(
    program: &Program,
) -> Result<HashMap<String, u32>, NativeCompileError> {
    native_variable_names(program)
        .into_iter()
        .enumerate()
        .map(|(index, name)| {
            let index =
                u32::try_from(index).map_err(|_| NativeCompileError::Capacity("variable slots"))?;
            Ok((name, index))
        })
        .collect()
}

/// 为「常量字符串键的 GetProp / OptionalGetProp」分配全局 IC 槽；返回每函数的 `dest → 槽下标`
/// 映射与总槽数。非字符串常量键（symbol / 数字 / 动态键）不分配，走宿主路径。
///
/// ValueId 是函数局部命名，故 `Const` 定义表必须按函数隔离，避免跨函数误匹配。
pub(crate) fn allocate_ic_slots(program: &Program) -> (Vec<HashMap<ValueId, u32>>, u32) {
    let mut per_function = Vec::with_capacity(program.functions().len());
    let mut slot_index = 0_u32;
    for function in program.functions() {
        let mut const_defs: HashMap<ValueId, ConstantId> = HashMap::new();
        for block in function.blocks() {
            for instruction in block.instructions() {
                if let Instruction::Const { dest, constant } = instruction {
                    const_defs.insert(*dest, *constant);
                }
            }
        }
        let mut slots = HashMap::new();
        for block in function.blocks() {
            for instruction in block.instructions() {
                let (dest, key) = match instruction {
                    Instruction::GetProp { dest, key, .. }
                    | Instruction::OptionalGetProp { dest, key, .. }
                    | Instruction::SetProp { dest, key, .. } => (*dest, *key),
                    _ => continue,
                };
                let Some(constant_id) = const_defs.get(&key) else {
                    continue;
                };
                let is_string = usize::try_from(constant_id.0)
                    .ok()
                    .and_then(|index| program.constants().get(index))
                    .is_some_and(|constant| matches!(constant, Constant::String(_)));
                if is_string {
                    slots.insert(dest, slot_index);
                    slot_index += 1;
                }
            }
        }
        per_function.push(slots);
    }
    (per_function, slot_index)
}

/// 每函数的反馈槽 plan：`(block, instruction) → 全局槽下标`。
///
/// 槽只按指令形态分配（Binary/Unary/Compare/CallBuiltin 与四种 call 系列全部计入，
/// 静态已证明 f64 或 typed-thunk 的指令不会写自己的槽），因此 base image 与运行时
/// 特化 overlay 对同一 Program 必然得到完全一致的编号——overlay 生成代码经由
/// vmctx 的 `feedback_slots_base` 继续写 base image 的槽，编号错位会把反馈记到
/// 别的调用点上。`LoadArgument`/`LoadCallEnv`/`FinishCall` 等内部 bookkeeping
/// 操作不分配槽；Shape IC 继续使用自己的 IC 槽。
#[derive(Debug, Default)]
pub(crate) struct FeedbackSitePlan {
    per_function: Vec<HashMap<(BasicBlockId, usize), u32>>,
    total: u32,
}

impl FeedbackSitePlan {
    pub(crate) fn total_slots(&self) -> u32 {
        self.total
    }

    pub(crate) fn function_slots(&self, index: usize) -> &HashMap<(BasicBlockId, usize), u32> {
        self.per_function
            .get(index)
            .expect("feedback plan covers every function")
    }
}

pub(crate) fn allocate_feedback_slots(program: &Program) -> FeedbackSitePlan {
    let mut per_function = Vec::with_capacity(program.functions().len());
    let mut slot_index = 0_u32;
    for function in program.functions() {
        let mut slots = HashMap::new();
        for block in function.blocks() {
            for (instruction_index, instruction) in block.instructions().iter().enumerate() {
                if instruction_owns_feedback_slot(instruction) {
                    slots.insert((block.id(), instruction_index), slot_index);
                    slot_index += 1;
                }
            }
        }
        per_function.push(slots);
    }
    FeedbackSitePlan {
        per_function,
        total: slot_index,
    }
}

/// 判定一条指令是否是「可观察动态语义」的反馈槽候选。
///
/// 只看指令形态、不看 `infer_f64_values` 的结论：静态证明会随特化种子变化，
/// 若槽编号依赖分析结果，base 与 overlay 的编号就会错位。
fn instruction_owns_feedback_slot(instruction: &Instruction) -> bool {
    matches!(
        instruction,
        Instruction::Binary { .. }
            | Instruction::Unary { .. }
            | Instruction::Compare { .. }
            | Instruction::CallBuiltin { .. }
            | Instruction::Call { .. }
            | Instruction::OptionalCall { .. }
            | Instruction::SuperCall { .. }
            | Instruction::ConstructCall { .. }
    )
}

/// Program 的反馈槽总数；cache 命中时与条目内记录的计数校验。
pub(crate) fn feedback_site_count(program: &Program) -> u32 {
    allocate_feedback_slots(program).total_slots()
}

pub(crate) fn compile_program(
    isa: cranelift_codegen::isa::OwnedTargetIsa,
    program: &Program,
    variable_slots: &HashMap<String, u32>,
) -> Result<NativeObject, NativeCompileError> {
    compile_program_inner(isa, program, variable_slots, false).map(|diagnostics| diagnostics.object)
}

pub(crate) fn compile_program_diagnostics(
    isa: cranelift_codegen::isa::OwnedTargetIsa,
    program: &Program,
    variable_slots: &HashMap<String, u32>,
) -> Result<crate::NativeCompilationDiagnostics, NativeCompileError> {
    compile_program_inner(isa, program, variable_slots, true)
}

/// 一个函数的并行 codegen 产物；合并阶段串行喂进 `ObjectModule`。
pub(crate) struct CompiledFunction {
    pub(crate) alignment: u64,
    pub(crate) bytes: Vec<u8>,
    pub(crate) relocs: Vec<ModuleReloc>,
    pub(crate) frame_bytes: u32,
    pub(crate) code_len: u64,
    pub(crate) unwind: UnwindInfo,
    pub(crate) clif: String,
    pub(crate) disassembly: String,
}

/// 并行 worker 需要的 module 声明快照。
///
/// worker 线程不持有 `ObjectModule`（它不是 `Sync`），只带走 import 一个已声明
/// 函数 / 数据对象所需的全部信息，等价于 `Module::declare_{func,data}_in_func`。
pub(crate) struct DeclaredFunction {
    id: FuncId,
    signature: Signature,
    colocated: bool,
}

pub(crate) struct DeclaredBarrierThunks {
    load: DeclaredFunction,
    store: DeclaredFunction,
}

struct BarrierThunks {
    load: ir::FuncRef,
    store: ir::FuncRef,
}

pub(crate) struct DeclaredData {
    id: DataId,
    colocated: bool,
    tls: bool,
}

impl DeclaredFunction {
    pub(crate) fn snapshot(declarations: &ModuleDeclarations, id: FuncId) -> Self {
        let decl = declarations.get_function_decl(id);
        Self {
            id,
            signature: decl.signature.clone(),
            colocated: decl.linkage.is_final(),
        }
    }

    /// 等价于 `Module::declare_func_in_func`，但只依赖快照。
    pub(crate) fn import(&self, func: &mut Function) -> ir::FuncRef {
        let signature = func.import_signature(self.signature.clone());
        let user_name_ref = func.declare_imported_user_function(ir::UserExternalName {
            namespace: 0,
            index: self.id.as_u32(),
        });
        func.import_function(ir::ExtFuncData {
            name: ir::ExternalName::user(user_name_ref),
            signature,
            colocated: self.colocated,
            patchable: false,
        })
    }
}

impl DeclaredBarrierThunks {
    pub(crate) fn snapshot(declarations: &ModuleDeclarations, load: FuncId, store: FuncId) -> Self {
        Self {
            load: DeclaredFunction::snapshot(declarations, load),
            store: DeclaredFunction::snapshot(declarations, store),
        }
    }

    fn import(&self, function: &mut Function) -> BarrierThunks {
        BarrierThunks {
            load: self.load.import(function),
            store: self.store.import(function),
        }
    }
}

impl DeclaredData {
    pub(crate) fn snapshot(declarations: &ModuleDeclarations, id: DataId) -> Self {
        let decl = declarations.get_data_decl(id);
        Self {
            id,
            colocated: decl.linkage.is_final(),
            tls: decl.tls,
        }
    }

    /// 等价于 `Module::declare_data_in_func`，但只依赖快照。
    fn import(&self, func: &mut Function) -> ir::GlobalValue {
        let user_name_ref = func.declare_imported_user_function(ir::UserExternalName {
            namespace: 1,
            index: self.id.as_u32(),
        });
        func.create_global_value(ir::GlobalValueData::Symbol {
            name: ir::ExternalName::user(user_name_ref),
            offset: ir::immediates::Imm64::new(0),
            colocated: self.colocated,
            tls: self.tls,
        })
    }
}

fn compile_program_inner(
    isa: cranelift_codegen::isa::OwnedTargetIsa,
    program: &Program,
    variable_slots: &HashMap<String, u32>,
    collect_diagnostics: bool,
) -> Result<crate::NativeCompilationDiagnostics, NativeCompileError> {
    if let Err(error) = program.verify() {
        let _ = error;
    }
    let module_isa = isa;
    let unwind_policy = UnwindPolicy::for_triple(module_isa.triple())?;
    // unwind 产出三平台统一归 `unwind` 模块所有：cranelift-object 的内置
    // `.eh_frame` 只在 `define_function_with_control_plane` 里被喂数据，而 codegen
    // 走并行 compile + `define_function_bytes`，内置路径拿不到 FDE。
    let builder = ObjectBuilder::new(
        module_isa.clone(),
        b"wjsm-native-image".to_vec(),
        Box::new(libcall_name),
    )
    .map_err(|error| NativeCompileError::Cranelift(error.to_string()))?;
    let mut module = ObjectModule::new(builder);
    let signature = slow_entry_signature(module.isa().default_call_conv());
    let function_ids = declare_functions(&mut module, program, &signature)?;
    let host_dispatcher = declare_host_dispatcher(&mut module)?;
    let (zgc_load_barrier, zgc_store_barrier) = declare_barrier_thunks(&mut module)?;
    let inferred_f64 = infer_f64_values(program);
    let math_thunks = declare_math_thunks(&mut module, program, &inferred_f64)?;
    let frame_locals = program.frame_local_variable_names_by_function();
    let boxed_frame_locals: Vec<BTreeSet<&str>> = program
        .functions()
        .iter()
        .enumerate()
        .zip(&frame_locals)
        .map(|((index, function), names)| {
            boxed_frame_local_names(function, names, &inferred_f64, index)
        })
        .collect();
    let root_plans: Vec<_> = program
        .functions()
        .par_iter()
        .enumerate()
        .map(|(index, function)| {
            let f64_values = inferred_f64
                .get(&FunctionId(
                    u32::try_from(index).expect("function index fits u32"),
                ))
                .expect("analysis covers every function");
            RootPlan::build(function, f64_values)
        })
        .collect();
    let root_capacities: Vec<_> = program
        .functions()
        .iter()
        .zip(&root_plans)
        .zip(&boxed_frame_locals)
        .map(|((function, plan), boxed)| root_frame_capacity(function, plan, boxed.len()))
        .collect();
    let max_roots = root_capacities.iter().copied().max().unwrap_or(0);
    let root_bitmaps = declare_root_bitmaps(&mut module, max_roots)?;

    let dispatcher_decl = DeclaredFunction::snapshot(module.declarations(), host_dispatcher);
    let barrier_thunks =
        DeclaredBarrierThunks::snapshot(module.declarations(), zgc_load_barrier, zgc_store_barrier);
    let math_thunk_decls: HashMap<Builtin, DeclaredFunction> = math_thunks
        .iter()
        .map(|(builtin, func_id)| {
            (
                *builtin,
                DeclaredFunction::snapshot(module.declarations(), *func_id),
            )
        })
        .collect();
    let bitmap_decls: Vec<DeclaredData> = root_bitmaps
        .iter()
        .map(|bitmap| DeclaredData::snapshot(module.declarations(), *bitmap))
        .collect();
    let target_config = module.target_config();

    // IC 槽预计算：常量字符串键的 GetProp 在编译期固定槽位，miss 回填由宿主完成。
    let (ic_slots, ic_slot_count) = allocate_ic_slots(program);
    // 反馈槽预计算：只按指令形态编号，保证与运行时特化 overlay 的编号一致。
    let feedback_plan = allocate_feedback_slots(program);

    // 每个函数的 lower + Cranelift compile 相互独立，只读上面的声明快照；
    // 合并进 object 的写入阶段仍然串行，保证 relocation / 符号表顺序确定。
    let compiled: Vec<CompiledFunction> = program
        .functions()
        .par_iter()
        .enumerate()
        .map(|(index, function)| {
            compile_one_function(
                &module_isa,
                target_config,
                program,
                function,
                index,
                &signature,
                function_ids[index],
                &dispatcher_decl,
                &barrier_thunks,
                &math_thunk_decls,
                &bitmap_decls,
                inferred_f64
                    .get(&FunctionId(
                        u32::try_from(index).expect("function index fits u32"),
                    ))
                    .expect("analysis covers every function"),
                variable_slots,
                &root_plans[index],
                root_capacities[index],
                &frame_locals[index],
                &boxed_frame_locals[index],
                &ic_slots[index],
                feedback_plan.function_slots(index),
                None,
                collect_diagnostics,
            )
        })
        .collect::<Result<Vec<_>, NativeCompileError>>()?;

    let mut frame_bytes = Vec::with_capacity(program.functions().len());
    let mut unwind_records: Vec<UnwindRecord> = Vec::with_capacity(program.functions().len());
    let mut clif = String::new();
    let mut disassembly = String::new();

    for (index, output) in compiled.into_iter().enumerate() {
        let function_id = function_ids[index];
        module
            .define_function_bytes(function_id, output.alignment, &output.bytes, &output.relocs)
            .map_err(|error| NativeCompileError::Cranelift(error.to_string()))?;
        frame_bytes.push(output.frame_bytes);
        unwind_records.push(UnwindRecord {
            function: function_id,
            code_len: output.code_len,
            info: output.unwind,
        });
        if collect_diagnostics {
            clif.push_str(&output.clif);
            disassembly.push_str(&output.disassembly);
        }
    }

    let mut product = module.finish();
    let systemv_cie = if unwind_policy == UnwindPolicy::WindowsPdata {
        None
    } else {
        Some(module_isa.create_systemv_cie().ok_or_else(|| {
            NativeCompileError::CompilerInvariant("ISA cannot create a System V CIE".into())
        })?)
    };
    write_object_unwind(
        &mut product,
        unwind_policy,
        unwind_records,
        systemv_cie,
        gimli_endian(module_isa.triple()),
    )?;
    let object = product
        .emit()
        .map_err(|error| NativeCompileError::Object(error.to_string()))?;
    Ok(crate::NativeCompilationDiagnostics {
        object: NativeObject {
            bytes: object.into(),
            frame_bytes,
            function_count: u32::try_from(program.functions().len())
                .map_err(|_| NativeCompileError::Capacity("function count"))?,
            ic_slot_count,
            feedback_slot_count: feedback_plan.total_slots(),
        },
        clif,
        disassembly,
    })
}

/// 单个函数的完整 codegen：IR → CLIF → 机器码 + relocation + unwind info。
/// 不接触 `ObjectModule`，可安全并行执行。
#[allow(clippy::too_many_arguments)]
pub(crate) fn compile_one_function(
    isa: &cranelift_codegen::isa::OwnedTargetIsa,
    target_config: cranelift_codegen::isa::TargetFrontendConfig,
    program: &Program,
    ir_function: &wjsm_ir::Function,
    index: usize,
    signature: &Signature,
    function_id: FuncId,
    dispatcher: &DeclaredFunction,
    barrier_thunks: &DeclaredBarrierThunks,
    math_thunks: &HashMap<Builtin, DeclaredFunction>,
    root_bitmaps: &[DeclaredData],
    f64_values: &HashSet<ValueId>,
    variable_slots: &HashMap<String, u32>,
    root_plan: &RootPlan,
    root_capacity: usize,
    frame_local_names: &BTreeSet<&str>,
    boxed_local_names: &BTreeSet<&str>,
    ic_slots: &HashMap<ValueId, u32>,
    feedback_slots: &HashMap<(BasicBlockId, usize), u32>,
    specialized_tags: Option<&[wjsm_native_abi::NativeFeedbackTag]>,
    collect_diagnostics: bool,
) -> Result<CompiledFunction, NativeCompileError> {
    let function_index =
        u32::try_from(index).map_err(|_| NativeCompileError::Capacity("function IDs"))?;
    let mut context = cranelift_codegen::Context::new();
    let mut builder_context = FunctionBuilderContext::new();
    context.set_disasm(collect_diagnostics);
    context.func.signature = signature.clone();
    context.func.name = UserFuncName::user(0, function_index);
    lower_function(
        &mut context.func,
        &mut builder_context,
        target_config,
        program,
        ir_function,
        function_index,
        dispatcher,
        barrier_thunks,
        math_thunks,
        f64_values,
        variable_slots,
        root_plan,
        root_capacity,
        root_bitmaps,
        frame_local_names,
        boxed_local_names,
        ic_slots,
        feedback_slots,
        specialized_tags,
    )
    .map_err(|error| NativeCompileError::Lowering {
        function: FunctionId(function_index),
        message: error.to_string(),
    })?;
    let clif = if collect_diagnostics {
        format!(
            ";; function {index}: {}\n{}\n",
            ir_function.name(),
            context.func.display()
        )
    } else {
        String::new()
    };

    context
        .compile(isa.as_ref(), &mut ControlPlane::default())
        .map_err(|error| NativeCompileError::Cranelift(error.inner.to_string()))?;
    let compiled = context
        .compiled_code()
        .ok_or_else(|| NativeCompileError::CompilerInvariant("missing compiled code".into()))?;
    if !compiled.buffer.traps().is_empty() {
        return Err(NativeCompileError::CompilerInvariant(format!(
            "function {index} contains a machine trap"
        )));
    }
    let disassembly = if collect_diagnostics {
        format!(
            ";; function {index}: {}\n{}\n",
            ir_function.name(),
            compiled.vcode.as_deref().unwrap_or("")
        )
    } else {
        String::new()
    };
    let frame_bytes = compiled
        .buffer
        .frame_layout()
        .ok_or_else(|| {
            NativeCompileError::CompilerInvariant(format!(
                "function {index} is missing frame metadata"
            ))
        })?
        .frame_to_fp_offset;
    let unwind = compiled
        .create_unwind_info(isa.as_ref())
        .map_err(|error| NativeCompileError::Cranelift(error.to_string()))?
        .ok_or(NativeCompileError::MissingUnwindInfo(FunctionId(
            function_index,
        )))?;
    validate_unwind_info(isa.triple(), &unwind, FunctionId(function_index))?;
    let alignment = u64::from(compiled.buffer.alignment);
    let code_len = u64::from(compiled.buffer.total_size());
    let bytes = compiled.buffer.data().to_vec();
    let relocs: Vec<ModuleReloc> = compiled
        .buffer
        .relocs()
        .iter()
        .map(|reloc| ModuleReloc::from_mach_reloc(reloc, &context.func, function_id))
        .collect();
    Ok(CompiledFunction {
        alignment,
        bytes,
        relocs,
        frame_bytes,
        code_len,
        unwind,
        clif,
        disassembly,
    })
}

fn declare_functions(
    module: &mut ObjectModule,
    program: &Program,
    signature: &Signature,
) -> Result<Vec<FuncId>, NativeCompileError> {
    program
        .functions()
        .iter()
        .enumerate()
        .map(|(index, _)| {
            module
                .declare_function(
                    &format!("wjsm_function_{index}"),
                    Linkage::Export,
                    signature,
                )
                .map_err(|error| NativeCompileError::Cranelift(error.to_string()))
        })
        .collect()
}

pub(crate) fn declare_host_dispatcher(
    module: &mut ObjectModule,
) -> Result<FuncId, NativeCompileError> {
    let pointer_type = module.target_config().pointer_type();
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(pointer_type));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(pointer_type));
    signature.params.push(AbiParam::new(types::I32));
    // ABI v10：反馈槽指针；非反馈调用点传 null。
    signature.params.push(AbiParam::new(pointer_type));
    signature.returns.push(AbiParam::new(types::I64));
    module
        .declare_function(
            HOST_OPERATION_SYMBOL.symbol_name(),
            Linkage::Import,
            &signature,
        )
        .map_err(|error| NativeCompileError::Cranelift(error.to_string()))
}

pub(crate) fn declare_barrier_thunks(
    module: &mut ObjectModule,
) -> Result<(FuncId, FuncId), NativeCompileError> {
    let pointer_type = module.target_config().pointer_type();
    let mut load_signature = module.make_signature();
    load_signature.params.push(AbiParam::new(pointer_type));
    load_signature.params.push(AbiParam::new(types::I32));
    load_signature.returns.push(AbiParam::new(types::I64));
    let load = module
        .declare_function(
            NativeHostSymbol::ZgcLoadBarrierAssist.symbol_name(),
            Linkage::Import,
            &load_signature,
        )
        .map_err(|error| NativeCompileError::Cranelift(error.to_string()))?;

    let mut store_signature = module.make_signature();
    store_signature.params.push(AbiParam::new(pointer_type));
    store_signature.params.push(AbiParam::new(types::I32));
    store_signature.params.push(AbiParam::new(types::I64));
    store_signature.params.push(AbiParam::new(types::I64));
    store_signature.returns.push(AbiParam::new(types::I32));
    let store = module
        .declare_function(
            NativeHostSymbol::ZgcStoreBarrier.symbol_name(),
            Linkage::Import,
            &store_signature,
        )
        .map_err(|error| NativeCompileError::Cranelift(error.to_string()))?;
    Ok((load, store))
}

/// 按需声明本程序真正走 typed 路径的 math thunk（模块级声明一次；函数内 import
/// 复用之，避免每条调用重建签名）。只有 `infer_f64_values` 已证明 dest 且实参
/// arity 与 thunk 签名一致的调用点才需要声明。
pub(crate) fn declare_math_thunks(
    module: &mut ObjectModule,
    program: &Program,
    inferred_f64: &HashMap<FunctionId, HashSet<ValueId>>,
) -> Result<HashMap<Builtin, FuncId>, NativeCompileError> {
    let mut used = HashSet::new();
    for (index, function) in program.functions().iter().enumerate() {
        let function_id = FunctionId(u32::try_from(index).expect("function index fits u32"));
        let f64_values = inferred_f64
            .get(&function_id)
            .expect("analysis covers every function");
        for block in function.blocks() {
            for instruction in block.instructions() {
                if let Instruction::CallBuiltin {
                    dest: Some(dest),
                    builtin,
                    args,
                } = instruction
                    && f64_values.contains(dest)
                    && NativeHostSymbol::for_builtin(*builtin).is_some_and(|symbol| {
                        args.len() == usize::from(symbol.signature().argument_count())
                    })
                {
                    used.insert(*builtin);
                }
            }
        }
    }
    let mut used: Vec<_> = used.into_iter().collect();
    used.sort_by_key(|builtin| builtin.wire_id());
    let unary_signature = math_thunk_signature(module, NativeSignature::F64Unary);
    let binary_signature = math_thunk_signature(module, NativeSignature::F64Binary);
    let mut declared = HashMap::with_capacity(used.len());
    for builtin in used {
        let symbol = NativeHostSymbol::for_builtin(builtin).expect("used 集合由 for_builtin 过滤");
        // 同一 arity 的全部 thunk 共享同一份 Cranelift 签名，避免逐符号重建。
        let signature = match symbol.signature() {
            NativeSignature::F64Unary => &unary_signature,
            NativeSignature::F64Binary => &binary_signature,
            NativeSignature::HostOperation
            | NativeSignature::ZgcLoadBarrier
            | NativeSignature::ZgcStoreBarrier => {
                unreachable!("math thunk 不存在 host 或 ZGC 屏障签名")
            }
        };
        let func_id = module
            .declare_function(symbol.symbol_name(), Linkage::Import, signature)
            .map_err(|error| NativeCompileError::Cranelift(error.to_string()))?;
        declared.insert(builtin, func_id);
    }
    Ok(declared)
}

fn math_thunk_signature(module: &ObjectModule, signature: NativeSignature) -> Signature {
    let mut clif_signature = module.make_signature();
    match signature {
        NativeSignature::F64Unary => {
            clif_signature.params.push(AbiParam::new(types::F64));
        }
        NativeSignature::F64Binary => {
            clif_signature.params.push(AbiParam::new(types::F64));
            clif_signature.params.push(AbiParam::new(types::F64));
        }
        NativeSignature::HostOperation
        | NativeSignature::ZgcLoadBarrier
        | NativeSignature::ZgcStoreBarrier => {
            unreachable!("math thunk 不存在 host 或 ZGC 屏障签名")
        }
    }
    clif_signature.returns.push(AbiParam::new(types::F64));
    clif_signature
}

pub(crate) fn slow_entry_signature(call_conv: CallConv) -> Signature {
    let mut signature = Signature::new(call_conv);
    signature.params.push(AbiParam::new(types::I64));
    signature.params.push(AbiParam::new(types::I64));
    signature.params.push(AbiParam::new(types::I64));
    signature.params.push(AbiParam::new(types::I32));
    signature.params.push(AbiParam::new(types::I32));
    signature.returns.push(AbiParam::new(types::I64));
    signature
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn lower_function(
    function: &mut Function,
    builder_context: &mut FunctionBuilderContext,
    target_config: cranelift_codegen::isa::TargetFrontendConfig,
    program: &Program,
    ir_function: &wjsm_ir::Function,
    function_index: u32,
    host_dispatcher: &DeclaredFunction,
    barrier_thunks: &DeclaredBarrierThunks,
    math_thunks: &HashMap<Builtin, DeclaredFunction>,
    f64_values: &HashSet<ValueId>,
    variable_slots: &HashMap<String, u32>,
    root_plan: &RootPlan,
    root_capacity: usize,
    root_bitmaps: &[DeclaredData],
    frame_local_names: &BTreeSet<&str>,
    boxed_local_names: &BTreeSet<&str>,
    ic_slots: &HashMap<ValueId, u32>,
    feedback_slots: &HashMap<(BasicBlockId, usize), u32>,
    specialized_tags: Option<&[wjsm_native_abi::NativeFeedbackTag]>,
) -> Result<()> {
    let slow_call_signature = function.signature.clone();
    let mut builder = FunctionBuilder::new(function, builder_context);
    let mut blocks = HashMap::with_capacity(ir_function.blocks().len());
    for block in ir_function.blocks() {
        blocks.insert(block.id(), builder.create_block());
    }
    let entry = *blocks
        .get(&ir_function.entry())
        .context("entry block is missing")?;
    builder.append_block_params_for_function_params(entry);

    let value_ids = collect_value_ids(ir_function);
    let mut variables = HashMap::with_capacity(value_ids.len());
    for value_id in value_ids {
        variables.insert(value_id, builder.declare_var(types::I64));
    }
    let mut frame_locals = frame_local_variables(frame_local_names);
    let boxed_local_order = boxed_local_order(boxed_local_names);
    let boxed_local_indices = frame_local_indices(&boxed_local_order);
    let phi_edges = collect_phi_edges(ir_function);
    let dispatcher_ref = host_dispatcher.import(builder.func);
    let mut imported_math_thunks: HashMap<Builtin, ir::FuncRef> =
        HashMap::with_capacity(math_thunks.len());
    let barrier_thunks = barrier_thunks.import(builder.func);
    let slow_call_signature = builder.import_signature(slow_call_signature);
    let ctx_value = builder.block_params(entry)[0];
    let constants = program.constants();
    // root frame 的基址值必须在入口块物化：入口块支配其余所有块，基址可跨块复用。

    builder.switch_to_block(entry);
    let mut root_frame = FrameLowering::new(&mut builder, root_bitmaps, root_capacity, ctx_value)?;
    root_frame.link(&mut builder, ctx_value)?;
    initialize_frame_locals(&mut builder, &mut frame_locals);
    pin_initialized_frame_locals(
        &mut root_frame,
        &mut builder,
        &frame_locals,
        &boxed_local_order,
    )?;
    lower_function_parameters(
        &mut builder,
        ir_function,
        variable_slots,
        dispatcher_ref,
        ctx_value,
        &mut root_frame,
        &frame_locals,
        &boxed_local_indices,
        specialized_tags,
    )?;

    for block in ir_function.blocks() {
        let clif_block = blocks[&block.id()];
        builder.switch_to_block(clif_block);
        root_frame.enter_block();
        let has_suspend = block.instructions().iter().any(|instruction| {
            matches!(
                instruction,
                Instruction::Suspend { .. } | Instruction::GeneratorSuspend { .. }
            )
        });
        for (instruction_index, instruction) in block.instructions().iter().enumerate() {
            if matches!(instruction, Instruction::Phi { .. }) {
                continue;
            }
            let roots = root_plan.before_instruction(block.id(), instruction_index);
            root_frame.publish(&mut builder, &variables, roots, &[])?;
            let feedback_ptr = feedback_slots
                .get(&(block.id(), instruction_index))
                .map(|slot| emit_feedback_slot_ptr(&mut builder, ctx_value, *slot))
                .transpose()?;
            lower_instruction(
                &mut builder,
                instruction,
                constants,
                function_index,
                &variables,
                dispatcher_ref,
                &barrier_thunks,
                ctx_value,
                f64_values,
                math_thunks,
                &mut imported_math_thunks,
                slow_call_signature,
                variable_slots,
                &mut root_frame,
                roots,
                &frame_locals,
                &boxed_local_indices,
                ic_slots,
                feedback_ptr,
            )?;
        }
        if has_suspend {
            continue;
        }
        root_frame.publish(
            &mut builder,
            &variables,
            root_plan.before_terminator(block.id()),
            &[],
        )?;
        lower_terminator(
            &mut builder,
            block.id(),
            block.terminator(),
            constants,
            &blocks,
            &variables,
            &phi_edges,
            dispatcher_ref,
            ctx_value,
            &mut root_frame,
        )?;
    }
    root_frame.finish(&mut builder);
    builder.seal_all_blocks();
    builder.finalize(target_config);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn lower_function_parameters(
    builder: &mut FunctionBuilder<'_>,
    function: &wjsm_ir::Function,
    variable_slots: &HashMap<String, u32>,
    dispatcher: ir::FuncRef,
    ctx: ir::Value,
    root_frame: &mut FrameLowering,
    frame_locals: &HashMap<String, Variable>,
    frame_local_indices: &HashMap<String, usize>,
    specialized_tags: Option<&[wjsm_native_abi::NativeFeedbackTag]>,
) -> Result<()> {
    let native_params = builder
        .block_params(builder.current_block().context("missing entry block")?)
        .to_vec();
    let env = native_params[1];
    let this_value = native_params[2];
    let args_base = builder.ins().uextend(types::I64, native_params[3]);
    let args_len = builder.ins().uextend(types::I64, native_params[4]);
    let entry_roots: &[ir::Value] = if function.params().len() >= 2 {
        &[env, this_value]
    } else if function.params().len() == 1 {
        &[env]
    } else {
        &[]
    };
    root_frame.publish(builder, &HashMap::new(), &[], entry_roots)?;
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
                || name == EVAL_SCOPE_ENV_PARAM)
        {
            name
        } else if index == 0 {
            "$env"
        } else if index == 1
            && uses_canonical_this
            && !function.name().ends_with("$async")
            && !function.name().ends_with("$asyncgen")
        {
            "$this"
        } else {
            name
        };
        let value = match index {
            0 => env,
            1 => this_value,
            _ => {
                // 特化 body：profile 覆盖的参数由 wrapper 的入口守卫背书，直接从
                // call arena 读取（wrapper 已验证 args_count 覆盖全部 tagged 参数，
                // 且每个参数的 tag 与 profile 一致），跳过 LoadArgument 的
                // dispatcher 往返；未覆盖的参数保留通用 LoadArgument 默认语义。
                if let Some(tags) = specialized_tags
                    && let Some(_tag) = tags.get(index - 2)
                {
                    let pointer_type = builder.func.dfg.value_type(ctx);
                    let args_base_u32 = builder.ins().uextend(types::I64, native_params[3]);
                    let arena_base = builder.ins().load(
                        pointer_type,
                        MemFlagsData::trusted(),
                        ctx,
                        vmctx_offset(offset_of!(NativeVmContext, call_arena_slots))?,
                    );
                    let slot_offset = i64::try_from(index - 2)
                        .context("parameter index exceeds i64")?
                        .checked_mul(size_of::<i64>() as i64)
                        .context("call arena offset overflows")?;
                    let args_base_bytes = builder.ins().ishl_imm_u(args_base_u32, 3);
                    let param_bytes = builder.ins().iadd_imm_s(args_base_bytes, slot_offset);
                    let address = builder.ins().iadd(arena_base, param_bytes);
                    builder
                        .ins()
                        .load(types::I64, MemFlagsData::trusted(), address, 0)
                } else {
                    let argument = builder.ins().iconst(
                        types::I64,
                        i64::try_from(index - 2).context("parameter index exceeds i64")?,
                    );
                    call_dispatcher(
                        builder,
                        root_frame,
                        dispatcher,
                        ctx,
                        NativeRuntimeOp::LoadArgument.id(),
                        &[args_base, args_len, argument],
                        None,
                    )?
                }
            }
        };
        if let Some(local) = frame_locals.get(storage_name).copied() {
            builder.def_var(local, value);
            if let Some(index) = frame_local_indices.get(storage_name).copied() {
                root_frame.update_pinned_local(builder, index, value)?;
            }
            continue;
        }
        let Some(slot) = variable_slots.get(storage_name).copied() else {
            continue;
        };
        let slot = builder.ins().iconst(types::I64, i64::from(slot));
        let _ = call_dispatcher(
            builder,
            root_frame,
            dispatcher,
            ctx,
            NativeRuntimeOp::StoreVar.id(),
            &[slot, value],
            None,
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn lower_instruction(
    builder: &mut FunctionBuilder<'_>,
    instruction: &Instruction,
    constants: &[Constant],
    function_index: u32,
    variables: &HashMap<ValueId, Variable>,
    dispatcher: ir::FuncRef,
    barrier_thunks: &BarrierThunks,
    ctx: ir::Value,
    f64_values: &HashSet<ValueId>,
    math_thunks: &HashMap<Builtin, DeclaredFunction>,
    imported_math_thunks: &mut HashMap<Builtin, ir::FuncRef>,
    slow_call_signature: ir::SigRef,
    variable_slots: &HashMap<String, u32>,
    root_frame: &mut FrameLowering,
    roots: &[ValueId],
    frame_locals: &HashMap<String, Variable>,
    frame_local_indices: &HashMap<String, usize>,
    ic_slots: &HashMap<ValueId, u32>,
    feedback_ptr: Option<ir::Value>,
) -> Result<()> {
    match instruction {
        Instruction::Const {
            dest,
            constant: constant_id,
        } => {
            let constant_index =
                usize::try_from(constant_id.0).context("constant index does not fit usize")?;
            let constant = constants
                .get(constant_index)
                .with_context(|| format!("constant {} is missing", constant_id.0))?;
            let native = match constant {
                Constant::Number(value) => {
                    builder.ins().iconst(types::I64, value::encode_f64(*value))
                }
                Constant::Bool(value) => {
                    builder.ins().iconst(types::I64, value::encode_bool(*value))
                }
                Constant::Null => builder.ins().iconst(types::I64, value::encode_null()),
                Constant::Undefined => builder.ins().iconst(types::I64, value::encode_undefined()),
                Constant::FunctionRef(function) => {
                    let index = builder.ins().iconst(types::I64, i64::from(function.0));
                    call_dispatcher(
                        builder,
                        root_frame,
                        dispatcher,
                        ctx,
                        NativeRuntimeOp::MaterializeFunction.id(),
                        &[index],
                        None,
                    )?
                }
                Constant::NativeCallableEval => builder
                    .ins()
                    .iconst(types::I64, value::encode_native_callable_idx(0)),
                Constant::ModuleId(module) => builder
                    .ins()
                    .iconst(types::I64, value::encode_f64(f64::from(module.0))),
                Constant::String(_) | Constant::BigInt(_) | Constant::RegExp { .. } => {
                    let operation = match constant {
                        Constant::String(_) => NativeRuntimeOp::MaterializeString,
                        Constant::BigInt(_) => NativeRuntimeOp::MaterializeBigInt,
                        Constant::RegExp { .. } => NativeRuntimeOp::MaterializeRegExp,
                        _ => unreachable!("guard restricts materialized constants"),
                    };
                    let index = builder.ins().iconst(types::I64, i64::from(constant_id.0));
                    let result = call_dispatcher(
                        builder,
                        root_frame,
                        dispatcher,
                        ctx,
                        operation.id(),
                        &[index],
                        None,
                    )?;
                    if matches!(constant, Constant::RegExp { .. }) {
                        return_if_exception(builder, result, root_frame, ctx)?;
                    }
                    result
                }
            };
            define_value(builder, variables, *dest, native)
        }
        Instruction::Binary { dest, op, lhs, rhs }
            if f64_values.contains(dest)
                && matches!(
                    op,
                    BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div
                ) =>
        {
            let lhs = use_value(builder, variables, *lhs)?;
            let rhs = use_value(builder, variables, *rhs)?;
            let lhs = builder
                .ins()
                .bitcast(types::F64, ir::MemFlagsData::new(), lhs);
            let rhs = builder
                .ins()
                .bitcast(types::F64, ir::MemFlagsData::new(), rhs);
            let result = match op {
                BinaryOp::Add => builder.ins().fadd(lhs, rhs),
                BinaryOp::Sub => builder.ins().fsub(lhs, rhs),
                BinaryOp::Mul => builder.ins().fmul(lhs, rhs),
                BinaryOp::Div => builder.ins().fdiv(lhs, rhs),
                _ => unreachable!("guard restricts direct f64 operations"),
            };
            let result = box_f64_result(builder, result);
            define_value(builder, variables, *dest, result)
        }
        Instruction::Binary { dest, op, lhs, rhs } => lower_dynamic_binary(
            builder,
            variables,
            root_frame,
            dispatcher,
            ctx,
            *dest,
            *op,
            *lhs,
            *rhs,
            feedback_ptr,
        ),
        Instruction::Unary { dest, op, value } => {
            if f64_values.contains(dest) && matches!(op, UnaryOp::Neg | UnaryOp::Pos) {
                let value = use_value(builder, variables, *value)?;
                let result = if *op == UnaryOp::Neg {
                    let value = builder
                        .ins()
                        .bitcast(types::F64, ir::MemFlagsData::new(), value);
                    let result = builder.ins().fneg(value);
                    box_f64_result(builder, result)
                } else {
                    value
                };
                define_value(builder, variables, *dest, result)
            } else {
                let operation = DYNAMIC_UNARY_BASE + u32::from(unary_tag(*op));
                let input = use_value(builder, variables, *value)?;
                let result = call_dispatcher(
                    builder,
                    root_frame,
                    dispatcher,
                    ctx,
                    operation,
                    &[input],
                    feedback_ptr,
                )?;
                define_value(builder, variables, *dest, result)
            }
        }
        Instruction::Compare { dest, op, lhs, rhs } => {
            let operation = DYNAMIC_COMPARE_BASE + u32::from(compare_tag(*op));
            let lhs = use_value(builder, variables, *lhs)?;
            let rhs = use_value(builder, variables, *rhs)?;
            let result = call_dispatcher(
                builder,
                root_frame,
                dispatcher,
                ctx,
                operation,
                &[lhs, rhs],
                feedback_ptr,
            )?;
            define_value(builder, variables, *dest, result)
        }
        // 已证明 f64 的单参数 Math builtin：直接发 CLIF 浮点指令，零 host 往返。
        // guard 即类型检查——参数未证明 f64 时本 arm 不匹配，落到下方通用 dispatcher 路径。
        Instruction::CallBuiltin {
            dest: Some(dest),
            builtin:
                builtin @ (Builtin::MathAbs
                | Builtin::MathSqrt
                | Builtin::MathCeil
                | Builtin::MathFloor
                | Builtin::MathTrunc
                | Builtin::MathFround),
            args,
        } if f64_values.contains(dest) && args.len() == 1 => {
            let input = use_value(builder, variables, args[0])?;
            let input = builder
                .ins()
                .bitcast(types::F64, ir::MemFlagsData::new(), input);
            let result = match builtin {
                Builtin::MathAbs => builder.ins().fabs(input),
                Builtin::MathSqrt => builder.ins().sqrt(input),
                Builtin::MathCeil => builder.ins().ceil(input),
                Builtin::MathFloor => builder.ins().floor(input),
                Builtin::MathTrunc => builder.ins().trunc(input),
                Builtin::MathFround => {
                    let narrowed = builder.ins().fdemote(types::F32, input);
                    builder.ins().fpromote(types::F64, narrowed)
                }
                _ => unreachable!("arm 模式已限定这六个 builtin"),
            };
            let result = box_f64_result(builder, result);
            define_value(builder, variables, *dest, result)
        }
        // 已证明 f64 的 21 个 libm Math builtin：typed native direct call。
        // guard 即类型检查——实参未证明 f64 时落入下方 dispatcher 路径，
        // 保留 to_number_coerced 与 BigInt TypeError 语义。
        Instruction::CallBuiltin {
            dest: Some(dest),
            builtin,
            args,
        } if f64_values.contains(dest)
            && NativeHostSymbol::for_builtin(*builtin).is_some_and(|symbol| {
                args.len() == usize::from(symbol.signature().argument_count())
            }) =>
        {
            let symbol = NativeHostSymbol::for_builtin(*builtin)
                .context("guard 已限制为 math thunk builtin")?;
            let thunk = import_math_thunk(builder, math_thunks, imported_math_thunks, *builtin)?;
            let result = match symbol.signature() {
                NativeSignature::F64Unary => {
                    let input = use_value(builder, variables, args[0])?;
                    let input = builder
                        .ins()
                        .bitcast(types::F64, ir::MemFlagsData::new(), input);
                    let call = builder.ins().call(thunk, &[input]);
                    *builder
                        .inst_results(call)
                        .first()
                        .context("typed math thunk returned no result")?
                }
                NativeSignature::F64Binary => {
                    let lhs = use_value(builder, variables, args[0])?;
                    let rhs = use_value(builder, variables, args[1])?;
                    let lhs = builder
                        .ins()
                        .bitcast(types::F64, ir::MemFlagsData::new(), lhs);
                    let rhs = builder
                        .ins()
                        .bitcast(types::F64, ir::MemFlagsData::new(), rhs);
                    let call = builder.ins().call(thunk, &[lhs, rhs]);
                    *builder
                        .inst_results(call)
                        .first()
                        .context("typed math thunk returned no result")?
                }
                NativeSignature::HostOperation
                | NativeSignature::ZgcLoadBarrier
                | NativeSignature::ZgcStoreBarrier => {
                    unreachable!("math thunk 不存在 host 或 ZGC 屏障签名")
                }
            };
            let result = box_f64_result(builder, result);
            define_value(builder, variables, *dest, result)
        }
        Instruction::CallBuiltin {
            dest,
            builtin,
            args,
        } => {
            let mut values = Vec::with_capacity(args.len());
            for arg in args {
                values.push(use_value(builder, variables, *arg)?);
            }
            let result = call_dispatcher(
                builder,
                root_frame,
                dispatcher,
                ctx,
                u32::from(builtin.wire_id()),
                &values,
                feedback_ptr,
            )?;
            if let Some(dest) = dest {
                define_value(builder, variables, *dest, result)?;
            }
            Ok(())
        }
        Instruction::Call {
            dest,
            callee,
            this_val,
            args,
        } => lower_call_instruction(
            builder,
            variables,
            dispatcher,
            ctx,
            slow_call_signature,
            *dest,
            *callee,
            *this_val,
            args,
            NativeRuntimeOp::PrepareCall,
            false,
            root_frame,
            roots,
            feedback_ptr,
        ),
        Instruction::SuperCall {
            dest,
            callee,
            this_val,
            args,
            forward_args,
        } => lower_call_instruction(
            builder,
            variables,
            dispatcher,
            ctx,
            slow_call_signature,
            *dest,
            *callee,
            *this_val,
            args,
            if *forward_args {
                NativeRuntimeOp::PrepareSuperCallForward
            } else {
                NativeRuntimeOp::PrepareSuperCall
            },
            *forward_args,
            root_frame,
            roots,
            feedback_ptr,
        ),
        Instruction::ConstructCall {
            dest,
            callee,
            this_val,
            args,
        } => lower_call_instruction(
            builder,
            variables,
            dispatcher,
            ctx,
            slow_call_signature,
            *dest,
            *callee,
            *this_val,
            args,
            NativeRuntimeOp::PrepareConstruct,
            false,
            root_frame,
            roots,
            feedback_ptr,
        ),
        Instruction::OptionalCall {
            dest,
            callee,
            this_val,
            args,
        } => lower_optional_call_instruction(
            builder,
            variables,
            dispatcher,
            ctx,
            slow_call_signature,
            *dest,
            *callee,
            *this_val,
            args,
            root_frame,
            roots,
            feedback_ptr,
        ),
        Instruction::StringConcatVa { dest, parts } => lower_value_operation(
            builder,
            variables,
            root_frame,
            dispatcher,
            ctx,
            NativeRuntimeOp::StringConcat,
            parts,
            Some(*dest),
        ),
        Instruction::NewPromise { dest } => {
            let result = call_dispatcher(
                builder,
                root_frame,
                dispatcher,
                ctx,
                Builtin::PromiseCreate.wire_id().into(),
                &[],
                None,
            )?;
            define_value(builder, variables, *dest, result)
        }
        Instruction::NewObject { dest, capacity } => {
            let capacity = builder.ins().iconst(types::I64, i64::from(*capacity));
            let result = call_dispatcher(
                builder,
                root_frame,
                dispatcher,
                ctx,
                NativeRuntimeOp::NewObject.id(),
                &[capacity],
                None,
            )?;
            define_value(builder, variables, *dest, result)
        }
        Instruction::GetProp { dest, object, key } => {
            if let Some(slot) = ic_slots.get(dest).copied() {
                lower_get_prop_ic(
                    builder,
                    variables,
                    root_frame,
                    dispatcher,
                    barrier_thunks,
                    ctx,
                    *dest,
                    *object,
                    *key,
                    slot,
                    roots,
                )
            } else {
                lower_value_operation(
                    builder,
                    variables,
                    root_frame,
                    dispatcher,
                    ctx,
                    NativeRuntimeOp::GetProp,
                    &[*object, *key],
                    Some(*dest),
                )
            }
        }
        Instruction::SetProp {
            dest,
            object,
            key,
            value,
        } => {
            if let Some(slot) = ic_slots.get(dest).copied() {
                lower_set_prop_ic(
                    builder,
                    variables,
                    root_frame,
                    dispatcher,
                    barrier_thunks,
                    ctx,
                    *dest,
                    *object,
                    *key,
                    *value,
                    slot,
                )
            } else {
                lower_value_operation(
                    builder,
                    variables,
                    root_frame,
                    dispatcher,
                    ctx,
                    NativeRuntimeOp::SetProp,
                    &[*object, *key, *value],
                    Some(*dest),
                )
            }
        }
        Instruction::CreateDataProperty {
            dest,
            object,
            key,
            value,
        } => lower_value_operation(
            builder,
            variables,
            root_frame,
            dispatcher,
            ctx,
            NativeRuntimeOp::CreateDataProperty,
            &[*object, *key, *value],
            Some(*dest),
        ),
        Instruction::DeleteProp { dest, object, key } => lower_value_operation(
            builder,
            variables,
            root_frame,
            dispatcher,
            ctx,
            NativeRuntimeOp::DeleteProp,
            &[*object, *key],
            Some(*dest),
        ),
        Instruction::SetProto { object, value } => lower_value_operation(
            builder,
            variables,
            root_frame,
            dispatcher,
            ctx,
            NativeRuntimeOp::SetProto,
            &[*object, *value],
            None,
        ),
        Instruction::NewArray { dest, capacity } => {
            let capacity = builder.ins().iconst(types::I64, i64::from(*capacity));
            let result = call_dispatcher(
                builder,
                root_frame,
                dispatcher,
                ctx,
                NativeRuntimeOp::NewArray.id(),
                &[capacity],
                None,
            )?;
            define_value(builder, variables, *dest, result)
        }
        Instruction::GetElem {
            dest,
            object,
            index,
        } => lower_value_operation(
            builder,
            variables,
            root_frame,
            dispatcher,
            ctx,
            NativeRuntimeOp::GetElem,
            &[*object, *index],
            Some(*dest),
        ),
        Instruction::SetElem {
            dest,
            object,
            index,
            value,
        } => lower_value_operation(
            builder,
            variables,
            root_frame,
            dispatcher,
            ctx,
            NativeRuntimeOp::SetElem,
            &[*object, *index, *value],
            Some(*dest),
        ),
        Instruction::OptionalGetProp { dest, object, key } => {
            if let Some(slot) = ic_slots.get(dest).copied() {
                lower_optional_get_prop_ic(
                    builder,
                    variables,
                    root_frame,
                    dispatcher,
                    barrier_thunks,
                    ctx,
                    *dest,
                    *object,
                    *key,
                    slot,
                    roots,
                )
            } else {
                lower_value_operation(
                    builder,
                    variables,
                    root_frame,
                    dispatcher,
                    ctx,
                    NativeRuntimeOp::OptionalGetProp,
                    &[*object, *key],
                    Some(*dest),
                )
            }
        }
        Instruction::OptionalGetElem { dest, object, key } => lower_value_operation(
            builder,
            variables,
            root_frame,
            dispatcher,
            ctx,
            NativeRuntimeOp::OptionalGetElem,
            &[*object, *key],
            Some(*dest),
        ),
        Instruction::GetSuperBase { dest } => {
            let result = call_dispatcher(
                builder,
                root_frame,
                dispatcher,
                ctx,
                NativeRuntimeOp::GetSuperBase.id(),
                &[],
                None,
            )?;
            define_value(builder, variables, *dest, result)
        }
        Instruction::GetSuperConstructor { dest } => {
            let result = call_dispatcher(
                builder,
                root_frame,
                dispatcher,
                ctx,
                NativeRuntimeOp::GetSuperConstructor.id(),
                &[],
                None,
            )?;
            define_value(builder, variables, *dest, result)
        }
        Instruction::ObjectSpread { dest, source } => lower_value_operation(
            builder,
            variables,
            root_frame,
            dispatcher,
            ctx,
            NativeRuntimeOp::ObjectSpread,
            &[*dest, *source],
            None,
        ),
        Instruction::GuardSameFunction {
            dest,
            callee,
            function,
        } => {
            let callee = use_value(builder, variables, *callee)?;
            let function = builder.ins().iconst(types::I64, i64::from(function.0));
            let result = call_dispatcher(
                builder,
                root_frame,
                dispatcher,
                ctx,
                NativeRuntimeOp::GuardSameFunction.id(),
                &[callee, function],
                None,
            )?;
            define_value(builder, variables, *dest, result)
        }
        Instruction::CollectRestArgs { dest, skip } => {
            let skip = builder.ins().iconst(types::I64, i64::from(*skip));
            let result = call_dispatcher(
                builder,
                root_frame,
                dispatcher,
                ctx,
                NativeRuntimeOp::CollectRestArguments.id(),
                &[skip],
                None,
            )?;
            define_value(builder, variables, *dest, result)
        }
        Instruction::IsException { dest, value: input } => {
            let input = use_value(builder, variables, *input)?;
            let condition = emit_is_exception(builder, input);
            let true_value = builder.ins().iconst(types::I64, value::encode_bool(true));
            let false_value = builder.ins().iconst(types::I64, value::encode_bool(false));
            let boolean = builder.ins().select(condition, true_value, false_value);
            define_value(builder, variables, *dest, boolean)
        }
        Instruction::EncodeException { dest, value: input } => {
            let input = use_value(builder, variables, *input)?;
            let result = call_dispatcher(
                builder,
                root_frame,
                dispatcher,
                ctx,
                NativeRuntimeOp::CreateException.id(),
                &[input],
                None,
            )?;
            define_value(builder, variables, *dest, result)
        }
        Instruction::PromiseResolve { promise, value } => lower_builtin_operation(
            builder,
            variables,
            root_frame,
            dispatcher,
            ctx,
            Builtin::PromiseInstanceResolve,
            &[*promise, *value],
            None,
        ),
        Instruction::PromiseReject { promise, reason } => lower_builtin_operation(
            builder,
            variables,
            root_frame,
            dispatcher,
            ctx,
            Builtin::PromiseInstanceReject,
            &[*promise, *reason],
            None,
        ),
        Instruction::ExceptionToObject { dest, value: input } => {
            let input = use_value(builder, variables, *input)?;
            let result = call_dispatcher(
                builder,
                root_frame,
                dispatcher,
                ctx,
                NativeRuntimeOp::ExceptionValue.id(),
                &[input],
                None,
            )?;
            define_value(builder, variables, *dest, result)
        }
        Instruction::StoreVar { name, value } => {
            let value = use_value(builder, variables, *value)?;
            if let Some(local) = frame_locals.get(name).copied() {
                builder.def_var(local, value);
                if let Some(index) = frame_local_indices.get(name).copied() {
                    root_frame.update_pinned_local(builder, index, value)?;
                }
                return Ok(());
            }
            let slot = variable_slots
                .get(name)
                .copied()
                .with_context(|| format!("variable slot is missing for {name}"))?;
            let slot = builder.ins().iconst(types::I64, i64::from(slot));
            let _ = call_dispatcher(
                builder,
                root_frame,
                dispatcher,
                ctx,
                NativeRuntimeOp::StoreVar.id(),
                &[slot, value],
                None,
            )?;
            Ok(())
        }
        Instruction::LoadVar { dest, name } => {
            if let Some(local) = frame_locals.get(name).copied() {
                let value = builder.use_var(local);
                return define_value(builder, variables, *dest, value);
            }
            let slot = variable_slots
                .get(name)
                .copied()
                .with_context(|| format!("variable slot is missing for {name}"))?;
            let slot = builder.ins().iconst(types::I64, i64::from(slot));
            let value = call_dispatcher(
                builder,
                root_frame,
                dispatcher,
                ctx,
                NativeRuntimeOp::LoadVar.id(),
                &[slot],
                None,
            )?;
            define_value(builder, variables, *dest, value)
        }

        Instruction::Suspend { promise, state } => {
            let promise = use_value(builder, variables, *promise)?;
            let suspend_state = builder
                .ins()
                .iconst(types::I64, value::encode_f64(f64::from(*state)));
            let result = call_dispatcher(
                builder,
                root_frame,
                dispatcher,
                ctx,
                Builtin::AsyncFunctionSuspend.wire_id().into(),
                &[promise, suspend_state],
                None,
            )?;
            root_frame.unlink(builder, ctx)?;
            builder.ins().return_(&[result]);
            Ok(())
        }
        Instruction::GeneratorSuspend { result, state } => {
            let result = use_value(builder, variables, *result)?;
            let continuation = call_dispatcher(
                builder,
                root_frame,
                dispatcher,
                ctx,
                NativeRuntimeOp::LoadCallEnv.id(),
                &[],
                None,
            )?;
            root_frame.publish(builder, variables, roots, &[continuation])?;
            let slot = builder.ins().iconst(types::I64, value::encode_f64(0.0));
            let suspend_state = builder
                .ins()
                .iconst(types::I64, value::encode_f64(f64::from(*state)));
            let _ = call_dispatcher(
                builder,
                root_frame,
                dispatcher,
                ctx,
                Builtin::ContinuationSaveVar.wire_id().into(),
                &[continuation, slot, suspend_state],
                None,
            )?;
            root_frame.unlink(builder, ctx)?;
            builder.ins().return_(&[result]);
            Ok(())
        }
        Instruction::DebugCheck { line, col } => {
            let function = builder.ins().iconst(types::I64, i64::from(function_index));
            let line = builder.ins().iconst(types::I64, i64::from(*line));
            let col = builder.ins().iconst(types::I64, i64::from(*col));
            let _ = call_dispatcher(
                builder,
                root_frame,
                dispatcher,
                ctx,
                NativeRuntimeOp::DebugCheck.id(),
                &[function, line, col],
                None,
            )?;
            Ok(())
        }
        unsupported => bail!("native lowering does not yet own instruction {unsupported}"),
    }
}

#[allow(clippy::too_many_arguments)]
fn lower_call_instruction(
    builder: &mut FunctionBuilder<'_>,
    variables: &HashMap<ValueId, Variable>,
    dispatcher: ir::FuncRef,
    ctx: ir::Value,
    slow_call_signature: ir::SigRef,
    destination: Option<ValueId>,
    callee: ValueId,
    this_value: ValueId,
    args: &[ValueId],
    operation: NativeRuntimeOp,
    forward_args: bool,
    root_frame: &mut FrameLowering,
    roots: &[ValueId],
    feedback_ptr: Option<ir::Value>,
) -> Result<()> {
    let callee = use_value(builder, variables, callee)?;
    let this_value = use_value(builder, variables, this_value)?;
    let mut call_args = Vec::with_capacity(if forward_args { 1 } else { args.len() + 1 });
    call_args.push(callee);
    if !forward_args {
        for argument in args {
            call_args.push(use_value(builder, variables, *argument)?);
        }
    }
    let entry = call_dispatcher(
        builder,
        root_frame,
        dispatcher,
        ctx,
        operation.id(),
        &call_args,
        feedback_ptr,
    )?;
    let args_len = if forward_args {
        let entry_block = builder
            .func
            .layout
            .entry_block()
            .context("native function is missing entry block")?;
        builder.block_params(entry_block)[4]
    } else {
        builder.ins().iconst(
            types::I32,
            i64::try_from(args.len()).context("call argument count exceeds i64")?,
        )
    };
    let active_len = builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        ctx,
        i32::try_from(offset_of!(NativeVmContext, call_arena_active_len))
            .context("call arena active length offset exceeds i32")?,
    );
    let args_base = builder.ins().isub(active_len, args_len);
    let env = call_dispatcher(
        builder,
        root_frame,
        dispatcher,
        ctx,
        NativeRuntimeOp::LoadCallEnv.id(),
        &[],
        None,
    )?;
    let call = builder.ins().call_indirect(
        slow_call_signature,
        entry,
        &[ctx, env, this_value, args_base, args_len],
    );
    let result = builder.inst_results(call)[0];
    root_frame.publish(builder, variables, roots, &[result])?;
    let _ = call_dispatcher(
        builder,
        root_frame,
        dispatcher,
        ctx,
        NativeRuntimeOp::FinishCall.id(),
        &[],
        None,
    )?;
    if let Some(destination) = destination {
        define_value(builder, variables, destination, result)?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn lower_optional_call_instruction(
    builder: &mut FunctionBuilder<'_>,
    variables: &HashMap<ValueId, Variable>,
    dispatcher: ir::FuncRef,
    ctx: ir::Value,
    slow_call_signature: ir::SigRef,
    destination: ValueId,
    callee: ValueId,
    this_value: ValueId,
    args: &[ValueId],
    root_frame: &mut FrameLowering,
    roots: &[ValueId],
    feedback_ptr: Option<ir::Value>,
) -> Result<()> {
    let encoded_callee = use_value(builder, variables, callee)?;
    let nullish = call_dispatcher(
        builder,
        root_frame,
        dispatcher,
        ctx,
        NativeRuntimeOp::UnaryIsNullish.id(),
        &[encoded_callee],
        None,
    )?;
    let is_nullish = builder.ins().icmp_imm_s(
        ir::condcodes::IntCC::Equal,
        nullish,
        value::encode_bool(true),
    );
    let skip_block = builder.create_block();
    let call_block = builder.create_block();
    let continuation = builder.create_block();
    builder
        .ins()
        .brif(is_nullish, skip_block, &[], call_block, &[]);

    builder.switch_to_block(skip_block);
    builder.seal_block(skip_block);
    let undefined = builder.ins().iconst(types::I64, value::encode_undefined());
    define_value(builder, variables, destination, undefined)?;
    builder.ins().jump(continuation, &[]);

    builder.switch_to_block(call_block);
    builder.seal_block(call_block);
    lower_call_instruction(
        builder,
        variables,
        dispatcher,
        ctx,
        slow_call_signature,
        Some(destination),
        callee,
        this_value,
        args,
        NativeRuntimeOp::PrepareCall,
        false,
        root_frame,
        roots,
        feedback_ptr,
    )?;
    builder.ins().jump(continuation, &[]);

    builder.switch_to_block(continuation);
    builder.seal_block(continuation);
    Ok(())
}
#[allow(clippy::too_many_arguments)]
fn lower_builtin_operation(
    builder: &mut FunctionBuilder<'_>,
    variables: &HashMap<ValueId, Variable>,
    root_frame: &mut FrameLowering,
    dispatcher: ir::FuncRef,
    ctx: ir::Value,
    builtin: Builtin,
    args: &[ValueId],
    feedback_ptr: Option<ir::Value>,
) -> Result<()> {
    let args = args
        .iter()
        .map(|value| use_value(builder, variables, *value))
        .collect::<Result<Vec<_>>>()?;
    let result = call_dispatcher(
        builder,
        root_frame,
        dispatcher,
        ctx,
        builtin.wire_id().into(),
        &args,
        feedback_ptr,
    )?;
    if builtin == Builtin::PromiseInstanceResolve || builtin == Builtin::PromiseInstanceReject {
        return Ok(());
    }
    let _ = result;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn lower_dynamic_binary(
    builder: &mut FunctionBuilder<'_>,
    variables: &HashMap<ValueId, Variable>,
    root_frame: &mut FrameLowering,
    dispatcher: ir::FuncRef,
    ctx: ir::Value,
    dest: ValueId,
    op: BinaryOp,
    lhs: ValueId,
    rhs: ValueId,
    feedback_ptr: Option<ir::Value>,
) -> Result<()> {
    let lhs = use_value(builder, variables, lhs)?;
    let rhs = use_value(builder, variables, rhs)?;

    // 位运算、% 与 ** 仍需 ToPrimitive/ToNumber/ToBigInt 等完整语义，继续走 dispatcher。
    if !matches!(
        op,
        BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div
    ) {
        let operation = DYNAMIC_BINARY_BASE + u32::from(binary_tag(op));
        let result = call_dispatcher(
            builder,
            root_frame,
            dispatcher,
            ctx,
            operation,
            &[lhs, rhs],
            feedback_ptr,
        )?;
        return define_value(builder, variables, dest, result);
    }

    // #389 的 number 快路径不经过 dispatcher，二元反馈必须在守卫前内联更新，
    // number/number 热路径才可被观察。更新覆盖 fast/slow 两条路径，因此下方
    // 慢路径的 dispatcher 调用传 null 槽，避免同一次执行重复计数。
    if let Some(slot) = feedback_ptr {
        let operation = DYNAMIC_BINARY_BASE + u32::from(binary_tag(op));
        emit_inline_binary_feedback(builder, ctx, slot, operation, lhs, rhs);
    }

    // 守卫：两边必须都是原始 f64（非 NaN-boxed 的 number）才走原生指令。
    // string 拼接、BigInt、对象 ToPrimitive 等 NaN-boxed 值一律 miss 落 dispatcher。
    let lhs_is_number = emit_is_number(builder, lhs);
    let rhs_is_number = emit_is_number(builder, rhs);
    let both_numbers = builder.ins().band(lhs_is_number, rhs_is_number);

    let fast_block = builder.create_block();
    let slow_block = builder.create_block();
    let merge_block = builder.create_block();
    builder
        .ins()
        .brif(both_numbers, fast_block, &[], slow_block, &[]);

    // 快路径：位模式即 IEEE-754 f64，直接 bitcast 后发原生浮点指令。
    // NaN 在 box_f64_result 中规范化为运行时一致的正向 quiet NaN。
    builder.switch_to_block(fast_block);
    builder.seal_block(fast_block);
    let lhs_f64 = builder
        .ins()
        .bitcast(types::F64, ir::MemFlagsData::new(), lhs);
    let rhs_f64 = builder
        .ins()
        .bitcast(types::F64, ir::MemFlagsData::new(), rhs);
    let result = match op {
        BinaryOp::Add => builder.ins().fadd(lhs_f64, rhs_f64),
        BinaryOp::Sub => builder.ins().fsub(lhs_f64, rhs_f64),
        BinaryOp::Mul => builder.ins().fmul(lhs_f64, rhs_f64),
        BinaryOp::Div => builder.ins().fdiv(lhs_f64, rhs_f64),
        _ => unreachable!("guard restricts guarded binary operations"),
    };
    let result = box_f64_result(builder, result);
    define_value(builder, variables, dest, result)?;
    builder.ins().jump(merge_block, &[]);

    // 慢路径：完整 JS 语义（ToPrimitive、string 拼接、BigInt、异常）。
    builder.switch_to_block(slow_block);
    builder.seal_block(slow_block);
    let operation = DYNAMIC_BINARY_BASE + u32::from(binary_tag(op));
    let result = call_dispatcher(
        builder,
        root_frame,
        dispatcher,
        ctx,
        operation,
        &[lhs, rhs],
        None,
    )?;
    define_value(builder, variables, dest, result)?;
    builder.ins().jump(merge_block, &[]);

    builder.switch_to_block(merge_block);
    builder.seal_block(merge_block);
    Ok(())
}

/// 反馈槽字段 offset（`wjsm-ir::constants` 的 u32）转 CLIF 用的 i32。
pub(crate) fn feedback_offset_i32(offset: u32) -> i32 {
    i32::try_from(offset).expect("feedback slot offset fits i32")
}

/// 计算一个 boxed 值的反馈 tag 码：number → `0x1f`，NaN-box 值 → 自身 tag。
pub(crate) fn emit_feedback_tag_code(
    builder: &mut FunctionBuilder<'_>,
    input: ir::Value,
) -> ir::Value {
    let is_number = emit_is_number(builder, input);
    let tag = builder.ins().ushr_imm_u(input, 32);
    let tag = builder.ins().band_imm_u(
        tag,
        i64::try_from(value::TAG_MASK).expect("tag mask fits i64"),
    );
    let number_code = builder.ins().iconst(
        types::I64,
        i64::from(wjsm_native_abi::NativeFeedbackTag::Number.code()),
    );
    builder.ins().select(is_number, number_code, tag)
}

/// 守卫二元运算的零分配反馈更新：与宿主 dispatcher 的记录共享同一套槽字段协议。
///
/// 只写 tag 签名与计数（目标字段恒 0：二元操作没有调用目标）；签名变化时
/// 重置连续计数。`ctx.flags` 未置反馈位时整个更新被跳过，generic 对照路径
/// 零开销。
fn emit_inline_binary_feedback(
    builder: &mut FunctionBuilder<'_>,
    ctx: ir::Value,
    slot: ir::Value,
    operation: u32,
    lhs: ir::Value,
    rhs: ir::Value,
) {
    let flags = builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        ctx,
        vmctx_offset(offset_of!(NativeVmContext, flags)).expect("vmctx flags offset fits i32"),
    );
    let enabled = builder.ins().band_imm_u(
        flags,
        i64::from(wjsm_native_abi::NATIVE_FLAGS_FEEDBACK_ENABLED),
    );
    let enabled = builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::NotEqual, enabled, 0);

    let update_block = builder.create_block();
    let skip_block = builder.create_block();
    let continuation = builder.create_block();
    builder
        .ins()
        .brif(enabled, update_block, &[], skip_block, &[]);

    builder.switch_to_block(update_block);
    builder.seal_block(update_block);
    let lhs_code = emit_feedback_tag_code(builder, lhs);
    let rhs_code = emit_feedback_tag_code(builder, rhs);
    // 签名 = count(2) | lhs_tag << 4 | rhs_tag << 10，与 encode_feedback_tag_signature 一致。
    let count = builder.ins().iconst(types::I64, 2);
    let lhs_shifted = builder.ins().ishl_imm_u(lhs_code, 4);
    let rhs_shifted = builder.ins().ishl_imm_u(rhs_code, 10);
    let signature = builder.ins().bor(count, lhs_shifted);
    let signature = builder.ins().bor(signature, rhs_shifted);
    let previous = builder.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        slot,
        feedback_offset_i32(constants::FEEDBACK_SLOT_TAG_SIGNATURE_OFFSET),
    );
    builder.ins().store(
        MemFlagsData::trusted(),
        signature,
        slot,
        feedback_offset_i32(constants::FEEDBACK_SLOT_TAG_SIGNATURE_OFFSET),
    );
    let same_signature = builder
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, previous, signature);
    let consecutive = builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        slot,
        feedback_offset_i32(constants::FEEDBACK_SLOT_CONSECUTIVE_OFFSET),
    );
    let incremented = builder.ins().iadd_imm_s(consecutive, 1);
    let restart = builder.ins().iconst(types::I32, 1);
    let next_consecutive = builder.ins().select(same_signature, incremented, restart);
    builder.ins().store(
        MemFlagsData::trusted(),
        next_consecutive,
        slot,
        feedback_offset_i32(constants::FEEDBACK_SLOT_CONSECUTIVE_OFFSET),
    );
    let total = builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        slot,
        feedback_offset_i32(constants::FEEDBACK_SLOT_TOTAL_OFFSET),
    );
    let total = builder.ins().iadd_imm_s(total, 1);
    builder.ins().store(
        MemFlagsData::trusted(),
        total,
        slot,
        feedback_offset_i32(constants::FEEDBACK_SLOT_TOTAL_OFFSET),
    );
    let operation_value = builder.ins().iconst(types::I32, i64::from(operation));
    builder.ins().store(
        MemFlagsData::trusted(),
        operation_value,
        slot,
        feedback_offset_i32(constants::FEEDBACK_SLOT_OPERATION_OFFSET),
    );
    let recording = builder
        .ins()
        .iconst(types::I32, i64::from(constants::FEEDBACK_STATE_RECORDING));
    builder.ins().store(
        MemFlagsData::trusted(),
        recording,
        slot,
        feedback_offset_i32(constants::FEEDBACK_SLOT_STATE_OFFSET),
    );
    builder.ins().jump(continuation, &[]);

    builder.switch_to_block(skip_block);
    builder.seal_block(skip_block);
    builder.ins().jump(continuation, &[]);

    builder.switch_to_block(continuation);
    builder.seal_block(continuation);
}

/// 常量字符串键的 GetProp 快路径入口：创建 merge 块后交给共享的非 nullish
/// IC 核心。GetProp 与 OptionalGetProp 的非 nullish 分支语义相同。
#[expect(
    clippy::too_many_arguments,
    reason = "与 lower_instruction 的既有参数集合保持一致，全部为 lowering 上下文"
)]
fn lower_get_prop_ic(
    builder: &mut FunctionBuilder<'_>,
    variables: &HashMap<ValueId, Variable>,
    root_frame: &mut FrameLowering,
    dispatcher: ir::FuncRef,
    barrier_thunks: &BarrierThunks,
    ctx: ir::Value,
    dest: ValueId,
    object: ValueId,
    key: ValueId,
    slot: u32,
    roots: &[ValueId],
) -> Result<()> {
    let merge_block = builder.create_block();
    lower_get_prop_ic_non_nullish(
        builder,
        variables,
        root_frame,
        dispatcher,
        barrier_thunks,
        ctx,
        dest,
        object,
        key,
        slot,
        roots,
        merge_block,
    )?;
    builder.switch_to_block(merge_block);
    builder.seal_block(merge_block);
    Ok(())
}

/// GetProp IC 的共享核心：调用方保证 `object` 已通过 OptionalGetProp 的
/// nullish 检查（GetProp 自身无需）。命中路径有三条：
/// - OWN_DATA：接收者 shape 命中后单 load 值槽；
/// - PROTO_DATA：接收者 shape + proto 世代命中后，从 holder 值槽 load；
/// - ACCESSOR：接收者 shape + proto 世代命中后 load getter，并直接
///   `invoke_callable(getter, receiver)`（不查属性表）。
///
/// 其余情况 miss 到 `GetPropIc` 走完整宿主 [[Get]] 并回填。
#[expect(
    clippy::too_many_arguments,
    reason = "与 lower_instruction 的既有参数集合保持一致，全部为 lowering 上下文"
)]
fn lower_get_prop_ic_non_nullish(
    builder: &mut FunctionBuilder<'_>,
    variables: &HashMap<ValueId, Variable>,
    root_frame: &mut FrameLowering,
    dispatcher: ir::FuncRef,
    barrier_thunks: &BarrierThunks,
    ctx: ir::Value,
    dest: ValueId,
    object: ValueId,
    key: ValueId,
    slot: u32,
    roots: &[ValueId],
    merge_block: ir::Block,
) -> Result<()> {
    let pointer_type = builder.func.dfg.value_type(ctx);
    let obj = use_value(builder, variables, object)?;
    let key_value = use_value(builder, variables, key)?;
    let box_base = i64::from_ne_bytes(value::BOX_BASE.to_ne_bytes());

    // vmctx 基址：句柄表 region 与当前 image 的 IC 区。
    let ht_base = builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        ctx,
        vmctx_offset(offset_of!(NativeVmContext, handle_table_base))?,
    );
    let ic_base = builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        ctx,
        vmctx_offset(offset_of!(NativeVmContext, ic_slots_base))?,
    );
    let barrier_state = builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        ctx,
        vmctx_offset(offset_of!(NativeVmContext, barrier_state))?,
    );

    // 标签检查：仅 NaN-box 的 TAG_OBJECT 才可解句柄读 entry。
    let boxed_bits = builder.ins().band_imm_s(obj, box_base);
    let is_boxed = builder
        .ins()
        .icmp_imm_s(ir::condcodes::IntCC::Equal, boxed_bits, box_base);
    let tag = builder.ins().ushr_imm_u(obj, 32);
    let tag = builder.ins().band_imm_u(
        tag,
        i64::try_from(value::TAG_MASK).expect("tag mask fits i64"),
    );
    let is_obj = builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        tag,
        i64::try_from(value::TAG_OBJECT).expect("object tag fits i64"),
    );
    let tag_ok = builder.ins().band(is_boxed, is_obj);

    // IC 槽指针：基于 ic_base（当前 image 的 IC 区，始终映射），放在入口块计算
    // 以支配所有后续分支（miss 分支需要它作为 GetPropIc 的回填目标）。
    let ic_ptr = builder.ins().iadd_imm_s(
        ic_base,
        i64::from(slot) * i64::from(constants::IC_SLOT_SIZE),
    );

    let entry_block = builder.create_block();
    let legacy_entry_block = builder.create_block();
    let zgc_kind_block = builder.create_block();
    let zgc_entry_block = builder.create_block();
    let zgc_fast_block = builder.create_block();
    let receiver_assist_block = builder.create_block();
    let shape_check_block = builder.create_block();
    builder.append_block_param(shape_check_block, types::I64);
    let shape_hit_block = builder.create_block();
    let own_hit_block = builder.create_block();
    let holder_block = builder.create_block();
    let holder_resolve_block = builder.create_block();
    let holder_legacy_block = builder.create_block();
    let holder_zgc_block = builder.create_block();
    let holder_fast_block = builder.create_block();
    let holder_assist_block = builder.create_block();
    let holder_addr_block = builder.create_block();
    builder.append_block_param(holder_addr_block, types::I64);
    let proto_hit_block = builder.create_block();
    let accessor_hit_block = builder.create_block();
    let miss_block = builder.create_block();
    // 第一级：标签必须是 TAG_OBJECT。**句柄表 entry 读取必须放在此分支之后**：
    // `trusted()`（notrap）load 允许 Cranelift 块内投机提前，若 entry 读取与
    // tag 检查同块，非对象值（字符串等）的 handle 可能落在未提交的 block，
    // 投机读取直接段错误。条件分支隔离后跨块提升不合法，entry 只在
    // `tag_ok` 为真（对象句柄必然已分配提交）后才读取。
    builder
        .ins()
        .brif(tag_ok, entry_block, &[], miss_block, &[]);

    // 第二级：读取接收者句柄 entry。Disabled 模式沿用稳定态快链；ZGC 只有偶数
    // access epoch 与稳定 entry 能直接使用地址，其余状态进入 no-GC load assist。
    builder.switch_to_block(entry_block);
    builder.seal_block(entry_block);
    let handle_idx = builder.ins().band_imm_u(obj, i64::from(u32::MAX));
    let handle_i32 = builder.ins().ireduce(types::I32, handle_idx);
    let entry_offset = builder.ins().ishl_imm_u(handle_idx, 3);
    let entry_addr = builder.ins().iadd(ht_base, entry_offset);
    let entry = builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), entry_addr, 0);
    let entry_state = builder.ins().band_imm_u(entry, 0xFFFF);
    let stable = builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::UnsignedGreaterThanOrEqual,
        entry_state,
        i64::from(constants::HANDLE_STATE_STABLE_MIN),
    );
    let logical_addr = builder.ins().ushr_imm_u(entry, 16);
    let heap_delta = builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        ctx,
        vmctx_offset(offset_of!(NativeVmContext, heap_object_delta))?,
    );
    // IC 槽（32 字节）：
    // word0 = shape_id(lo32) | value_index(hi32)
    // word1 = kind(lo32) | proto_generation(hi32)
    // word2 = holder_handle(lo32) | expected_proto(hi32)
    let ic_word0 = builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), ic_ptr, 0);
    let ic_shape = builder.ins().band_imm_u(ic_word0, i64::from(u32::MAX));
    let ic_val_idx = builder.ins().ushr_imm_u(ic_word0, 32);
    let ic_word1 = builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), ic_ptr, 8);
    let ic_kind = builder.ins().band_imm_u(ic_word1, i64::from(u32::MAX));
    let ic_generation = builder.ins().ushr_imm_u(ic_word1, 32);
    let ic_word2 = builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), ic_ptr, 16);
    let ic_holder = builder.ins().band_imm_u(ic_word2, i64::from(u32::MAX));
    let ic_expected_proto = builder.ins().ushr_imm_u(ic_word2, 32);
    let kind_own = builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        ic_kind,
        i64::from(constants::IC_KIND_OWN_DATA),
    );
    let kind_proto = builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        ic_kind,
        i64::from(constants::IC_KIND_PROTO_DATA),
    );
    let kind_accessor = builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        ic_kind,
        i64::from(constants::IC_KIND_ACCESSOR),
    );
    let kind_holder = builder.ins().bor(kind_proto, kind_accessor);
    let kind_supported = builder.ins().bor(kind_own, kind_holder);
    let barrier_disabled = builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::Equal, barrier_state, 0);
    builder.ins().brif(
        barrier_disabled,
        legacy_entry_block,
        &[],
        zgc_kind_block,
        &[],
    );

    builder.switch_to_block(legacy_entry_block);
    builder.seal_block(legacy_entry_block);
    let legacy_ok = builder.ins().band(stable, kind_supported);
    builder.ins().brif(
        legacy_ok,
        shape_check_block,
        &[ir::BlockArg::Value(logical_addr)],
        miss_block,
        &[],
    );

    builder.switch_to_block(zgc_kind_block);
    builder.seal_block(zgc_kind_block);
    builder
        .ins()
        .brif(kind_supported, zgc_entry_block, &[], miss_block, &[]);

    builder.switch_to_block(zgc_entry_block);
    builder.seal_block(zgc_entry_block);
    let epoch_addr = builder.ins().iadd_imm_s(
        barrier_state,
        i64::try_from(offset_of!(NativeBarrierState, access_epoch))
            .expect("access epoch offset fits i64"),
    );
    let access_epoch = builder
        .ins()
        .atomic_load(types::I64, MemFlagsData::trusted(), epoch_addr);
    let epoch_bit = builder.ins().band_imm_u(access_epoch, 1);
    let epoch_even = builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::Equal, epoch_bit, 0);
    let direct = builder.ins().band(stable, epoch_even);
    builder
        .ins()
        .brif(direct, zgc_fast_block, &[], receiver_assist_block, &[]);

    builder.switch_to_block(zgc_fast_block);
    builder.seal_block(zgc_fast_block);
    increment_barrier_counter(
        builder,
        barrier_state,
        offset_of!(NativeBarrierState, load_fast_events),
    );
    builder
        .ins()
        .jump(shape_check_block, &[ir::BlockArg::Value(logical_addr)]);

    builder.switch_to_block(receiver_assist_block);
    builder.seal_block(receiver_assist_block);
    let call = builder.ins().call(barrier_thunks.load, &[ctx, handle_i32]);
    let assisted = builder.inst_results(call)[0];
    let assisted_ok = builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::NotEqual, assisted, 0);
    builder.ins().brif(
        assisted_ok,
        shape_check_block,
        &[ir::BlockArg::Value(assisted)],
        miss_block,
        &[],
    );

    // 第三级：对象地址已经过稳定态检查或 load assist，读取 shape 并与 IC 槽比对。
    builder.switch_to_block(shape_check_block);
    builder.seal_block(shape_check_block);
    let logical_addr = builder.block_params(shape_check_block)[0];
    let addr = builder.ins().iadd(logical_addr, heap_delta);
    let obj_word = builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), addr, 8);
    let obj_shape = builder.ins().ushr_imm_u(obj_word, 32);
    let shape_match = builder
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, obj_shape, ic_shape);
    builder
        .ins()
        .brif(shape_match, shape_hit_block, &[], miss_block, &[]);

    // shape 命中后按 kind 分派：OWN_DATA 直达自有值槽；其余先校验直接原型与世代。
    builder.switch_to_block(shape_hit_block);
    builder.seal_block(shape_hit_block);
    builder
        .ins()
        .brif(kind_own, own_hit_block, &[], holder_block, &[]);

    // ProtoData / Accessor：同一 shape 的 receiver 可以有不同直接原型，故先比较
    // 对象头里的 proto handle；再比较原型世代以覆盖链上属性或原型变化。
    builder.switch_to_block(holder_block);
    builder.seal_block(holder_block);
    let receiver_header = builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), addr, 0);
    let receiver_proto = builder
        .ins()
        .band_imm_u(receiver_header, i64::from(u32::MAX));
    let proto_match = builder.ins().icmp(
        ir::condcodes::IntCC::Equal,
        receiver_proto,
        ic_expected_proto,
    );
    let current_generation = builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        ctx,
        vmctx_offset(offset_of!(NativeVmContext, proto_generation))?,
    );
    let current_generation = builder.ins().uextend(types::I64, current_generation);
    let generation_match = builder.ins().icmp(
        ir::condcodes::IntCC::Equal,
        current_generation,
        ic_generation,
    );
    let holder_valid = builder.ins().band(proto_match, generation_match);
    builder
        .ins()
        .brif(holder_valid, holder_resolve_block, &[], miss_block, &[]);

    // 解析 holder_handle → holder entry → holder 地址；ZGC holder 与 receiver 使用
    // 同一 access epoch 协议，odd epoch 或 relocating entry 必须进入 load assist。
    builder.switch_to_block(holder_resolve_block);
    builder.seal_block(holder_resolve_block);
    let holder_entry_offset = builder.ins().ishl_imm_u(ic_holder, 3);
    let holder_entry_addr = builder.ins().iadd(ht_base, holder_entry_offset);
    let holder_entry =
        builder
            .ins()
            .load(types::I64, MemFlagsData::trusted(), holder_entry_addr, 0);
    let holder_state = builder.ins().band_imm_u(holder_entry, 0xFFFF);
    let holder_stable = builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::UnsignedGreaterThanOrEqual,
        holder_state,
        i64::from(constants::HANDLE_STATE_STABLE_MIN),
    );
    let holder_logical_addr = builder.ins().ushr_imm_u(holder_entry, 16);
    builder.ins().brif(
        barrier_disabled,
        holder_legacy_block,
        &[],
        holder_zgc_block,
        &[],
    );

    builder.switch_to_block(holder_legacy_block);
    builder.seal_block(holder_legacy_block);
    builder.ins().brif(
        holder_stable,
        holder_addr_block,
        &[ir::BlockArg::Value(holder_logical_addr)],
        miss_block,
        &[],
    );

    builder.switch_to_block(holder_zgc_block);
    builder.seal_block(holder_zgc_block);
    let holder_epoch_addr = builder.ins().iadd_imm_s(
        barrier_state,
        i64::try_from(offset_of!(NativeBarrierState, access_epoch))
            .expect("access epoch offset fits i64"),
    );
    let holder_epoch =
        builder
            .ins()
            .atomic_load(types::I64, MemFlagsData::trusted(), holder_epoch_addr);
    let holder_epoch_bit = builder.ins().band_imm_u(holder_epoch, 1);
    let holder_epoch_even =
        builder
            .ins()
            .icmp_imm_u(ir::condcodes::IntCC::Equal, holder_epoch_bit, 0);
    let holder_direct = builder.ins().band(holder_stable, holder_epoch_even);
    builder.ins().brif(
        holder_direct,
        holder_fast_block,
        &[],
        holder_assist_block,
        &[],
    );

    builder.switch_to_block(holder_fast_block);
    builder.seal_block(holder_fast_block);
    increment_barrier_counter(
        builder,
        barrier_state,
        offset_of!(NativeBarrierState, load_fast_events),
    );
    builder.ins().jump(
        holder_addr_block,
        &[ir::BlockArg::Value(holder_logical_addr)],
    );

    builder.switch_to_block(holder_assist_block);
    builder.seal_block(holder_assist_block);
    let holder_i32 = builder.ins().ireduce(types::I32, ic_holder);
    let call = builder.ins().call(barrier_thunks.load, &[ctx, holder_i32]);
    let assisted_holder = builder.inst_results(call)[0];
    let assisted_holder_ok =
        builder
            .ins()
            .icmp_imm_u(ir::condcodes::IntCC::NotEqual, assisted_holder, 0);
    builder.ins().brif(
        assisted_holder_ok,
        holder_addr_block,
        &[ir::BlockArg::Value(assisted_holder)],
        miss_block,
        &[],
    );

    builder.switch_to_block(holder_addr_block);
    builder.seal_block(holder_addr_block);
    let holder_logical_addr = builder.block_params(holder_addr_block)[0];
    let holder_addr = builder.ins().iadd(holder_logical_addr, heap_delta);
    builder
        .ins()
        .brif(kind_accessor, accessor_hit_block, &[], proto_hit_block, &[]);

    // OWN_DATA 命中：`HEAP_OBJECT_HEADER_SIZE + value_index * 8` 处单 load。
    builder.switch_to_block(own_hit_block);
    builder.seal_block(own_hit_block);
    let value_shift = builder.ins().ishl_imm_u(ic_val_idx, 3); // × 值槽 8 字节
    let value_offset = builder
        .ins()
        .iadd_imm_s(value_shift, i64::from(constants::HEAP_OBJECT_HEADER_SIZE));
    let value_addr = builder.ins().iadd(addr, value_offset);
    let value = builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), value_addr, 0);
    define_value(builder, variables, dest, value)?;
    builder.ins().jump(merge_block, &[]);

    // PROTO_DATA 命中：从 holder 对象的值槽 load。
    builder.switch_to_block(proto_hit_block);
    builder.seal_block(proto_hit_block);
    let proto_value_shift = builder.ins().ishl_imm_u(ic_val_idx, 3);
    let proto_value_offset = builder.ins().iadd_imm_s(
        proto_value_shift,
        i64::from(constants::HEAP_OBJECT_HEADER_SIZE),
    );
    let proto_value_addr = builder.ins().iadd(holder_addr, proto_value_offset);
    let proto_value = builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), proto_value_addr, 0);
    define_value(builder, variables, dest, proto_value)?;
    builder.ins().jump(merge_block, &[]);

    // ACCESSOR 命中：load getter 后直接走宿主 invoke_callable。getter 是刚从
    // 堆里读出的临时句柄，必须作为临时 root 发布后再发起可能触发 GC 的调用。
    builder.switch_to_block(accessor_hit_block);
    builder.seal_block(accessor_hit_block);
    let getter_shift = builder.ins().ishl_imm_u(ic_val_idx, 3);
    let getter_offset = builder
        .ins()
        .iadd_imm_s(getter_shift, i64::from(constants::HEAP_OBJECT_HEADER_SIZE));
    let getter_addr = builder.ins().iadd(holder_addr, getter_offset);
    let getter = builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), getter_addr, 0);
    root_frame.publish(builder, variables, roots, &[getter])?;
    let result = call_dispatcher(
        builder,
        root_frame,
        dispatcher,
        ctx,
        NativeRuntimeOp::GetPropAccessor.id(),
        &[getter, obj],
        None,
    )?;
    define_value(builder, variables, dest, result)?;
    builder.ins().jump(merge_block, &[]);

    // miss：宿主完整 [[Get]] + IC 回填；`ic_ptr` 作为回填目标传入。
    builder.switch_to_block(miss_block);
    builder.seal_block(miss_block);
    let result = call_dispatcher(
        builder,
        root_frame,
        dispatcher,
        ctx,
        NativeRuntimeOp::GetPropIc.id(),
        &[obj, key_value, ic_ptr],
        None,
    )?;
    define_value(builder, variables, dest, result)?;
    builder.ins().jump(merge_block, &[]);

    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "与 lower_instruction 的既有参数集合保持一致，全部为 lowering 上下文"
)]
fn lower_set_prop_ic(
    builder: &mut FunctionBuilder<'_>,
    variables: &HashMap<ValueId, Variable>,
    root_frame: &mut FrameLowering,
    dispatcher: ir::FuncRef,
    barrier_thunks: &BarrierThunks,
    ctx: ir::Value,
    dest: ValueId,
    object: ValueId,
    key: ValueId,
    value: ValueId,
    slot: u32,
) -> Result<()> {
    let pointer_type = builder.func.dfg.value_type(ctx);
    let obj = use_value(builder, variables, object)?;
    let key_value = use_value(builder, variables, key)?;
    let stored = use_value(builder, variables, value)?;
    let box_base = i64::from_ne_bytes(value::BOX_BASE.to_ne_bytes());

    // vmctx 基址：句柄表 region 与当前 image 的 IC 区。
    let ht_base = builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        ctx,
        vmctx_offset(offset_of!(NativeVmContext, handle_table_base))?,
    );
    let ic_base = builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        ctx,
        vmctx_offset(offset_of!(NativeVmContext, ic_slots_base))?,
    );
    let barrier_state = builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        ctx,
        vmctx_offset(offset_of!(NativeVmContext, barrier_state))?,
    );

    // 标签检查：仅 NaN-box 的 TAG_OBJECT 才可解句柄读 entry。
    let boxed_bits = builder.ins().band_imm_s(obj, box_base);
    let is_boxed = builder
        .ins()
        .icmp_imm_s(ir::condcodes::IntCC::Equal, boxed_bits, box_base);
    let tag = builder.ins().ushr_imm_u(obj, 32);
    let tag = builder.ins().band_imm_u(
        tag,
        i64::try_from(value::TAG_MASK).expect("tag mask fits i64"),
    );
    let is_obj = builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        tag,
        i64::try_from(value::TAG_OBJECT).expect("object tag fits i64"),
    );
    let tag_ok = builder.ins().band(is_boxed, is_obj);

    // IC 槽指针：基于 ic_base（当前 image 的 IC 区，始终映射），放在本块计算
    // 以支配所有后续分支（miss 分支需要它作为 SetPropIc 的回填目标）。
    let ic_ptr = builder.ins().iadd_imm_s(
        ic_base,
        i64::from(slot) * i64::from(constants::IC_SLOT_SIZE),
    );

    let entry_block = builder.create_block();
    let legacy_entry_block = builder.create_block();
    let zgc_kind_block = builder.create_block();
    let zgc_entry_block = builder.create_block();
    let zgc_fast_block = builder.create_block();
    let receiver_assist_block = builder.create_block();
    let shape_check_block = builder.create_block();
    builder.append_block_param(shape_check_block, types::I64);
    builder.append_block_param(shape_check_block, types::I8);
    let hit_block = builder.create_block();
    let zgc_store_mode_block = builder.create_block();
    let legacy_store_block = builder.create_block();
    let zgc_direct_store_block = builder.create_block();
    let barrier_store_block = builder.create_block();
    let store_done_block = builder.create_block();
    let miss_block = builder.create_block();
    let merge_block = builder.create_block();
    builder
        .ins()
        .brif(tag_ok, entry_block, &[], miss_block, &[]);

    builder.switch_to_block(entry_block);
    builder.seal_block(entry_block);
    let handle_idx = builder.ins().band_imm_u(obj, i64::from(u32::MAX));
    let handle_i32 = builder.ins().ireduce(types::I32, handle_idx);
    let entry_offset = builder.ins().ishl_imm_u(handle_idx, 3);
    let entry_addr = builder.ins().iadd(ht_base, entry_offset);
    let entry = builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), entry_addr, 0);
    let state = builder.ins().band_imm_u(entry, 0xFFFF);
    let stable = builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::UnsignedGreaterThanOrEqual,
        state,
        i64::from(constants::HANDLE_STATE_STABLE_MIN),
    );
    let logical_addr = builder.ins().ushr_imm_u(entry, 16);
    let heap_delta = builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        ctx,
        vmctx_offset(offset_of!(NativeVmContext, heap_object_delta))?,
    );

    let ic_word0 = builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), ic_ptr, 0);
    let ic_shape = builder.ins().band_imm_u(ic_word0, i64::from(u32::MAX));
    let ic_val_idx = builder.ins().ushr_imm_u(ic_word0, 32);
    let ic_word1 = builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), ic_ptr, 8);
    let ic_kind = builder.ins().band_imm_u(ic_word1, i64::from(u32::MAX));
    let kind_own = builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        ic_kind,
        i64::from(constants::IC_KIND_OWN_DATA),
    );
    let barrier_disabled = builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::Equal, barrier_state, 0);
    builder.ins().brif(
        barrier_disabled,
        legacy_entry_block,
        &[],
        zgc_kind_block,
        &[],
    );

    builder.switch_to_block(legacy_entry_block);
    builder.seal_block(legacy_entry_block);
    let legacy_ok = builder.ins().band(stable, kind_own);
    let direct_store = builder.ins().iconst(types::I8, 1);
    builder.ins().brif(
        legacy_ok,
        shape_check_block,
        &[
            ir::BlockArg::Value(logical_addr),
            ir::BlockArg::Value(direct_store),
        ],
        miss_block,
        &[],
    );

    builder.switch_to_block(zgc_kind_block);
    builder.seal_block(zgc_kind_block);
    builder
        .ins()
        .brif(kind_own, zgc_entry_block, &[], miss_block, &[]);

    builder.switch_to_block(zgc_entry_block);
    builder.seal_block(zgc_entry_block);
    let epoch_addr = builder.ins().iadd_imm_s(
        barrier_state,
        i64::try_from(offset_of!(NativeBarrierState, access_epoch))
            .expect("access epoch offset fits i64"),
    );
    let access_epoch = builder
        .ins()
        .atomic_load(types::I64, MemFlagsData::trusted(), epoch_addr);
    let epoch_bit = builder.ins().band_imm_u(access_epoch, 1);
    let epoch_even = builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::Equal, epoch_bit, 0);
    let direct_resolve = builder.ins().band(stable, epoch_even);
    builder.ins().brif(
        direct_resolve,
        zgc_fast_block,
        &[],
        receiver_assist_block,
        &[],
    );

    builder.switch_to_block(zgc_fast_block);
    builder.seal_block(zgc_fast_block);
    let phase_addr = builder.ins().iadd_imm_s(
        barrier_state,
        i64::try_from(offset_of!(NativeBarrierState, phase)).expect("phase offset fits i64"),
    );
    let phase = builder
        .ins()
        .atomic_load(types::I64, MemFlagsData::trusted(), phase_addr);
    let marking = builder
        .ins()
        .band_imm_u(phase, NATIVE_BARRIER_MARKING_MASK as i64);
    let marking_idle = builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::Equal, marking, 0);
    let stable_young = builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        state,
        i64::from(constants::HANDLE_STATE_STABLE_YOUNG),
    );
    let direct_store = builder.ins().band(marking_idle, stable_young);
    builder.ins().jump(
        shape_check_block,
        &[
            ir::BlockArg::Value(logical_addr),
            ir::BlockArg::Value(direct_store),
        ],
    );

    builder.switch_to_block(receiver_assist_block);
    builder.seal_block(receiver_assist_block);
    let call = builder.ins().call(barrier_thunks.load, &[ctx, handle_i32]);
    let assisted = builder.inst_results(call)[0];
    let assisted_ok = builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::NotEqual, assisted, 0);
    let no_direct_store = builder.ins().iconst(types::I8, 0);
    builder.ins().brif(
        assisted_ok,
        shape_check_block,
        &[
            ir::BlockArg::Value(assisted),
            ir::BlockArg::Value(no_direct_store),
        ],
        miss_block,
        &[],
    );

    builder.switch_to_block(shape_check_block);
    builder.seal_block(shape_check_block);
    let logical_addr = builder.block_params(shape_check_block)[0];
    let direct_store = builder.block_params(shape_check_block)[1];
    let addr = builder.ins().iadd(logical_addr, heap_delta);
    let obj_word = builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), addr, 8);
    let obj_shape = builder.ins().ushr_imm_u(obj_word, 32);
    let shape_match = builder
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, obj_shape, ic_shape);
    builder
        .ins()
        .brif(shape_match, hit_block, &[], miss_block, &[]);

    builder.switch_to_block(hit_block);
    builder.seal_block(hit_block);
    let value_shift = builder.ins().ishl_imm_u(ic_val_idx, 3);
    let value_offset = builder
        .ins()
        .iadd_imm_s(value_shift, i64::from(constants::HEAP_OBJECT_HEADER_SIZE));
    let logical_slot = builder.ins().iadd(logical_addr, value_offset);
    let value_addr = builder.ins().iadd(addr, value_offset);
    builder.ins().brif(
        barrier_disabled,
        legacy_store_block,
        &[],
        zgc_store_mode_block,
        &[],
    );

    builder.switch_to_block(zgc_store_mode_block);
    builder.seal_block(zgc_store_mode_block);
    builder.ins().brif(
        direct_store,
        zgc_direct_store_block,
        &[],
        barrier_store_block,
        &[],
    );

    builder.switch_to_block(legacy_store_block);
    builder.seal_block(legacy_store_block);
    builder
        .ins()
        .store(MemFlagsData::trusted(), stored, value_addr, 0);
    builder.ins().jump(store_done_block, &[]);

    builder.switch_to_block(zgc_direct_store_block);
    builder.seal_block(zgc_direct_store_block);
    builder
        .ins()
        .atomic_store(MemFlagsData::trusted(), stored, value_addr);
    increment_barrier_counter(
        builder,
        barrier_state,
        offset_of!(NativeBarrierState, store_fast_events),
    );
    builder.ins().jump(store_done_block, &[]);

    builder.switch_to_block(barrier_store_block);
    builder.seal_block(barrier_store_block);
    let call = builder.ins().call(
        barrier_thunks.store,
        &[ctx, handle_i32, logical_slot, stored],
    );
    let status = builder.inst_results(call)[0];
    let stored_ok = builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::Equal, status, 0);
    builder
        .ins()
        .brif(stored_ok, store_done_block, &[], miss_block, &[]);

    builder.switch_to_block(store_done_block);
    builder.seal_block(store_done_block);
    define_value(builder, variables, dest, stored)?;
    builder.ins().jump(merge_block, &[]);

    // miss：宿主完整 [[Set]] + IC 回填；`ic_ptr` 作为回填目标传入。
    builder.switch_to_block(miss_block);
    builder.seal_block(miss_block);
    let result = call_dispatcher(
        builder,
        root_frame,
        dispatcher,
        ctx,
        NativeRuntimeOp::SetPropIc.id(),
        &[obj, key_value, stored, ic_ptr],
        None,
    )?;
    define_value(builder, variables, dest, result)?;
    builder.ins().jump(merge_block, &[]);

    builder.switch_to_block(merge_block);
    builder.seal_block(merge_block);
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "与 lower_instruction 的既有参数集合保持一致，全部为 lowering 上下文"
)]
fn lower_optional_get_prop_ic(
    builder: &mut FunctionBuilder<'_>,
    variables: &HashMap<ValueId, Variable>,
    root_frame: &mut FrameLowering,
    dispatcher: ir::FuncRef,
    barrier_thunks: &BarrierThunks,
    ctx: ir::Value,
    dest: ValueId,
    object: ValueId,
    key: ValueId,
    slot: u32,
    roots: &[ValueId],
) -> Result<()> {
    let obj = use_value(builder, variables, object)?;

    // 第零级：null / undefined 检查。
    let is_null = builder
        .ins()
        .icmp_imm_s(ir::condcodes::IntCC::Equal, obj, value::encode_null());
    let is_undefined =
        builder
            .ins()
            .icmp_imm_s(ir::condcodes::IntCC::Equal, obj, value::encode_undefined());
    let is_nullish = builder.ins().bor(is_null, is_undefined);

    let nullish_block = builder.create_block();
    let ic_entry_block = builder.create_block();
    let merge_block = builder.create_block();

    builder
        .ins()
        .brif(is_nullish, nullish_block, &[], ic_entry_block, &[]);

    // nullish 分支：提前返回 undefined。
    builder.switch_to_block(nullish_block);
    builder.seal_block(nullish_block);
    let undefined = builder.ins().iconst(types::I64, value::encode_undefined());
    define_value(builder, variables, dest, undefined)?;
    builder.ins().jump(merge_block, &[]);

    // IC 分支入口：非 nullish 值走与 GetProp 相同的共享核心。
    builder.switch_to_block(ic_entry_block);
    builder.seal_block(ic_entry_block);
    lower_get_prop_ic_non_nullish(
        builder,
        variables,
        root_frame,
        dispatcher,
        barrier_thunks,
        ctx,
        dest,
        object,
        key,
        slot,
        roots,
        merge_block,
    )?;

    builder.switch_to_block(merge_block);
    builder.seal_block(merge_block);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn lower_value_operation(
    builder: &mut FunctionBuilder<'_>,
    variables: &HashMap<ValueId, Variable>,
    root_frame: &mut FrameLowering,
    dispatcher: ir::FuncRef,
    ctx: ir::Value,
    operation: NativeRuntimeOp,
    args: &[ValueId],
    destination: Option<ValueId>,
) -> Result<()> {
    let args = args
        .iter()
        .map(|value| use_value(builder, variables, *value))
        .collect::<Result<Vec<_>>>()?;
    let result = call_dispatcher(
        builder,
        root_frame,
        dispatcher,
        ctx,
        operation.id(),
        &args,
        None,
    )?;
    if let Some(destination) = destination {
        define_value(builder, variables, destination, result)?;
    }
    Ok(())
}

/// 每个函数按需 import 一次 typed math thunk，同函数内所有调用点复用同一 `FuncRef`。
fn import_math_thunk(
    builder: &mut FunctionBuilder<'_>,
    math_thunks: &HashMap<Builtin, DeclaredFunction>,
    imported: &mut HashMap<Builtin, ir::FuncRef>,
    builtin: Builtin,
) -> Result<ir::FuncRef> {
    if let Some(func_ref) = imported.get(&builtin).copied() {
        return Ok(func_ref);
    }
    let declaration = math_thunks
        .get(&builtin)
        .with_context(|| format!("math thunk {builtin:?} 未声明"))?;
    let func_ref = declaration.import(builder.func);
    imported.insert(builtin, func_ref);
    Ok(func_ref)
}

/// 计算反馈槽地址：`ctx.feedback_slots_base + slot × FEEDBACK_SLOT_SIZE`。
///
/// 反馈区由当前 base image 持有；特化 overlay 永不激活为 current image，其生成
/// 代码同样经由 vmctx 基址写 base image 的槽，编号在两种编译间保持一致。
fn emit_feedback_slot_ptr(
    builder: &mut FunctionBuilder<'_>,
    ctx: ir::Value,
    slot: u32,
) -> Result<ir::Value> {
    let pointer_type = builder.func.dfg.value_type(ctx);
    let base = builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        ctx,
        vmctx_offset(offset_of!(NativeVmContext, feedback_slots_base))?,
    );
    let offset = i64::from(slot)
        .checked_mul(i64::from(constants::FEEDBACK_SLOT_SIZE))
        .context("feedback slot offset exceeds i64")?;
    Ok(builder.ins().iadd_imm_s(base, offset))
}

fn call_dispatcher(
    builder: &mut FunctionBuilder<'_>,
    frame: &mut FrameLowering,
    dispatcher: ir::FuncRef,
    ctx: ir::Value,
    operation: u32,
    args: &[ir::Value],
    feedback_slot: Option<ir::Value>,
) -> Result<ir::Value> {
    let byte_len = args
        .len()
        .checked_mul(size_of::<i64>())
        .ok_or_else(|| anyhow!("host operation argument bytes overflow"))?;
    let byte_len = u32::try_from(byte_len).context("host operation argument area exceeds u32")?;
    let pointer_type = builder.func.dfg.value_type(ctx);
    let args_pointer = if args.is_empty() {
        builder.ins().iconst(pointer_type, 0)
    } else {
        let base = frame.reserve_arena(byte_len);
        for (index, value) in args.iter().enumerate() {
            let offset = index
                .checked_mul(size_of::<i64>())
                .and_then(|offset| i32::try_from(offset).ok())
                .context("host operation stack slot offset exceeds i32")?;
            builder
                .ins()
                .store(MemFlagsData::trusted(), *value, base, offset);
        }
        base
    };
    let operation = builder.ins().iconst(types::I32, i64::from(operation));
    let count = builder.ins().iconst(
        types::I32,
        i64::try_from(args.len()).context("host operation argument count exceeds i64")?,
    );
    let feedback_slot = match feedback_slot {
        Some(slot) => slot,
        None => builder.ins().iconst(pointer_type, 0),
    };
    let call = builder.ins().call(
        dispatcher,
        &[ctx, operation, args_pointer, count, feedback_slot],
    );
    builder
        .inst_results(call)
        .first()
        .copied()
        .context("host dispatcher returned no result")
}

#[allow(clippy::too_many_arguments)]
fn lower_terminator(
    builder: &mut FunctionBuilder<'_>,
    predecessor: BasicBlockId,
    terminator: &Terminator,
    constants: &[Constant],
    blocks: &HashMap<BasicBlockId, ir::Block>,
    variables: &HashMap<ValueId, Variable>,
    phi_edges: &HashMap<(BasicBlockId, BasicBlockId), Vec<(ValueId, ValueId)>>,
    dispatcher: ir::FuncRef,
    ctx: ir::Value,
    root_frame: &mut FrameLowering,
) -> Result<()> {
    match terminator {
        Terminator::Return { value } => {
            let result = match value {
                Some(value) => use_value(builder, variables, *value)?,
                None => builder.ins().iconst(types::I64, value::encode_undefined()),
            };
            root_frame.unlink(builder, ctx)?;
            builder.ins().return_(&[result]);
        }
        Terminator::Jump { target } => {
            if target.0 <= predecessor.0 {
                lower_cooperative_poll(builder, dispatcher, ctx, root_frame)?;
            }
            define_phi_edge(builder, variables, phi_edges, predecessor, *target)?;
            builder.ins().jump(blocks[target], &[]);
        }
        Terminator::Branch {
            condition,
            true_block,
            false_block,
        } => {
            if true_block.0 <= predecessor.0 || false_block.0 <= predecessor.0 {
                lower_cooperative_poll(builder, dispatcher, ctx, root_frame)?;
            }
            let condition = use_value(builder, variables, *condition)?;
            let condition = call_dispatcher(
                builder,
                root_frame,
                dispatcher,
                ctx,
                NativeRuntimeOp::IsTruthy.id(),
                &[condition],
                None,
            )?;
            let condition = builder.ins().icmp_imm_s(
                ir::condcodes::IntCC::NotEqual,
                condition,
                value::encode_bool(false),
            );
            define_phi_edge(builder, variables, phi_edges, predecessor, *true_block)?;
            define_phi_edge(builder, variables, phi_edges, predecessor, *false_block)?;
            builder
                .ins()
                .brif(condition, blocks[true_block], &[], blocks[false_block], &[]);
        }
        Terminator::Switch {
            value,
            cases,
            default_block,
            ..
        } => {
            if cases.iter().any(|case| case.target.0 <= predecessor.0)
                || default_block.0 <= predecessor.0
            {
                lower_cooperative_poll(builder, dispatcher, ctx, root_frame)?;
            }
            let value = use_value(builder, variables, *value)?;
            if cases.is_empty() {
                define_phi_edge(builder, variables, phi_edges, predecessor, *default_block)?;
                builder.ins().jump(blocks[default_block], &[]);
            } else {
                for (index, case) in cases.iter().enumerate() {
                    let constant = constants
                        .get(
                            usize::try_from(case.constant.0)
                                .context("switch constant index exceeds usize")?,
                        )
                        .with_context(|| {
                            format!("switch constant {} is missing", case.constant.0)
                        })?;
                    let immediate = switch_constant_immediate(constant)?;
                    let condition =
                        builder
                            .ins()
                            .icmp_imm_u(ir::condcodes::IntCC::Equal, value, immediate);
                    define_phi_edge(builder, variables, phi_edges, predecessor, case.target)?;
                    if index + 1 == cases.len() {
                        define_phi_edge(
                            builder,
                            variables,
                            phi_edges,
                            predecessor,
                            *default_block,
                        )?;
                        builder.ins().brif(
                            condition,
                            blocks[&case.target],
                            &[],
                            blocks[default_block],
                            &[],
                        );
                    } else {
                        let next_case = builder.create_block();
                        builder
                            .ins()
                            .brif(condition, blocks[&case.target], &[], next_case, &[]);
                        builder.switch_to_block(next_case);
                    }
                }
            }
        }
        Terminator::Throw { value } => {
            let value = use_value(builder, variables, *value)?;
            let exception = call_dispatcher(
                builder,
                root_frame,
                dispatcher,
                ctx,
                NativeRuntimeOp::CreateException.id(),
                &[value],
                None,
            )?;
            root_frame.unlink(builder, ctx)?;
            builder.ins().return_(&[exception]);
        }
        Terminator::Unreachable => {
            let result = builder.ins().iconst(types::I64, value::encode_undefined());
            root_frame.unlink(builder, ctx)?;
            builder.ins().return_(&[result]);
        }
    }
    Ok(())
}

pub(crate) fn emit_is_number(builder: &mut FunctionBuilder<'_>, input: ir::Value) -> ir::Value {
    let box_base = i64::from_ne_bytes(value::BOX_BASE.to_ne_bytes());
    let boxed_bits = builder.ins().band_imm_s(input, box_base);
    builder
        .ins()
        .icmp_imm_s(ir::condcodes::IntCC::NotEqual, boxed_bits, box_base)
}

fn emit_is_exception(builder: &mut FunctionBuilder<'_>, input: ir::Value) -> ir::Value {
    let box_base = i64::from_ne_bytes(value::BOX_BASE.to_ne_bytes());
    let boxed_bits = builder.ins().band_imm_s(input, box_base);
    let boxed = builder
        .ins()
        .icmp_imm_s(ir::condcodes::IntCC::Equal, boxed_bits, box_base);
    let tag = builder.ins().ushr_imm_u(input, 32);
    let tag = builder.ins().band_imm_u(
        tag,
        i64::try_from(value::TAG_MASK).expect("tag mask fits i64"),
    );
    let exception_tag = builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        tag,
        i64::try_from(value::TAG_EXCEPTION).expect("exception tag fits i64"),
    );
    builder.ins().band(boxed, exception_tag)
}

fn return_if_exception(
    builder: &mut FunctionBuilder<'_>,
    result: ir::Value,
    root_frame: &mut FrameLowering,
    ctx: ir::Value,
) -> Result<()> {
    let is_exception = emit_is_exception(builder, result);
    let exception_block = builder.create_block();
    let continue_block = builder.create_block();
    builder
        .ins()
        .brif(is_exception, exception_block, &[], continue_block, &[]);
    builder.switch_to_block(exception_block);
    root_frame.unlink(builder, ctx)?;
    builder.ins().return_(&[result]);
    builder.switch_to_block(continue_block);
    Ok(())
}

fn lower_cooperative_poll(
    builder: &mut FunctionBuilder<'_>,
    dispatcher: ir::FuncRef,
    ctx: ir::Value,
    root_frame: &mut FrameLowering,
) -> Result<()> {
    let budget_addr = builder.ins().iadd_imm_s(
        ctx,
        i64::from(vmctx_offset(offset_of!(
            NativeVmContext,
            stack_budget_bytes
        ))?),
    );
    let budget = builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), budget_addr, 0);
    let step_i64 = i64::try_from(COOPERATIVE_POLL_STEP_BYTES).expect("poll step fits i64");
    // 预算已 ≤ 步长（含耗尽的 0）→ 慢路径：进 dispatcher 轮询并重置预算。
    let exhausted = builder.ins().icmp_imm_s(
        ir::condcodes::IntCC::UnsignedLessThanOrEqual,
        budget,
        step_i64,
    );
    let slow_block = builder.create_block();
    let fast_block = builder.create_block();
    builder
        .ins()
        .brif(exhausted, slow_block, &[], fast_block, &[]);

    // 快路径：预算充足，扣减步长后继续回边，不调用 dispatcher。
    builder.switch_to_block(fast_block);
    builder.seal_block(fast_block);
    let step = builder.ins().iconst(types::I64, step_i64);
    let remaining = builder.ins().isub(budget, step);
    builder
        .ins()
        .store(MemFlagsData::trusted(), remaining, budget_addr, 0);
    let continue_block = builder.create_block();
    builder.ins().jump(continue_block, &[]);

    // 慢路径：预算耗尽，进 dispatcher 轮询（inspector / GC / 外部事件 / 期限）；
    // 宿主在 CooperativePoll 处理中把预算重置回初始值。
    builder.switch_to_block(slow_block);
    builder.seal_block(slow_block);
    let result = call_dispatcher(
        builder,
        root_frame,
        dispatcher,
        ctx,
        NativeRuntimeOp::CooperativePoll.id(),
        &[],
        None,
    )?;
    return_if_exception(builder, result, root_frame, ctx)?;
    builder.ins().jump(continue_block, &[]);

    builder.switch_to_block(continue_block);
    builder.seal_block(continue_block);
    Ok(())
}

fn switch_constant_immediate(constant: &Constant) -> Result<i64> {
    match constant {
        Constant::Number(number) => Ok(value::encode_f64(*number)),
        Constant::Bool(boolean) => Ok(value::encode_bool(*boolean)),
        Constant::Null => Ok(value::encode_null()),
        Constant::Undefined => Ok(value::encode_undefined()),
        Constant::FunctionRef(_) => bail!("function references are not valid switch keys"),
        Constant::NativeCallableEval => Ok(value::encode_native_callable_idx(0)),
        Constant::ModuleId(module) => Ok(value::encode_f64(f64::from(module.0))),
        Constant::String(_) | Constant::BigInt(_) | Constant::RegExp { .. } => {
            bail!("materialized constants are not valid switch keys")
        }
    }
}

fn define_phi_edge(
    builder: &mut FunctionBuilder<'_>,
    variables: &HashMap<ValueId, Variable>,
    phi_edges: &HashMap<(BasicBlockId, BasicBlockId), Vec<(ValueId, ValueId)>>,
    predecessor: BasicBlockId,
    target: BasicBlockId,
) -> Result<()> {
    if let Some(assignments) = phi_edges.get(&(predecessor, target)) {
        let values: Vec<_> = assignments
            .iter()
            .map(|(_, source)| use_value(builder, variables, *source))
            .collect::<Result<_>>()?;
        for ((dest, _), value) in assignments.iter().zip(values) {
            define_value(builder, variables, *dest, value)?;
        }
    }
    Ok(())
}

fn collect_phi_edges(
    function: &wjsm_ir::Function,
) -> HashMap<(BasicBlockId, BasicBlockId), Vec<(ValueId, ValueId)>> {
    let mut edges = HashMap::new();
    for block in function.blocks() {
        for instruction in block.instructions() {
            if let Instruction::Phi { dest, sources } = instruction {
                for source in sources {
                    edges
                        .entry((source.predecessor, block.id()))
                        .or_insert_with(Vec::new)
                        .push((*dest, source.value));
                }
            }
        }
    }
    edges
}

fn collect_value_ids(function: &wjsm_ir::Function) -> HashSet<ValueId> {
    let mut ids = HashSet::new();
    for block in function.blocks() {
        for instruction in block.instructions() {
            collect_instruction_values(instruction, &mut ids);
        }
        match block.terminator() {
            Terminator::Return { value: Some(value) } | Terminator::Throw { value } => {
                ids.insert(*value);
            }
            Terminator::Branch { condition, .. }
            | Terminator::Switch {
                value: condition, ..
            } => {
                ids.insert(*condition);
            }
            Terminator::Return { value: None }
            | Terminator::Jump { .. }
            | Terminator::Unreachable => {}
        }
    }
    ids
}

fn collect_instruction_values(instruction: &Instruction, ids: &mut HashSet<ValueId>) {
    let mut instruction = instruction.clone();
    instruction.remap_values(&mut |value| {
        ids.insert(value);
        value
    });
}

fn frame_local_variables(names: &BTreeSet<&str>) -> HashMap<String, Variable> {
    names
        .iter()
        .map(|name| ((*name).to_owned(), Variable::from_u32(0)))
        .collect()
}

fn initialize_frame_locals(
    builder: &mut FunctionBuilder<'_>,
    locals: &mut HashMap<String, Variable>,
) {
    for variable in locals.values_mut() {
        *variable = builder.declare_var(types::I64);
        let undefined = builder.ins().iconst(types::I64, value::encode_undefined());
        builder.def_var(*variable, undefined);
    }
}

fn boxed_local_order(names: &BTreeSet<&str>) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

fn frame_local_indices(order: &[String]) -> HashMap<String, usize> {
    order
        .iter()
        .enumerate()
        .map(|(index, name)| (name.clone(), index))
        .collect()
}

fn pin_initialized_frame_locals(
    root_frame: &mut FrameLowering,
    builder: &mut FunctionBuilder<'_>,
    locals: &HashMap<String, Variable>,
    order: &[String],
) -> Result<()> {
    let values: Vec<ir::Value> = order
        .iter()
        .map(|name| {
            let variable = locals
                .get(name)
                .copied()
                .with_context(|| format!("frame-local variable {name} is missing"))?;
            Ok(builder.use_var(variable))
        })
        .collect::<Result<_>>()?;
    root_frame.pin_frame_locals(builder, &values)
}

pub(crate) fn boxed_frame_local_names<'a>(
    function: &'a wjsm_ir::Function,
    frame_locals: &BTreeSet<&'a str>,
    inferred_f64: &HashMap<FunctionId, HashSet<ValueId>>,
    index: usize,
) -> BTreeSet<&'a str> {
    let function_id = FunctionId(u32::try_from(index).expect("function index fits u32"));
    let Some(f64_values) = inferred_f64.get(&function_id) else {
        return frame_locals.clone();
    };
    let mut f64_locals = BTreeSet::new();
    let mut mixed_locals = BTreeSet::new();
    for block in function.blocks() {
        for instruction in block.instructions() {
            if let Instruction::StoreVar { name, value } = instruction
                && frame_locals.contains(name.as_str())
            {
                if f64_values.contains(value) {
                    if !mixed_locals.contains(name.as_str()) {
                        f64_locals.insert(name.as_str());
                    }
                } else {
                    f64_locals.remove(name.as_str());
                    mixed_locals.insert(name.as_str());
                }
            }
        }
    }
    frame_locals
        .iter()
        .copied()
        .filter(|name| !f64_locals.contains(name))
        .collect()
}

fn box_f64_result(builder: &mut FunctionBuilder<'_>, result: ir::Value) -> ir::Value {
    let is_nan = builder
        .ins()
        .fcmp(ir::condcodes::FloatCC::Unordered, result, result);
    let bits = builder
        .ins()
        .bitcast(types::I64, ir::MemFlagsData::new(), result);
    let canonical_nan = builder
        .ins()
        .iconst(types::I64, value::encode_f64(f64::NAN));
    builder.ins().select(is_nan, canonical_nan, bits)
}

fn use_value(
    builder: &mut FunctionBuilder<'_>,
    variables: &HashMap<ValueId, Variable>,
    value: ValueId,
) -> Result<ir::Value> {
    let variable = variables
        .get(&value)
        .copied()
        .with_context(|| format!("value {} has no native variable", value.0))?;
    Ok(builder.use_var(variable))
}

fn define_value(
    builder: &mut FunctionBuilder<'_>,
    variables: &HashMap<ValueId, Variable>,
    value: ValueId,
    native_value: ir::Value,
) -> Result<()> {
    let variable = variables
        .get(&value)
        .copied()
        .with_context(|| format!("value {} has no native variable", value.0))?;
    builder.def_var(variable, native_value);
    Ok(())
}

fn binary_tag(op: BinaryOp) -> u16 {
    match op {
        BinaryOp::Add => 0,
        BinaryOp::Sub => 1,
        BinaryOp::Mul => 2,
        BinaryOp::Div => 3,
        BinaryOp::Mod => 4,
        BinaryOp::Exp => 5,
        BinaryOp::BitAnd => 6,
        BinaryOp::BitOr => 7,
        BinaryOp::BitXor => 8,
        BinaryOp::Shl => 9,
        BinaryOp::Shr => 10,
        BinaryOp::UShr => 11,
    }
}

fn unary_tag(op: UnaryOp) -> u16 {
    match op {
        UnaryOp::Not => 0,
        UnaryOp::Neg => 1,
        UnaryOp::Pos => 2,
        UnaryOp::BitNot => 3,
        UnaryOp::Void => 4,
        UnaryOp::IsNullish => 5,
        UnaryOp::Delete => 6,
    }
}

fn compare_tag(op: CompareOp) -> u16 {
    match op {
        CompareOp::StrictEq => 0,
        CompareOp::StrictNotEq => 1,
    }
}

pub(crate) fn libcall_name(libcall: ir::LibCall) -> String {
    use ir::LibCall;
    match libcall {
        LibCall::Memcpy => "wjsm_native_memory_copy".into(),
        LibCall::Memset => "wjsm_native_memory_fill".into(),
        LibCall::Memmove => "wjsm_native_memory_move".into(),
        LibCall::Memcmp => "wjsm_native_memory_compare".into(),
        forbidden => format!("__wjsm_forbidden_libcall_{forbidden:?}"),
    }
}

/// 把 object 侧 endianness 转成 gimli 的 writer endian；本后端只支持小端目标。
pub(crate) fn gimli_endian(triple: &target_lexicon::Triple) -> gimli::RunTimeEndian {
    match triple.endianness().unwrap() {
        target_lexicon::Endianness::Little => gimli::RunTimeEndian::Little,
        target_lexicon::Endianness::Big => gimli::RunTimeEndian::Big,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wjsm_ir::{BasicBlock, Constant, Function};

    #[test]
    fn infer_f64_values_requires_exact_math_arity() {
        let mut program = Program::new();
        let number = program.add_constant(Constant::Number(0.5));
        let mut function = Function::new("main", BasicBlockId(0));
        let mut block = BasicBlock::new(BasicBlockId(0));
        block.push_instruction(Instruction::Const {
            dest: ValueId(0),
            constant: number,
        });
        block.push_instruction(Instruction::Const {
            dest: ValueId(1),
            constant: number,
        });
        block.push_instruction(Instruction::CallBuiltin {
            dest: Some(ValueId(2)),
            builtin: Builtin::MathSin,
            args: vec![ValueId(0)],
        });
        block.push_instruction(Instruction::CallBuiltin {
            dest: Some(ValueId(3)),
            builtin: Builtin::MathSin,
            args: vec![],
        });
        block.push_instruction(Instruction::CallBuiltin {
            dest: Some(ValueId(4)),
            builtin: Builtin::MathSin,
            args: vec![ValueId(0), ValueId(1)],
        });
        block.push_instruction(Instruction::CallBuiltin {
            dest: Some(ValueId(5)),
            builtin: Builtin::MathPow,
            args: vec![ValueId(0), ValueId(1)],
        });
        block.push_instruction(Instruction::CallBuiltin {
            dest: Some(ValueId(6)),
            builtin: Builtin::MathPow,
            args: vec![ValueId(0)],
        });
        block.push_instruction(Instruction::CallBuiltin {
            dest: Some(ValueId(7)),
            builtin: Builtin::MathPow,
            args: vec![ValueId(0), ValueId(100)],
        });
        block.set_terminator(Terminator::Return { value: None });
        function.push_block(block);
        program.push_function(function);

        let inferred = infer_f64_values(&program);
        let f64_values = &inferred[&FunctionId(0)];
        assert!(f64_values.contains(&ValueId(0)));
        assert!(f64_values.contains(&ValueId(1)));
        assert!(f64_values.contains(&ValueId(2)));
        assert!(f64_values.contains(&ValueId(5)));
        assert!(!f64_values.contains(&ValueId(3)));
        assert!(!f64_values.contains(&ValueId(4)));
        assert!(!f64_values.contains(&ValueId(6)));
        assert!(!f64_values.contains(&ValueId(7)));
    }

    #[test]
    fn infer_f64_values_propagates_through_direct_function_calls() {
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
        caller_block.push_instruction(Instruction::Call {
            dest: Some(ValueId(2)),
            callee: ValueId(1),
            this_val: ValueId(0),
            args: vec![ValueId(0)],
        });
        caller_block.set_terminator(Terminator::Return {
            value: Some(ValueId(2)),
        });
        caller.push_block(caller_block);
        program.push_function(caller);

        let mut callee = Function::new("add1", BasicBlockId(0));
        callee.set_params(vec!["$env".into(), "$this".into(), "x".into()]);
        let mut callee_block = BasicBlock::new(BasicBlockId(0));
        callee_block.push_instruction(Instruction::LoadVar {
            dest: ValueId(0),
            name: "x".into(),
        });
        callee_block.push_instruction(Instruction::Const {
            dest: ValueId(1),
            constant: number,
        });
        callee_block.push_instruction(Instruction::Binary {
            dest: ValueId(2),
            op: BinaryOp::Add,
            lhs: ValueId(0),
            rhs: ValueId(1),
        });
        callee_block.set_terminator(Terminator::Return {
            value: Some(ValueId(2)),
        });
        callee.push_block(callee_block);
        program.push_function(callee);

        let inferred = infer_f64_values(&program);
        assert!(inferred[&FunctionId(0)].contains(&ValueId(2)));
        assert!(inferred[&FunctionId(1)].contains(&ValueId(0)));
        assert!(inferred[&FunctionId(1)].contains(&ValueId(2)));
    }

    #[test]
    fn infer_f64_values_rejects_escaped_function_references() {
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
        caller_block.push_instruction(Instruction::StoreVar {
            name: "escaped".into(),
            value: ValueId(1),
        });
        caller_block.push_instruction(Instruction::Call {
            dest: Some(ValueId(2)),
            callee: ValueId(1),
            this_val: ValueId(0),
            args: vec![ValueId(0)],
        });
        caller_block.set_terminator(Terminator::Return { value: None });
        caller.push_block(caller_block);
        program.push_function(caller);

        let mut callee = Function::new("add1", BasicBlockId(0));
        callee.set_params(vec!["$env".into(), "$this".into(), "x".into()]);
        let mut callee_block = BasicBlock::new(BasicBlockId(0));
        callee_block.push_instruction(Instruction::LoadVar {
            dest: ValueId(0),
            name: "x".into(),
        });
        callee_block.push_instruction(Instruction::Const {
            dest: ValueId(1),
            constant: number,
        });
        callee_block.push_instruction(Instruction::Binary {
            dest: ValueId(2),
            op: BinaryOp::Add,
            lhs: ValueId(0),
            rhs: ValueId(1),
        });
        callee_block.set_terminator(Terminator::Return {
            value: Some(ValueId(2)),
        });
        callee.push_block(callee_block);
        program.push_function(callee);

        let inferred = infer_f64_values(&program);
        assert!(!inferred[&FunctionId(0)].contains(&ValueId(2)));
        assert!(!inferred[&FunctionId(1)].contains(&ValueId(0)));
        assert!(!inferred[&FunctionId(1)].contains(&ValueId(2)));
    }
}
