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
use crate::fast_call::{
    compile_slow_trampoline, fast_entry_signature, fast_js_arity, is_fast_call_eligible,
    js_param_count,
};
use crate::root_plan::RootPlan;
use crate::safepoint_free::infer_safepoint_free_functions;
use crate::template_meta::{
    TemplateOriginMap, TrioField, build_template_origin_maps, plan_ic_slots,
    template_property_index_for_key, trio_field_for_access,
};
use crate::unwind::{UnwindPolicy, UnwindRecord, validate_unwind_info, write_object_unwind};
use crate::value_repr::{
    ValueRepr, box_f64_result, define_value_as, define_value_boxed, define_value_f64, unbox_f64,
    use_value_as, use_value_boxed, use_value_f64,
};
use crate::{NativeCompileError, NativeObject};

const HOST_OPERATION_SYMBOL: NativeHostSymbol = NativeHostSymbol::HostOperationDispatcher;
const STRING_ADD_SYMBOL: NativeHostSymbol = NativeHostSymbol::StringAdd;
const STRING_BUILDER_FINISH_SYMBOL: NativeHostSymbol = NativeHostSymbol::StringBuilderFinish;
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
    /// 块内各 root 槽当前持有的 ValueId；跨块必须清空（前驱可能发布了不同内容）。
    /// 暂存但尚未落地的 root 集合。发布推迟到下一个可 GC 调用点，
    /// 非安全点之间的 root frame 内容对 GC 不可见，无需维护。
    staged_roots: Vec<ValueId>,
    staged_dirty: bool,
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
            staged_roots: Vec::new(),
            staged_dirty: false,
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

    /// 暂存本指令的 root 集合，不产出任何指令。
    ///
    /// GC 只在安全点读取 root frame，两次安全点之间它的内容不可观察；因此发布
    /// 推迟到真正可能收集的调用点，非安全点指令不再各自重写 bitmap 与槽位。
    fn stage(&mut self, roots: &[ValueId]) {
        self.staged_roots.clear();
        self.staged_roots.extend_from_slice(roots);
        self.staged_dirty = true;
    }

    /// 在可 GC / 可重入调用之前把暂存的 root 集合真正写入 root frame。
    fn flush(&mut self, builder: &mut FunctionBuilder<'_>, variables: &ValueRepr) -> Result<()> {
        if !self.staged_dirty {
            return Ok(());
        }
        let roots = std::mem::take(&mut self.staged_roots);
        let result = self.publish(builder, variables, &roots, &[]);
        self.staged_roots = roots;
        result
    }

    /// 无条件写入全部活 root 与 bitmap。
    ///
    /// 不做「槽内已是同一值就跳过」的块内 memo：发布点已下沉到 IC miss 等子块，
    /// 这类块并不支配后续发布点，跨块复用记忆会漏写槽位并让 GC 扫到陈旧句柄。
    fn publish(
        &mut self,
        builder: &mut FunctionBuilder<'_>,
        variables: &ValueRepr,
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
        let local_base = self.pinned_local_count;
        for (index, root) in roots.iter().enumerate() {
            let value = use_value_boxed(builder, variables, *root)?;
            builder.ins().store(
                MemFlagsData::trusted(),
                value,
                self.roots_base,
                slot_offset(local_base + index, "native root spill")?,
            );
        }
        for (index, temporary) in temporaries.iter().enumerate() {
            let slot = roots.len() + index;
            builder.ins().store(
                MemFlagsData::trusted(),
                *temporary,
                self.roots_base,
                slot_offset(local_base + slot, "native temporary root spill")?,
            );
        }
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
        self.staged_dirty = false;
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

    /// 进入新块：丢弃上一块留下的暂存集合，本块的每条指令会重新暂存。
    fn enter_block(&mut self) {
        self.staged_roots.clear();
        self.staged_dirty = false;
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
pub(crate) fn vmctx_offset(offset: usize) -> Result<i32> {
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
            // 预留一个临时槽（多预留不影响正确性）。GetPropGuarded 的慢路径
            // 复用同一 IC 核心，同样预留。
            Instruction::GetProp { .. }
            | Instruction::OptionalGetProp { .. }
            | Instruction::GetPropGuarded { .. } => 1,
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

/// 为「常量字符串键的 GetProp / OptionalGetProp / SetProp」分配全局 IC 槽。
pub(crate) fn allocate_ic_slots(program: &Program) -> (Vec<HashMap<ValueId, u32>>, u32) {
    let plan = plan_ic_slots(program);
    (plan.per_function, plan.total)
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
    pub(crate) fn signature(&self) -> &Signature {
        &self.signature
    }

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

/// 单函数 codegen 的完整输入。与 `ObjectModule` 解耦，可供并行 compile 与特化 overlay 共用。
///
/// `'s` 是栈帧局部名的字符串生命周期，必须与 `'a` 分开：`BTreeSet` 对元素类型不变。
pub(crate) struct FunctionCompileInput<'a, 's> {
    pub isa: &'a cranelift_codegen::isa::OwnedTargetIsa,
    pub target_config: cranelift_codegen::isa::TargetFrontendConfig,
    pub program: &'a Program,
    pub ir_function: &'a wjsm_ir::Function,
    pub index: usize,
    pub signature: &'a Signature,
    pub function_id: FuncId,
    pub dispatcher: &'a DeclaredFunction,
    pub barrier_thunks: &'a DeclaredBarrierThunks,
    pub string_add: &'a DeclaredFunction,
    pub string_builder_finish: &'a DeclaredFunction,
    pub math_thunks: &'a HashMap<Builtin, DeclaredFunction>,
    pub root_bitmaps: &'a [DeclaredData],
    pub f64_values: &'a HashSet<ValueId>,
    /// `f64_values` 中**可靠**证明的子集：只有它才允许提升成 F64 机器变量。
    /// 运行时反馈推测出来的 number 由守卫兜底，位模式没有静态保证。
    pub typed_f64_values: &'a HashSet<ValueId>,
    pub int32_values: &'a HashSet<ValueId>,
    pub variable_slots: &'a HashMap<String, u32>,
    pub root_plan: &'a RootPlan,
    pub root_capacity: usize,
    pub frame_local_names: &'a BTreeSet<&'s str>,
    pub boxed_local_names: &'a BTreeSet<&'s str>,
    pub ic_slots: &'a HashMap<ValueId, u32>,
    pub template_origins: &'a TemplateOriginMap,
    pub feedback_slots: &'a HashMap<(BasicBlockId, usize), u32>,
    pub specialized_tags: Option<&'a [wjsm_native_abi::NativeFeedbackTag]>,
    pub function_decls: &'a [DeclaredFunction],
    pub direct_callable_functions: &'a HashSet<FunctionId>,
    pub safepoint_free: bool,
    pub collect_diagnostics: bool,
}

/// 指令级 lowering 的共享可变上下文。
struct LoweringCx<'a, 'f> {
    builder: &'a mut FunctionBuilder<'f>,
    variables: &'a ValueRepr,
    root_frame: Option<&'a mut FrameLowering>,
    dispatcher: ir::FuncRef,
    string_add: ir::FuncRef,
    string_builder_finish: ir::FuncRef,
    ctx: ir::Value,
    env: ir::Value,
    this_value: ir::Value,
    function_index: u32,
    current_block: BasicBlockId,
    target_config: cranelift_codegen::isa::TargetFrontendConfig,
    /// 入口块缓存的 handle / IC / barrier 基址，函数内 IC 命中路径复用。
    ht_base: ir::Value,
    ic_base: ir::Value,
    barrier_state: ir::Value,
}

impl LoweringCx<'_, '_> {
    fn stage_roots(&mut self, roots: &[ValueId]) {
        if let Some(root_frame) = self.root_frame.as_mut() {
            root_frame.stage(roots);
        }
    }

    fn flush_roots(&mut self) -> Result<()> {
        if let Some(root_frame) = self.root_frame.as_mut() {
            root_frame.flush(self.builder, self.variables)?;
        }
        Ok(())
    }

    fn unlink_roots(&mut self) -> Result<()> {
        if let Some(root_frame) = self.root_frame.as_mut() {
            root_frame.unlink(self.builder, self.ctx)?;
        }
        Ok(())
    }

    fn enter_root_block(&mut self) {
        if let Some(root_frame) = self.root_frame.as_mut() {
            root_frame.enter_block();
        }
    }

    fn finish_roots(&mut self) {
        if let Some(root_frame) = self.root_frame.as_mut() {
            root_frame.finish(self.builder);
        }
    }

    fn publish_roots(&mut self, roots: &[ValueId], temporaries: &[ir::Value]) -> Result<()> {
        if let Some(root_frame) = self.root_frame.as_mut() {
            root_frame.publish(self.builder, self.variables, roots, temporaries)?;
        }
        Ok(())
    }

    fn update_pinned_local(&mut self, index: usize, value: ir::Value) -> Result<()> {
        if let Some(root_frame) = self.root_frame.as_mut() {
            root_frame.update_pinned_local(self.builder, index, value)?;
        }
        Ok(())
    }

    /// 统一的宿主分派入口：dispatcher 可能触发 GC 与重入，调用前必须落地 root。
    fn call(
        &mut self,
        operation: u32,
        args: &[ir::Value],
        feedback: Option<ir::Value>,
    ) -> Result<ir::Value> {
        self.flush_roots()?;
        call_dispatcher(
            self.builder,
            self.root_frame.as_deref_mut(),
            self.dispatcher,
            self.ctx,
            operation,
            args,
            feedback,
        )
    }

    fn stage(&mut self, roots: &[ValueId]) {
        self.stage_roots(roots);
    }

    /// 在可 GC / 可重入调用之前落地暂存的 root 集合。
    fn flush(&mut self) -> Result<()> {
        self.flush_roots()
    }
}

/// 指令 lowering 所需的只读/半可变表。
struct InstructionTables<'a> {
    constants: &'a [Constant],
    function_index: u32,
    barrier_thunks: &'a BarrierThunks,
    f64_values: &'a HashSet<ValueId>,
    int32_values: &'a HashSet<ValueId>,
    speculative: bool,
    constant_defs: &'a HashMap<ValueId, ConstantId>,
    math_thunks: &'a HashMap<Builtin, DeclaredFunction>,
    hoisted_constants: &'a HashMap<ConstantId, ir::Value>,
    imported_math_thunks: &'a mut HashMap<Builtin, ir::FuncRef>,
    slow_call_signature: ir::SigRef,
    variable_slots: &'a HashMap<String, u32>,
    frame_locals: &'a HashMap<String, Variable>,
    frame_local_indices: &'a HashMap<String, usize>,
    ic_slots: &'a HashMap<ValueId, u32>,
    template_origins: &'a TemplateOriginMap,
    function_decls: &'a [DeclaredFunction],
    imported_function_decls: &'a mut HashMap<FunctionId, ir::FuncRef>,
    direct_callable_functions: &'a HashSet<FunctionId>,
}

/// 调用类指令的操作数。
struct CallLowering<'a> {
    destination: Option<ValueId>,
    callee: ValueId,
    this_value: ValueId,
    args: &'a [ValueId],
    operation: NativeRuntimeOp,
    forward_args: bool,
}

/// 属性访问 IC 的操作数。
#[derive(Clone, Copy)]
struct PropAccess {
    dest: ValueId,
    object: ValueId,
    key: ValueId,
    slot: u32,
    trio_field: Option<TrioField>,
}

fn prop_access(
    tables: &InstructionTables<'_>,
    dest: ValueId,
    object: ValueId,
    key: ValueId,
    slot: u32,
) -> PropAccess {
    PropAccess {
        dest,
        object,
        key,
        slot,
        trio_field: trio_field_for_access(
            tables.constants,
            tables.constant_defs,
            tables.template_origins,
            object,
            key,
        ),
    }
}

fn load_ic_value_index(
    builder: &mut FunctionBuilder<'_>,
    ic_ptr: ir::Value,
    ic_word0: ir::Value,
    trio_field: Option<TrioField>,
) -> ir::Value {
    match trio_field {
        None | Some(TrioField::Name) => builder.ins().ushr_imm_u(ic_word0, 32),
        Some(TrioField::Value) => {
            let index = builder.ins().load(
                types::I32,
                MemFlagsData::trusted(),
                ic_ptr,
                i32::try_from(constants::IC_SLOT_TRIO_VALUE_INDEX_OFFSET)
                    .expect("trio value index offset fits i32"),
            );
            builder.ins().uextend(types::I64, index)
        }
        Some(TrioField::Length) => {
            let index = builder.ins().load(
                types::I32,
                MemFlagsData::trusted(),
                ic_ptr,
                i32::try_from(constants::IC_SLOT_TRIO_LENGTH_INDEX_OFFSET)
                    .expect("trio length index offset fits i32"),
            );
            builder.ins().uextend(types::I64, index)
        }
    }
}

fn ic_kind_is_own_hit(
    builder: &mut FunctionBuilder<'_>,
    ic_kind: ir::Value,
    trio_field: Option<TrioField>,
) -> ir::Value {
    let expected = if trio_field.is_some() {
        constants::IC_KIND_OWN_DATA_TRIO
    } else {
        constants::IC_KIND_OWN_DATA
    };
    builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::Equal, ic_kind, i64::from(expected))
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
    let call_conv = module.isa().default_call_conv();
    let signature = slow_entry_signature(call_conv);
    let function_ids = declare_functions(&mut module, program, &signature)?;
    let mut fast_ids: Vec<Option<FuncId>> = Vec::with_capacity(program.functions().len());
    let mut fast_signatures: Vec<Option<Signature>> = Vec::with_capacity(program.functions().len());
    for (index, function) in program.functions().iter().enumerate() {
        if is_fast_call_eligible(function) {
            let fast_signature = fast_entry_signature(call_conv, js_param_count(function));
            let fast_id = module
                .declare_function(
                    &format!("wjsm_function_{index}_fast"),
                    Linkage::Local,
                    &fast_signature,
                )
                .map_err(|error| NativeCompileError::Cranelift(error.to_string()))?;
            fast_ids.push(Some(fast_id));
            fast_signatures.push(Some(fast_signature));
        } else {
            fast_ids.push(None);
            fast_signatures.push(None);
        }
    }
    let host_dispatcher = declare_host_dispatcher(&mut module)?;
    let string_add = declare_string_add_thunk(&mut module)?;
    let string_builder_finish = declare_string_builder_finish_thunk(&mut module)?;
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
    let string_add_decl = DeclaredFunction::snapshot(module.declarations(), string_add);
    let string_builder_finish_decl =
        DeclaredFunction::snapshot(module.declarations(), string_builder_finish);
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
    let template_origins = build_template_origin_maps(program);
    // 反馈槽预计算：只按指令形态编号，保证与运行时特化 overlay 的编号一致。
    let feedback_plan = allocate_feedback_slots(program);

    // 每个函数的 lower + Cranelift compile 相互独立，只读上面的声明快照；
    // 合并进 object 的写入阶段仍然串行，保证 relocation / 符号表顺序确定。
    let function_decls: Vec<DeclaredFunction> = function_ids
        .iter()
        .enumerate()
        .map(|(index, slow_id)| {
            let id = fast_ids[index].unwrap_or(*slow_id);
            DeclaredFunction::snapshot(module.declarations(), id)
        })
        .collect();
    let fast_decls: Vec<Option<DeclaredFunction>> = fast_ids
        .iter()
        .copied()
        .map(|id| id.map(|id| DeclaredFunction::snapshot(module.declarations(), id)))
        .collect();
    let direct_callable_functions: HashSet<FunctionId> = program
        .functions()
        .iter()
        .enumerate()
        .filter(|(_, f)| f.direct_callable())
        .map(|(i, _)| FunctionId(u32::try_from(i).expect("function index fits u32")))
        .collect();
    let safepoint_free_functions = infer_safepoint_free_functions(program, variable_slots);
    let compiled: Vec<CompiledFunction> = program
        .functions()
        .par_iter()
        .enumerate()
        .map(|(index, function)| {
            let function_id = FunctionId(u32::try_from(index).expect("function index fits u32"));
            let safepoint_free = safepoint_free_functions.contains(&function_id);
            let body_signature = fast_signatures[index].as_ref().unwrap_or(&signature);
            let body_id = fast_ids[index].unwrap_or(function_ids[index]);
            compile_one_function(&FunctionCompileInput {
                isa: &module_isa,
                target_config,
                program,
                ir_function: function,
                index,
                signature: body_signature,
                function_id: body_id,
                dispatcher: &dispatcher_decl,
                string_add: &string_add_decl,
                string_builder_finish: &string_builder_finish_decl,
                barrier_thunks: &barrier_thunks,
                math_thunks: &math_thunk_decls,
                root_bitmaps: &bitmap_decls,
                f64_values: inferred_f64
                    .get(&FunctionId(
                        u32::try_from(index).expect("function index fits u32"),
                    ))
                    .expect("analysis covers every function"),
                // base 编译没有反馈推测，静态分析结果本身就是可靠集合。
                typed_f64_values: inferred_f64
                    .get(&FunctionId(
                        u32::try_from(index).expect("function index fits u32"),
                    ))
                    .expect("analysis covers every function"),
                variable_slots,
                root_plan: &root_plans[index],
                root_capacity: if safepoint_free {
                    0
                } else {
                    root_capacities[index]
                },
                frame_local_names: &frame_locals[index],
                boxed_local_names: &boxed_frame_locals[index],
                ic_slots: &ic_slots[index],
                template_origins: &template_origins[index],
                feedback_slots: feedback_plan.function_slots(index),
                specialized_tags: None,
                int32_values: &HashSet::new(),
                function_decls: &function_decls,
                direct_callable_functions: &direct_callable_functions,
                safepoint_free,
                collect_diagnostics,
            })
        })
        .collect::<Result<Vec<_>, NativeCompileError>>()?;

    let trampolines: Vec<Option<CompiledFunction>> = program
        .functions()
        .par_iter()
        .enumerate()
        .map(|(index, function)| {
            let Some(body) = fast_decls[index].as_ref() else {
                return Ok(None);
            };
            let function_index = u32::try_from(index).expect("function index fits u32");
            compile_slow_trampoline(
                &module_isa,
                target_config,
                &signature,
                function_ids[index],
                body,
                js_param_count(function),
                function_index,
                function.name(),
                collect_diagnostics,
            )
            .map(Some)
        })
        .collect::<Result<Vec<_>, NativeCompileError>>()?;

    let mut frame_bytes = Vec::with_capacity(program.functions().len());
    let mut unwind_records: Vec<UnwindRecord> = Vec::with_capacity(
        program.functions().len() + fast_ids.iter().filter(|id| id.is_some()).count(),
    );
    let mut clif = String::new();
    let mut disassembly = String::new();

    for (index, (output, trampoline)) in compiled.into_iter().zip(trampolines).enumerate() {
        let body_id = fast_ids[index].unwrap_or(function_ids[index]);
        module
            .define_function_bytes(body_id, output.alignment, &output.bytes, &output.relocs)
            .map_err(|error| NativeCompileError::Cranelift(error.to_string()))?;
        frame_bytes.push(output.frame_bytes);
        unwind_records.push(UnwindRecord {
            function: body_id,
            code_len: output.code_len,
            info: output.unwind,
        });
        if collect_diagnostics {
            clif.push_str(&output.clif);
            disassembly.push_str(&output.disassembly);
        }
        if let Some(trampoline) = trampoline {
            module
                .define_function_bytes(
                    function_ids[index],
                    trampoline.alignment,
                    &trampoline.bytes,
                    &trampoline.relocs,
                )
                .map_err(|error| NativeCompileError::Cranelift(error.to_string()))?;
            unwind_records.push(UnwindRecord {
                function: function_ids[index],
                code_len: trampoline.code_len,
                info: trampoline.unwind,
            });
            if collect_diagnostics {
                clif.push_str(&trampoline.clif);
                disassembly.push_str(&trampoline.disassembly);
            }
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
pub(crate) fn compile_one_function(
    input: &FunctionCompileInput<'_, '_>,
) -> Result<CompiledFunction, NativeCompileError> {
    let function_index =
        u32::try_from(input.index).map_err(|_| NativeCompileError::Capacity("function IDs"))?;
    let mut context = cranelift_codegen::Context::new();
    let mut builder_context = FunctionBuilderContext::new();
    context.set_disasm(input.collect_diagnostics);
    context.func.signature = input.signature.clone();
    context.func.name = UserFuncName::user(0, function_index);
    lower_function(&mut context.func, &mut builder_context, input).map_err(|error| {
        NativeCompileError::Lowering {
            function: FunctionId(function_index),
            message: error.to_string(),
        }
    })?;
    let clif = if input.collect_diagnostics {
        format!(
            ";; function {}: {}\n{}\n",
            input.index,
            input.ir_function.name(),
            context.func.display()
        )
    } else {
        String::new()
    };
    finish_compiled_function(
        context,
        input.isa.as_ref(),
        input.function_id,
        function_index,
        clif,
        input.collect_diagnostics,
        input.ir_function.name(),
        false,
    )
}

pub(crate) fn finish_compiled_function(
    mut context: cranelift_codegen::Context,
    isa: &dyn cranelift_codegen::isa::TargetIsa,
    function_id: FuncId,
    function_index: u32,
    clif: String,
    collect_diagnostics: bool,
    ir_name: &str,
    trampoline: bool,
) -> Result<CompiledFunction, NativeCompileError> {
    context
        .compile(isa, &mut ControlPlane::default())
        .map_err(|error| NativeCompileError::Cranelift(format!("{:#?}", error.inner)))?;
    let compiled = context
        .compiled_code()
        .ok_or_else(|| NativeCompileError::CompilerInvariant("missing compiled code".into()))?;
    if !compiled.buffer.traps().is_empty() {
        return Err(NativeCompileError::CompilerInvariant(format!(
            "function {} contains a machine trap",
            function_index
        )));
    }
    let kind = if trampoline { "trampoline" } else { "function" };
    let disassembly = if collect_diagnostics {
        format!(
            ";; {kind} {}: {}\n{}\n",
            function_index,
            ir_name,
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
                "function {function_index} is missing frame metadata"
            ))
        })?
        .frame_to_fp_offset;
    let unwind = compiled
        .create_unwind_info(isa)
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

pub(crate) fn declare_string_add_thunk(
    module: &mut ObjectModule,
) -> Result<FuncId, NativeCompileError> {
    let pointer_type = module.target_config().pointer_type();
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(pointer_type));
    signature.params.push(AbiParam::new(types::I64));
    signature.params.push(AbiParam::new(types::I64));
    signature.returns.push(AbiParam::new(types::I64));
    module
        .declare_function(STRING_ADD_SYMBOL.symbol_name(), Linkage::Import, &signature)
        .map_err(|error| NativeCompileError::Cranelift(error.to_string()))
}
pub(crate) fn declare_string_builder_finish_thunk(
    module: &mut ObjectModule,
) -> Result<FuncId, NativeCompileError> {
    let pointer_type = module.target_config().pointer_type();
    let mut signature = module.make_signature();
    signature.params.push(AbiParam::new(pointer_type));
    signature.params.push(AbiParam::new(types::I64));
    signature.returns.push(AbiParam::new(types::I64));
    module
        .declare_function(
            STRING_BUILDER_FINISH_SYMBOL.symbol_name(),
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
            | NativeSignature::ValueBinary
            | NativeSignature::ValueUnary
            | NativeSignature::ValueTernary
            | NativeSignature::ValueBinaryF64
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
        | NativeSignature::ValueBinary
        | NativeSignature::ValueUnary
        | NativeSignature::ValueTernary
        | NativeSignature::ValueBinaryF64
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

pub(crate) fn lower_function(
    function: &mut Function,
    builder_context: &mut FunctionBuilderContext,
    input: &FunctionCompileInput<'_, '_>,
) -> Result<()> {
    let target_config = input.target_config;
    let program = input.program;
    let ir_function = input.ir_function;
    let function_index = u32::try_from(input.index).context("function index exceeds u32")?;
    let host_dispatcher = input.dispatcher;
    let string_add = input.string_add;
    let string_builder_finish = input.string_builder_finish;
    let math_thunks = input.math_thunks;
    let f64_values = input.f64_values;
    let int32_values = input.int32_values;
    let variable_slots = input.variable_slots;
    let root_plan = input.root_plan;
    let root_capacity = input.root_capacity;
    let root_bitmaps = input.root_bitmaps;
    let frame_local_names = input.frame_local_names;
    let boxed_local_names = input.boxed_local_names;
    let ic_slots = input.ic_slots;
    let template_origins = input.template_origins;
    let feedback_slots = input.feedback_slots;
    let specialized_tags = input.specialized_tags;
    let safepoint_free = input.safepoint_free;
    let slow_call_signature = slow_entry_signature(function.signature.call_conv);
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
    let has_suspend = ir_function.blocks().iter().any(|block| {
        block.instructions().iter().any(|instruction| {
            matches!(
                instruction,
                Instruction::Suspend { .. } | Instruction::GeneratorSuspend { .. }
            )
        })
    });
    let mut variables = ValueRepr::plan(
        ir_function,
        input.typed_f64_values,
        frame_local_names,
        has_suspend,
    );
    variables.declare_values(&mut builder, &value_ids);
    let mut frame_locals = frame_local_variables(frame_local_names);
    let boxed_local_order = boxed_local_order(boxed_local_names);
    let boxed_local_indices = frame_local_indices(&boxed_local_order);
    let phi_edges = collect_phi_edges(ir_function);
    let dispatcher_ref = host_dispatcher.import(builder.func);
    let string_add_ref = string_add.import(builder.func);
    let string_builder_finish_ref = string_builder_finish.import(builder.func);
    let mut imported_math_thunks: HashMap<Builtin, ir::FuncRef> =
        HashMap::with_capacity(math_thunks.len());
    let barrier_thunks = input.barrier_thunks.import(builder.func);
    let slow_call_signature = builder.import_signature(slow_call_signature);
    let ctx_value = builder.block_params(entry)[0];
    let env_param = builder.block_params(entry)[1];
    let this_param = builder.block_params(entry)[2];
    let constants = program.constants();
    let boolean_values = infer_boolean_values(ir_function, constants);
    let constant_defs = ir_function
        .blocks()
        .iter()
        .flat_map(|block| block.instructions())
        .filter_map(|instruction| match instruction {
            Instruction::Const { dest, constant } => Some((*dest, *constant)),
            _ => None,
        })
        .collect::<HashMap<_, _>>();
    let immutable_constant_ids = immutable_constant_ids(ir_function, constants);
    // root frame 的基址值必须在入口块物化：入口块支配其余所有块，基址可跨块复用。

    builder.switch_to_block(entry);
    let mut root_frame = if safepoint_free {
        None
    } else {
        let frame = FrameLowering::new(&mut builder, root_bitmaps, root_capacity, ctx_value)?;
        frame.link(&mut builder, ctx_value)?;
        Some(frame)
    };
    initialize_frame_locals(&mut builder, &mut frame_locals, &variables);
    if let Some(root_frame) = root_frame.as_mut() {
        pin_initialized_frame_locals(root_frame, &mut builder, &frame_locals, &boxed_local_order)?;
    }
    let pointer_type = builder.func.dfg.value_type(ctx_value);
    let ht_base = builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        ctx_value,
        vmctx_offset(offset_of!(NativeVmContext, handle_table_base))?,
    );
    let ic_base = builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        ctx_value,
        vmctx_offset(offset_of!(NativeVmContext, ic_slots_base))?,
    );
    let barrier_state = builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        ctx_value,
        vmctx_offset(offset_of!(NativeVmContext, barrier_state))?,
    );
    {
        let mut cx = LoweringCx {
            builder: &mut builder,
            variables: &variables,
            root_frame: root_frame.as_mut(),
            dispatcher: dispatcher_ref,
            string_add: string_add_ref,
            string_builder_finish: string_builder_finish_ref,
            ctx: ctx_value,
            env: env_param,
            this_value: this_param,
            function_index,
            current_block: ir_function.entry(),
            target_config,
            ht_base,
            ic_base,
            barrier_state,
        };
        lower_function_parameters(
            &mut cx,
            ir_function,
            variable_slots,
            &frame_locals,
            &boxed_local_indices,
            specialized_tags,
        )?;
        cx.stage(root_plan.before_instruction(ir_function.entry(), 0));
        let mut hoisted_constants = HashMap::with_capacity(immutable_constant_ids.len());
        for constant_id in &immutable_constant_ids {
            let constant = &constants
                [usize::try_from(constant_id.0).context("constant index does not fit usize")?];
            let result = match constant {
                // 字符串常量 install 期已发布进 vmctx 常量数组：入口两条 load 直读，
                // 替代旧的 MaterializeString 宿主往返（每次函数调用 ~40ns/常量）。
                Constant::String(_) => {
                    let pointer_type = cx.builder.func.dfg.value_type(cx.ctx);
                    let base = cx.builder.ins().load(
                        pointer_type,
                        MemFlagsData::trusted(),
                        cx.ctx,
                        vmctx_offset(offset_of!(NativeVmContext, string_constants_base))?,
                    );
                    let address = cx
                        .builder
                        .ins()
                        .iadd_imm_u(base, i64::from(constant_id.0) * 8);
                    cx.builder
                        .ins()
                        .load(types::I64, MemFlagsData::trusted(), address, 0)
                }
                Constant::BigInt(_) => {
                    let index = cx
                        .builder
                        .ins()
                        .iconst(types::I64, i64::from(constant_id.0));
                    cx.call(NativeRuntimeOp::MaterializeBigInt.id(), &[index], None)?
                }
                _ => unreachable!("immutable constant collector only returns strings and BigInts"),
            };
            hoisted_constants.insert(*constant_id, result);
        }
        let mut imported_function_decls: HashMap<FunctionId, ir::FuncRef> = HashMap::new();
        let mut tables = InstructionTables {
            constants,
            function_index,
            barrier_thunks: &barrier_thunks,
            f64_values,
            int32_values,
            speculative: input.specialized_tags.is_some(),
            constant_defs: &constant_defs,
            math_thunks,
            hoisted_constants: &hoisted_constants,
            imported_math_thunks: &mut imported_math_thunks,
            slow_call_signature,
            variable_slots,
            frame_locals: &frame_locals,
            frame_local_indices: &boxed_local_indices,
            ic_slots,
            template_origins,
            function_decls: input.function_decls,
            imported_function_decls: &mut imported_function_decls,
            direct_callable_functions: input.direct_callable_functions,
        };

        let headers = wjsm_ir::typed_cfg::loop_headers(ir_function);
        let entry_body = cx.builder.create_block();
        emit_resume_dispatch(&mut cx, ir_function, &blocks, &headers, entry_body)?;
        blocks.insert(ir_function.entry(), entry_body);

        for block in ir_function.blocks() {
            let clif_block = blocks[&block.id()];
            cx.builder.switch_to_block(clif_block);
            cx.current_block = block.id();
            cx.enter_root_block();
            let has_suspend = block.instructions().iter().any(|instruction| {
                matches!(
                    instruction,
                    Instruction::Suspend { .. } | Instruction::GeneratorSuspend { .. }
                )
            });
            let first_non_phi = block
                .instructions()
                .iter()
                .position(|instruction| !matches!(instruction, Instruction::Phi { .. }))
                .unwrap_or(block.instructions().len());
            let is_header = headers.contains(&block.id());
            for (instruction_index, instruction) in block.instructions().iter().enumerate() {
                if matches!(instruction, Instruction::Phi { .. }) {
                    continue;
                }
                if is_header && instruction_index == first_non_phi {
                    let lives = wjsm_ir::typed_cfg::loop_header_live_phis(ir_function, block.id());
                    if input.specialized_tags.is_some() {
                        emit_overlay_header_guards(&mut cx, &tables, block.id(), &lives)?;
                    } else {
                        emit_osr_poll(&mut cx, &tables, block.id(), &lives)?;
                    }
                }
                let roots = root_plan.before_instruction(block.id(), instruction_index);
                cx.stage(roots);
                let ctx = cx.ctx;
                let feedback_ptr = feedback_slots
                    .get(&(block.id(), instruction_index))
                    .map(|slot| emit_feedback_slot_ptr(cx.builder, ctx, *slot))
                    .transpose()?;
                lower_instruction(&mut cx, &mut tables, instruction, roots, feedback_ptr)?;
            }
            if has_suspend {
                continue;
            }
            cx.stage(root_plan.before_terminator(block.id()));
            lower_terminator(
                &mut cx,
                block.id(),
                block.terminator(),
                constants,
                &boolean_values,
                &blocks,
                &phi_edges,
            )?;
        }
        cx.finish_roots();
        cx.builder.seal_all_blocks();
    }

    fn immutable_constant_ids(
        function: &wjsm_ir::Function,
        constants: &[Constant],
    ) -> Vec<ConstantId> {
        let mut ids = HashSet::new();
        for block in function.blocks() {
            for instruction in block.instructions() {
                let Instruction::Const { constant, .. } = instruction else {
                    continue;
                };
                let Some(value) = constants.get(constant.0 as usize) else {
                    continue;
                };
                if matches!(value, Constant::String(_) | Constant::BigInt(_)) {
                    ids.insert(*constant);
                }
            }
        }
        let mut ids: Vec<_> = ids.into_iter().collect();
        ids.sort_unstable_by_key(|constant| constant.0);

        ids
    }
    builder.finalize(target_config);
    Ok(())
}

fn lower_function_parameters(
    cx: &mut LoweringCx<'_, '_>,
    function: &wjsm_ir::Function,
    variable_slots: &HashMap<String, u32>,
    frame_locals: &HashMap<String, Variable>,
    frame_local_indices: &HashMap<String, usize>,
    specialized_tags: Option<&[wjsm_native_abi::NativeFeedbackTag]>,
) -> Result<()> {
    let native_params = cx
        .builder
        .block_params(cx.builder.current_block().context("missing entry block")?)
        .to_vec();
    let env = native_params[1];
    let this_value = native_params[2];
    let fast_arity = fast_js_arity(&cx.builder.func.signature);
    let entry_roots: &[ir::Value] = if function.params().len() >= 2 {
        &[env, this_value]
    } else if function.params().len() == 1 {
        &[env]
    } else {
        &[]
    };
    cx.publish_roots(&[], entry_roots)?;
    let uses_canonical_this = function.blocks().iter().any(|block| {
        block.instructions().iter().any(|instruction| {
            matches!(
                instruction,
                Instruction::LoadVar { name, .. } | Instruction::StoreVar { name, .. }
                    if name == "$this"
            )
        })
    });
    let mut param_values = Vec::with_capacity(function.params().len());
    for (index, _) in function.params().iter().enumerate() {
        let value = match index {
            0 => env,
            1 => this_value,
            _ => load_js_parameter(cx, &native_params, index, fast_arity, specialized_tags)?,
        };
        param_values.push(value);
    }
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
        let value = param_values[index];
        if let Some(local) = frame_locals.get(storage_name).copied() {
            // typed 局部只在「该名字的全部 load 都已证明 f64」时成立，入参的
            // NaN-Box 位模式此时就是 double，入口一次 bitcast 即可。
            let native = if cx.variables.is_typed_local(storage_name) {
                unbox_f64(cx.builder, value)
            } else {
                value
            };
            cx.builder.def_var(local, native);
            if let Some(index) = frame_local_indices.get(storage_name).copied() {
                cx.update_pinned_local(index, native)?;
            }
            continue;
        }
        let Some(slot) = variable_slots.get(storage_name).copied() else {
            continue;
        };
        cx.publish_roots(&[], &[value])?;
        let slot = cx.builder.ins().iconst(types::I64, i64::from(slot));
        let _ = cx.call(NativeRuntimeOp::StoreVar.id(), &[slot, value], None)?;
    }
    Ok(())
}

fn load_js_parameter(
    cx: &mut LoweringCx<'_, '_>,
    native_params: &[ir::Value],
    index: usize,
    fast_arity: Option<usize>,
    specialized_tags: Option<&[wjsm_native_abi::NativeFeedbackTag]>,
) -> Result<ir::Value> {
    let param_idx = index - 2;
    if let Some(arity) = fast_arity {
        if param_idx < arity
            && let Some(value) = native_params.get(3 + param_idx).copied()
        {
            return Ok(value);
        }
        return Ok(cx
            .builder
            .ins()
            .iconst(types::I64, value::encode_undefined()));
    }
    let pointer_type = cx.builder.func.dfg.value_type(cx.ctx);
    let args_base_u32 = cx.builder.ins().uextend(types::I64, native_params[3]);
    let arena_base = cx.builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, call_arena_slots))?,
    );
    let param_idx_u32 = u32::try_from(param_idx).context("parameter index exceeds u32")?;
    let slot_offset = i64::from(param_idx_u32)
        .checked_mul(size_of::<i64>() as i64)
        .context("call arena offset overflows")?;
    let args_base_bytes = cx.builder.ins().ishl_imm_u(args_base_u32, 3);
    let param_bytes = cx.builder.ins().iadd_imm_s(args_base_bytes, slot_offset);
    if let Some(tags) = specialized_tags
        && tags.get(param_idx).is_some()
    {
        let address = cx.builder.ins().iadd(arena_base, param_bytes);
        return Ok(cx
            .builder
            .ins()
            .load(types::I64, MemFlagsData::trusted(), address, 0));
    }
    let in_bounds = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::UnsignedGreaterThan,
        native_params[4],
        i64::from(param_idx_u32),
    );
    let address = cx.builder.ins().iadd(arena_base, param_bytes);
    let loaded = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), address, 0);
    let undefined = cx
        .builder
        .ins()
        .iconst(types::I64, value::encode_undefined());
    Ok(cx.builder.ins().select(in_bounds, loaded, undefined))
}

fn lower_native_object_allocation(
    cx: &mut LoweringCx<'_, '_>,
    dest: ValueId,
    capacity: u32,
    array: bool,
) -> Result<()> {
    // 与宿主首次扩容策略一致：空对象预留常见构造器字段，避免尚未物化对象
    // 在第一次属性写入时立即搬迁。
    let capacity = if array {
        capacity
    } else {
        capacity.max(constants::HEAP_OBJECT_INITIAL_VALUE_CAPACITY)
    };
    let bytes = u64::from(capacity)
        .checked_mul(u64::from(constants::HEAP_OBJECT_VALUE_SLOT_SIZE))
        .and_then(|payload| payload.checked_add(u64::from(constants::HEAP_OBJECT_HEADER_SIZE)))
        .and_then(|bytes| bytes.checked_add(7))
        .map(|bytes| bytes & !7)
        .context("native object allocation size overflows")?;
    let bytes = i64::try_from(bytes).context("native object allocation size exceeds i64")?;
    let fast_block = cx.builder.create_block();
    let slow_block = cx.builder.create_block();
    let merge_block = cx.builder.create_block();
    cx.builder.append_block_param(merge_block, types::I64);

    let flags = cx.builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, allocation_fast_flags))?,
    );
    let allocation_flag = if array {
        wjsm_native_abi::NATIVE_ALLOCATION_FAST_ARRAY
    } else {
        wjsm_native_abi::NATIVE_ALLOCATION_FAST_OBJECT
    };
    let enabled = cx
        .builder
        .ins()
        .band_imm_u(flags, i64::from(allocation_flag));
    let enabled = cx
        .builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::NotEqual, enabled, 0);
    let small_limit = cx.builder.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, allocation_small_limit))?,
    );
    let bytes_value = cx.builder.ins().iconst(types::I64, bytes);
    let small = cx.builder.ins().icmp(
        ir::condcodes::IntCC::UnsignedLessThanOrEqual,
        bytes_value,
        small_limit,
    );
    let top = cx.builder.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, bump_ptr))?,
    );
    let limit = cx.builder.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, bump_limit))?,
    );
    let end = cx.builder.ins().iadd(top, bytes_value);
    let object_fits =
        cx.builder
            .ins()
            .icmp(ir::condcodes::IntCC::UnsignedLessThanOrEqual, end, limit);
    let cursor = cx.builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, bump_handle_cursor))?,
    );
    let handle_limit = cx.builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, bump_handle_limit))?,
    );
    let handle_fits =
        cx.builder
            .ins()
            .icmp(ir::condcodes::IntCC::UnsignedLessThan, cursor, handle_limit);
    let ready = cx.builder.ins().band(enabled, small);
    let ready = cx.builder.ins().band(ready, object_fits);
    let ready = cx.builder.ins().band(ready, handle_fits);
    cx.builder
        .ins()
        .brif(ready, fast_block, &[], slow_block, &[]);

    cx.builder.switch_to_block(fast_block);
    cx.builder.seal_block(fast_block);
    let prototype = cx.builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(if array {
            offset_of!(NativeVmContext, array_prototype_handle)
        } else {
            offset_of!(NativeVmContext, object_prototype_handle)
        })?,
    );
    let prototype = cx.builder.ins().uextend(types::I64, prototype);
    let heap_type = if array {
        wjsm_ir::HEAP_TYPE_ARRAY
    } else {
        wjsm_ir::HEAP_TYPE_OBJECT
    };
    let type_word = cx.builder.ins().iconst(
        types::I64,
        i64::try_from(u64::from(heap_type) << 32).expect("heap type word fits i64"),
    );
    let header_word = cx.builder.ins().bor(prototype, type_word);
    let heap_delta = cx.builder.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, heap_object_delta))?,
    );
    let address = cx.builder.ins().iadd(top, heap_delta);
    cx.builder
        .ins()
        .store(MemFlagsData::trusted(), header_word, address, 0);
    let capacity_i32 = cx.builder.ins().iconst(types::I32, i64::from(capacity));
    let zero_i32 = cx.builder.ins().iconst(types::I32, 0);
    let first_header_value = if array { zero_i32 } else { capacity_i32 };
    let second_header_value = if array { capacity_i32 } else { zero_i32 };
    cx.builder.ins().store(
        MemFlagsData::trusted(),
        first_header_value,
        address,
        i32::try_from(if array {
            constants::HEAP_ARRAY_LENGTH_OFFSET
        } else {
            constants::HEAP_OBJECT_VALUE_CAPACITY_OFFSET
        })
        .expect("capacity or length offset fits i32"),
    );
    cx.builder.ins().store(
        MemFlagsData::trusted(),
        second_header_value,
        address,
        i32::try_from(if array {
            constants::HEAP_ARRAY_CAPACITY_OFFSET
        } else {
            constants::HEAP_OBJECT_SHAPE_ID_OFFSET
        })
        .expect("capacity or shape offset fits i32"),
    );
    let handle_value = cx.builder.ins().uextend(types::I64, cursor);
    cx.builder.ins().store(
        MemFlagsData::trusted(),
        handle_value,
        address,
        i32::try_from(constants::HEAP_OBJECT_GC_WORD_OFFSET).expect("GC word offset fits i32"),
    );
    let handle_table = cx.ht_base;
    let entry_offset = cx.builder.ins().ishl_imm_u(handle_value, 3);
    let entry_address = cx.builder.ins().iadd(handle_table, entry_offset);
    let object_bits = cx.builder.ins().ishl_imm_u(top, 16);
    let stable_state = cx
        .builder
        .ins()
        .iconst(types::I64, i64::from(constants::HANDLE_STATE_STABLE_YOUNG));
    let entry_value = cx.builder.ins().bor(object_bits, stable_state);
    cx.builder
        .ins()
        .atomic_store(MemFlagsData::trusted(), entry_value, entry_address);
    cx.builder.ins().store(
        MemFlagsData::trusted(),
        end,
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, bump_ptr))?,
    );
    let next_handle = cx.builder.ins().iadd_imm_u(cursor, 1);
    cx.builder.ins().store(
        MemFlagsData::trusted(),
        next_handle,
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, bump_handle_cursor))?,
    );
    let tag = if array {
        value::TAG_ARRAY
    } else {
        value::TAG_OBJECT
    };
    let value_prefix = cx.builder.ins().iconst(
        types::I64,
        i64::from_ne_bytes((value::BOX_BASE | (tag << 32)).to_ne_bytes()),
    );
    let encoded = cx.builder.ins().bor(handle_value, value_prefix);
    cx.builder
        .ins()
        .jump(merge_block, &[ir::BlockArg::Value(encoded)]);

    cx.builder.switch_to_block(slow_block);
    cx.builder.seal_block(slow_block);
    let slow_capacity = cx.builder.ins().iconst(types::I64, i64::from(capacity));
    let slow = cx.call(
        if array {
            NativeRuntimeOp::NewArray.id()
        } else {
            NativeRuntimeOp::NewObject.id()
        },
        &[slow_capacity],
        None,
    )?;
    cx.builder
        .ins()
        .jump(merge_block, &[ir::BlockArg::Value(slow)]);

    cx.builder.switch_to_block(merge_block);
    cx.builder.seal_block(merge_block);
    define_value_boxed(
        cx.builder,
        cx.variables,
        dest,
        cx.builder.block_params(merge_block)[0],
    )
}

fn object_template_meta_index(constants: &[Constant], template: ConstantId) -> Option<u32> {
    crate::template_meta::object_template_meta_index(constants, template)
}

fn emit_load_object_template_meta_word(
    cx: &mut LoweringCx<'_, '_>,
    meta_index: u32,
    word_index: u32,
) -> Result<ir::Value> {
    let pointer_type = cx.builder.func.dfg.value_type(cx.ctx);
    let meta_base = cx.builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, object_template_meta_base))?,
    );
    let entry_offset = (u64::from(meta_index) * u64::from(constants::OBJECT_TEMPLATE_META_WORDS)
        + u64::from(word_index))
    .checked_mul(4)
    .context("object template meta offset overflows")?;
    let entry_offset =
        i64::try_from(entry_offset).context("object template meta offset exceeds i64")?;
    let address = cx.builder.ins().iadd_imm_s(meta_base, entry_offset);
    let word = cx
        .builder
        .ins()
        .load(types::I32, MemFlagsData::trusted(), address, 0);
    Ok(cx.builder.ins().uextend(types::I64, word))
}

/// 模板自有数据属性的编译期槽偏移：空 shape 按键序追加时 `slot_index == prop_index`。
fn template_value_slot_offset(prop_index: u32) -> Result<i32> {
    let scaled = u64::from(prop_index)
        .checked_mul(u64::from(constants::HEAP_OBJECT_VALUE_SLOT_SIZE))
        .context("template slot scale overflows")?;
    let offset = u64::from(constants::HEAP_OBJECT_HEADER_SIZE)
        .checked_add(scaled)
        .context("template slot offset overflows")?;
    i32::try_from(offset).context("template slot offset exceeds i32")
}

fn template_hit_args(
    logical_addr: ir::Value,
    direct_store: Option<ir::Value>,
) -> Vec<ir::BlockArg> {
    let mut args = vec![ir::BlockArg::Value(logical_addr)];
    if let Some(flag) = direct_store {
        args.push(ir::BlockArg::Value(flag));
    }
    args
}

struct TemplateReceiver {
    handle_i32: ir::Value,
    heap_delta: ir::Value,
    barrier_disabled: ir::Value,
}

/// 模板对象：标签 / 句柄 / epoch / 烘焙 shape 命中后跳到 `hit_block`。
///
/// `store` 时 `hit_block` 额外接收 `direct_store: i8`（young + 未标记）。
fn emit_template_receiver_guard(
    cx: &mut LoweringCx<'_, '_>,
    barrier_thunks: &BarrierThunks,
    object: ValueId,
    meta_index: u32,
    hit_block: ir::Block,
    fallback_block: ir::Block,
    store: bool,
) -> Result<TemplateReceiver> {
    let pointer_type = cx.builder.func.dfg.value_type(cx.ctx);
    let obj = use_value_boxed(cx.builder, cx.variables, object)?;
    let box_base = i64::from_ne_bytes(value::BOX_BASE.to_ne_bytes());
    let ht_base = cx.ht_base;
    let barrier_state = cx.barrier_state;
    let boxed_bits = cx.builder.ins().band_imm_s(obj, box_base);
    let is_boxed = cx
        .builder
        .ins()
        .icmp_imm_s(ir::condcodes::IntCC::Equal, boxed_bits, box_base);
    let tag = cx.builder.ins().ushr_imm_u(obj, 32);
    let tag = cx.builder.ins().band_imm_u(
        tag,
        i64::try_from(value::TAG_MASK).expect("tag mask fits i64"),
    );
    let is_obj = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        tag,
        i64::try_from(value::TAG_OBJECT).expect("object tag fits i64"),
    );
    let tag_ok = cx.builder.ins().band(is_boxed, is_obj);

    let entry_block = cx.builder.create_block();
    let legacy_entry_block = cx.builder.create_block();
    let zgc_entry_block = cx.builder.create_block();
    let zgc_fast_block = cx.builder.create_block();
    let receiver_assist_block = cx.builder.create_block();
    let shape_check_block = cx.builder.create_block();
    cx.builder.append_block_param(shape_check_block, types::I64);
    if store {
        cx.builder.append_block_param(shape_check_block, types::I8);
    }

    cx.builder
        .ins()
        .brif(tag_ok, entry_block, &[], fallback_block, &[]);

    cx.builder.switch_to_block(entry_block);
    cx.builder.seal_block(entry_block);
    let handle_idx = cx.builder.ins().band_imm_u(obj, i64::from(u32::MAX));
    let handle_i32 = cx.builder.ins().ireduce(types::I32, handle_idx);
    let entry_offset = cx.builder.ins().ishl_imm_u(handle_idx, 3);
    let entry_addr = cx.builder.ins().iadd(ht_base, entry_offset);
    let entry = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), entry_addr, 0);
    let entry_state = cx.builder.ins().band_imm_u(entry, 0xFFFF);
    let stable = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::UnsignedGreaterThanOrEqual,
        entry_state,
        i64::from(constants::HANDLE_STATE_STABLE_MIN),
    );
    let logical_addr = cx.builder.ins().ushr_imm_u(entry, 16);
    let heap_delta = cx.builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, heap_object_delta))?,
    );
    let barrier_disabled =
        cx.builder
            .ins()
            .icmp_imm_u(ir::condcodes::IntCC::Equal, barrier_state, 0);
    cx.builder.ins().brif(
        barrier_disabled,
        legacy_entry_block,
        &[],
        zgc_entry_block,
        &[],
    );

    cx.builder.switch_to_block(legacy_entry_block);
    cx.builder.seal_block(legacy_entry_block);
    let legacy_direct = store.then(|| cx.builder.ins().iconst(types::I8, 1));
    let legacy_args = template_hit_args(logical_addr, legacy_direct);
    cx.builder
        .ins()
        .brif(stable, shape_check_block, &legacy_args, fallback_block, &[]);

    cx.builder.switch_to_block(zgc_entry_block);
    cx.builder.seal_block(zgc_entry_block);
    let epoch_addr = cx.builder.ins().iadd_imm_s(
        barrier_state,
        i64::try_from(offset_of!(NativeBarrierState, access_epoch))
            .expect("access epoch offset fits i64"),
    );
    let access_epoch =
        cx.builder
            .ins()
            .atomic_load(types::I64, MemFlagsData::trusted(), epoch_addr);
    let epoch_bit = cx.builder.ins().band_imm_u(access_epoch, 1);
    let epoch_even = cx
        .builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::Equal, epoch_bit, 0);
    let direct = cx.builder.ins().band(stable, epoch_even);
    cx.builder
        .ins()
        .brif(direct, zgc_fast_block, &[], receiver_assist_block, &[]);

    cx.builder.switch_to_block(zgc_fast_block);
    cx.builder.seal_block(zgc_fast_block);
    let zgc_direct = if store {
        let phase_addr = cx.builder.ins().iadd_imm_s(
            barrier_state,
            i64::try_from(offset_of!(NativeBarrierState, phase)).expect("phase offset fits i64"),
        );
        let phase = cx
            .builder
            .ins()
            .atomic_load(types::I64, MemFlagsData::trusted(), phase_addr);
        let marking = cx
            .builder
            .ins()
            .band_imm_u(phase, NATIVE_BARRIER_MARKING_MASK as i64);
        let marking_idle = cx
            .builder
            .ins()
            .icmp_imm_u(ir::condcodes::IntCC::Equal, marking, 0);
        let stable_young = cx.builder.ins().icmp_imm_u(
            ir::condcodes::IntCC::Equal,
            entry_state,
            i64::from(constants::HANDLE_STATE_STABLE_YOUNG),
        );
        Some(cx.builder.ins().band(marking_idle, stable_young))
    } else {
        increment_barrier_counter(
            cx.builder,
            barrier_state,
            offset_of!(NativeBarrierState, load_fast_events),
        );
        None
    };
    let fast_args = template_hit_args(logical_addr, zgc_direct);
    cx.builder.ins().jump(shape_check_block, &fast_args);

    cx.builder.switch_to_block(receiver_assist_block);
    cx.builder.seal_block(receiver_assist_block);
    let call = cx
        .builder
        .ins()
        .call(barrier_thunks.load, &[cx.ctx, handle_i32]);
    let assisted = cx.builder.inst_results(call)[0];
    let assisted_ok = cx
        .builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::NotEqual, assisted, 0);
    let assist_direct = store.then(|| cx.builder.ins().iconst(types::I8, 0));
    let assist_args = template_hit_args(assisted, assist_direct);
    cx.builder.ins().brif(
        assisted_ok,
        shape_check_block,
        &assist_args,
        fallback_block,
        &[],
    );

    cx.builder.switch_to_block(shape_check_block);
    cx.builder.seal_block(shape_check_block);
    let logical_addr = cx.builder.block_params(shape_check_block)[0];
    let direct_store = store.then(|| cx.builder.block_params(shape_check_block)[1]);
    let addr = cx.builder.ins().iadd(logical_addr, heap_delta);
    let baked_shape = emit_load_object_template_meta_word(cx, meta_index, 0)?;
    let obj_word = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), addr, 8);
    let obj_shape = cx.builder.ins().ushr_imm_u(obj_word, 32);
    let shape_match = cx
        .builder
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, obj_shape, baked_shape);
    let hit_args = template_hit_args(logical_addr, direct_store);
    cx.builder
        .ins()
        .brif(shape_match, hit_block, &hit_args, fallback_block, &[]);

    Ok(TemplateReceiver {
        handle_i32,
        heap_delta,
        barrier_disabled,
    })
}

/// 在新分配对象已知 value slot 上直写属性值（仅用于 unboxed 数字等无需 store barrier 的值）。
fn lower_create_data_property_fast(
    cx: &mut LoweringCx<'_, '_>,
    logical_addr: ir::Value,
    heap_delta: ir::Value,
    prop_index: u32,
    stored: ir::Value,
) -> Result<()> {
    let offset = template_value_slot_offset(prop_index)?;
    let addr = cx.builder.ins().iadd(logical_addr, heap_delta);
    cx.builder
        .ins()
        .store(MemFlagsData::trusted(), stored, addr, offset);
    Ok(())
}

/// 模板对象自有数据属性读：shape 命中后 `load [obj+imm]`，失配回落 fallback。
fn lower_get_template_prop_inline(
    cx: &mut LoweringCx<'_, '_>,
    barrier_thunks: &BarrierThunks,
    dest: ValueId,
    object: ValueId,
    meta_index: u32,
    prop_index: u32,
    merge_block: ir::Block,
    fallback_block: ir::Block,
) -> Result<()> {
    let hit_block = cx.builder.create_block();
    cx.builder.append_block_param(hit_block, types::I64);
    let receiver = emit_template_receiver_guard(
        cx,
        barrier_thunks,
        object,
        meta_index,
        hit_block,
        fallback_block,
        false,
    )?;
    cx.builder.switch_to_block(hit_block);
    cx.builder.seal_block(hit_block);
    let logical_addr = cx.builder.block_params(hit_block)[0];
    let addr = cx.builder.ins().iadd(logical_addr, receiver.heap_delta);
    let offset = template_value_slot_offset(prop_index)?;
    let loaded = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), addr, offset);
    define_value_boxed(cx.builder, cx.variables, dest, loaded)?;
    cx.builder.ins().jump(merge_block, &[]);
    Ok(())
}

/// licm elem-guard 的 pre-header 守卫：宿主一次性校验数组与元素 shape，
/// 产出编码布尔（只读、不分配、不执行用户代码，多执行一次不可观察）。
fn lower_elem_shape_guard(
    cx: &mut LoweringCx<'_, '_>,
    constants: &[Constant],
    dest: ValueId,
    array: ValueId,
    template: ConstantId,
) -> Result<()> {
    let Some(meta_index) = object_template_meta_index(constants, template) else {
        bail!("elem_shape_guard template constant is invalid");
    };
    let array = use_value_boxed(cx.builder, cx.variables, array)?;
    let meta_index = cx.builder.ins().iconst(types::I64, i64::from(meta_index));
    let result = cx.call(
        NativeRuntimeOp::ElemShapeGuard.id(),
        &[array, meta_index],
        None,
    )?;
    define_value_boxed(cx.builder, cx.variables, dest, result)
}

/// `GetPropGuarded` 的操作数组。
struct GuardedPropAccess {
    dest: ValueId,
    object: ValueId,
    key: ValueId,
    guard: ValueId,
    template: ConstantId,
}

/// 守卫属性读：守卫为真时 receiver 已被 pre-header 的 `ElemShapeGuard` 证明
/// 持有模板烘焙 shape，跳过逐迭代 tag/shape/proto 检查，解句柄后按模板槽
/// 偏移单指令直读；其余情况先把守卫值置 false（单向闩锁，宿主回退可能执行
/// 用户代码），再走与普通 `GetProp` 完全一致的 IC / 宿主路径。
fn lower_get_prop_guarded(
    cx: &mut LoweringCx<'_, '_>,
    tables: &InstructionTables<'_>,
    access: GuardedPropAccess,
    roots: &[ValueId],
) -> Result<()> {
    let GuardedPropAccess {
        dest,
        object,
        key,
        guard,
        template,
    } = access;
    let prop_index =
        template_property_index_for_key(tables.constants, tables.constant_defs, template, key)
            .context("get_prop_guarded key must be a template own key")?;
    let offset = template_value_slot_offset(prop_index)?;

    let fast_block = cx.builder.create_block();
    let slow_block = cx.builder.create_block();
    let merge_block = cx.builder.create_block();
    let guard_value = use_value_boxed(cx.builder, cx.variables, guard)?;
    let guard_on = cx.builder.ins().icmp_imm_s(
        ir::condcodes::IntCC::Equal,
        guard_value,
        value::encode_bool(true),
    );
    cx.builder
        .ins()
        .brif(guard_on, fast_block, &[], slow_block, &[]);

    cx.builder.switch_to_block(fast_block);
    cx.builder.seal_block(fast_block);
    emit_guarded_slot_read(
        cx,
        tables.barrier_thunks,
        dest,
        object,
        offset,
        merge_block,
        slow_block,
    )?;

    // 慢路径入口：先熄灭守卫再走通用路径（IC / 宿主可能执行用户代码）。
    cx.builder.switch_to_block(slow_block);
    cx.builder.seal_block(slow_block);
    let disabled = cx
        .builder
        .ins()
        .iconst(types::I64, value::encode_bool(false));
    define_value_boxed(cx.builder, cx.variables, guard, disabled)?;
    if let Some(slot) = tables.ic_slots.get(&dest).copied() {
        lower_get_prop_ic_non_nullish(
            cx,
            tables.barrier_thunks,
            prop_access(tables, dest, object, key, slot),
            roots,
            merge_block,
        )?;
    } else {
        lower_value_operation(cx, NativeRuntimeOp::GetProp, &[object, key], Some(dest))?;
        cx.builder.ins().jump(merge_block, &[]);
    }

    cx.builder.switch_to_block(merge_block);
    cx.builder.seal_block(merge_block);
    Ok(())
}

/// 守卫为真时的模板槽直读：不做 tag/shape 检查（pre-header 已一次性证明
/// receiver 是烘焙 shape 的普通对象），只保留句柄稳定态 / access epoch
/// 协议——GC 可能在循环回边 safepoint 重定位对象，句柄解析不是循环不变量。
/// 句柄表 entry 的 trusted load 必须留在守卫分支之后的独立块内，防止
/// Cranelift 把它投机提前到守卫为假（object 可能非对象）的路径上。
fn emit_guarded_slot_read(
    cx: &mut LoweringCx<'_, '_>,
    barrier_thunks: &BarrierThunks,
    dest: ValueId,
    object: ValueId,
    offset: i32,
    merge_block: ir::Block,
    slow_block: ir::Block,
) -> Result<()> {
    let pointer_type = cx.builder.func.dfg.value_type(cx.ctx);
    let obj = use_value_boxed(cx.builder, cx.variables, object)?;
    let ht_base = cx.ht_base;
    let barrier_state = cx.barrier_state;

    let legacy_block = cx.builder.create_block();
    let zgc_block = cx.builder.create_block();
    let zgc_fast_block = cx.builder.create_block();
    let assist_block = cx.builder.create_block();
    let hit_block = cx.builder.create_block();
    cx.builder.append_block_param(hit_block, types::I64);

    let handle_idx = cx.builder.ins().band_imm_u(obj, i64::from(u32::MAX));
    let handle_i32 = cx.builder.ins().ireduce(types::I32, handle_idx);
    let entry_offset = cx.builder.ins().ishl_imm_u(handle_idx, 3);
    let entry_addr = cx.builder.ins().iadd(ht_base, entry_offset);
    let entry = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), entry_addr, 0);
    let entry_state = cx.builder.ins().band_imm_u(entry, 0xFFFF);
    let stable = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::UnsignedGreaterThanOrEqual,
        entry_state,
        i64::from(constants::HANDLE_STATE_STABLE_MIN),
    );
    let logical_addr = cx.builder.ins().ushr_imm_u(entry, 16);
    let heap_delta = cx.builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, heap_object_delta))?,
    );
    let barrier_disabled =
        cx.builder
            .ins()
            .icmp_imm_u(ir::condcodes::IntCC::Equal, barrier_state, 0);
    cx.builder
        .ins()
        .brif(barrier_disabled, legacy_block, &[], zgc_block, &[]);

    cx.builder.switch_to_block(legacy_block);
    cx.builder.seal_block(legacy_block);
    cx.builder.ins().brif(
        stable,
        hit_block,
        &[ir::BlockArg::Value(logical_addr)],
        slow_block,
        &[],
    );

    cx.builder.switch_to_block(zgc_block);
    cx.builder.seal_block(zgc_block);
    let epoch_addr = cx.builder.ins().iadd_imm_s(
        barrier_state,
        i64::try_from(offset_of!(NativeBarrierState, access_epoch))
            .expect("access epoch offset fits i64"),
    );
    let access_epoch =
        cx.builder
            .ins()
            .atomic_load(types::I64, MemFlagsData::trusted(), epoch_addr);
    let epoch_bit = cx.builder.ins().band_imm_u(access_epoch, 1);
    let epoch_even = cx
        .builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::Equal, epoch_bit, 0);
    let direct = cx.builder.ins().band(stable, epoch_even);
    cx.builder
        .ins()
        .brif(direct, zgc_fast_block, &[], assist_block, &[]);

    cx.builder.switch_to_block(zgc_fast_block);
    cx.builder.seal_block(zgc_fast_block);
    increment_barrier_counter(
        cx.builder,
        barrier_state,
        offset_of!(NativeBarrierState, load_fast_events),
    );
    cx.builder
        .ins()
        .jump(hit_block, &[ir::BlockArg::Value(logical_addr)]);

    cx.builder.switch_to_block(assist_block);
    cx.builder.seal_block(assist_block);
    let call = cx
        .builder
        .ins()
        .call(barrier_thunks.load, &[cx.ctx, handle_i32]);
    let assisted = cx.builder.inst_results(call)[0];
    let assisted_ok = cx
        .builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::NotEqual, assisted, 0);
    cx.builder.ins().brif(
        assisted_ok,
        hit_block,
        &[ir::BlockArg::Value(assisted)],
        slow_block,
        &[],
    );

    cx.builder.switch_to_block(hit_block);
    cx.builder.seal_block(hit_block);
    let logical_addr = cx.builder.block_params(hit_block)[0];
    let addr = cx.builder.ins().iadd(logical_addr, heap_delta);
    let loaded = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), addr, offset);
    define_value_boxed(cx.builder, cx.variables, dest, loaded)?;
    cx.builder.ins().jump(merge_block, &[]);
    Ok(())
}

/// 模板对象自有数据属性写：shape 命中后 `store [obj+imm]`，失配回落 fallback。
fn lower_set_template_prop_inline(
    cx: &mut LoweringCx<'_, '_>,
    barrier_thunks: &BarrierThunks,
    dest: ValueId,
    object: ValueId,
    value: ValueId,
    meta_index: u32,
    prop_index: u32,
    merge_block: ir::Block,
    fallback_block: ir::Block,
) -> Result<()> {
    let stored = use_value_boxed(cx.builder, cx.variables, value)?;
    let hit_block = cx.builder.create_block();
    cx.builder.append_block_param(hit_block, types::I64);
    cx.builder.append_block_param(hit_block, types::I8);
    let receiver = emit_template_receiver_guard(
        cx,
        barrier_thunks,
        object,
        meta_index,
        hit_block,
        fallback_block,
        true,
    )?;
    emit_template_own_store(
        cx,
        barrier_thunks,
        dest,
        stored,
        receiver,
        prop_index,
        hit_block,
        merge_block,
        fallback_block,
    )
}

fn emit_template_own_store(
    cx: &mut LoweringCx<'_, '_>,
    barrier_thunks: &BarrierThunks,
    dest: ValueId,
    stored: ir::Value,
    receiver: TemplateReceiver,
    prop_index: u32,
    hit_block: ir::Block,
    merge_block: ir::Block,
    fallback_block: ir::Block,
) -> Result<()> {
    let offset = template_value_slot_offset(prop_index)?;
    let legacy_store_block = cx.builder.create_block();
    let zgc_store_mode_block = cx.builder.create_block();
    let zgc_direct_store_block = cx.builder.create_block();
    let scalar_elide_block = cx.builder.create_block();
    let barrier_store_block = cx.builder.create_block();
    let store_done_block = cx.builder.create_block();

    cx.builder.switch_to_block(hit_block);
    cx.builder.seal_block(hit_block);
    let logical_addr = cx.builder.block_params(hit_block)[0];
    let direct_store = cx.builder.block_params(hit_block)[1];
    let addr = cx.builder.ins().iadd(logical_addr, receiver.heap_delta);
    let logical_slot = cx.builder.ins().iadd_imm_s(logical_addr, i64::from(offset));
    let value_addr = cx.builder.ins().iadd_imm_s(addr, i64::from(offset));
    cx.builder.ins().brif(
        receiver.barrier_disabled,
        legacy_store_block,
        &[],
        zgc_store_mode_block,
        &[],
    );

    cx.builder.switch_to_block(zgc_store_mode_block);
    cx.builder.seal_block(zgc_store_mode_block);
    cx.builder.ins().brif(
        direct_store,
        zgc_direct_store_block,
        &[],
        scalar_elide_block,
        &[],
    );

    cx.builder.switch_to_block(scalar_elide_block);
    cx.builder.seal_block(scalar_elide_block);
    let stored_unboxed = emit_unboxed_nanbox_predicate(cx.builder, stored);
    let old = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), addr, offset);
    let old_unboxed = emit_unboxed_nanbox_predicate(cx.builder, old);
    let scalar_direct = cx.builder.ins().band(stored_unboxed, old_unboxed);
    cx.builder.ins().brif(
        scalar_direct,
        zgc_direct_store_block,
        &[],
        barrier_store_block,
        &[],
    );

    cx.builder.switch_to_block(legacy_store_block);
    cx.builder.seal_block(legacy_store_block);
    cx.builder
        .ins()
        .store(MemFlagsData::trusted(), stored, addr, offset);
    cx.builder.ins().jump(store_done_block, &[]);

    cx.builder.switch_to_block(zgc_direct_store_block);
    cx.builder.seal_block(zgc_direct_store_block);
    cx.builder
        .ins()
        .atomic_store(MemFlagsData::trusted(), stored, value_addr);
    increment_barrier_counter(
        cx.builder,
        cx.barrier_state,
        offset_of!(NativeBarrierState, store_fast_events),
    );
    cx.builder.ins().jump(store_done_block, &[]);

    cx.builder.switch_to_block(barrier_store_block);
    cx.builder.seal_block(barrier_store_block);
    let call = cx.builder.ins().call(
        barrier_thunks.store,
        &[cx.ctx, receiver.handle_i32, logical_slot, stored],
    );
    let status = cx.builder.inst_results(call)[0];
    let stored_ok = cx
        .builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::Equal, status, 0);
    cx.builder
        .ins()
        .brif(stored_ok, store_done_block, &[], fallback_block, &[]);

    cx.builder.switch_to_block(store_done_block);
    cx.builder.seal_block(store_done_block);
    define_value_boxed(cx.builder, cx.variables, dest, stored)?;
    cx.builder.ins().jump(merge_block, &[]);
    Ok(())
}

fn lower_get_prop_with_template_or_ic(
    cx: &mut LoweringCx<'_, '_>,
    tables: &InstructionTables<'_>,
    barrier_thunks: &BarrierThunks,
    dest: ValueId,
    object: ValueId,
    key: ValueId,
    roots: &[ValueId],
) -> Result<()> {
    let template_inline = tables.template_origins.get(&object).and_then(|site| {
        template_property_index_for_key(tables.constants, tables.constant_defs, site.template, key)
            .map(|prop_index| (site.meta_index, prop_index))
    });
    if let Some((meta_index, prop_index)) = template_inline {
        let merge_block = cx.builder.create_block();
        let fallback_block = cx.builder.create_block();
        lower_get_template_prop_inline(
            cx,
            barrier_thunks,
            dest,
            object,
            meta_index,
            prop_index,
            merge_block,
            fallback_block,
        )?;
        cx.builder.switch_to_block(fallback_block);
        cx.builder.seal_block(fallback_block);
        lower_value_operation(cx, NativeRuntimeOp::GetProp, &[object, key], Some(dest))?;
        cx.builder.ins().jump(merge_block, &[]);
        cx.builder.switch_to_block(merge_block);
        cx.builder.seal_block(merge_block);
        return Ok(());
    }
    if let Some(slot) = tables.ic_slots.get(&dest).copied() {
        lower_get_prop_ic(
            cx,
            barrier_thunks,
            prop_access(tables, dest, object, key, slot),
            roots,
        )
    } else {
        lower_value_operation(cx, NativeRuntimeOp::GetProp, &[object, key], Some(dest))
    }
}

fn lower_set_prop_with_template_or_ic(
    cx: &mut LoweringCx<'_, '_>,
    tables: &InstructionTables<'_>,
    barrier_thunks: &BarrierThunks,
    dest: ValueId,
    object: ValueId,
    key: ValueId,
    value: ValueId,
) -> Result<()> {
    let template_inline = tables.template_origins.get(&object).and_then(|site| {
        template_property_index_for_key(tables.constants, tables.constant_defs, site.template, key)
            .map(|prop_index| (site.meta_index, prop_index))
    });
    if let Some((meta_index, prop_index)) = template_inline {
        let merge_block = cx.builder.create_block();
        let fallback_block = cx.builder.create_block();
        lower_set_template_prop_inline(
            cx,
            barrier_thunks,
            dest,
            object,
            value,
            meta_index,
            prop_index,
            merge_block,
            fallback_block,
        )?;
        cx.builder.switch_to_block(fallback_block);
        cx.builder.seal_block(fallback_block);
        lower_value_operation(
            cx,
            NativeRuntimeOp::SetProp,
            &[object, key, value],
            Some(dest),
        )?;
        cx.builder.ins().jump(merge_block, &[]);
        cx.builder.switch_to_block(merge_block);
        cx.builder.seal_block(merge_block);
        return Ok(());
    }
    if let Some(slot) = tables.ic_slots.get(&dest).copied() {
        lower_set_prop_ic(
            cx,
            barrier_thunks,
            prop_access(tables, dest, object, key, slot),
            value,
        )
    } else {
        lower_value_operation(
            cx,
            NativeRuntimeOp::SetProp,
            &[object, key, value],
            Some(dest),
        )
    }
}

fn emit_init_object_literal_heap_value_guard(
    cx: &mut LoweringCx<'_, '_>,
    values: &[ValueId],
    slow_block: ir::Block,
) -> Result<()> {
    let box_base = i64::from_ne_bytes(value::BOX_BASE.to_ne_bytes());
    let mut check_block = cx
        .builder
        .current_block()
        .context("init_object_literal guard requires an active block")?;
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            cx.builder.switch_to_block(check_block);
            cx.builder.seal_block(check_block);
        }
        let stored = use_value_boxed(cx.builder, cx.variables, *value)?;
        let boxed_bits = cx.builder.ins().band_imm_s(stored, box_base);
        let is_heap_value =
            cx.builder
                .ins()
                .icmp_imm_s(ir::condcodes::IntCC::Equal, boxed_bits, box_base);
        let next_block = cx.builder.create_block();
        cx.builder
            .ins()
            .brif(is_heap_value, slow_block, &[], next_block, &[]);
        check_block = next_block;
    }
    cx.builder.switch_to_block(check_block);
    cx.builder.seal_block(check_block);
    Ok(())
}

fn lower_init_object_literal(
    cx: &mut LoweringCx<'_, '_>,
    _tables: &BarrierThunks,
    constants: &[Constant],
    dest: ValueId,
    template: ConstantId,
    values: &[ValueId],
) -> Result<()> {
    let Some(meta_index) = object_template_meta_index(constants, template) else {
        bail!("init_object_literal template constant is invalid");
    };
    let Constant::ObjectTemplate { keys } = constants
        .get(usize::try_from(template.0).context("template index")?)
        .context("missing object template constant")?
    else {
        bail!("init_object_literal template constant is invalid");
    };
    if keys.len() != values.len() {
        bail!("init_object_literal value count mismatch");
    }
    let prop_count = u32::try_from(keys.len()).context("property count exceeds u32")?;

    let fast_block = cx.builder.create_block();
    let slow_block = cx.builder.create_block();
    let merge_block = cx.builder.create_block();
    cx.builder.append_block_param(merge_block, types::I64);

    let meta_count = cx.builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, object_template_meta_count))?,
    );
    let meta_index_i32 = cx.builder.ins().iconst(types::I32, i64::from(meta_index));
    let meta_ready = cx.builder.ins().icmp(
        ir::condcodes::IntCC::UnsignedLessThan,
        meta_index_i32,
        meta_count,
    );
    cx.builder
        .ins()
        .brif(meta_ready, fast_block, &[], slow_block, &[]);

    cx.builder.switch_to_block(fast_block);
    cx.builder.seal_block(fast_block);

    let shape_id = emit_load_object_template_meta_word(cx, meta_index, 0)?;
    let _slot_count = emit_load_object_template_meta_word(cx, meta_index, 1)?;
    let capacity = emit_load_object_template_meta_word(cx, meta_index, 2)?;

    let flags = cx.builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, allocation_fast_flags))?,
    );
    let enabled = cx.builder.ins().band_imm_u(
        flags,
        i64::from(wjsm_native_abi::NATIVE_ALLOCATION_FAST_OBJECT),
    );
    let enabled = cx
        .builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::NotEqual, enabled, 0);
    let small_limit = cx.builder.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, allocation_small_limit))?,
    );
    let slot_bytes = cx
        .builder
        .ins()
        .imul_imm_u(capacity, i64::from(constants::HEAP_OBJECT_VALUE_SLOT_SIZE));
    let header_bytes = cx
        .builder
        .ins()
        .iconst(types::I64, i64::from(constants::HEAP_OBJECT_HEADER_SIZE));
    let mut bytes_value = cx.builder.ins().iadd(slot_bytes, header_bytes);
    bytes_value = cx.builder.ins().iadd_imm_u(bytes_value, 7);
    bytes_value = cx.builder.ins().band_imm_s(bytes_value, !7);
    let small = cx.builder.ins().icmp(
        ir::condcodes::IntCC::UnsignedLessThanOrEqual,
        bytes_value,
        small_limit,
    );
    let top = cx.builder.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, bump_ptr))?,
    );
    let limit = cx.builder.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, bump_limit))?,
    );
    let end = cx.builder.ins().iadd(top, bytes_value);
    let object_fits =
        cx.builder
            .ins()
            .icmp(ir::condcodes::IntCC::UnsignedLessThanOrEqual, end, limit);
    let cursor = cx.builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, bump_handle_cursor))?,
    );
    let handle_limit = cx.builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, bump_handle_limit))?,
    );
    let handle_fits =
        cx.builder
            .ins()
            .icmp(ir::condcodes::IntCC::UnsignedLessThan, cursor, handle_limit);
    let mut ready = cx.builder.ins().band(enabled, small);
    ready = cx.builder.ins().band(ready, object_fits);
    ready = cx.builder.ins().band(ready, handle_fits);
    let fast_alloc_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(ready, fast_alloc_block, &[], slow_block, &[]);

    cx.builder.switch_to_block(fast_alloc_block);
    cx.builder.seal_block(fast_alloc_block);
    emit_init_object_literal_heap_value_guard(cx, values, slow_block)?;
    let prototype = cx.builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, object_prototype_handle))?,
    );
    let prototype = cx.builder.ins().uextend(types::I64, prototype);
    let type_word = cx.builder.ins().iconst(
        types::I64,
        i64::try_from(u64::from(wjsm_ir::HEAP_TYPE_OBJECT) << 32).expect("heap type word fits i64"),
    );
    let header_word = cx.builder.ins().bor(prototype, type_word);
    let heap_delta = cx.builder.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, heap_object_delta))?,
    );
    let address = cx.builder.ins().iadd(top, heap_delta);
    cx.builder
        .ins()
        .store(MemFlagsData::trusted(), header_word, address, 0);
    let capacity_i32 = cx.builder.ins().ireduce(types::I32, capacity);
    let shape_i32 = cx.builder.ins().ireduce(types::I32, shape_id);
    cx.builder.ins().store(
        MemFlagsData::trusted(),
        capacity_i32,
        address,
        i32::try_from(constants::HEAP_OBJECT_VALUE_CAPACITY_OFFSET)
            .expect("capacity offset fits i32"),
    );
    cx.builder.ins().store(
        MemFlagsData::trusted(),
        shape_i32,
        address,
        i32::try_from(constants::HEAP_OBJECT_SHAPE_ID_OFFSET).expect("shape offset fits i32"),
    );
    let handle_value = cx.builder.ins().uextend(types::I64, cursor);
    cx.builder.ins().store(
        MemFlagsData::trusted(),
        handle_value,
        address,
        i32::try_from(constants::HEAP_OBJECT_GC_WORD_OFFSET).expect("GC word offset fits i32"),
    );
    let handle_table = cx.ht_base;
    let entry_offset = cx.builder.ins().ishl_imm_u(handle_value, 3);
    let entry_address = cx.builder.ins().iadd(handle_table, entry_offset);
    let object_bits = cx.builder.ins().ishl_imm_u(top, 16);
    let stable_state = cx
        .builder
        .ins()
        .iconst(types::I64, i64::from(constants::HANDLE_STATE_STABLE_YOUNG));
    let entry_value = cx.builder.ins().bor(object_bits, stable_state);
    cx.builder
        .ins()
        .atomic_store(MemFlagsData::trusted(), entry_value, entry_address);
    cx.builder.ins().store(
        MemFlagsData::trusted(),
        end,
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, bump_ptr))?,
    );
    let next_handle = cx.builder.ins().iadd_imm_u(cursor, 1);
    cx.builder.ins().store(
        MemFlagsData::trusted(),
        next_handle,
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, bump_handle_cursor))?,
    );
    let logical_addr = top;
    for property in 0..prop_count {
        let stored = use_value_boxed(
            cx.builder,
            cx.variables,
            values[usize::try_from(property).expect("property index fits usize")],
        )?;
        lower_create_data_property_fast(cx, logical_addr, heap_delta, property, stored)?;
    }
    let value_prefix = cx.builder.ins().iconst(
        types::I64,
        i64::from_ne_bytes((value::BOX_BASE | (value::TAG_OBJECT << 32)).to_ne_bytes()),
    );
    let encoded = cx.builder.ins().bor(handle_value, value_prefix);
    cx.builder
        .ins()
        .jump(merge_block, &[ir::BlockArg::Value(encoded)]);

    cx.builder.switch_to_block(slow_block);
    cx.builder.seal_block(slow_block);
    let mut call_args = vec![cx.builder.ins().iconst(types::I64, i64::from(meta_index))];
    for value in values {
        call_args.push(use_value_boxed(cx.builder, cx.variables, *value)?);
    }
    let slow = cx.call(NativeRuntimeOp::InitObjectLiteral.id(), &call_args, None)?;
    cx.builder
        .ins()
        .jump(merge_block, &[ir::BlockArg::Value(slow)]);

    cx.builder.switch_to_block(merge_block);
    cx.builder.seal_block(merge_block);
    define_value_boxed(
        cx.builder,
        cx.variables,
        dest,
        cx.builder.block_params(merge_block)[0],
    )
}

fn lower_instruction(
    cx: &mut LoweringCx<'_, '_>,
    tables: &mut InstructionTables<'_>,
    instruction: &Instruction,
    roots: &[ValueId],
    feedback_ptr: Option<ir::Value>,
) -> Result<()> {
    match instruction {
        Instruction::Const {
            dest,
            constant: constant_id,
        } => {
            let constant_index =
                usize::try_from(constant_id.0).context("constant index does not fit usize")?;
            let constant = tables
                .constants
                .get(constant_index)
                .with_context(|| format!("constant {} is missing", constant_id.0))?;
            // typed 目标直接物化成浮点常量，省掉「iconst + bitcast」这对指令。
            if let Constant::Number(number) = constant
                && cx.variables.is_typed_value(*dest)
            {
                let canonical = f64::from_bits(value::encode_f64(*number) as u64);
                let native = cx.builder.ins().f64const(canonical);
                return define_value_f64(cx.builder, cx.variables, *dest, native);
            }
            let native = match constant {
                Constant::Number(value) => cx
                    .builder
                    .ins()
                    .iconst(types::I64, value::encode_f64(*value)),
                Constant::Bool(value) => cx
                    .builder
                    .ins()
                    .iconst(types::I64, value::encode_bool(*value)),
                Constant::Null => cx.builder.ins().iconst(types::I64, value::encode_null()),
                Constant::Undefined => cx
                    .builder
                    .ins()
                    .iconst(types::I64, value::encode_undefined()),
                Constant::Uninitialized => cx
                    .builder
                    .ins()
                    .iconst(types::I64, value::encode_uninitialized()),
                Constant::FunctionRef(function) => {
                    let index = cx.builder.ins().iconst(types::I64, i64::from(function.0));
                    cx.call(NativeRuntimeOp::MaterializeFunction.id(), &[index], None)?
                }
                Constant::NativeCallableEval => cx
                    .builder
                    .ins()
                    .iconst(types::I64, value::encode_native_callable_idx(0)),
                Constant::ModuleId(module) => cx
                    .builder
                    .ins()
                    .iconst(types::I64, value::encode_f64(f64::from(module.0))),
                Constant::String(_) | Constant::BigInt(_) => tables
                    .hoisted_constants
                    .get(constant_id)
                    .copied()
                    .context("immutable constant was not hoisted")?,
                Constant::ArrayTemplate(_) => {
                    bail!("array templates are materialized by clone_array_template")
                }
                Constant::ObjectTemplate { .. } => {
                    bail!("object templates are materialized by init_object_literal")
                }
                Constant::RegExp { .. } => {
                    let index = cx
                        .builder
                        .ins()
                        .iconst(types::I64, i64::from(constant_id.0));
                    let result =
                        cx.call(NativeRuntimeOp::MaterializeRegExp.id(), &[index], None)?;
                    return_if_exception(cx.builder, result, cx.root_frame.as_deref_mut(), cx.ctx)?;
                    result
                }
            };
            define_value_boxed(cx.builder, cx.variables, *dest, native)
        }
        Instruction::Binary { dest, op, lhs, rhs }
            if tables.speculative
                && tables.int32_values.contains(dest)
                && matches!(op, BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul) =>
        {
            let lhs_val = use_value_f64(cx.builder, cx.variables, *lhs)?;
            let rhs_val = use_value_f64(cx.builder, cx.variables, *rhs)?;
            let result = emit_i32_arithmetic(cx, *op, lhs_val, rhs_val)?;
            define_value_f64(cx.builder, cx.variables, *dest, result)
        }
        Instruction::Binary { dest, op, lhs, rhs }
            if tables.f64_values.contains(dest)
                && matches!(
                    op,
                    BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div
                ) =>
        {
            let lhs = use_value_f64(cx.builder, cx.variables, *lhs)?;
            let rhs = use_value_f64(cx.builder, cx.variables, *rhs)?;
            let result = match op {
                BinaryOp::Add => cx.builder.ins().fadd(lhs, rhs),
                BinaryOp::Sub => cx.builder.ins().fsub(lhs, rhs),
                BinaryOp::Mul => cx.builder.ins().fmul(lhs, rhs),
                BinaryOp::Div => cx.builder.ins().fdiv(lhs, rhs),
                _ => unreachable!("guard restricts direct f64 operations"),
            };
            if cx.variables.is_typed_value(*dest) {
                return define_value_f64(cx.builder, cx.variables, *dest, result);
            }
            let result = box_f64_arithmetic(cx.builder, *op, result);
            define_value_boxed(cx.builder, cx.variables, *dest, result)
        }
        Instruction::Binary { dest, op, lhs, rhs } => {
            lower_dynamic_binary(cx, *dest, *op, *lhs, *rhs, feedback_ptr, tables.f64_values)
        }
        Instruction::Unary { dest, op, value } => {
            if tables.f64_values.contains(dest) && matches!(op, UnaryOp::Neg | UnaryOp::Pos) {
                if *op == UnaryOp::Neg {
                    let input = use_value_f64(cx.builder, cx.variables, *value)?;
                    let result = cx.builder.ins().fneg(input);
                    return define_value_f64(cx.builder, cx.variables, *dest, result);
                }
                // 一元 `+` 对已证明 number 是恒等运算，按目标表示原样搬运即可。
                let dest_is_typed = cx.variables.is_typed_value(*dest);
                let native = use_value_as(cx.builder, cx.variables, dest_is_typed, *value)?;
                define_value_as(cx.builder, cx.variables, *dest, native)
            } else {
                let operation = DYNAMIC_UNARY_BASE + u32::from(unary_tag(*op));
                let input = use_value_boxed(cx.builder, cx.variables, *value)?;
                let result = cx.call(operation, &[input], feedback_ptr)?;
                define_value_boxed(cx.builder, cx.variables, *dest, result)
            }
        }
        Instruction::Compare { dest, op, lhs, rhs } if op.is_relational() => {
            if tables.speculative
                && tables.int32_values.contains(lhs)
                && tables.int32_values.contains(rhs)
            {
                let lhs_val = use_value_f64(cx.builder, cx.variables, *lhs)?;
                let rhs_val = use_value_f64(cx.builder, cx.variables, *rhs)?;
                let result = emit_i32_relational(cx.builder, lhs_val, rhs_val, *op)?;
                define_value_boxed(cx.builder, cx.variables, *dest, result)
            } else if tables.f64_values.contains(lhs) && tables.f64_values.contains(rhs) {
                let lhs_val = use_value_f64(cx.builder, cx.variables, *lhs)?;
                let rhs_val = use_value_f64(cx.builder, cx.variables, *rhs)?;
                let result = emit_f64_relational(cx.builder, lhs_val, rhs_val, *op);
                define_value_boxed(cx.builder, cx.variables, *dest, result)
            } else {
                let lhs_val = use_value_boxed(cx.builder, cx.variables, *lhs)?;
                let rhs_val = use_value_boxed(cx.builder, cx.variables, *rhs)?;
                let reverse = matches!(*op, CompareOp::Gt | CompareOp::LtEq);
                let invert = matches!(*op, CompareOp::LtEq | CompareOp::GtEq);
                let reverse_v = cx
                    .builder
                    .ins()
                    .iconst(types::I64, value::encode_bool(reverse));
                let invert_v = cx
                    .builder
                    .ins()
                    .iconst(types::I64, value::encode_bool(invert));
                let result = cx.call(
                    u32::from(Builtin::AbstractCompare.wire_id()),
                    &[lhs_val, rhs_val, reverse_v, invert_v],
                    feedback_ptr,
                )?;
                define_value_boxed(cx.builder, cx.variables, *dest, result)
            }
        }
        Instruction::Compare { dest, op, lhs, rhs } => {
            let operation = DYNAMIC_COMPARE_BASE + u32::from(compare_tag(*op));
            lower_strict_eq(
                cx,
                tables.barrier_thunks,
                *dest,
                *lhs,
                *rhs,
                StrictEqMode {
                    slow_operation: operation,
                    invert: *op == CompareOp::StrictNotEq,
                },
                feedback_ptr,
            )
        }
        Instruction::CallBuiltin {
            dest: Some(dest),
            builtin: Builtin::AbstractCompare,
            args,
        } if args.len() == 4 => {
            let reverse = use_value_boxed(cx.builder, cx.variables, args[2])?;
            let invert = use_value_boxed(cx.builder, cx.variables, args[3])?;
            if tables.f64_values.contains(&args[0]) && tables.f64_values.contains(&args[1]) {
                let lhs = use_value_f64(cx.builder, cx.variables, args[0])?;
                let rhs = use_value_f64(cx.builder, cx.variables, args[1])?;
                let result = emit_f64_abstract_compare(cx.builder, lhs, rhs, reverse, invert);
                define_value_boxed(cx.builder, cx.variables, *dest, result)?;
            } else {
                let lhs = use_value_boxed(cx.builder, cx.variables, args[0])?;
                let rhs = use_value_boxed(cx.builder, cx.variables, args[1])?;
                let box_base = i64::from_ne_bytes(value::BOX_BASE.to_ne_bytes());
                let lhs_masked = cx.builder.ins().band_imm_s(lhs, box_base);
                let lhs_is_f64 = cx.builder.ins().icmp_imm_s(
                    ir::condcodes::IntCC::NotEqual,
                    lhs_masked,
                    box_base,
                );
                let rhs_masked = cx.builder.ins().band_imm_s(rhs, box_base);
                let rhs_is_f64 = cx.builder.ins().icmp_imm_s(
                    ir::condcodes::IntCC::NotEqual,
                    rhs_masked,
                    box_base,
                );
                let both_f64 = cx.builder.ins().band(lhs_is_f64, rhs_is_f64);

                let fast_block = cx.builder.create_block();
                let slow_block = cx.builder.create_block();
                let merge_block = cx.builder.create_block();
                cx.builder.append_block_param(merge_block, types::I64);

                cx.builder
                    .ins()
                    .brif(both_f64, fast_block, &[], slow_block, &[]);

                cx.builder.switch_to_block(fast_block);
                cx.builder.seal_block(fast_block);
                let lhs_f64 = unbox_f64(cx.builder, lhs);
                let rhs_f64 = unbox_f64(cx.builder, rhs);
                let fast_result =
                    emit_f64_abstract_compare(cx.builder, lhs_f64, rhs_f64, reverse, invert);
                cx.builder
                    .ins()
                    .jump(merge_block, &[ir::BlockArg::Value(fast_result)]);

                cx.builder.switch_to_block(slow_block);
                cx.builder.seal_block(slow_block);
                let slow_result = cx.call(
                    u32::from(Builtin::AbstractCompare.wire_id()),
                    &[lhs, rhs, reverse, invert],
                    feedback_ptr,
                )?;
                cx.builder
                    .ins()
                    .jump(merge_block, &[ir::BlockArg::Value(slow_result)]);

                cx.builder.switch_to_block(merge_block);
                cx.builder.seal_block(merge_block);
                let result = cx.builder.block_params(merge_block)[0];
                define_value_boxed(cx.builder, cx.variables, *dest, result)?;
            }
            Ok(())
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
        } if tables.f64_values.contains(dest) && args.len() == 1 => {
            let input = use_value_f64(cx.builder, cx.variables, args[0])?;
            let result = match builtin {
                Builtin::MathAbs => cx.builder.ins().fabs(input),
                Builtin::MathSqrt => cx.builder.ins().sqrt(input),
                Builtin::MathCeil => cx.builder.ins().ceil(input),
                Builtin::MathFloor => cx.builder.ins().floor(input),
                Builtin::MathTrunc => cx.builder.ins().trunc(input),
                Builtin::MathFround => {
                    let narrowed = cx.builder.ins().fdemote(types::F32, input);
                    cx.builder.ins().fpromote(types::F64, narrowed)
                }
                _ => unreachable!("arm 模式已限定这六个 builtin"),
            };
            define_value_f64(cx.builder, cx.variables, *dest, result)
        }
        Instruction::CallBuiltin {
            dest: Some(dest),
            builtin:
                builtin @ (Builtin::StringCharCodeAt | Builtin::StringCharAt | Builtin::StringAt),
            args,
        } if matches!(args.len(), 1 | 2) => lower_string_char_builtin(
            cx,
            tables.barrier_thunks,
            *dest,
            *builtin,
            args,
            feedback_ptr,
        ),
        Instruction::CallBuiltin {
            dest: Some(dest),
            builtin: Builtin::IsString,
            args,
        } if args.len() == 1 => {
            let encoded = use_value_boxed(cx.builder, cx.variables, args[0])?;
            let inline = emit_inline_string_predicate(cx.builder, encoded);
            let box_base = i64::from_ne_bytes(value::BOX_BASE.to_ne_bytes());
            let boxed = cx.builder.ins().band_imm_s(encoded, box_base);
            let boxed = cx
                .builder
                .ins()
                .icmp_imm_s(ir::condcodes::IntCC::Equal, boxed, box_base);
            let tag = cx.builder.ins().ushr_imm_u(encoded, 32);
            let tag = cx.builder.ins().band_imm_u(
                tag,
                i64::try_from(value::TAG_MASK).expect("tag mask fits i64"),
            );
            let is_string = cx.builder.ins().icmp_imm_u(
                ir::condcodes::IntCC::Equal,
                tag,
                i64::try_from(value::TAG_STRING).expect("string tag fits i64"),
            );
            let tag_word = cx.builder.ins().ushr_imm_u(encoded, 32);
            let runtime_flag = cx.builder.ins().band_imm_u(
                tag_word,
                i64::try_from(value::STRING_RUNTIME_HANDLE_FLAG).expect("runtime flag fits i64"),
            );
            let is_runtime =
                cx.builder
                    .ins()
                    .icmp_imm_u(ir::condcodes::IntCC::NotEqual, runtime_flag, 0);
            let valid_handle = cx.builder.ins().band(boxed, is_string);
            let valid_handle = cx.builder.ins().band(valid_handle, is_runtime);
            let valid = cx.builder.ins().bor(inline, valid_handle);
            let yes = cx
                .builder
                .ins()
                .iconst(types::I64, value::encode_bool(true));
            let no = cx
                .builder
                .ins()
                .iconst(types::I64, value::encode_bool(false));
            let result = cx.builder.ins().select(valid, yes, no);
            define_value_boxed(cx.builder, cx.variables, *dest, result)
        }
        Instruction::CallBuiltin {
            dest: Some(dest),
            builtin: Builtin::StrictEq,
            args,
        } if args.len() == 2 => lower_strict_eq(
            cx,
            tables.barrier_thunks,
            *dest,
            args[0],
            args[1],
            StrictEqMode {
                slow_operation: u32::from(Builtin::StrictEq.wire_id()),
                invert: false,
            },
            feedback_ptr,
        ),
        // 非逃逸累加器追加：JIT 内联写入 payload 并就地更新 length；最后片段
        // 按运行时类型分派字符串直拷 / 安全整数 itoa，容量不足、builder 首建
        // 或其余形态回落宿主 thunk。
        Instruction::CallBuiltin {
            dest: Some(dest),
            builtin: Builtin::StringBuilderAppend,
            args,
        } if args.len() >= 2 => lower_string_builder_append(cx, *dest, args, feedback_ptr),
        Instruction::CallBuiltin {
            dest,
            builtin: Builtin::StringBuilderFinish,
            args,
        } if args.len() == 1 => {
            let builder = use_value_boxed(cx.builder, cx.variables, args[0])?;
            cx.flush()?;
            let call = cx
                .builder
                .ins()
                .call(cx.string_builder_finish, &[cx.ctx, builder]);
            if let Some(dest) = dest {
                let result = cx.builder.inst_results(call)[0];
                define_value_boxed(cx.builder, cx.variables, *dest, result)?;
            }
            Ok(())
        }
        // 已证明 f64 的 21 个 libm Math builtin：typed native direct call。
        // guard 即类型检查——实参未证明 f64 时落入下方 dispatcher 路径，
        // 保留 to_number_coerced 与 BigInt TypeError 语义。
        Instruction::CallBuiltin {
            dest: Some(dest),
            builtin,
            args,
        } if tables.f64_values.contains(dest)
            && NativeHostSymbol::for_builtin(*builtin).is_some_and(|symbol| {
                args.len() == usize::from(symbol.signature().argument_count())
            }) =>
        {
            let symbol = NativeHostSymbol::for_builtin(*builtin)
                .context("guard 已限制为 math thunk builtin")?;
            let thunk = import_math_thunk(
                cx.builder,
                tables.math_thunks,
                tables.imported_math_thunks,
                *builtin,
            )?;
            let result = match symbol.signature() {
                NativeSignature::F64Unary => {
                    let input = use_value_f64(cx.builder, cx.variables, args[0])?;
                    let call = cx.builder.ins().call(thunk, &[input]);
                    *cx.builder
                        .inst_results(call)
                        .first()
                        .context("typed math thunk returned no result")?
                }
                NativeSignature::F64Binary => {
                    let lhs = use_value_f64(cx.builder, cx.variables, args[0])?;
                    let rhs = use_value_f64(cx.builder, cx.variables, args[1])?;
                    let call = cx.builder.ins().call(thunk, &[lhs, rhs]);
                    *cx.builder
                        .inst_results(call)
                        .first()
                        .context("typed math thunk returned no result")?
                }
                NativeSignature::HostOperation
                | NativeSignature::ValueBinary
                | NativeSignature::ValueUnary
                | NativeSignature::ValueTernary
                | NativeSignature::ValueBinaryF64
                | NativeSignature::ZgcLoadBarrier
                | NativeSignature::ZgcStoreBarrier => {
                    unreachable!("math thunk 不存在 host 或 ZGC 屏障签名")
                }
            };
            define_value_f64(cx.builder, cx.variables, *dest, result)
        }
        Instruction::CallBuiltin {
            dest: Some(dest),
            builtin: Builtin::StringSlice,
            args,
        } if !args.is_empty() => lower_string_slice_builtin(cx, *dest, args, feedback_ptr),
        Instruction::CallBuiltin {
            dest: None,
            builtin: Builtin::ArrayPush,
            args,
        } if args.len() == 2 => {
            lower_array_push_inline(cx, tables.barrier_thunks, args[0], args[1])
        }
        Instruction::CallBuiltin {
            dest,
            builtin,
            args,
        } => {
            let mut values = Vec::with_capacity(args.len());
            for arg in args {
                values.push(use_value_boxed(cx.builder, cx.variables, *arg)?);
            }
            let result = cx.call(u32::from(builtin.wire_id()), &values, feedback_ptr)?;
            if let Some(dest) = dest {
                define_value_boxed(cx.builder, cx.variables, *dest, result)?;
            }
            Ok(())
        }
        Instruction::Call {
            dest,
            callee,
            this_val,
            args,
        } => {
            let direct_callee = tables
                .constant_defs
                .get(callee)
                .and_then(|c| tables.constants.get(c.0 as usize))
                .and_then(|c| match c {
                    Constant::FunctionRef(target) => Some(*target),
                    _ => None,
                });
            if let Some(target) = direct_callee
                && tables.direct_callable_functions.contains(&target)
                && let Some(decl) = tables.function_decls.get(target.0 as usize)
            {
                let func_ref = *tables
                    .imported_function_decls
                    .entry(target)
                    .or_insert_with(|| decl.import(cx.builder.func));
                if let Some(arity) = fast_js_arity(decl.signature()) {
                    lower_fast_direct_call_instruction(
                        cx, func_ref, *dest, *this_val, args, roots, arity,
                    )
                } else {
                    lower_direct_call_instruction(cx, func_ref, *dest, *this_val, args, roots)
                }
            } else {
                lower_call_instruction(
                    cx,
                    tables.slow_call_signature,
                    CallLowering {
                        destination: *dest,
                        callee: *callee,
                        this_value: *this_val,
                        args,
                        operation: NativeRuntimeOp::PrepareCall,
                        forward_args: false,
                    },
                    roots,
                    feedback_ptr,
                )
            }
        }
        Instruction::SuperCall {
            dest,
            callee,
            this_val,
            args,
            forward_args,
        } => lower_call_instruction(
            cx,
            tables.slow_call_signature,
            CallLowering {
                destination: *dest,
                callee: *callee,
                this_value: *this_val,
                args,
                operation: if *forward_args {
                    NativeRuntimeOp::PrepareSuperCallForward
                } else {
                    NativeRuntimeOp::PrepareSuperCall
                },
                forward_args: *forward_args,
            },
            roots,
            feedback_ptr,
        ),
        Instruction::ConstructCall {
            dest,
            callee,
            this_val,
            args,
        } => lower_call_instruction(
            cx,
            tables.slow_call_signature,
            CallLowering {
                destination: *dest,
                callee: *callee,
                this_value: *this_val,
                args,
                operation: NativeRuntimeOp::PrepareConstruct,
                forward_args: false,
            },
            roots,
            feedback_ptr,
        ),
        Instruction::OptionalCall {
            dest,
            callee,
            this_val,
            args,
        } => lower_optional_call_instruction(
            cx,
            tables.slow_call_signature,
            CallLowering {
                destination: Some(*dest),
                callee: *callee,
                this_value: *this_val,
                args,
                operation: NativeRuntimeOp::PrepareCall,
                forward_args: false,
            },
            roots,
            feedback_ptr,
        ),
        Instruction::StringConcatVa { dest, parts } => {
            lower_value_operation(cx, NativeRuntimeOp::StringConcat, parts, Some(*dest))
        }
        Instruction::NewPromise { dest } => {
            lower_native_object_allocation(cx, *dest, 2, false)?;
            let object = use_value_boxed(cx.builder, cx.variables, *dest)?;
            let initialized = cx.call(NativeRuntimeOp::InitPromise.id(), &[object], None)?;
            define_value_boxed(cx.builder, cx.variables, *dest, initialized)
        }
        Instruction::NewObject { dest, capacity } => {
            lower_native_object_allocation(cx, *dest, *capacity, false)
        }
        Instruction::GetProp { dest, object, key } => lower_get_prop_with_template_or_ic(
            cx,
            tables,
            tables.barrier_thunks,
            *dest,
            *object,
            *key,
            roots,
        ),
        Instruction::SetProp {
            dest,
            object,
            key,
            value,
        } => lower_set_prop_with_template_or_ic(
            cx,
            tables,
            tables.barrier_thunks,
            *dest,
            *object,
            *key,
            *value,
        ),
        Instruction::CreateDataProperty {
            dest,
            object,
            key,
            value,
        } => lower_value_operation(
            cx,
            NativeRuntimeOp::CreateDataProperty,
            &[*object, *key, *value],
            Some(*dest),
        ),
        Instruction::DeleteProp { dest, object, key } => lower_value_operation(
            cx,
            NativeRuntimeOp::DeleteProp,
            &[*object, *key],
            Some(*dest),
        ),
        Instruction::SetProto { object, value } => {
            lower_value_operation(cx, NativeRuntimeOp::SetProto, &[*object, *value], None)
        }
        Instruction::NewArray { dest, capacity } => {
            lower_native_object_allocation(cx, *dest, *capacity, true)
        }
        Instruction::CloneArrayTemplate { dest, template } => {
            let template = cx.builder.ins().iconst(types::I64, i64::from(template.0));
            let result = cx.call(NativeRuntimeOp::CloneArrayTemplate.id(), &[template], None)?;
            define_value_boxed(cx.builder, cx.variables, *dest, result)
        }
        Instruction::InitObjectLiteral {
            dest,
            template,
            values,
        } => lower_init_object_literal(
            cx,
            tables.barrier_thunks,
            tables.constants,
            *dest,
            *template,
            values,
        ),
        Instruction::GetElem {
            dest,
            object,
            index,
        } => lower_string_element(cx, tables.barrier_thunks, *dest, *object, *index, None),
        Instruction::ElemShapeGuard {
            dest,
            array,
            template,
        } => lower_elem_shape_guard(cx, tables.constants, *dest, *array, *template),
        Instruction::GetElemGuarded {
            dest,
            object,
            index,
            guard,
        } => lower_string_element(
            cx,
            tables.barrier_thunks,
            *dest,
            *object,
            *index,
            Some(*guard),
        ),
        Instruction::GetPropGuarded {
            dest,
            object,
            key,
            guard,
            template,
        } => lower_get_prop_guarded(
            cx,
            tables,
            GuardedPropAccess {
                dest: *dest,
                object: *object,
                key: *key,
                guard: *guard,
                template: *template,
            },
            roots,
        ),
        Instruction::SetElem {
            dest,
            object,
            index,
            value,
        } => lower_set_elem(cx, tables.barrier_thunks, *dest, *object, *index, *value),
        Instruction::OptionalGetProp { dest, object, key } => {
            if let Some(slot) = tables.ic_slots.get(dest).copied() {
                lower_optional_get_prop_ic(
                    cx,
                    tables.barrier_thunks,
                    prop_access(tables, *dest, *object, *key, slot),
                    roots,
                )
            } else {
                lower_value_operation(
                    cx,
                    NativeRuntimeOp::OptionalGetProp,
                    &[*object, *key],
                    Some(*dest),
                )
            }
        }
        Instruction::OptionalGetElem { dest, object, key } => lower_value_operation(
            cx,
            NativeRuntimeOp::OptionalGetElem,
            &[*object, *key],
            Some(*dest),
        ),
        Instruction::GetSuperBase { dest } => {
            let result = cx.call(NativeRuntimeOp::GetSuperBase.id(), &[], None)?;
            define_value_boxed(cx.builder, cx.variables, *dest, result)
        }
        Instruction::GetSuperConstructor { dest } => {
            let result = cx.call(NativeRuntimeOp::GetSuperConstructor.id(), &[], None)?;
            define_value_boxed(cx.builder, cx.variables, *dest, result)
        }
        Instruction::ObjectSpread {
            dest,
            object,
            source,
        } => lower_value_operation(
            cx,
            NativeRuntimeOp::ObjectSpread,
            &[*object, *source],
            // 结果槽：成功为 object，getter/Proxy 抛错为 TAG_EXCEPTION，
            // 丢弃它会吞掉 CopyDataProperties 的异常。
            Some(*dest),
        ),
        Instruction::GuardSameFunction {
            dest,
            callee,
            function,
        } => {
            let callee = use_value_boxed(cx.builder, cx.variables, *callee)?;
            let function = cx.builder.ins().iconst(types::I64, i64::from(function.0));
            let result = cx.call(
                NativeRuntimeOp::GuardSameFunction.id(),
                &[callee, function],
                None,
            )?;
            define_value_boxed(cx.builder, cx.variables, *dest, result)
        }
        Instruction::CollectRestArgs { dest, skip } => {
            let skip = cx.builder.ins().iconst(types::I64, i64::from(*skip));
            let result = cx.call(NativeRuntimeOp::CollectRestArguments.id(), &[skip], None)?;
            define_value_boxed(cx.builder, cx.variables, *dest, result)
        }
        Instruction::IsException { dest, value: input } => {
            let input = use_value_boxed(cx.builder, cx.variables, *input)?;
            let condition = emit_is_exception(cx.builder, input);
            let true_value = cx
                .builder
                .ins()
                .iconst(types::I64, value::encode_bool(true));
            let false_value = cx
                .builder
                .ins()
                .iconst(types::I64, value::encode_bool(false));
            let boolean = cx.builder.ins().select(condition, true_value, false_value);
            define_value_boxed(cx.builder, cx.variables, *dest, boolean)
        }
        Instruction::EncodeException { dest, value: input } => {
            let input = use_value_boxed(cx.builder, cx.variables, *input)?;
            let result = cx.call(NativeRuntimeOp::CreateException.id(), &[input], None)?;
            define_value_boxed(cx.builder, cx.variables, *dest, result)
        }
        Instruction::PromiseResolve { promise, value } => lower_builtin_operation(
            cx,
            Builtin::PromiseInstanceResolve,
            &[*promise, *value],
            None,
        ),
        Instruction::PromiseReject { promise, reason } => lower_builtin_operation(
            cx,
            Builtin::PromiseInstanceReject,
            &[*promise, *reason],
            None,
        ),
        Instruction::ExceptionToObject { dest, value: input } => {
            let input = use_value_boxed(cx.builder, cx.variables, *input)?;
            let result = cx.call(NativeRuntimeOp::ExceptionValue.id(), &[input], None)?;
            define_value_boxed(cx.builder, cx.variables, *dest, result)
        }
        Instruction::StoreVar { name, value } => {
            if let Some(local) = tables.frame_locals.get(name).copied() {
                // typed 局部与 typed 源同为浮点表示时这里不产出任何转换指令，
                // 归纳变量的回写因此留在浮点寄存器内。
                let typed_local = cx.variables.is_typed_local(name);
                let native = use_value_as(cx.builder, cx.variables, typed_local, *value)?;
                cx.builder.def_var(local, native);
                if let Some(index) = tables.frame_local_indices.get(name).copied() {
                    cx.update_pinned_local(index, native)?;
                }
                return Ok(());
            }
            let value = use_value_boxed(cx.builder, cx.variables, *value)?;
            let slot = tables
                .variable_slots
                .get(name)
                .copied()
                .with_context(|| format!("variable slot is missing for {name}"))?;
            let slot = cx.builder.ins().iconst(types::I64, i64::from(slot));
            let _ = cx.call(NativeRuntimeOp::StoreVar.id(), &[slot, value], None)?;
            Ok(())
        }
        Instruction::LoadVar { dest, name } => {
            if let Some(local) = tables.frame_locals.get(name).copied() {
                let typed_local = cx.variables.is_typed_local(name);
                let value = cx.builder.use_var(local);
                return if typed_local {
                    define_value_f64(cx.builder, cx.variables, *dest, value)
                } else {
                    define_value_boxed(cx.builder, cx.variables, *dest, value)
                };
            }
            let slot = tables
                .variable_slots
                .get(name)
                .copied()
                .with_context(|| format!("variable slot is missing for {name}"))?;
            let slot = cx.builder.ins().iconst(types::I64, i64::from(slot));
            let value = cx.call(NativeRuntimeOp::LoadVar.id(), &[slot], None)?;
            define_value_boxed(cx.builder, cx.variables, *dest, value)
        }

        Instruction::Suspend { promise, state } => {
            let promise = use_value_boxed(cx.builder, cx.variables, *promise)?;
            let suspend_state = cx
                .builder
                .ins()
                .iconst(types::I64, value::encode_f64(f64::from(*state)));
            let result = cx.call(
                Builtin::AsyncFunctionSuspend.wire_id().into(),
                &[promise, suspend_state],
                None,
            )?;
            cx.unlink_roots()?;
            cx.builder.ins().return_(&[result]);
            Ok(())
        }
        Instruction::GeneratorSuspend { result, state } => {
            let result = use_value_boxed(cx.builder, cx.variables, *result)?;
            let continuation = cx.call(NativeRuntimeOp::LoadCallEnv.id(), &[], None)?;
            cx.publish_roots(roots, &[continuation])?;
            let slot = cx.builder.ins().iconst(types::I64, value::encode_f64(0.0));
            let suspend_state = cx
                .builder
                .ins()
                .iconst(types::I64, value::encode_f64(f64::from(*state)));
            let _ = cx.call(
                Builtin::ContinuationSaveVar.wire_id().into(),
                &[continuation, slot, suspend_state],
                None,
            )?;
            cx.unlink_roots()?;
            cx.builder.ins().return_(&[result]);
            Ok(())
        }
        Instruction::DebugCheck { line, col } => {
            let function = cx
                .builder
                .ins()
                .iconst(types::I64, i64::from(tables.function_index));
            let line = cx.builder.ins().iconst(types::I64, i64::from(*line));
            let col = cx.builder.ins().iconst(types::I64, i64::from(*col));
            let _ = cx.call(
                NativeRuntimeOp::DebugCheck.id(),
                &[function, line, col],
                None,
            )?;
            Ok(())
        }
        unsupported => bail!("native lowering does not yet own instruction {unsupported}"),
    }
}

fn lower_fast_direct_call_instruction(
    cx: &mut LoweringCx<'_, '_>,
    target: ir::FuncRef,
    destination: Option<ValueId>,
    this_value: ValueId,
    args: &[ValueId],
    roots: &[ValueId],
    arity: usize,
) -> Result<()> {
    let this_value = use_value_boxed(cx.builder, cx.variables, this_value)?;
    let undefined = cx
        .builder
        .ins()
        .iconst(types::I64, value::encode_undefined());
    let undefined_env = cx
        .builder
        .ins()
        .iconst(types::I64, value::encode_undefined());
    let mut call_args = Vec::with_capacity(3 + arity);
    call_args.push(cx.ctx);
    call_args.push(undefined_env);
    call_args.push(this_value);
    for index in 0..arity {
        if let Some(argument) = args.get(index) {
            call_args.push(use_value_boxed(cx.builder, cx.variables, *argument)?);
        } else {
            call_args.push(undefined);
        }
    }

    let depth_offset = i32::try_from(offset_of!(NativeVmContext, js_call_depth))
        .context("js call depth offset exceeds i32")?;
    let depth = cx
        .builder
        .ins()
        .load(types::I32, MemFlagsData::trusted(), cx.ctx, depth_offset);
    let new_depth = cx.builder.ins().iadd_imm_s(depth, 1);
    cx.builder
        .ins()
        .store(MemFlagsData::trusted(), new_depth, cx.ctx, depth_offset);

    cx.flush()?;
    let call = cx.builder.ins().call(target, &call_args);
    let result = cx.builder.inst_results(call)[0];

    cx.builder
        .ins()
        .store(MemFlagsData::trusted(), depth, cx.ctx, depth_offset);

    cx.publish_roots(roots, &[result])?;
    if let Some(destination) = destination {
        define_value_boxed(cx.builder, cx.variables, destination, result)?;
    }
    Ok(())
}

fn lower_direct_call_instruction(
    cx: &mut LoweringCx<'_, '_>,
    target: ir::FuncRef,
    destination: Option<ValueId>,
    this_value: ValueId,
    args: &[ValueId],
    roots: &[ValueId],
) -> Result<()> {
    let this_value = use_value_boxed(cx.builder, cx.variables, this_value)?;
    let active_len_offset = i32::try_from(offset_of!(NativeVmContext, call_arena_active_len))
        .context("call arena active length offset exceeds i32")?;
    let active_len = cx.builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        cx.ctx,
        active_len_offset,
    );
    let pointer_type = cx.builder.func.dfg.value_type(cx.ctx);
    let arena_base = cx.builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, call_arena_slots))?,
    );
    let active_len_u64 = cx.builder.ins().uextend(types::I64, active_len);
    let active_len_bytes = cx.builder.ins().ishl_imm_u(active_len_u64, 3);
    let base_addr = cx.builder.ins().iadd(arena_base, active_len_bytes);

    for (i, arg) in args.iter().enumerate() {
        let arg_val = use_value_boxed(cx.builder, cx.variables, *arg)?;
        let offset = i32::try_from(i * size_of::<i64>()).context("argument offset exceeds i32")?;
        cx.builder
            .ins()
            .store(MemFlagsData::trusted(), arg_val, base_addr, offset);
    }

    let args_len = u32::try_from(args.len()).context("args len exceeds u32")?;
    let new_active_len = cx.builder.ins().iadd_imm_s(active_len, i64::from(args_len));
    cx.builder.ins().store(
        MemFlagsData::trusted(),
        new_active_len,
        cx.ctx,
        active_len_offset,
    );

    let depth_offset = i32::try_from(offset_of!(NativeVmContext, js_call_depth))
        .context("js call depth offset exceeds i32")?;
    let depth = cx
        .builder
        .ins()
        .load(types::I32, MemFlagsData::trusted(), cx.ctx, depth_offset);
    let new_depth = cx.builder.ins().iadd_imm_s(depth, 1);
    cx.builder
        .ins()
        .store(MemFlagsData::trusted(), new_depth, cx.ctx, depth_offset);

    let undefined_env = cx
        .builder
        .ins()
        .iconst(types::I64, value::encode_undefined());
    let args_len_val = cx.builder.ins().iconst(types::I32, i64::from(args_len));

    cx.flush()?;
    let call = cx.builder.ins().call(
        target,
        &[cx.ctx, undefined_env, this_value, active_len, args_len_val],
    );
    let result = cx.builder.inst_results(call)[0];

    cx.builder
        .ins()
        .store(MemFlagsData::trusted(), depth, cx.ctx, depth_offset);
    cx.builder.ins().store(
        MemFlagsData::trusted(),
        active_len,
        cx.ctx,
        active_len_offset,
    );

    cx.publish_roots(roots, &[result])?;
    if let Some(destination) = destination {
        define_value_boxed(cx.builder, cx.variables, destination, result)?;
    }
    Ok(())
}

fn lower_call_instruction(
    cx: &mut LoweringCx<'_, '_>,
    slow_call_signature: ir::SigRef,
    call: CallLowering<'_>,
    roots: &[ValueId],
    feedback_ptr: Option<ir::Value>,
) -> Result<()> {
    let CallLowering {
        destination,
        callee,
        this_value,
        args,
        operation,
        forward_args,
    } = call;
    let callee = use_value_boxed(cx.builder, cx.variables, callee)?;
    let this_value = use_value_boxed(cx.builder, cx.variables, this_value)?;
    let mut call_args = Vec::with_capacity(if forward_args { 1 } else { args.len() + 1 });
    call_args.push(callee);
    if !forward_args {
        for argument in args {
            call_args.push(use_value_boxed(cx.builder, cx.variables, *argument)?);
        }
    }
    let entry = cx.call(operation.id(), &call_args, feedback_ptr)?;
    let args_len = if forward_args {
        let entry_block = cx
            .builder
            .func
            .layout
            .entry_block()
            .context("native function is missing entry block")?;
        cx.builder.block_params(entry_block)[4]
    } else {
        cx.builder.ins().iconst(
            types::I32,
            i64::try_from(args.len()).context("call argument count exceeds i64")?,
        )
    };
    let active_len = cx.builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        cx.ctx,
        i32::try_from(offset_of!(NativeVmContext, call_arena_active_len))
            .context("call arena active length offset exceeds i32")?,
    );
    let args_base = cx.builder.ins().isub(active_len, args_len);
    let env = cx.call(NativeRuntimeOp::LoadCallEnv.id(), &[], None)?;
    cx.flush()?;
    let call = cx.builder.ins().call_indirect(
        slow_call_signature,
        entry,
        &[cx.ctx, env, this_value, args_base, args_len],
    );
    let result = cx.builder.inst_results(call)[0];
    cx.publish_roots(roots, &[result])?;
    let _ = cx.call(NativeRuntimeOp::FinishCall.id(), &[], None)?;
    if let Some(destination) = destination {
        define_value_boxed(cx.builder, cx.variables, destination, result)?;
    }
    Ok(())
}

fn lower_optional_call_instruction(
    cx: &mut LoweringCx<'_, '_>,
    slow_call_signature: ir::SigRef,
    call: CallLowering<'_>,
    roots: &[ValueId],
    feedback_ptr: Option<ir::Value>,
) -> Result<()> {
    let CallLowering {
        destination,
        callee,
        this_value,
        args,
        ..
    } = call;
    let destination = destination.context("optional call requires a destination")?;
    let encoded_callee = use_value_boxed(cx.builder, cx.variables, callee)?;
    let nullish = cx.call(
        NativeRuntimeOp::UnaryIsNullish.id(),
        &[encoded_callee],
        None,
    )?;
    let is_nullish = cx.builder.ins().icmp_imm_s(
        ir::condcodes::IntCC::Equal,
        nullish,
        value::encode_bool(true),
    );
    let skip_block = cx.builder.create_block();
    let call_block = cx.builder.create_block();
    let continuation = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(is_nullish, skip_block, &[], call_block, &[]);

    cx.builder.switch_to_block(skip_block);
    cx.builder.seal_block(skip_block);
    let undefined = cx
        .builder
        .ins()
        .iconst(types::I64, value::encode_undefined());
    define_value_boxed(cx.builder, cx.variables, destination, undefined)?;
    cx.builder.ins().jump(continuation, &[]);

    cx.builder.switch_to_block(call_block);
    cx.builder.seal_block(call_block);
    lower_call_instruction(
        cx,
        slow_call_signature,
        CallLowering {
            destination: Some(destination),
            callee,
            this_value,
            args,
            operation: NativeRuntimeOp::PrepareCall,
            forward_args: false,
        },
        roots,
        feedback_ptr,
    )?;
    cx.builder.ins().jump(continuation, &[]);

    cx.builder.switch_to_block(continuation);
    cx.builder.seal_block(continuation);
    Ok(())
}
fn lower_builtin_operation(
    cx: &mut LoweringCx<'_, '_>,
    builtin: Builtin,
    args: &[ValueId],
    feedback_ptr: Option<ir::Value>,
) -> Result<()> {
    let args = args
        .iter()
        .map(|value| use_value_boxed(cx.builder, cx.variables, *value))
        .collect::<Result<Vec<_>>>()?;
    let result = cx.call(builtin.wire_id().into(), &args, feedback_ptr)?;
    if builtin == Builtin::PromiseInstanceResolve || builtin == Builtin::PromiseInstanceReject {
        return Ok(());
    }
    let _ = result;
    Ok(())
}

/// `lhs` / `rhs` 是原始 f64（非 NaN-Box 编码）；`reverse` / `invert` 仍是编码布尔。
fn emit_f64_abstract_compare(
    builder: &mut FunctionBuilder<'_>,
    lhs: ir::Value,
    rhs: ir::Value,
    reverse: ir::Value,
    invert: ir::Value,
) -> ir::Value {
    let normal = builder
        .ins()
        .fcmp(ir::condcodes::FloatCC::LessThan, lhs, rhs);
    let reversed = builder
        .ins()
        .fcmp(ir::condcodes::FloatCC::LessThan, rhs, lhs);
    let true_value = builder.ins().iconst(types::I64, value::encode_bool(true));
    let reverse = builder
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, reverse, true_value);
    let invert = builder
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, invert, true_value);
    let relation = builder.ins().select(reverse, reversed, normal);
    let ordered = builder
        .ins()
        .fcmp(ir::condcodes::FloatCC::Ordered, lhs, rhs);
    let not_relation = builder.ins().bnot(relation);
    let inverted = builder.ins().band(ordered, not_relation);
    let condition = builder.ins().select(invert, inverted, relation);
    let false_value = builder.ins().iconst(types::I64, value::encode_bool(false));
    builder.ins().select(condition, true_value, false_value)
}

/// `lhs` / `rhs` 是原始 f64（非 NaN-Box 编码）。
fn emit_f64_relational(
    builder: &mut FunctionBuilder<'_>,
    lhs: ir::Value,
    rhs: ir::Value,
    op: CompareOp,
) -> ir::Value {
    let cc = match op {
        CompareOp::Lt => ir::condcodes::FloatCC::LessThan,
        CompareOp::Gt => ir::condcodes::FloatCC::GreaterThan,
        CompareOp::LtEq => ir::condcodes::FloatCC::LessThanOrEqual,
        CompareOp::GtEq => ir::condcodes::FloatCC::GreaterThanOrEqual,
        _ => ir::condcodes::FloatCC::LessThan,
    };
    let condition = builder.ins().fcmp(cc, lhs, rhs);
    let true_value = builder.ins().iconst(types::I64, value::encode_bool(true));
    let false_value = builder.ins().iconst(types::I64, value::encode_bool(false));
    builder.ins().select(condition, true_value, false_value)
}

/// 把一个原始 f64 收窄成 i32，并返回「收窄是否无损」。
fn f64_to_i32(builder: &mut FunctionBuilder<'_>, float: ir::Value) -> (ir::Value, ir::Value) {
    let sat = builder.ins().fcvt_to_sint_sat(types::I32, float);
    let back = builder.ins().fcvt_from_sint(types::F64, sat);
    let ordered = builder
        .ins()
        .fcmp(ir::condcodes::FloatCC::Equal, float, back);
    (sat, ordered)
}

fn emit_i32_relational(
    builder: &mut FunctionBuilder<'_>,
    lhs: ir::Value,
    rhs: ir::Value,
    op: CompareOp,
) -> Result<ir::Value> {
    let (lhs_i, lhs_ok) = f64_to_i32(builder, lhs);
    let (rhs_i, rhs_ok) = f64_to_i32(builder, rhs);
    let both = builder.ins().band(lhs_ok, rhs_ok);
    let cc = match op {
        CompareOp::Lt => ir::condcodes::IntCC::SignedLessThan,
        CompareOp::Gt => ir::condcodes::IntCC::SignedGreaterThan,
        CompareOp::LtEq => ir::condcodes::IntCC::SignedLessThanOrEqual,
        CompareOp::GtEq => ir::condcodes::IntCC::SignedGreaterThanOrEqual,
        _ => ir::condcodes::IntCC::SignedLessThan,
    };
    let cmp = builder.ins().icmp(cc, lhs_i, rhs_i);
    let cmp = builder.ins().band(cmp, both);
    let true_value = builder.ins().iconst(types::I64, value::encode_bool(true));
    let false_value = builder.ins().iconst(types::I64, value::encode_bool(false));
    Ok(builder.ins().select(cmp, true_value, false_value))
}

fn emit_i32_arithmetic(
    cx: &mut LoweringCx<'_, '_>,
    op: BinaryOp,
    lhs: ir::Value,
    rhs: ir::Value,
) -> Result<ir::Value> {
    let (lhs_i, lhs_ok) = f64_to_i32(cx.builder, lhs);
    let (rhs_i, rhs_ok) = f64_to_i32(cx.builder, rhs);
    let both = cx.builder.ins().band(lhs_ok, rhs_ok);
    let (sum, overflow) = match op {
        BinaryOp::Add => cx.builder.ins().sadd_overflow(lhs_i, rhs_i),
        BinaryOp::Sub => cx.builder.ins().ssub_overflow(lhs_i, rhs_i),
        BinaryOp::Mul => cx.builder.ins().smul_overflow(lhs_i, rhs_i),
        _ => unreachable!("guard restricts int32 arithmetic"),
    };
    let overflow_i64 = cx.builder.ins().uextend(types::I64, overflow);
    let fail_ov = cx
        .builder
        .ins()
        .icmp_imm_s(ir::condcodes::IntCC::NotEqual, overflow_i64, 0);
    let not_ok = cx.builder.ins().bnot(both);
    let fail = cx.builder.ins().bor(fail_ov, not_ok);
    let deopt_block = cx.builder.create_block();
    let cont = cx.builder.create_block();
    cx.builder.ins().brif(fail, deopt_block, &[], cont, &[]);
    cx.builder.switch_to_block(deopt_block);
    emit_deopt_to_generic(cx, cx.current_block, &[])?;
    cx.builder.switch_to_block(cont);
    cx.builder.seal_block(cont);
    let widened = cx.builder.ins().sextend(types::I64, sum);
    Ok(cx.builder.ins().fcvt_from_sint(types::F64, widened))
}

fn emit_deopt_to_generic(
    cx: &mut LoweringCx<'_, '_>,
    block: BasicBlockId,
    lives: &[ValueId],
) -> Result<()> {
    store_resume_lives(cx, lives)?;
    let function = cx
        .builder
        .ins()
        .iconst(types::I64, i64::from(cx.function_index));
    let block_id = cx.builder.ins().iconst(types::I64, i64::from(block.0));
    let count = cx
        .builder
        .ins()
        .iconst(types::I64, i64::try_from(lives.len()).unwrap_or(0));
    let env = cx.env;
    let this_value = cx.this_value;
    let result = cx.call(
        NativeRuntimeOp::DeoptToGeneric.id(),
        &[function, block_id, env, this_value, count],
        None,
    )?;
    cx.unlink_roots()?;
    cx.builder.ins().return_(&[result]);
    Ok(())
}

fn store_resume_lives(cx: &mut LoweringCx<'_, '_>, lives: &[ValueId]) -> Result<()> {
    let pointer_type = cx.builder.func.dfg.value_type(cx.ctx);
    let slots = cx.builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, resume_live_slots))?,
    );
    for (index, live) in lives.iter().enumerate() {
        let value = use_value_boxed(cx.builder, cx.variables, *live)?;
        let offset = i32::try_from(index * 8).context("resume live offset")?;
        cx.builder
            .ins()
            .store(MemFlagsData::trusted(), value, slots, offset);
    }
    let count = cx
        .builder
        .ins()
        .iconst(types::I32, i64::try_from(lives.len()).unwrap_or(0));
    cx.builder.ins().store(
        MemFlagsData::trusted(),
        count,
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, resume_live_count))?,
    );
    Ok(())
}

fn emit_resume_dispatch(
    cx: &mut LoweringCx<'_, '_>,
    function: &wjsm_ir::Function,
    blocks: &HashMap<BasicBlockId, ir::Block>,
    headers: &[BasicBlockId],
    entry_body: ir::Block,
) -> Result<()> {
    let resume = cx.builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, resume_block_plus_one))?,
    );
    let has_resume = cx
        .builder
        .ins()
        .icmp_imm_s(ir::condcodes::IntCC::NotEqual, resume, 0);
    let dispatch = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(has_resume, dispatch, &[], entry_body, &[]);
    cx.builder.switch_to_block(dispatch);
    let func = cx.builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, resume_function_id))?,
    );
    let mine = cx.builder.ins().icmp_imm_s(
        ir::condcodes::IntCC::Equal,
        func,
        i64::from(cx.function_index),
    );
    let take = cx.builder.create_block();
    cx.builder.ins().brif(mine, take, &[], entry_body, &[]);
    cx.builder.switch_to_block(take);
    let zero = cx.builder.ins().iconst(types::I32, 0);
    cx.builder.ins().store(
        MemFlagsData::trusted(),
        zero,
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, resume_block_plus_one))?,
    );
    let wanted = cx.builder.ins().iadd_imm_s(resume, -1);
    let pointer_type = cx.builder.func.dfg.value_type(cx.ctx);
    let slots = cx.builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, resume_live_slots))?,
    );
    for header in headers {
        let hit =
            cx.builder
                .ins()
                .icmp_imm_s(ir::condcodes::IntCC::Equal, wanted, i64::from(header.0));
        let restore = cx.builder.create_block();
        let skip = cx.builder.create_block();
        cx.builder.ins().brif(hit, restore, &[], skip, &[]);
        cx.builder.switch_to_block(restore);
        let lives = wjsm_ir::typed_cfg::loop_header_live_phis(function, *header);
        for (index, live) in lives.iter().enumerate() {
            let offset = i32::try_from(index * 8).context("resume live offset")?;
            let value = cx
                .builder
                .ins()
                .load(types::I64, MemFlagsData::trusted(), slots, offset);
            define_value_boxed(cx.builder, cx.variables, *live, value)?;
        }
        let target = if *header == function.entry() {
            entry_body
        } else {
            blocks[header]
        };
        cx.builder.ins().jump(target, &[]);
        cx.builder.switch_to_block(skip);
    }
    cx.builder.ins().jump(entry_body, &[]);
    Ok(())
}

fn emit_osr_poll(
    cx: &mut LoweringCx<'_, '_>,
    tables: &InstructionTables<'_>,
    header: BasicBlockId,
    lives: &[ValueId],
) -> Result<()> {
    let pointer_type = cx.builder.func.dfg.value_type(cx.ctx);
    let table = cx.builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, function_table))?,
    );
    let stride = i64::try_from(std::mem::size_of::<wjsm_native_abi::NativeFunctionEntry>())
        .context("function entry size")?;
    let offset = i64::from(cx.function_index) * stride;
    let entry = cx.builder.ins().iadd_imm_s(table, offset);
    let osr = cx.builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        entry,
        i32::try_from(offset_of!(wjsm_native_abi::NativeFunctionEntry, osr_entry))
            .context("osr_entry offset")?,
    );
    let has = cx
        .builder
        .ins()
        .icmp_imm_s(ir::condcodes::IntCC::NotEqual, osr, 0);
    let take = cx.builder.create_block();
    let cont = cx.builder.create_block();
    cx.builder.ins().brif(has, take, &[], cont, &[]);
    cx.builder.switch_to_block(take);
    store_resume_lives(cx, lives)?;
    let plus = cx.builder.ins().iconst(types::I32, i64::from(header.0) + 1);
    cx.builder.ins().store(
        MemFlagsData::trusted(),
        plus,
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, resume_block_plus_one))?,
    );
    let func = cx
        .builder
        .ins()
        .iconst(types::I32, i64::from(cx.function_index));
    cx.builder.ins().store(
        MemFlagsData::trusted(),
        func,
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, resume_function_id))?,
    );
    let args_base = cx.builder.ins().iconst(types::I32, 0);
    let args_count = cx.builder.ins().iconst(types::I32, 0);
    let inst = cx.builder.ins().call_indirect(
        tables.slow_call_signature,
        osr,
        &[cx.ctx, cx.env, cx.this_value, args_base, args_count],
    );
    let result = cx.builder.inst_results(inst)[0];
    cx.unlink_roots()?;
    cx.builder.ins().return_(&[result]);
    cx.builder.switch_to_block(cont);
    cx.builder.seal_block(cont);
    Ok(())
}

fn emit_overlay_header_guards(
    cx: &mut LoweringCx<'_, '_>,
    tables: &InstructionTables<'_>,
    header: BasicBlockId,
    lives: &[ValueId],
) -> Result<()> {
    for live in lives {
        if !tables.f64_values.contains(live) && !tables.int32_values.contains(live) {
            continue;
        }
        // 常驻 F64 寄存器的值按构造就是 number，守卫恒真；发它反而会把
        // 原始 NaN 位模式误判成 NaN-Box 前缀而无谓 deopt。
        if cx.variables.is_typed_value(*live) {
            continue;
        }
        let encoded = use_value_boxed(cx.builder, cx.variables, *live)?;
        let ok = emit_is_number(cx.builder, encoded);
        let deopt = cx.builder.create_block();
        let cont = cx.builder.create_block();
        cx.builder.ins().brif(ok, cont, &[], deopt, &[]);
        cx.builder.switch_to_block(deopt);
        emit_deopt_to_generic(cx, header, lives)?;
        cx.builder.switch_to_block(cont);
        cx.builder.seal_block(cont);
    }
    Ok(())
}

fn emit_inline_string_predicate(
    builder: &mut FunctionBuilder<'_>,
    encoded: ir::Value,
) -> ir::Value {
    let box_base = i64::from_ne_bytes(value::BOX_BASE.to_ne_bytes());
    let boxed = builder.ins().band_imm_u(encoded, box_base);
    let boxed = builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::Equal, boxed, box_base);
    let marker_bits = builder.ins().band_imm_u(
        encoded,
        i64::try_from(value::INLINE_STRING_MARKER_MASK).expect("SSO marker mask fits i64"),
    );
    let is_ascii = builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        marker_bits,
        i64::try_from(value::INLINE_STRING_MARKER << value::INLINE_STRING_MARKER_SHIFT)
            .expect("ASCII SSO marker fits i64"),
    );
    let is_latin1 = builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        marker_bits,
        i64::try_from(value::INLINE_STRING_LATIN1_MARKER << value::INLINE_STRING_MARKER_SHIFT)
            .expect("Latin-1 SSO marker fits i64"),
    );
    let reserved = builder.ins().band_imm_u(
        encoded,
        i64::try_from(value::INLINE_STRING_RESERVED_MASK).expect("SSO reserved mask fits i64"),
    );
    let reserved_zero = builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::Equal, reserved, 0);
    let length = builder
        .ins()
        .ushr_imm_u(encoded, i64::from(value::INLINE_STRING_LENGTH_SHIFT));
    let length = builder.ins().band_imm_u(length, 0b111);
    let length_ok = builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::UnsignedLessThanOrEqual,
        length,
        i64::try_from(value::INLINE_STRING_MAX_LEN).expect("SSO length fits i64"),
    );
    let ascii_ok = builder.ins().band(is_ascii, reserved_zero);
    let ascii_ok = builder.ins().band(ascii_ok, length_ok);
    let latin1_ok = builder.ins().band(is_latin1, length_ok);
    let kind_ok = builder.ins().bor(ascii_ok, latin1_ok);
    builder.ins().band(boxed, kind_ok)
}

/// 判定 NaN-box 之外的标量（热路径上几乎都是 number）。
///
/// 无 `BOX_BASE` 的值不是 handle-backed reference：着色是空操作，SATB / Mark /
/// remset 都不触发，IC 命中且 access epoch 为偶时可以跳过 store barrier thunk。
fn emit_unboxed_nanbox_predicate(
    builder: &mut FunctionBuilder<'_>,
    encoded: ir::Value,
) -> ir::Value {
    let box_base = i64::from_ne_bytes(value::BOX_BASE.to_ne_bytes());
    let boxed_bits = builder.ins().band_imm_s(encoded, box_base);
    builder
        .ins()
        .icmp_imm_s(ir::condcodes::IntCC::NotEqual, boxed_bits, box_base)
}

/// 把运行时字符串句柄解析为当前读取作用域内稳定的堆地址。
///
/// 每次调用都生成独立控制流；地址不跨块记忆，避免 ZGC epoch 变化后复用旧地址。
fn emit_string_address(
    cx: &mut LoweringCx<'_, '_>,
    barrier_thunks: &BarrierThunks,
    encoded: ir::Value,
    miss_block: ir::Block,
) -> Result<ir::Value> {
    let pointer_type = cx.builder.func.dfg.value_type(cx.ctx);
    let entry_block = cx.builder.create_block();
    let legacy_block = cx.builder.create_block();
    let zgc_block = cx.builder.create_block();
    let fast_block = cx.builder.create_block();
    let assist_block = cx.builder.create_block();
    let resolved_block = cx.builder.create_block();
    cx.builder.append_block_param(resolved_block, types::I64);

    let boxed_bits = cx
        .builder
        .ins()
        .band_imm_s(encoded, i64::from_ne_bytes(value::BOX_BASE.to_ne_bytes()));
    let is_boxed = cx.builder.ins().icmp_imm_s(
        ir::condcodes::IntCC::Equal,
        boxed_bits,
        i64::from_ne_bytes(value::BOX_BASE.to_ne_bytes()),
    );
    let tag_word = cx.builder.ins().ushr_imm_u(encoded, 32);
    let tag = cx.builder.ins().band_imm_u(
        tag_word,
        i64::try_from(value::TAG_MASK).expect("tag mask fits i64"),
    );
    let is_string = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        tag,
        i64::try_from(value::TAG_STRING).expect("string tag fits i64"),
    );
    let runtime_flag = cx.builder.ins().band_imm_u(
        tag_word,
        i64::try_from(value::STRING_RUNTIME_HANDLE_FLAG).expect("runtime flag fits i64"),
    );
    let is_runtime = cx
        .builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::NotEqual, runtime_flag, 0);
    let valid = cx.builder.ins().band(is_boxed, is_string);
    let valid = cx.builder.ins().band(valid, is_runtime);
    let inline = emit_inline_string_predicate(cx.builder, encoded);
    let inline = cx.builder.ins().bnot(inline);
    let valid = cx.builder.ins().band(valid, inline);
    cx.builder
        .ins()
        .brif(valid, entry_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(entry_block);
    cx.builder.seal_block(entry_block);
    let handle = cx.builder.ins().band_imm_u(encoded, i64::from(u32::MAX));
    let handle_i32 = cx.builder.ins().ireduce(types::I32, handle);
    let handle_table = cx.ht_base;
    let entry_offset = cx.builder.ins().ishl_imm_u(handle, 3);
    let entry_address = cx.builder.ins().iadd(handle_table, entry_offset);
    let entry = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), entry_address, 0);
    let state = cx.builder.ins().band_imm_u(entry, 0xffff);
    let stable = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::UnsignedGreaterThanOrEqual,
        state,
        i64::from(constants::HANDLE_STATE_STABLE_MIN),
    );
    let logical_address = cx.builder.ins().ushr_imm_u(entry, 16);
    let barrier_state = cx.barrier_state;
    let barrier_disabled =
        cx.builder
            .ins()
            .icmp_imm_u(ir::condcodes::IntCC::Equal, barrier_state, 0);
    cx.builder
        .ins()
        .brif(barrier_disabled, legacy_block, &[], zgc_block, &[]);

    cx.builder.switch_to_block(legacy_block);
    cx.builder.seal_block(legacy_block);
    cx.builder.ins().brif(
        stable,
        resolved_block,
        &[ir::BlockArg::Value(logical_address)],
        miss_block,
        &[],
    );

    cx.builder.switch_to_block(zgc_block);
    cx.builder.seal_block(zgc_block);
    let epoch_address = cx.builder.ins().iadd_imm_s(
        barrier_state,
        i64::try_from(offset_of!(NativeBarrierState, access_epoch))
            .expect("access epoch offset fits i64"),
    );
    let epoch = cx
        .builder
        .ins()
        .atomic_load(types::I64, MemFlagsData::trusted(), epoch_address);
    let epoch_bit = cx.builder.ins().band_imm_u(epoch, 1);
    let epoch_even = cx
        .builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::Equal, epoch_bit, 0);
    let direct = cx.builder.ins().band(stable, epoch_even);
    cx.builder
        .ins()
        .brif(direct, fast_block, &[], assist_block, &[]);

    cx.builder.switch_to_block(fast_block);
    cx.builder.seal_block(fast_block);
    increment_barrier_counter(
        cx.builder,
        barrier_state,
        offset_of!(NativeBarrierState, load_fast_events),
    );
    cx.builder
        .ins()
        .jump(resolved_block, &[ir::BlockArg::Value(logical_address)]);

    cx.builder.switch_to_block(assist_block);
    cx.builder.seal_block(assist_block);
    let call = cx
        .builder
        .ins()
        .call(barrier_thunks.load, &[cx.ctx, handle_i32]);
    let assisted = cx.builder.inst_results(call)[0];
    let assisted_ok = cx
        .builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::NotEqual, assisted, 0);
    cx.builder.ins().brif(
        assisted_ok,
        resolved_block,
        &[ir::BlockArg::Value(assisted)],
        miss_block,
        &[],
    );

    cx.builder.switch_to_block(resolved_block);
    cx.builder.seal_block(resolved_block);
    let logical_address = cx.builder.block_params(resolved_block)[0];
    let heap_delta = cx.builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, heap_object_delta))?,
    );
    Ok(cx.builder.ins().iadd(logical_address, heap_delta))
}

/// 把运行时数组句柄解析为当前读取作用域内稳定的堆地址。
fn emit_array_address(
    cx: &mut LoweringCx<'_, '_>,
    barrier_thunks: &BarrierThunks,
    encoded: ir::Value,
    miss_block: ir::Block,
) -> Result<ir::Value> {
    let pointer_type = cx.builder.func.dfg.value_type(cx.ctx);
    let entry_block = cx.builder.create_block();
    let legacy_block = cx.builder.create_block();
    let zgc_block = cx.builder.create_block();
    let fast_block = cx.builder.create_block();
    let assist_block = cx.builder.create_block();
    let resolved_block = cx.builder.create_block();
    cx.builder.append_block_param(resolved_block, types::I64);

    let boxed_bits = cx
        .builder
        .ins()
        .band_imm_s(encoded, i64::from_ne_bytes(value::BOX_BASE.to_ne_bytes()));
    let is_boxed = cx.builder.ins().icmp_imm_s(
        ir::condcodes::IntCC::Equal,
        boxed_bits,
        i64::from_ne_bytes(value::BOX_BASE.to_ne_bytes()),
    );
    let tag_word = cx.builder.ins().ushr_imm_u(encoded, 32);
    let tag = cx.builder.ins().band_imm_u(
        tag_word,
        i64::try_from(value::TAG_MASK).expect("tag mask fits i64"),
    );
    let is_array = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        tag,
        i64::try_from(value::TAG_ARRAY).expect("array tag fits i64"),
    );
    let valid = cx.builder.ins().band(is_boxed, is_array);
    cx.builder
        .ins()
        .brif(valid, entry_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(entry_block);
    cx.builder.seal_block(entry_block);
    let handle = cx.builder.ins().band_imm_u(encoded, i64::from(u32::MAX));
    let handle_i32 = cx.builder.ins().ireduce(types::I32, handle);
    let handle_table = cx.ht_base;
    let entry_offset = cx.builder.ins().ishl_imm_u(handle, 3);
    let entry_address = cx.builder.ins().iadd(handle_table, entry_offset);
    let entry = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), entry_address, 0);
    let state = cx.builder.ins().band_imm_u(entry, 0xffff);
    let stable = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::UnsignedGreaterThanOrEqual,
        state,
        i64::from(constants::HANDLE_STATE_STABLE_MIN),
    );
    let logical_address = cx.builder.ins().ushr_imm_u(entry, 16);
    let barrier_state = cx.barrier_state;
    let barrier_disabled =
        cx.builder
            .ins()
            .icmp_imm_u(ir::condcodes::IntCC::Equal, barrier_state, 0);
    cx.builder
        .ins()
        .brif(barrier_disabled, legacy_block, &[], zgc_block, &[]);

    cx.builder.switch_to_block(legacy_block);
    cx.builder.seal_block(legacy_block);
    cx.builder.ins().brif(
        stable,
        resolved_block,
        &[ir::BlockArg::Value(logical_address)],
        miss_block,
        &[],
    );

    cx.builder.switch_to_block(zgc_block);
    cx.builder.seal_block(zgc_block);
    let epoch_address = cx.builder.ins().iadd_imm_s(
        barrier_state,
        i64::try_from(offset_of!(NativeBarrierState, access_epoch))
            .expect("access epoch offset fits i64"),
    );
    let epoch = cx
        .builder
        .ins()
        .atomic_load(types::I64, MemFlagsData::trusted(), epoch_address);
    let epoch_bit = cx.builder.ins().band_imm_u(epoch, 1);
    let epoch_even = cx
        .builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::Equal, epoch_bit, 0);
    let direct = cx.builder.ins().band(stable, epoch_even);
    cx.builder
        .ins()
        .brif(direct, fast_block, &[], assist_block, &[]);

    cx.builder.switch_to_block(fast_block);
    cx.builder.seal_block(fast_block);
    increment_barrier_counter(
        cx.builder,
        barrier_state,
        offset_of!(NativeBarrierState, load_fast_events),
    );
    cx.builder
        .ins()
        .jump(resolved_block, &[ir::BlockArg::Value(logical_address)]);

    cx.builder.switch_to_block(assist_block);
    cx.builder.seal_block(assist_block);
    let call = cx
        .builder
        .ins()
        .call(barrier_thunks.load, &[cx.ctx, handle_i32]);
    let assisted = cx.builder.inst_results(call)[0];
    let assisted_ok = cx
        .builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::NotEqual, assisted, 0);
    cx.builder.ins().brif(
        assisted_ok,
        resolved_block,
        &[ir::BlockArg::Value(assisted)],
        miss_block,
        &[],
    );

    cx.builder.switch_to_block(resolved_block);
    cx.builder.seal_block(resolved_block);
    let logical_address = cx.builder.block_params(resolved_block)[0];
    let heap_delta = cx.builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, heap_object_delta))?,
    );
    Ok(cx.builder.ins().iadd(logical_address, heap_delta))
}

fn emit_nonnegative_integer_index(
    builder: &mut FunctionBuilder<'_>,
    encoded: ir::Value,
) -> (ir::Value, ir::Value) {
    let is_number = emit_is_number(builder, encoded);
    let number = builder
        .ins()
        .bitcast(types::F64, ir::MemFlagsData::new(), encoded);
    let index = builder.ins().fcvt_to_uint_sat(types::I64, number);
    let roundtrip = builder.ins().fcvt_from_uint(types::F64, index);
    let exact = builder
        .ins()
        .fcmp(ir::condcodes::FloatCC::Equal, number, roundtrip);
    let below_sentinel = builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::UnsignedLessThan,
        index,
        i64::from(u32::MAX),
    );
    let valid = builder.ins().band(is_number, exact);
    (index, builder.ins().band(valid, below_sentinel))
}

fn emit_flat_string_code_unit(
    cx: &mut LoweringCx<'_, '_>,
    address: ir::Value,
    index: ir::Value,
    miss_block: ir::Block,
    out_of_bounds_block: ir::Block,
) -> ir::Value {
    let latin1_block = cx.builder.create_block();
    let utf16_block = cx.builder.create_block();
    let payload_block = cx.builder.create_block();
    cx.builder.append_block_param(payload_block, types::I64);

    let header = cx.builder.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        address,
        i32::try_from(constants::HEAP_STRING_LENGTH_OFFSET).expect("length offset fits i32"),
    );
    let length = cx.builder.ins().band_imm_u(header, i64::from(u32::MAX));
    let in_bounds = cx
        .builder
        .ins()
        .icmp(ir::condcodes::IntCC::UnsignedLessThan, index, length);
    let repr_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(in_bounds, repr_block, &[], out_of_bounds_block, &[]);

    cx.builder.switch_to_block(repr_block);
    cx.builder.seal_block(repr_block);
    let first_word = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), address, 0);
    let repr = cx.builder.ins().ushr_imm_u(
        first_word,
        i64::from(constants::HEAP_STRING_REPR_OFFSET * 8),
    );
    let repr = cx.builder.ins().band_imm_u(repr, 0xff);
    let is_latin1 = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        repr,
        i64::from(constants::STRING_REPR_LATIN1_FLAT),
    );
    let is_utf16 = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        repr,
        i64::from(constants::STRING_REPR_UTF16_FLAT),
    );
    let flat_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(is_latin1, latin1_block, &[], flat_block, &[]);

    cx.builder.switch_to_block(flat_block);
    cx.builder.seal_block(flat_block);
    cx.builder
        .ins()
        .brif(is_utf16, utf16_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(latin1_block);
    cx.builder.seal_block(latin1_block);
    let payload = cx
        .builder
        .ins()
        .iadd_imm_s(address, i64::from(constants::HEAP_STRING_PAYLOAD_OFFSET));
    let unit_address = cx.builder.ins().iadd(payload, index);
    let unit = cx
        .builder
        .ins()
        .load(types::I8, MemFlagsData::trusted(), unit_address, 0);
    let unit = cx.builder.ins().uextend(types::I64, unit);
    cx.builder
        .ins()
        .jump(payload_block, &[ir::BlockArg::Value(unit)]);

    cx.builder.switch_to_block(utf16_block);
    cx.builder.seal_block(utf16_block);
    let payload = cx
        .builder
        .ins()
        .iadd_imm_s(address, i64::from(constants::HEAP_STRING_PAYLOAD_OFFSET));
    let byte_offset = cx.builder.ins().ishl_imm_u(index, 1);
    let unit_address = cx.builder.ins().iadd(payload, byte_offset);
    let unit = cx
        .builder
        .ins()
        .load(types::I16, MemFlagsData::trusted(), unit_address, 0);
    let unit = cx.builder.ins().uextend(types::I64, unit);
    cx.builder
        .ins()
        .jump(payload_block, &[ir::BlockArg::Value(unit)]);

    cx.builder.switch_to_block(payload_block);
    cx.builder.seal_block(payload_block);
    cx.builder.block_params(payload_block)[0]
}

fn emit_latin1_char_handle(
    cx: &mut LoweringCx<'_, '_>,
    unit: ir::Value,
    miss_block: ir::Block,
) -> Result<ir::Value> {
    let pointer_type = cx.builder.func.dfg.value_type(cx.ctx);
    let cached_block = cx.builder.create_block();
    let is_latin1 = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::UnsignedLessThanOrEqual,
        unit,
        i64::from(u8::MAX),
    );
    cx.builder
        .ins()
        .brif(is_latin1, cached_block, &[], miss_block, &[]);
    cx.builder.switch_to_block(cached_block);
    cx.builder.seal_block(cached_block);
    let table = cx.builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, latin1_char_strings))?,
    );
    let offset = cx.builder.ins().ishl_imm_u(unit, 3);
    let address = cx.builder.ins().iadd(table, offset);
    Ok(cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), address, 0))
}
fn emit_mixed_string_equal(
    cx: &mut LoweringCx<'_, '_>,
    left_address: ir::Value,
    right_address: ir::Value,
    length: ir::Value,
    left_latin1: ir::Value,
    right_latin1: ir::Value,
    result_blocks: (ir::Block, ir::Block),
) {
    let (equal_block, false_block) = result_blocks;
    let loop_block = cx.builder.create_block();
    let left_select_block = cx.builder.create_block();
    let left_utf16_block = cx.builder.create_block();
    let left_latin1_block = cx.builder.create_block();
    let left_latin_right_latin_block = cx.builder.create_block();
    let left_latin_right_utf16_block = cx.builder.create_block();
    let left_utf16_right_latin_block = cx.builder.create_block();
    let left_utf16_right_utf16_block = cx.builder.create_block();
    let units_block = cx.builder.create_block();
    cx.builder.append_block_param(loop_block, types::I64);
    cx.builder.append_block_param(units_block, types::I64);
    cx.builder.append_block_param(units_block, types::I64);

    let zero = cx.builder.ins().iconst(types::I64, 0);
    cx.builder
        .ins()
        .jump(loop_block, &[ir::BlockArg::Value(zero)]);
    cx.builder.switch_to_block(loop_block);
    let index = cx.builder.block_params(loop_block)[0];
    let done = cx.builder.ins().icmp(
        ir::condcodes::IntCC::UnsignedGreaterThanOrEqual,
        index,
        length,
    );
    cx.builder
        .ins()
        .brif(done, equal_block, &[], left_select_block, &[]);
    cx.builder.switch_to_block(left_select_block);
    cx.builder.seal_block(left_select_block);
    let left_payload = cx.builder.ins().iadd_imm_s(
        left_address,
        i64::from(constants::HEAP_STRING_PAYLOAD_OFFSET),
    );
    let right_payload = cx.builder.ins().iadd_imm_s(
        right_address,
        i64::from(constants::HEAP_STRING_PAYLOAD_OFFSET),
    );
    let left_byte_offset = index;
    let left_word_offset = cx.builder.ins().ishl_imm_u(index, 1);
    cx.builder
        .ins()
        .brif(left_latin1, left_latin1_block, &[], left_utf16_block, &[]);

    cx.builder.switch_to_block(left_latin1_block);
    cx.builder.seal_block(left_latin1_block);
    cx.builder.ins().brif(
        right_latin1,
        left_latin_right_latin_block,
        &[],
        left_latin_right_utf16_block,
        &[],
    );

    cx.builder.switch_to_block(left_utf16_block);
    cx.builder.seal_block(left_utf16_block);
    cx.builder.ins().brif(
        right_latin1,
        left_utf16_right_latin_block,
        &[],
        left_utf16_right_utf16_block,
        &[],
    );

    cx.builder.switch_to_block(left_latin_right_latin_block);
    cx.builder.seal_block(left_latin_right_latin_block);
    let left_address = cx.builder.ins().iadd(left_payload, left_byte_offset);
    let left = cx
        .builder
        .ins()
        .load(types::I8, MemFlagsData::trusted(), left_address, 0);
    let right_address = cx.builder.ins().iadd(right_payload, left_byte_offset);
    let right = cx
        .builder
        .ins()
        .load(types::I8, MemFlagsData::trusted(), right_address, 0);
    let left = cx.builder.ins().uextend(types::I64, left);
    let right = cx.builder.ins().uextend(types::I64, right);
    cx.builder.ins().jump(
        units_block,
        &[ir::BlockArg::Value(left), ir::BlockArg::Value(right)],
    );

    cx.builder.switch_to_block(left_latin_right_utf16_block);
    cx.builder.seal_block(left_latin_right_utf16_block);
    let left_address = cx.builder.ins().iadd(left_payload, left_byte_offset);
    let left = cx
        .builder
        .ins()
        .load(types::I8, MemFlagsData::trusted(), left_address, 0);
    let right_address = cx.builder.ins().iadd(right_payload, left_word_offset);
    let right = cx
        .builder
        .ins()
        .load(types::I16, MemFlagsData::trusted(), right_address, 0);
    let left = cx.builder.ins().uextend(types::I64, left);
    let right = cx.builder.ins().uextend(types::I64, right);
    cx.builder.ins().jump(
        units_block,
        &[ir::BlockArg::Value(left), ir::BlockArg::Value(right)],
    );

    cx.builder.switch_to_block(left_utf16_right_latin_block);
    cx.builder.seal_block(left_utf16_right_latin_block);
    let left_address = cx.builder.ins().iadd(left_payload, left_word_offset);
    let left = cx
        .builder
        .ins()
        .load(types::I16, MemFlagsData::trusted(), left_address, 0);
    let right_address = cx.builder.ins().iadd(right_payload, left_byte_offset);
    let right = cx
        .builder
        .ins()
        .load(types::I8, MemFlagsData::trusted(), right_address, 0);
    let left = cx.builder.ins().uextend(types::I64, left);
    let right = cx.builder.ins().uextend(types::I64, right);
    cx.builder.ins().jump(
        units_block,
        &[ir::BlockArg::Value(left), ir::BlockArg::Value(right)],
    );

    cx.builder.switch_to_block(left_utf16_right_utf16_block);
    cx.builder.seal_block(left_utf16_right_utf16_block);
    let left_address = cx.builder.ins().iadd(left_payload, left_word_offset);
    let left = cx
        .builder
        .ins()
        .load(types::I16, MemFlagsData::trusted(), left_address, 0);
    let right_address = cx.builder.ins().iadd(right_payload, left_word_offset);
    let right = cx
        .builder
        .ins()
        .load(types::I16, MemFlagsData::trusted(), right_address, 0);
    let left = cx.builder.ins().uextend(types::I64, left);
    let right = cx.builder.ins().uextend(types::I64, right);
    cx.builder.ins().jump(
        units_block,
        &[ir::BlockArg::Value(left), ir::BlockArg::Value(right)],
    );

    cx.builder.switch_to_block(units_block);
    cx.builder.seal_block(units_block);
    let left = cx.builder.block_params(units_block)[0];
    let right = cx.builder.block_params(units_block)[1];
    let same = cx
        .builder
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, left, right);
    let next_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(same, next_block, &[], false_block, &[]);
    cx.builder.switch_to_block(next_block);
    cx.builder.seal_block(next_block);
    let index = cx.builder.block_params(loop_block)[0];
    let next_index = cx.builder.ins().iadd_imm_s(index, 1);
    cx.builder
        .ins()
        .jump(loop_block, &[ir::BlockArg::Value(next_index)]);
    cx.builder.seal_block(loop_block);
}

/// `GetElem` / `GetElemGuarded` 共用的元素读取 lowering。`guard` 给定时
/// （Guarded 变体），miss 路径进入宿主前先把守卫值置 false——宿主完整
/// `[[Get]]` 可能执行用户代码（原型链上的索引 accessor 等），单向闩锁保证
/// 同循环所有 `GetPropGuarded` 快路径随之失效；快路径（packed 数组直读 /
/// 字符串码元）不经过宿主，无需触碰守卫。
fn lower_string_element(
    cx: &mut LoweringCx<'_, '_>,
    barrier_thunks: &BarrierThunks,
    dest: ValueId,
    object: ValueId,
    index: ValueId,
    guard: Option<ValueId>,
) -> Result<()> {
    let object = use_value_boxed(cx.builder, cx.variables, object)?;
    let encoded_index = use_value_boxed(cx.builder, cx.variables, index)?;
    let (index, valid_index) = emit_nonnegative_integer_index(cx.builder, encoded_index);
    let index_block = cx.builder.create_block();
    let inline_string_block = cx.builder.create_block();
    let inline_char_block = cx.builder.create_block();
    let string_block = cx.builder.create_block();
    let dispatch_block = cx.builder.create_block();
    let array_dispatch_block = cx.builder.create_block();
    let array_block = cx.builder.create_block();
    let miss_block = cx.builder.create_block();
    let out_of_bounds_block = cx.builder.create_block();
    let merge_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(valid_index, index_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(index_block);
    cx.builder.seal_block(index_block);
    let is_inline = emit_inline_string_predicate(cx.builder, object);
    cx.builder
        .ins()
        .brif(is_inline, inline_string_block, &[], dispatch_block, &[]);

    cx.builder.switch_to_block(inline_string_block);
    cx.builder.seal_block(inline_string_block);
    let inline_length = cx
        .builder
        .ins()
        .ushr_imm_u(object, i64::from(value::INLINE_STRING_LENGTH_SHIFT));
    let inline_length = cx.builder.ins().band_imm_u(inline_length, 0b111);
    let inline_in_bounds =
        cx.builder
            .ins()
            .icmp(ir::condcodes::IntCC::UnsignedLessThan, index, inline_length);
    cx.builder.ins().brif(
        inline_in_bounds,
        inline_char_block,
        &[],
        out_of_bounds_block,
        &[],
    );

    cx.builder.switch_to_block(inline_char_block);
    cx.builder.seal_block(inline_char_block);
    let inline_latin1_char_block = cx.builder.create_block();
    let inline_ascii_char_block = cx.builder.create_block();
    let is_inline_latin1 = emit_is_inline_latin1_marker(cx.builder, object);
    cx.builder.ins().brif(
        is_inline_latin1,
        inline_latin1_char_block,
        &[],
        inline_ascii_char_block,
        &[],
    );

    cx.builder.switch_to_block(inline_ascii_char_block);
    cx.builder.seal_block(inline_ascii_char_block);
    let inline_unit = emit_extract_inline_ascii_unit(cx.builder, object, index);
    let inline_result = emit_inline_ascii_char_value(cx, inline_unit);
    define_value_boxed(cx.builder, cx.variables, dest, inline_result)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(inline_latin1_char_block);
    cx.builder.seal_block(inline_latin1_char_block);
    let inline_unit = emit_extract_inline_latin1_unit(cx.builder, object, index);
    let inline_result = emit_latin1_char_handle(cx, inline_unit, miss_block)?;
    define_value_boxed(cx.builder, cx.variables, dest, inline_result)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(dispatch_block);
    cx.builder.seal_block(dispatch_block);
    let tag_word = cx.builder.ins().ushr_imm_u(object, 32);
    let tag = cx.builder.ins().band_imm_u(
        tag_word,
        i64::try_from(value::TAG_MASK).expect("tag mask fits i64"),
    );
    let is_string = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        tag,
        i64::try_from(value::TAG_STRING).expect("string tag fits i64"),
    );
    let is_array = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        tag,
        i64::try_from(value::TAG_ARRAY).expect("array tag fits i64"),
    );
    cx.builder
        .ins()
        .brif(is_string, string_block, &[], array_dispatch_block, &[]);

    cx.builder.switch_to_block(array_dispatch_block);
    cx.builder.seal_block(array_dispatch_block);
    cx.builder
        .ins()
        .brif(is_array, array_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(string_block);
    cx.builder.seal_block(string_block);
    let address = emit_string_address(cx, barrier_thunks, object, miss_block)?;
    let unit = emit_flat_string_code_unit(cx, address, index, miss_block, out_of_bounds_block);
    let result = emit_latin1_char_handle(cx, unit, miss_block)?;
    define_value_boxed(cx.builder, cx.variables, dest, result)?;
    cx.builder.ins().jump(merge_block, &[]);

    // 数组读取
    cx.builder.switch_to_block(array_block);
    cx.builder.seal_block(array_block);
    let address = emit_array_address(cx, barrier_thunks, object, miss_block)?;
    let header = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), address, 0);
    let kind = cx.builder.ins().ushr_imm_u(header, 40);
    let kind = cx.builder.ins().band_imm_u(kind, 0xff);
    let is_dict = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        kind,
        i64::from(wjsm_ir::constants::ARRAY_KIND_DICTIONARY),
    );
    let dict_check_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(is_dict, miss_block, &[], dict_check_block, &[]);

    cx.builder.switch_to_block(dict_check_block);
    cx.builder.seal_block(dict_check_block);
    let shape = cx.builder.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        address,
        i32::try_from(constants::HEAP_ARRAY_LENGTH_OFFSET).expect("length offset fits i32"),
    );
    let length = cx.builder.ins().band_imm_u(shape, i64::from(u32::MAX));
    let capacity = cx.builder.ins().ushr_imm_u(shape, 32);
    let index_u64 = index;
    let in_length =
        cx.builder
            .ins()
            .icmp(ir::condcodes::IntCC::UnsignedLessThan, index_u64, length);
    let in_capacity =
        cx.builder
            .ins()
            .icmp(ir::condcodes::IntCC::UnsignedLessThan, index_u64, capacity);
    let in_bounds = cx.builder.ins().band(in_length, in_capacity);

    let elem_read_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(in_bounds, elem_read_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(elem_read_block);
    cx.builder.seal_block(elem_read_block);
    let index_bytes = cx.builder.ins().ishl_imm_u(index_u64, 3);
    let elem_offset = cx
        .builder
        .ins()
        .iadd_imm_s(index_bytes, i64::from(constants::HEAP_OBJECT_HEADER_SIZE));
    let elem_addr = cx.builder.ins().iadd(address, elem_offset);
    let elem_val = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), elem_addr, 0);

    let hole_val = cx
        .builder
        .ins()
        .iconst(types::I64, value::encode_array_hole());
    let is_hole = cx
        .builder
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, elem_val, hole_val);
    let elem_hit_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(is_hole, miss_block, &[], elem_hit_block, &[]);

    cx.builder.switch_to_block(elem_hit_block);
    cx.builder.seal_block(elem_hit_block);
    let clean_elem = emit_strip_gc_color(cx.builder, elem_val);
    define_value_boxed(cx.builder, cx.variables, dest, clean_elem)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(out_of_bounds_block);
    cx.builder.seal_block(out_of_bounds_block);
    let undefined = cx
        .builder
        .ins()
        .iconst(types::I64, value::encode_undefined());
    define_value_boxed(cx.builder, cx.variables, dest, undefined)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(miss_block);
    cx.builder.seal_block(miss_block);
    if let Some(guard) = guard {
        let disabled = cx
            .builder
            .ins()
            .iconst(types::I64, value::encode_bool(false));
        define_value_boxed(cx.builder, cx.variables, guard, disabled)?;
    }
    let result = cx.call(
        NativeRuntimeOp::GetElem.id(),
        &[object, encoded_index],
        None,
    )?;
    define_value_boxed(cx.builder, cx.variables, dest, result)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(merge_block);
    cx.builder.seal_block(merge_block);
    Ok(())
}

/// packed 数组在索引处写入；仅处理非字典、索引在 capacity 内且值为非 boxed 数字的快路径。
fn lower_packed_array_store(
    cx: &mut LoweringCx<'_, '_>,
    barrier_thunks: &BarrierThunks,
    object: ir::Value,
    index: ir::Value,
    stored: ir::Value,
    dest: Option<ValueId>,
    miss_block: ir::Block,
    merge_block: ir::Block,
) -> Result<()> {
    let box_base = i64::from_ne_bytes(value::BOX_BASE.to_ne_bytes());
    let boxed_bits = cx.builder.ins().band_imm_s(stored, box_base);
    let is_heap_value =
        cx.builder
            .ins()
            .icmp_imm_s(ir::condcodes::IntCC::Equal, boxed_bits, box_base);
    let needs_host = cx.builder.create_block();
    let array_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(is_heap_value, needs_host, &[], array_block, &[]);

    cx.builder.switch_to_block(needs_host);
    cx.builder.seal_block(needs_host);
    cx.builder.ins().jump(miss_block, &[]);

    cx.builder.switch_to_block(array_block);
    cx.builder.seal_block(array_block);
    let address = emit_array_address(cx, barrier_thunks, object, miss_block)?;
    let header = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), address, 0);
    let kind = cx.builder.ins().ushr_imm_u(header, 40);
    let kind = cx.builder.ins().band_imm_u(kind, 0xff);
    let is_dict = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        kind,
        i64::from(wjsm_ir::constants::ARRAY_KIND_DICTIONARY),
    );
    let dict_check_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(is_dict, miss_block, &[], dict_check_block, &[]);

    cx.builder.switch_to_block(dict_check_block);
    cx.builder.seal_block(dict_check_block);
    let shape = cx.builder.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        address,
        i32::try_from(constants::HEAP_ARRAY_LENGTH_OFFSET).expect("length offset fits i32"),
    );
    let length = cx.builder.ins().band_imm_u(shape, i64::from(u32::MAX));
    let capacity = cx.builder.ins().ushr_imm_u(shape, 32);
    let index_u64 = index;
    let in_capacity =
        cx.builder
            .ins()
            .icmp(ir::condcodes::IntCC::UnsignedLessThan, index_u64, capacity);
    let not_past_length = cx.builder.ins().icmp(
        ir::condcodes::IntCC::UnsignedLessThanOrEqual,
        index_u64,
        length,
    );
    let append = cx
        .builder
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, index_u64, length);
    let in_bounds = cx.builder.ins().band(in_capacity, not_past_length);
    let store_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(in_bounds, store_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(store_block);
    cx.builder.seal_block(store_block);
    let index_bytes = cx.builder.ins().ishl_imm_u(index_u64, 3);
    let elem_offset = cx
        .builder
        .ins()
        .iadd_imm_s(index_bytes, i64::from(constants::HEAP_OBJECT_HEADER_SIZE));
    let elem_addr = cx.builder.ins().iadd(address, elem_offset);
    cx.builder
        .ins()
        .store(MemFlagsData::trusted(), stored, elem_addr, 0);
    let after_store_block = cx.builder.create_block();
    let append_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(append, append_block, &[], after_store_block, &[]);

    cx.builder.switch_to_block(after_store_block);
    cx.builder.seal_block(after_store_block);
    if let Some(dest) = dest {
        define_value_boxed(cx.builder, cx.variables, dest, stored)?;
    }
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(append_block);
    cx.builder.seal_block(append_block);
    let new_length = cx.builder.ins().iadd_imm_u(index_u64, 1);
    let new_shape = cx.builder.ins().ishl_imm_u(capacity, 32);
    let new_shape = cx.builder.ins().bor(new_shape, new_length);
    cx.builder.ins().store(
        MemFlagsData::trusted(),
        new_shape,
        address,
        i32::try_from(constants::HEAP_ARRAY_LENGTH_OFFSET).expect("length offset fits i32"),
    );
    if let Some(dest) = dest {
        define_value_boxed(cx.builder, cx.variables, dest, stored)?;
    }
    cx.builder.ins().jump(merge_block, &[]);
    Ok(())
}

fn lower_set_elem(
    cx: &mut LoweringCx<'_, '_>,
    barrier_thunks: &BarrierThunks,
    dest: ValueId,
    object: ValueId,
    index: ValueId,
    stored: ValueId,
) -> Result<()> {
    let object_val = use_value_boxed(cx.builder, cx.variables, object)?;
    let encoded_index = use_value_boxed(cx.builder, cx.variables, index)?;
    let stored_val = use_value_boxed(cx.builder, cx.variables, stored)?;
    let (index_val, valid_index) = emit_nonnegative_integer_index(cx.builder, encoded_index);
    let index_block = cx.builder.create_block();
    let array_block = cx.builder.create_block();
    let miss_block = cx.builder.create_block();
    let merge_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(valid_index, index_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(index_block);
    cx.builder.seal_block(index_block);
    let tag_word = cx.builder.ins().ushr_imm_u(object_val, 32);
    let tag = cx.builder.ins().band_imm_u(
        tag_word,
        i64::try_from(value::TAG_MASK).expect("tag mask fits i64"),
    );
    let is_array = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        tag,
        i64::try_from(value::TAG_ARRAY).expect("array tag fits i64"),
    );
    cx.builder
        .ins()
        .brif(is_array, array_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(array_block);
    cx.builder.seal_block(array_block);
    lower_packed_array_store(
        cx,
        barrier_thunks,
        object_val,
        index_val,
        stored_val,
        Some(dest),
        miss_block,
        merge_block,
    )?;

    cx.builder.switch_to_block(miss_block);
    cx.builder.seal_block(miss_block);
    let result = cx.call(
        NativeRuntimeOp::SetElem.id(),
        &[object_val, encoded_index, stored_val],
        None,
    )?;
    define_value_boxed(cx.builder, cx.variables, dest, result)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(merge_block);
    cx.builder.seal_block(merge_block);
    Ok(())
}

fn lower_array_push_inline(
    cx: &mut LoweringCx<'_, '_>,
    barrier_thunks: &BarrierThunks,
    object: ValueId,
    stored: ValueId,
) -> Result<()> {
    let object_val = use_value_boxed(cx.builder, cx.variables, object)?;
    let stored_val = use_value_boxed(cx.builder, cx.variables, stored)?;
    let miss_block = cx.builder.create_block();
    let array_block = cx.builder.create_block();
    let merge_block = cx.builder.create_block();
    let tag_word = cx.builder.ins().ushr_imm_u(object_val, 32);
    let tag = cx.builder.ins().band_imm_u(
        tag_word,
        i64::try_from(value::TAG_MASK).expect("tag mask fits i64"),
    );
    let is_array = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        tag,
        i64::try_from(value::TAG_ARRAY).expect("array tag fits i64"),
    );
    cx.builder
        .ins()
        .brif(is_array, array_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(array_block);
    cx.builder.seal_block(array_block);
    let address = emit_array_address(cx, barrier_thunks, object_val, miss_block)?;
    let shape = cx.builder.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        address,
        i32::try_from(constants::HEAP_ARRAY_LENGTH_OFFSET).expect("length offset fits i32"),
    );
    let length = cx.builder.ins().band_imm_u(shape, i64::from(u32::MAX));
    lower_packed_array_store(
        cx,
        barrier_thunks,
        object_val,
        length,
        stored_val,
        None,
        miss_block,
        merge_block,
    )?;

    cx.builder.switch_to_block(miss_block);
    cx.builder.seal_block(miss_block);
    let _ = cx.call(
        u32::from(Builtin::ArrayPush.wire_id()),
        &[object_val, stored_val],
        None,
    )?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(merge_block);
    cx.builder.seal_block(merge_block);
    Ok(())
}

fn emit_inline_ascii_only_predicate(
    builder: &mut FunctionBuilder<'_>,
    encoded: ir::Value,
) -> ir::Value {
    let is_inline = emit_inline_string_predicate(builder, encoded);
    let marker_bits = builder.ins().band_imm_u(
        encoded,
        i64::try_from(value::INLINE_STRING_MARKER_MASK).expect("SSO marker mask fits i64"),
    );
    let is_ascii = builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        marker_bits,
        i64::try_from(value::INLINE_STRING_MARKER << value::INLINE_STRING_MARKER_SHIFT)
            .expect("ASCII SSO marker fits i64"),
    );
    builder.ins().band(is_inline, is_ascii)
}

fn emit_extract_inline_ascii_unit(
    builder: &mut FunctionBuilder<'_>,
    receiver: ir::Value,
    index: ir::Value,
) -> ir::Value {
    let shift = builder.ins().ishl_imm_u(index, 3);
    let shift = builder.ins().isub(shift, index);
    let unit = builder.ins().ushr(receiver, shift);
    builder.ins().band_imm_u(unit, 0x7f)
}

fn emit_is_inline_latin1_marker(
    builder: &mut FunctionBuilder<'_>,
    encoded: ir::Value,
) -> ir::Value {
    let marker_bits = builder.ins().band_imm_u(
        encoded,
        i64::try_from(value::INLINE_STRING_MARKER_MASK).expect("SSO marker mask fits i64"),
    );
    builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        marker_bits,
        i64::try_from(value::INLINE_STRING_LATIN1_MARKER << value::INLINE_STRING_MARKER_SHIFT)
            .expect("Latin-1 SSO marker fits i64"),
    )
}

fn emit_extract_inline_latin1_unit(
    builder: &mut FunctionBuilder<'_>,
    receiver: ir::Value,
    index: ir::Value,
) -> ir::Value {
    let payload = builder.ins().band_imm_u(
        receiver,
        i64::try_from(value::INLINE_STRING_PAYLOAD_MASK).expect("SSO payload mask fits i64"),
    );
    let shift = builder.ins().ishl_imm_u(index, 3);
    let unit = builder.ins().ushr(payload, shift);
    builder.ins().band_imm_u(unit, 0xff)
}

fn emit_unsigned_min(
    builder: &mut FunctionBuilder<'_>,
    lhs: ir::Value,
    rhs: ir::Value,
) -> ir::Value {
    let less = builder
        .ins()
        .icmp(ir::condcodes::IntCC::UnsignedLessThan, lhs, rhs);
    builder.ins().select(less, lhs, rhs)
}

fn emit_unsigned_max(
    builder: &mut FunctionBuilder<'_>,
    lhs: ir::Value,
    rhs: ir::Value,
) -> ir::Value {
    let greater = builder
        .ins()
        .icmp(ir::condcodes::IntCC::UnsignedGreaterThan, lhs, rhs);
    builder.ins().select(greater, lhs, rhs)
}

fn emit_relative_slice_index(
    builder: &mut FunctionBuilder<'_>,
    encoded: ir::Value,
    length: ir::Value,
) -> ir::Value {
    let number = builder
        .ins()
        .bitcast(types::F64, ir::MemFlagsData::new(), encoded);
    let index = builder.ins().fcvt_to_sint_sat(types::I64, number);
    let negative = builder
        .ins()
        .icmp_imm_s(ir::condcodes::IntCC::SignedLessThan, index, 0);
    let relative = builder.ins().iadd(length, index);
    let zero = builder.ins().iconst(types::I64, 0);
    let clamped_negative = emit_unsigned_max(builder, relative, zero);
    let clamped_positive = emit_unsigned_min(builder, index, length);
    builder
        .ins()
        .select(negative, clamped_negative, clamped_positive)
}

fn emit_pack_inline_ascii_slice(
    cx: &mut LoweringCx<'_, '_>,
    receiver: ir::Value,
    start: ir::Value,
    end: ir::Value,
) -> ir::Value {
    let result_len = cx.builder.ins().isub(end, start);
    let head = cx.builder.create_block();
    cx.builder.append_block_param(head, types::I64);
    cx.builder.append_block_param(head, types::I64);
    let done = cx.builder.create_block();
    cx.builder.append_block_param(done, types::I64);
    let body = cx.builder.create_block();

    let base = cx.builder.ins().iconst(
        types::I64,
        i64::from_ne_bytes(value::BOX_BASE.to_ne_bytes()),
    );
    let marker = cx.builder.ins().iconst(
        types::I64,
        i64::try_from(value::INLINE_STRING_MARKER << value::INLINE_STRING_MARKER_SHIFT)
            .expect("SSO marker fits i64"),
    );
    let base_payload = cx.builder.ins().bor(base, marker);
    let zero = cx.builder.ins().iconst(types::I64, 0);
    cx.builder.ins().jump(
        head,
        &[ir::BlockArg::Value(zero), ir::BlockArg::Value(base_payload)],
    );

    cx.builder.switch_to_block(head);
    let index = cx.builder.block_params(head)[0];
    let payload = cx.builder.block_params(head)[1];
    let finished = cx.builder.ins().icmp(
        ir::condcodes::IntCC::UnsignedGreaterThanOrEqual,
        index,
        result_len,
    );
    cx.builder
        .ins()
        .brif(finished, done, &[ir::BlockArg::Value(payload)], body, &[]);

    cx.builder.switch_to_block(body);
    let src_index = cx.builder.ins().iadd(start, index);
    let unit = emit_extract_inline_ascii_unit(cx.builder, receiver, src_index);
    let shift = cx.builder.ins().ishl_imm_u(index, 3);
    let shift = cx.builder.ins().isub(shift, index);
    let shifted = cx.builder.ins().ishl(unit, shift);
    let merged = cx.builder.ins().bor(payload, shifted);
    let next = cx.builder.ins().iadd_imm_u(index, 1);
    cx.builder.ins().jump(
        head,
        &[ir::BlockArg::Value(next), ir::BlockArg::Value(merged)],
    );

    cx.builder.switch_to_block(done);
    cx.builder.seal_block(head);
    cx.builder.seal_block(body);
    let payload = cx.builder.block_params(done)[0];
    let length_bits = cx
        .builder
        .ins()
        .ishl_imm_u(result_len, i64::from(value::INLINE_STRING_LENGTH_SHIFT));
    cx.builder.ins().bor(payload, length_bits)
}

fn lower_string_slice_builtin(
    cx: &mut LoweringCx<'_, '_>,
    dest: ValueId,
    args: &[ValueId],
    feedback_ptr: Option<ir::Value>,
) -> Result<()> {
    let receiver = use_value_boxed(cx.builder, cx.variables, args[0])?;
    let encoded_start = if let Some(start) = args.get(1) {
        use_value_boxed(cx.builder, cx.variables, *start)?
    } else {
        cx.builder.ins().iconst(types::I64, value::encode_f64(0.0))
    };
    let encoded_end = if let Some(end) = args.get(2) {
        Some(use_value_boxed(cx.builder, cx.variables, *end)?)
    } else {
        None
    };

    let ascii_block = cx.builder.create_block();
    let miss_block = cx.builder.create_block();
    let merge_block = cx.builder.create_block();
    let is_ascii = emit_inline_ascii_only_predicate(cx.builder, receiver);
    cx.builder
        .ins()
        .brif(is_ascii, ascii_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(ascii_block);
    cx.builder.seal_block(ascii_block);
    let inline_length = cx
        .builder
        .ins()
        .ushr_imm_u(receiver, i64::from(value::INLINE_STRING_LENGTH_SHIFT));
    let inline_length = cx.builder.ins().band_imm_u(inline_length, 0b111);
    let start_is_number = emit_is_number(cx.builder, encoded_start);
    let end_is_number = if let Some(encoded_end) = encoded_end {
        emit_is_number(cx.builder, encoded_end)
    } else {
        cx.builder.ins().iconst(types::I8, 1)
    };
    let bounds_block = cx.builder.create_block();
    let bounds_ok = cx.builder.ins().band(start_is_number, end_is_number);
    cx.builder
        .ins()
        .brif(bounds_ok, bounds_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(bounds_block);
    cx.builder.seal_block(bounds_block);
    let start = emit_relative_slice_index(cx.builder, encoded_start, inline_length);
    let end = if let Some(encoded_end) = encoded_end {
        emit_relative_slice_index(cx.builder, encoded_end, inline_length)
    } else {
        inline_length
    };
    let empty_block = cx.builder.create_block();
    let slice_block = cx.builder.create_block();
    let end_before_start =
        cx.builder
            .ins()
            .icmp(ir::condcodes::IntCC::UnsignedLessThan, end, start);
    cx.builder
        .ins()
        .brif(end_before_start, empty_block, &[], slice_block, &[]);

    cx.builder.switch_to_block(empty_block);
    cx.builder.seal_block(empty_block);
    let empty = cx.builder.ins().iconst(
        types::I64,
        value::encode_inline_ascii(b"").expect("empty inline ascii"),
    );
    define_value_boxed(cx.builder, cx.variables, dest, empty)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(slice_block);
    cx.builder.seal_block(slice_block);
    let sliced = emit_pack_inline_ascii_slice(cx, receiver, start, end);
    define_value_boxed(cx.builder, cx.variables, dest, sliced)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(miss_block);
    cx.builder.seal_block(miss_block);
    let mut call_args = vec![receiver, encoded_start];
    if let Some(end) = encoded_end {
        call_args.push(end);
    }
    let result = cx.call(
        u32::from(Builtin::StringSlice.wire_id()),
        &call_args,
        feedback_ptr,
    )?;
    define_value_boxed(cx.builder, cx.variables, dest, result)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(merge_block);
    cx.builder.seal_block(merge_block);
    Ok(())
}

fn emit_inline_ascii_char_value(cx: &mut LoweringCx<'_, '_>, unit: ir::Value) -> ir::Value {
    let base = cx.builder.ins().iconst(
        types::I64,
        i64::from_ne_bytes(value::BOX_BASE.to_ne_bytes()),
    );
    let marker = cx.builder.ins().iconst(
        types::I64,
        i64::try_from(value::INLINE_STRING_MARKER << value::INLINE_STRING_MARKER_SHIFT)
            .expect("SSO marker fits i64"),
    );
    let length = cx
        .builder
        .ins()
        .iconst(types::I64, 1_i64 << value::INLINE_STRING_LENGTH_SHIFT);
    let result = cx.builder.ins().bor(base, marker);
    let result = cx.builder.ins().bor(result, length);
    let unit = cx.builder.ins().band_imm_u(unit, 0x7f);
    cx.builder.ins().bor(result, unit)
}

fn lower_string_char_builtin(
    cx: &mut LoweringCx<'_, '_>,
    barrier_thunks: &BarrierThunks,
    dest: ValueId,
    builtin: Builtin,
    args: &[ValueId],
    feedback_ptr: Option<ir::Value>,
) -> Result<()> {
    let receiver = use_value_boxed(cx.builder, cx.variables, args[0])?;
    let encoded_index = if let Some(index) = args.get(1) {
        use_value_boxed(cx.builder, cx.variables, *index)?
    } else {
        cx.builder.ins().iconst(types::I64, value::encode_f64(0.0))
    };
    let (index, valid_index) = emit_nonnegative_integer_index(cx.builder, encoded_index);
    let index_block = cx.builder.create_block();
    let inline_string_block = cx.builder.create_block();
    let inline_char_block = cx.builder.create_block();
    let string_block = cx.builder.create_block();
    let miss_block = cx.builder.create_block();
    let out_of_bounds_block = cx.builder.create_block();
    let merge_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(valid_index, index_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(index_block);
    cx.builder.seal_block(index_block);
    let is_inline = emit_inline_string_predicate(cx.builder, receiver);
    cx.builder
        .ins()
        .brif(is_inline, inline_string_block, &[], string_block, &[]);

    cx.builder.switch_to_block(inline_string_block);
    cx.builder.seal_block(inline_string_block);
    let inline_length = cx
        .builder
        .ins()
        .ushr_imm_u(receiver, i64::from(value::INLINE_STRING_LENGTH_SHIFT));
    let inline_length = cx.builder.ins().band_imm_u(inline_length, 0b111);
    let in_bounds =
        cx.builder
            .ins()
            .icmp(ir::condcodes::IntCC::UnsignedLessThan, index, inline_length);
    cx.builder
        .ins()
        .brif(in_bounds, inline_char_block, &[], out_of_bounds_block, &[]);

    cx.builder.switch_to_block(inline_char_block);
    cx.builder.seal_block(inline_char_block);
    let inline_latin1_char_block = cx.builder.create_block();
    let inline_ascii_char_block = cx.builder.create_block();
    let is_inline_latin1 = emit_is_inline_latin1_marker(cx.builder, receiver);
    cx.builder.ins().brif(
        is_inline_latin1,
        inline_latin1_char_block,
        &[],
        inline_ascii_char_block,
        &[],
    );

    cx.builder.switch_to_block(inline_ascii_char_block);
    cx.builder.seal_block(inline_ascii_char_block);
    let ascii_unit = emit_extract_inline_ascii_unit(cx.builder, receiver, index);
    let result = if builtin == Builtin::StringCharCodeAt {
        let unit = cx.builder.ins().fcvt_from_uint(types::F64, ascii_unit);
        box_f64_result(cx.builder, unit)
    } else {
        emit_inline_ascii_char_value(cx, ascii_unit)
    };
    define_value_boxed(cx.builder, cx.variables, dest, result)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(inline_latin1_char_block);
    cx.builder.seal_block(inline_latin1_char_block);
    let latin1_unit = emit_extract_inline_latin1_unit(cx.builder, receiver, index);
    let result = if builtin == Builtin::StringCharCodeAt {
        let unit = cx.builder.ins().fcvt_from_uint(types::F64, latin1_unit);
        box_f64_result(cx.builder, unit)
    } else {
        emit_latin1_char_handle(cx, latin1_unit, miss_block)?
    };
    define_value_boxed(cx.builder, cx.variables, dest, result)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(string_block);
    cx.builder.seal_block(string_block);
    let address = emit_string_address(cx, barrier_thunks, receiver, miss_block)?;
    let unit = emit_flat_string_code_unit(cx, address, index, miss_block, out_of_bounds_block);
    let result = if builtin == Builtin::StringCharCodeAt {
        let unit = cx.builder.ins().fcvt_from_uint(types::F64, unit);
        box_f64_result(cx.builder, unit)
    } else {
        emit_latin1_char_handle(cx, unit, miss_block)?
    };
    define_value_boxed(cx.builder, cx.variables, dest, result)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(out_of_bounds_block);
    cx.builder.seal_block(out_of_bounds_block);
    if builtin == Builtin::StringCharCodeAt {
        let result = cx
            .builder
            .ins()
            .iconst(types::I64, value::encode_f64(f64::NAN));
        define_value_boxed(cx.builder, cx.variables, dest, result)?;
        cx.builder.ins().jump(merge_block, &[]);
    } else {
        cx.builder.ins().jump(miss_block, &[]);
    }

    cx.builder.switch_to_block(miss_block);
    cx.builder.seal_block(miss_block);
    let result = cx.call(
        u32::from(builtin.wire_id()),
        &[receiver, encoded_index],
        feedback_ptr,
    )?;
    define_value_boxed(cx.builder, cx.variables, dest, result)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(merge_block);
    cx.builder.seal_block(merge_block);
    Ok(())
}

// ── 非逃逸字符串累加器的内联追加（阶段 3）──
//
// 快路径条件：current 是堆内 BUILDER、全部片段为 flat 字符串（数字片段要求
// 安全整数）、剩余容量充足，并且 entry 稳定 + ZGC 搬迁未激活（access epoch
// 为偶）。满足时直接把码元写入 payload 并就地更新 length，零宿主往返；容量
// 不足（增长走宿主搬迁）或任何守卫不满足时回落宿主 thunk，语义与未内联时
// 完全一致。并发标记期间照常直写：payload 与 length 属纯数据，标记器不扫描
// builder 载荷，宿主侧 `write_string_payload` 同样不因标记活跃而阻塞。

/// 写入路径专用的保守字符串地址解析。
///
/// 与 `emit_string_address` 的差异：不做 load assist。assist 之后并发搬迁仍
/// 可能与随后的直写竞争，因此只有在 entry 稳定且 ZGC 的 access epoch 为偶
/// （搬迁未激活）时才返回地址，其余一律进 miss 块。
fn emit_idle_string_address(
    cx: &mut LoweringCx<'_, '_>,
    encoded: ir::Value,
    miss_block: ir::Block,
) -> Result<ir::Value> {
    let pointer_type = cx.builder.func.dfg.value_type(cx.ctx);
    let entry_block = cx.builder.create_block();
    let boxed_bits = cx
        .builder
        .ins()
        .band_imm_s(encoded, i64::from_ne_bytes(value::BOX_BASE.to_ne_bytes()));
    let is_boxed = cx.builder.ins().icmp_imm_s(
        ir::condcodes::IntCC::Equal,
        boxed_bits,
        i64::from_ne_bytes(value::BOX_BASE.to_ne_bytes()),
    );
    let tag_word = cx.builder.ins().ushr_imm_u(encoded, 32);
    let tag = cx.builder.ins().band_imm_u(
        tag_word,
        i64::try_from(value::TAG_MASK).expect("tag mask fits i64"),
    );
    let is_string = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        tag,
        i64::try_from(value::TAG_STRING).expect("string tag fits i64"),
    );
    let runtime_flag = cx.builder.ins().band_imm_u(
        tag_word,
        i64::try_from(value::STRING_RUNTIME_HANDLE_FLAG).expect("runtime flag fits i64"),
    );
    let is_runtime = cx
        .builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::NotEqual, runtime_flag, 0);
    let valid = cx.builder.ins().band(is_boxed, is_string);
    let valid = cx.builder.ins().band(valid, is_runtime);
    let inline = emit_inline_string_predicate(cx.builder, encoded);
    let not_inline = cx.builder.ins().bnot(inline);
    let valid = cx.builder.ins().band(valid, not_inline);
    cx.builder
        .ins()
        .brif(valid, entry_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(entry_block);
    cx.builder.seal_block(entry_block);
    let handle = cx.builder.ins().band_imm_u(encoded, i64::from(u32::MAX));
    let handle_table = cx.ht_base;
    let entry_offset = cx.builder.ins().ishl_imm_u(handle, 3);
    let entry_address = cx.builder.ins().iadd(handle_table, entry_offset);
    let entry = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), entry_address, 0);
    let state = cx.builder.ins().band_imm_u(entry, 0xffff);
    let stable = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::UnsignedGreaterThanOrEqual,
        state,
        i64::from(constants::HANDLE_STATE_STABLE_MIN),
    );
    let logical_address = cx.builder.ins().ushr_imm_u(entry, 16);
    let barrier_state = cx.barrier_state;
    let barrier_disabled =
        cx.builder
            .ins()
            .icmp_imm_u(ir::condcodes::IntCC::Equal, barrier_state, 0);
    let legacy_block = cx.builder.create_block();
    let zgc_block = cx.builder.create_block();
    let fast_block = cx.builder.create_block();
    let resolved_block = cx.builder.create_block();
    cx.builder.append_block_param(resolved_block, types::I64);
    cx.builder
        .ins()
        .brif(barrier_disabled, legacy_block, &[], zgc_block, &[]);

    cx.builder.switch_to_block(legacy_block);
    cx.builder.seal_block(legacy_block);
    cx.builder.ins().brif(
        stable,
        resolved_block,
        &[ir::BlockArg::Value(logical_address)],
        miss_block,
        &[],
    );

    cx.builder.switch_to_block(zgc_block);
    cx.builder.seal_block(zgc_block);
    let epoch_address = cx.builder.ins().iadd_imm_s(
        barrier_state,
        i64::try_from(offset_of!(NativeBarrierState, access_epoch))
            .expect("access epoch offset fits i64"),
    );
    let epoch = cx
        .builder
        .ins()
        .atomic_load(types::I64, MemFlagsData::trusted(), epoch_address);
    let epoch_bit = cx.builder.ins().band_imm_u(epoch, 1);
    let epoch_even = cx
        .builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::Equal, epoch_bit, 0);
    let direct = cx.builder.ins().band(stable, epoch_even);
    cx.builder
        .ins()
        .brif(direct, fast_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(fast_block);
    cx.builder.seal_block(fast_block);
    increment_barrier_counter(
        cx.builder,
        barrier_state,
        offset_of!(NativeBarrierState, store_fast_events),
    );
    cx.builder
        .ins()
        .jump(resolved_block, &[ir::BlockArg::Value(logical_address)]);

    cx.builder.switch_to_block(resolved_block);
    cx.builder.seal_block(resolved_block);
    let logical_address = cx.builder.block_params(resolved_block)[0];
    let heap_delta = cx.builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, heap_object_delta))?,
    );
    Ok(cx.builder.ins().iadd(logical_address, heap_delta))
}

/// 读字符串头 `+0` word 并提取 repr 字节（`+5`）。
fn emit_string_repr(builder: &mut FunctionBuilder<'_>, address: ir::Value) -> ir::Value {
    let first_word = builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), address, 0);
    let repr = builder.ins().ushr_imm_u(
        first_word,
        i64::from(constants::HEAP_STRING_REPR_OFFSET * 8),
    );
    builder.ins().band_imm_u(repr, 0xff)
}

/// 内联追加的 builder 状态：对象地址、当前码元长度与字节容量。
struct InlineBuilderState {
    address: ir::Value,
    length: ir::Value,
    capacity: ir::Value,
}

/// 解析累加器 current 为 BUILDER repr 的堆对象并读出长度/容量；其余形态进
/// miss（首建 builder、flat 化后的再追加都由宿主处理）。
fn emit_inline_builder_state(
    cx: &mut LoweringCx<'_, '_>,
    current: ir::Value,
    miss_block: ir::Block,
) -> Result<InlineBuilderState> {
    let address = emit_idle_string_address(cx, current, miss_block)?;
    let repr = emit_string_repr(cx.builder, address);
    let is_builder = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        repr,
        i64::from(constants::STRING_REPR_BUILDER),
    );
    let builder_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(is_builder, builder_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(builder_block);
    cx.builder.seal_block(builder_block);
    let length_capacity = cx.builder.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        address,
        i32::try_from(constants::HEAP_STRING_LENGTH_OFFSET).expect("length offset fits i32"),
    );
    let length = cx
        .builder
        .ins()
        .band_imm_u(length_capacity, i64::from(u32::MAX));
    let capacity = cx.builder.ins().ushr_imm_u(length_capacity, 32);
    Ok(InlineBuilderState {
        address,
        length,
        capacity,
    })
}

/// 已解析的 flat 字符串片段：payload 地址、是否 Latin-1、码元数。
struct InlineStringPart {
    payload: ir::Value,
    is_latin1: ir::Value,
    units: ir::Value,
}

/// 解析字符串片段；仅 Latin-1/UTF-16 flat 直拷，Cons/Slice/builder 片段进 miss。
fn emit_inline_string_part(
    cx: &mut LoweringCx<'_, '_>,
    encoded: ir::Value,
    miss_block: ir::Block,
) -> Result<InlineStringPart> {
    let address = emit_idle_string_address(cx, encoded, miss_block)?;
    let repr = emit_string_repr(cx.builder, address);
    let is_latin1 = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        repr,
        i64::from(constants::STRING_REPR_LATIN1_FLAT),
    );
    let is_utf16 = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        repr,
        i64::from(constants::STRING_REPR_UTF16_FLAT),
    );
    let flat = cx.builder.ins().bor(is_latin1, is_utf16);
    let flat_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(flat, flat_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(flat_block);
    cx.builder.seal_block(flat_block);
    let length_word = cx.builder.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        address,
        i32::try_from(constants::HEAP_STRING_LENGTH_OFFSET).expect("length offset fits i32"),
    );
    let units = cx
        .builder
        .ins()
        .band_imm_u(length_word, i64::from(u32::MAX));
    let payload = cx
        .builder
        .ins()
        .iadd_imm_s(address, i64::from(constants::HEAP_STRING_PAYLOAD_OFFSET));
    Ok(InlineStringPart {
        payload,
        is_latin1,
        units,
    })
}

/// UTF-16 flat 片段 → builder payload 的逐码元拷贝循环。
fn emit_copy_utf16_part(
    cx: &mut LoweringCx<'_, '_>,
    part: &InlineStringPart,
    dst: ir::Value,
    done_block: ir::Block,
) {
    let head = cx.builder.create_block();
    cx.builder.append_block_param(head, types::I64);
    let zero = cx.builder.ins().iconst(types::I64, 0);
    cx.builder.ins().jump(head, &[ir::BlockArg::Value(zero)]);

    cx.builder.switch_to_block(head);
    let index = cx.builder.block_params(head)[0];
    let more = cx
        .builder
        .ins()
        .icmp(ir::condcodes::IntCC::UnsignedLessThan, index, part.units);
    let body = cx.builder.create_block();
    cx.builder.ins().brif(more, body, &[], done_block, &[]);

    cx.builder.switch_to_block(body);
    let byte_offset = cx.builder.ins().ishl_imm_u(index, 1);
    let src = cx.builder.ins().iadd(part.payload, byte_offset);
    let unit = cx
        .builder
        .ins()
        .load(types::I16, MemFlagsData::trusted(), src, 0);
    let dst_unit = cx.builder.ins().iadd(dst, byte_offset);
    cx.builder
        .ins()
        .store(MemFlagsData::trusted(), unit, dst_unit, 0);
    let next = cx.builder.ins().iadd_imm_u(index, 1);
    cx.builder.ins().jump(head, &[ir::BlockArg::Value(next)]);
    cx.builder.seal_block(head);
    cx.builder.seal_block(body);
}

/// Latin-1 flat 片段 → builder UTF-16 payload 的逐码元加宽拷贝循环。
fn emit_copy_latin1_part(
    cx: &mut LoweringCx<'_, '_>,
    part: &InlineStringPart,
    dst: ir::Value,
    done_block: ir::Block,
) {
    let head = cx.builder.create_block();
    cx.builder.append_block_param(head, types::I64);
    let zero = cx.builder.ins().iconst(types::I64, 0);
    cx.builder.ins().jump(head, &[ir::BlockArg::Value(zero)]);

    cx.builder.switch_to_block(head);
    let index = cx.builder.block_params(head)[0];
    let more = cx
        .builder
        .ins()
        .icmp(ir::condcodes::IntCC::UnsignedLessThan, index, part.units);
    let body = cx.builder.create_block();
    cx.builder.ins().brif(more, body, &[], done_block, &[]);

    cx.builder.switch_to_block(body);
    let src = cx.builder.ins().iadd(part.payload, index);
    let unit = cx
        .builder
        .ins()
        .load(types::I8, MemFlagsData::trusted(), src, 0);
    let unit = cx.builder.ins().uextend(types::I16, unit);
    let dst_byte_offset = cx.builder.ins().ishl_imm_u(index, 1);
    let dst_unit = cx.builder.ins().iadd(dst, dst_byte_offset);
    cx.builder
        .ins()
        .store(MemFlagsData::trusted(), unit, dst_unit, 0);
    let next = cx.builder.ins().iadd_imm_u(index, 1);
    cx.builder.ins().jump(head, &[ir::BlockArg::Value(next)]);
    cx.builder.seal_block(head);
    cx.builder.seal_block(body);
}

/// 按片段表示分派拷贝循环，返回继续块。
fn emit_copy_part_dispatch(
    cx: &mut LoweringCx<'_, '_>,
    part: &InlineStringPart,
    dst: ir::Value,
) -> ir::Block {
    let done = cx.builder.create_block();
    let latin1_head = cx.builder.create_block();
    let utf16_head = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(part.is_latin1, latin1_head, &[], utf16_head, &[]);

    cx.builder.switch_to_block(latin1_head);
    cx.builder.seal_block(latin1_head);
    emit_copy_latin1_part(cx, part, dst, done);

    cx.builder.switch_to_block(utf16_head);
    cx.builder.seal_block(utf16_head);
    emit_copy_utf16_part(cx, part, dst, done);

    cx.builder.switch_to_block(done);
    cx.builder.seal_block(done);
    done
}

/// `0 ≤ magnitude ≤ 2^53-1` 的十进制位数（1..=16）：对 10 的幂做比较阶梯，
/// 无除法。
fn emit_decimal_digit_count(builder: &mut FunctionBuilder<'_>, magnitude: ir::Value) -> ir::Value {
    let mut digits = builder.ins().iconst(types::I64, 1);
    for exponent in 1..=15 {
        let threshold = builder.ins().iconst(
            types::I64,
            10_i64.pow(u32::try_from(exponent).expect("≤15")),
        );
        let reached = builder.ins().icmp(
            ir::condcodes::IntCC::UnsignedGreaterThanOrEqual,
            magnitude,
            threshold,
        );
        let count = builder.ins().iconst(types::I64, i64::from(exponent) + 1);
        digits = builder.ins().select(reached, count, digits);
    }
    digits
}

/// 宿主回落的统一出口：dispatcher 承载全部通用语义（builder 首建、增长、
/// 非安全整数格式化、非字符串片段）。
fn emit_string_builder_append_miss(
    cx: &mut LoweringCx<'_, '_>,
    dest: ValueId,
    args: &[ir::Value],
    feedback_ptr: Option<ir::Value>,
    miss_block: ir::Block,
    merge_block: ir::Block,
) -> Result<()> {
    cx.builder.switch_to_block(miss_block);
    cx.builder.seal_block(miss_block);
    let result = cx.call(
        u32::from(Builtin::StringBuilderAppend.wire_id()),
        args,
        feedback_ptr,
    )?;
    define_value_boxed(cx.builder, cx.variables, dest, result)?;
    cx.builder.ins().jump(merge_block, &[]);
    Ok(())
}

/// 内联更新 builder length（`+8` word 低 32 位，高 32 位 capacity 不动）。
fn emit_store_builder_length(
    cx: &mut LoweringCx<'_, '_>,
    builder: &InlineBuilderState,
    total_units: ir::Value,
) {
    let length = cx.builder.ins().ireduce(types::I32, total_units);
    cx.builder.ins().store(
        MemFlagsData::trusted(),
        length,
        builder.address,
        i32::try_from(constants::HEAP_STRING_LENGTH_OFFSET).expect("length offset fits i32"),
    );
}

/// `string + f64` 片段的数字位写入:负号、位数阶梯与两位一组的 itoa 反向
/// 写入。调用后当前块保持打开,由调用方收尾。
fn emit_append_number_digits(
    cx: &mut LoweringCx<'_, '_>,
    payload_base: ir::Value,
    start_units: ir::Value,
    negative: ir::Value,
    magnitude: ir::Value,
    digits: ir::Value,
    len_store_block: ir::Block,
) {
    let minus_block = cx.builder.create_block();
    let digits_entry = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(negative, minus_block, &[], digits_entry, &[]);

    cx.builder.switch_to_block(minus_block);
    cx.builder.seal_block(minus_block);
    let minus_offset_bytes = cx.builder.ins().ishl_imm_u(start_units, 1);
    let minus_address = cx.builder.ins().iadd(payload_base, minus_offset_bytes);
    let minus = cx.builder.ins().iconst(types::I16, i64::from(b'-'));
    cx.builder
        .ins()
        .store(MemFlagsData::trusted(), minus, minus_address, 0);
    cx.builder.ins().jump(digits_entry, &[]);

    cx.builder.switch_to_block(digits_entry);
    cx.builder.seal_block(digits_entry);
    let write_pos = cx.builder.ins().iadd(start_units, digits);
    let digit_loop = cx.builder.create_block();
    cx.builder.append_block_param(digit_loop, types::I64);
    cx.builder.append_block_param(digit_loop, types::I64);
    let is_zero = cx
        .builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::Equal, magnitude, 0);
    let zero_block = cx.builder.create_block();
    cx.builder.ins().brif(
        is_zero,
        zero_block,
        &[],
        digit_loop,
        &[
            ir::BlockArg::Value(magnitude),
            ir::BlockArg::Value(write_pos),
        ],
    );

    cx.builder.switch_to_block(zero_block);
    cx.builder.seal_block(zero_block);
    let zero_char = cx.builder.ins().iconst(types::I16, i64::from(b'0'));
    let zero_pos = cx.builder.ins().iadd_imm_u(write_pos, -1);
    let zero_offset = cx.builder.ins().ishl_imm_u(zero_pos, 1);
    let zero_address = cx.builder.ins().iadd(payload_base, zero_offset);
    cx.builder
        .ins()
        .store(MemFlagsData::trusted(), zero_char, zero_address, 0);
    cx.builder.ins().jump(len_store_block, &[]);

    cx.builder.switch_to_block(digit_loop);
    let m = cx.builder.block_params(digit_loop)[0];
    let pos = cx.builder.block_params(digit_loop)[1];
    let done = cx
        .builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::Equal, m, 0);
    let leading_block = cx.builder.create_block();
    let single_block = cx.builder.create_block();
    let pair_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(done, len_store_block, &[], leading_block, &[]);

    cx.builder.switch_to_block(leading_block);
    cx.builder.seal_block(leading_block);
    let leading = cx
        .builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::UnsignedLessThan, m, 10);
    cx.builder
        .ins()
        .brif(leading, single_block, &[], pair_block, &[]);

    cx.builder.switch_to_block(pair_block);
    cx.builder.seal_block(pair_block);
    let high = cx.builder.ins().udiv_imm_u(m, 100);
    let pair = cx.builder.ins().urem_imm_u(m, 100);
    let tens = cx.builder.ins().udiv_imm_u(pair, 10);
    let ones = cx.builder.ins().urem_imm_u(pair, 10);
    emit_store_digit(cx, payload_base, pos, -1, ones);
    emit_store_digit(cx, payload_base, pos, -2, tens);
    let next_pos = cx.builder.ins().iadd_imm_u(pos, -2);
    cx.builder.ins().jump(
        digit_loop,
        &[ir::BlockArg::Value(high), ir::BlockArg::Value(next_pos)],
    );
    cx.builder.seal_block(digit_loop);

    cx.builder.switch_to_block(single_block);
    cx.builder.seal_block(single_block);
    emit_store_digit(cx, payload_base, pos, -1, m);
    cx.builder.ins().jump(len_store_block, &[]);
}

/// 在 payload 起算的 `pos + delta` 绝对码元位写入一个 '0' 起始的数字码元。
fn emit_store_digit(
    cx: &mut LoweringCx<'_, '_>,
    payload_base: ir::Value,
    pos: ir::Value,
    delta: i64,
    digit: ir::Value,
) {
    let at = cx.builder.ins().iadd_imm_u(pos, delta);
    let offset = cx.builder.ins().ishl_imm_u(at, 1);
    let address = cx.builder.ins().iadd(payload_base, offset);
    let ascii_zero = cx.builder.ins().iconst(types::I64, i64::from(b'0'));
    let unit = cx.builder.ins().iadd(digit, ascii_zero);
    let unit = cx.builder.ins().ireduce(types::I16, unit);
    cx.builder
        .ins()
        .store(MemFlagsData::trusted(), unit, address, 0);
}

/// 非逃逸累加器的内联追加:前缀片段必须为 flat 字符串;最后一个片段按运行时
/// 类型分派——字符串直拷、数字走安全整数 itoa,其余形态(对象/BigInt/非 flat
/// 数字、需要增长)回落宿主 thunk,语义与未内联时完全一致。
fn lower_string_builder_append(
    cx: &mut LoweringCx<'_, '_>,
    dest: ValueId,
    args: &[ValueId],
    feedback_ptr: Option<ir::Value>,
) -> Result<()> {
    let mut values = Vec::with_capacity(args.len());
    for arg in args {
        values.push(use_value_boxed(cx.builder, cx.variables, *arg)?);
    }
    let last = *values
        .last()
        .context("string builder append needs a part")?;
    let miss_block = cx.builder.create_block();
    let merge_block = cx.builder.create_block();

    let builder_state = emit_inline_builder_state(cx, values[0], miss_block)?;
    let mut prefix_parts = Vec::with_capacity(values.len() - 2);
    for encoded in &values[1..values.len() - 1] {
        prefix_parts.push(emit_inline_string_part(cx, *encoded, miss_block)?);
    }

    // 最后片段:先按 flat 字符串解析,tag/repr 不符再进数字分派。
    let number_check_block = cx.builder.create_block();
    let last_part = emit_inline_string_part(cx, last, number_check_block)?;

    // ── 字符串路径:全部片段直拷。──
    let mut string_total = builder_state.length;
    for part in &prefix_parts {
        string_total = cx.builder.ins().iadd(string_total, part.units);
    }
    string_total = cx.builder.ins().iadd(string_total, last_part.units);
    let string_bytes = cx.builder.ins().ishl_imm_u(string_total, 1);
    let string_fits = cx.builder.ins().icmp(
        ir::condcodes::IntCC::UnsignedLessThanOrEqual,
        string_bytes,
        builder_state.capacity,
    );
    let string_write_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(string_fits, string_write_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(string_write_block);
    cx.builder.seal_block(string_write_block);
    let payload_base = cx.builder.ins().iadd_imm_s(
        builder_state.address,
        i64::from(constants::HEAP_STRING_PAYLOAD_OFFSET),
    );
    let mut cursor_units = builder_state.length;
    for part in prefix_parts.iter().chain(std::iter::once(&last_part)) {
        let cursor_bytes = cx.builder.ins().ishl_imm_u(cursor_units, 1);
        let part_dst = cx.builder.ins().iadd(payload_base, cursor_bytes);
        let done = emit_copy_part_dispatch(cx, part, part_dst);
        cx.builder.switch_to_block(done);
        cursor_units = cx.builder.ins().iadd(cursor_units, part.units);
    }
    emit_store_builder_length(cx, &builder_state, string_total);
    define_value_boxed(cx.builder, cx.variables, dest, values[0])?;
    cx.builder.ins().jump(merge_block, &[]);

    // ── 数字路径:末片段是 Number 且为安全整数时内联 itoa。──
    cx.builder.switch_to_block(number_check_block);
    cx.builder.seal_block(number_check_block);
    let is_number = emit_is_number(cx.builder, last);
    let classify_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(is_number, classify_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(classify_block);
    cx.builder.seal_block(classify_block);
    // NaN/±Inf 超出安全整数范围(有序比较对 NaN 恒假),小数无法经 i64
    // roundtrip,全部回落宿主的完整 Number→String 语义。
    let number = cx
        .builder
        .ins()
        .bitcast(types::F64, ir::MemFlagsData::new(), last);
    let magnitude_f64 = cx.builder.ins().fabs(number);
    let bound = cx.builder.ins().f64const(9_007_199_254_740_991.0);
    let in_range = cx.builder.ins().fcmp(
        ir::condcodes::FloatCC::LessThanOrEqual,
        magnitude_f64,
        bound,
    );
    let as_int = cx.builder.ins().fcvt_to_sint_sat(types::I64, number);
    let roundtrip = cx.builder.ins().fcvt_from_sint(types::F64, as_int);
    let exact = cx
        .builder
        .ins()
        .fcmp(ir::condcodes::FloatCC::Equal, number, roundtrip);
    let number_ok = cx.builder.ins().band(in_range, exact);
    let number_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(number_ok, number_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(number_block);
    cx.builder.seal_block(number_block);
    let negative = cx
        .builder
        .ins()
        .icmp_imm_s(ir::condcodes::IntCC::SignedLessThan, as_int, 0);
    let negated = cx.builder.ins().ineg(as_int);
    let magnitude = cx.builder.ins().select(negative, negated, as_int);
    let digits = emit_decimal_digit_count(cx.builder, magnitude);
    let one = cx.builder.ins().iconst(types::I64, 1);
    let zero_units = cx.builder.ins().iconst(types::I64, 0);
    let negative_units = cx.builder.ins().select(negative, one, zero_units);
    let mut number_total = builder_state.length;
    for part in &prefix_parts {
        number_total = cx.builder.ins().iadd(number_total, part.units);
    }
    number_total = cx.builder.ins().iadd(number_total, negative_units);
    number_total = cx.builder.ins().iadd(number_total, digits);
    let number_bytes = cx.builder.ins().ishl_imm_u(number_total, 1);
    let number_fits = cx.builder.ins().icmp(
        ir::condcodes::IntCC::UnsignedLessThanOrEqual,
        number_bytes,
        builder_state.capacity,
    );
    let number_write_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(number_fits, number_write_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(number_write_block);
    cx.builder.seal_block(number_write_block);
    let payload_base = cx.builder.ins().iadd_imm_s(
        builder_state.address,
        i64::from(constants::HEAP_STRING_PAYLOAD_OFFSET),
    );
    let mut cursor_units = builder_state.length;
    for part in &prefix_parts {
        let cursor_bytes = cx.builder.ins().ishl_imm_u(cursor_units, 1);
        let part_dst = cx.builder.ins().iadd(payload_base, cursor_bytes);
        let done = emit_copy_part_dispatch(cx, part, part_dst);
        cx.builder.switch_to_block(done);
        cursor_units = cx.builder.ins().iadd(cursor_units, part.units);
    }
    let len_store_block = cx.builder.create_block();
    emit_append_number_digits(
        cx,
        payload_base,
        cursor_units,
        negative,
        magnitude,
        digits,
        len_store_block,
    );

    cx.builder.switch_to_block(len_store_block);
    cx.builder.seal_block(len_store_block);
    emit_store_builder_length(cx, &builder_state, number_total);
    define_value_boxed(cx.builder, cx.variables, dest, values[0])?;
    cx.builder.ins().jump(merge_block, &[]);

    emit_string_builder_append_miss(cx, dest, &values, feedback_ptr, miss_block, merge_block)?;

    cx.builder.switch_to_block(merge_block);
    cx.builder.seal_block(merge_block);
    Ok(())
}

#[derive(Clone, Copy)]
struct StrictEqMode {
    slow_operation: u32,
    invert: bool,
}

fn lower_strict_eq(
    cx: &mut LoweringCx<'_, '_>,
    barrier_thunks: &BarrierThunks,
    dest: ValueId,
    lhs_id: ValueId,
    rhs_id: ValueId,
    mode: StrictEqMode,
    feedback_ptr: Option<ir::Value>,
) -> Result<()> {
    let lhs = use_value_boxed(cx.builder, cx.variables, lhs_id)?;
    let rhs = use_value_boxed(cx.builder, cx.variables, rhs_id)?;
    if let Some(slot) = feedback_ptr {
        emit_inline_binary_feedback(cx.builder, cx.ctx, slot, mode.slow_operation, lhs, rhs);
    }
    let lhs_plain = emit_strip_gc_color(cx.builder, lhs);
    let rhs_plain = emit_strip_gc_color(cx.builder, rhs);
    let same_plain = cx
        .builder
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, lhs_plain, rhs_plain);
    let same_raw = cx.builder.ins().icmp(ir::condcodes::IntCC::Equal, lhs, rhs);
    let tag_word = cx.builder.ins().ushr_imm_u(lhs_plain, 32);
    let tag = cx.builder.ins().band_imm_u(
        tag_word,
        i64::try_from(value::TAG_MASK).expect("tag mask fits i64"),
    );
    let is_string = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        tag,
        i64::try_from(value::TAG_STRING).expect("string tag fits i64"),
    );
    let runtime_flag = cx.builder.ins().band_imm_u(
        tag_word,
        i64::try_from(value::STRING_RUNTIME_HANDLE_FLAG).expect("runtime flag fits i64"),
    );
    let is_runtime = cx
        .builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::NotEqual, runtime_flag, 0);
    let same_runtime_string = cx.builder.ins().band(same_plain, is_string);
    let same_runtime_string = cx.builder.ins().band(same_runtime_string, is_runtime);
    let lhs_inline = emit_inline_string_predicate(cx.builder, lhs_plain);
    let rhs_inline = emit_inline_string_predicate(cx.builder, rhs_plain);
    let any_inline = cx.builder.ins().bor(lhs_inline, rhs_inline);
    let both_inline = cx.builder.ins().band(lhs_inline, rhs_inline);
    let same_block = cx.builder.create_block();
    let compare_block = cx.builder.create_block();
    let inline_block = cx.builder.create_block();
    let inline_equal_block = cx.builder.create_block();
    let non_inline_block = cx.builder.create_block();
    let miss_block = cx.builder.create_block();
    let false_block = cx.builder.create_block();
    let merge_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(any_inline, inline_block, &[], non_inline_block, &[]);

    cx.builder.switch_to_block(inline_block);
    cx.builder.seal_block(inline_block);
    cx.builder
        .ins()
        .brif(both_inline, inline_equal_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(inline_equal_block);
    cx.builder.seal_block(inline_equal_block);
    let true_value = cx
        .builder
        .ins()
        .iconst(types::I64, value::encode_bool(!mode.invert));
    let false_value = cx
        .builder
        .ins()
        .iconst(types::I64, value::encode_bool(mode.invert));
    let result = cx.builder.ins().select(same_raw, true_value, false_value);
    define_value_boxed(cx.builder, cx.variables, dest, result)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(non_inline_block);
    cx.builder.seal_block(non_inline_block);
    cx.builder
        .ins()
        .brif(same_runtime_string, same_block, &[], compare_block, &[]);

    cx.builder.switch_to_block(same_block);
    cx.builder.seal_block(same_block);
    let result = cx
        .builder
        .ins()
        .iconst(types::I64, value::encode_bool(!mode.invert));
    define_value_boxed(cx.builder, cx.variables, dest, result)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(compare_block);
    cx.builder.seal_block(compare_block);
    let left_address = emit_string_address(cx, barrier_thunks, lhs, miss_block)?;
    let right_address = emit_string_address(cx, barrier_thunks, rhs, miss_block)?;
    let left_header = cx.builder.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        left_address,
        i32::try_from(constants::HEAP_STRING_LENGTH_OFFSET).expect("length offset fits i32"),
    );
    let right_header = cx.builder.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        right_address,
        i32::try_from(constants::HEAP_STRING_LENGTH_OFFSET).expect("length offset fits i32"),
    );
    let left_length = cx
        .builder
        .ins()
        .band_imm_u(left_header, i64::from(u32::MAX));
    let right_length = cx
        .builder
        .ins()
        .band_imm_u(right_header, i64::from(u32::MAX));
    let same_length = cx
        .builder
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, left_length, right_length);
    let hash_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(same_length, hash_block, &[], false_block, &[]);

    cx.builder.switch_to_block(hash_block);
    cx.builder.seal_block(hash_block);
    let left_hash = cx.builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        left_address,
        i32::try_from(constants::HEAP_STRING_HASH_OFFSET).expect("hash offset fits i32"),
    );
    let right_hash = cx.builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        right_address,
        i32::try_from(constants::HEAP_STRING_HASH_OFFSET).expect("hash offset fits i32"),
    );
    let left_hash_ready = cx
        .builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::NotEqual, left_hash, 0);
    let right_hash_ready =
        cx.builder
            .ins()
            .icmp_imm_u(ir::condcodes::IntCC::NotEqual, right_hash, 0);
    let hashes_ready = cx.builder.ins().band(left_hash_ready, right_hash_ready);
    let same_hash = cx
        .builder
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, left_hash, right_hash);
    let hashes_not_ready = cx.builder.ins().bnot(hashes_ready);
    let content_possible = cx.builder.ins().bor(hashes_not_ready, same_hash);
    let repr_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(content_possible, repr_block, &[], false_block, &[]);

    cx.builder.switch_to_block(repr_block);
    cx.builder.seal_block(repr_block);
    let left_word = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), left_address, 0);
    let right_word = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), right_address, 0);
    let repr_shift = i64::from(constants::HEAP_STRING_REPR_OFFSET * 8);
    let left_repr = cx.builder.ins().ushr_imm_u(left_word, repr_shift);
    let left_repr = cx.builder.ins().band_imm_u(left_repr, 0xff);
    let right_repr = cx.builder.ins().ushr_imm_u(right_word, repr_shift);
    let right_repr = cx.builder.ins().band_imm_u(right_repr, 0xff);
    let same_repr = cx
        .builder
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, left_repr, right_repr);
    let left_latin1 = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        left_repr,
        i64::from(constants::STRING_REPR_LATIN1_FLAT),
    );
    let left_utf16 = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        left_repr,
        i64::from(constants::STRING_REPR_UTF16_FLAT),
    );
    let right_latin1 = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        right_repr,
        i64::from(constants::STRING_REPR_LATIN1_FLAT),
    );
    let right_utf16 = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        right_repr,
        i64::from(constants::STRING_REPR_UTF16_FLAT),
    );
    let left_flat = cx.builder.ins().bor(left_latin1, left_utf16);
    let right_flat = cx.builder.ins().bor(right_latin1, right_utf16);
    let both_flat = cx.builder.ins().band(left_flat, right_flat);
    let same_flat = cx.builder.ins().band(same_repr, both_flat);
    let content_block = cx.builder.create_block();
    let mixed_check_block = cx.builder.create_block();
    let mixed_block = cx.builder.create_block();
    let mixed_equal_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(same_flat, content_block, &[], mixed_check_block, &[]);
    cx.builder.switch_to_block(mixed_check_block);
    cx.builder.seal_block(mixed_check_block);
    cx.builder
        .ins()
        .brif(both_flat, mixed_block, &[], miss_block, &[]);
    cx.builder.switch_to_block(mixed_block);
    cx.builder.seal_block(mixed_block);
    emit_mixed_string_equal(
        cx,
        left_address,
        right_address,
        left_length,
        left_latin1,
        right_latin1,
        (mixed_equal_block, false_block),
    );
    cx.builder.switch_to_block(mixed_equal_block);
    cx.builder.seal_block(mixed_equal_block);
    let result = cx
        .builder
        .ins()
        .iconst(types::I64, value::encode_bool(!mode.invert));
    define_value_boxed(cx.builder, cx.variables, dest, result)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(content_block);
    cx.builder.seal_block(content_block);
    let left_payload = cx.builder.ins().iadd_imm_s(
        left_address,
        i64::from(constants::HEAP_STRING_PAYLOAD_OFFSET),
    );
    let right_payload = cx.builder.ins().iadd_imm_s(
        right_address,
        i64::from(constants::HEAP_STRING_PAYLOAD_OFFSET),
    );
    let utf16_bytes = cx.builder.ins().ishl_imm_u(left_length, 1);
    let byte_length = cx
        .builder
        .ins()
        .select(left_latin1, left_length, utf16_bytes);
    let comparison =
        cx.builder
            .call_memcmp(cx.target_config, left_payload, right_payload, byte_length);
    let equal = cx
        .builder
        .ins()
        .icmp_imm_s(ir::condcodes::IntCC::Equal, comparison, 0);
    let true_value = cx
        .builder
        .ins()
        .iconst(types::I64, value::encode_bool(true));
    let false_value = cx
        .builder
        .ins()
        .iconst(types::I64, value::encode_bool(false));
    let result = if mode.invert {
        cx.builder.ins().select(equal, false_value, true_value)
    } else {
        cx.builder.ins().select(equal, true_value, false_value)
    };
    define_value_boxed(cx.builder, cx.variables, dest, result)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(false_block);
    cx.builder.seal_block(false_block);
    let result = cx
        .builder
        .ins()
        .iconst(types::I64, value::encode_bool(mode.invert));
    define_value_boxed(cx.builder, cx.variables, dest, result)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(miss_block);
    cx.builder.seal_block(miss_block);
    let result = cx.call(mode.slow_operation, &[lhs, rhs], None)?;
    define_value_boxed(cx.builder, cx.variables, dest, result)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(merge_block);
    cx.builder.seal_block(merge_block);
    Ok(())
}

fn lower_dynamic_binary(
    cx: &mut LoweringCx<'_, '_>,
    dest: ValueId,
    op: BinaryOp,
    lhs: ValueId,
    rhs: ValueId,
    feedback_ptr: Option<ir::Value>,
    f64_values: &HashSet<ValueId>,
) -> Result<()> {
    let lhs_id = lhs;
    let rhs_id = rhs;
    let lhs = use_value_boxed(cx.builder, cx.variables, lhs)?;
    let rhs = use_value_boxed(cx.builder, cx.variables, rhs)?;

    // 位运算、% 与 ** 仍需 ToPrimitive/ToNumber/ToBigInt 等完整语义，继续走 dispatcher。
    if !matches!(
        op,
        BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div
    ) {
        let operation = DYNAMIC_BINARY_BASE + u32::from(binary_tag(op));
        let result = cx.call(operation, &[lhs, rhs], feedback_ptr)?;
        return define_value_boxed(cx.builder, cx.variables, dest, result);
    }

    // #389 的 number 快路径不经过 dispatcher，二元反馈必须在守卫前内联更新，
    // number/number 热路径才可被观察。更新覆盖 fast/slow 两条路径，因此下方
    // 慢路径的 dispatcher 调用传 null 槽，避免同一次执行重复计数。
    if let Some(slot) = feedback_ptr {
        let operation = DYNAMIC_BINARY_BASE + u32::from(binary_tag(op));
        emit_inline_binary_feedback(cx.builder, cx.ctx, slot, operation, lhs, rhs);
    }

    // number/number 直接发原生浮点指令；已证明的 f64 操作数跳过 is_number。
    // 加法只要原始操作数一侧已是字符串，ToPrimitive 后必进入字符串拼接。
    let lhs_is_number = emit_number_or_proven_f64(cx.builder, lhs, lhs_id, f64_values);
    let rhs_is_number = emit_number_or_proven_f64(cx.builder, rhs, rhs_id, f64_values);
    let both_numbers = cx.builder.ins().band(lhs_is_number, rhs_is_number);

    let number_block = cx.builder.create_block();
    let non_number_block = cx.builder.create_block();
    let merge_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(both_numbers, number_block, &[], non_number_block, &[]);

    cx.builder.switch_to_block(number_block);
    cx.builder.seal_block(number_block);
    let lhs_f64 = cx
        .builder
        .ins()
        .bitcast(types::F64, ir::MemFlagsData::new(), lhs);
    let rhs_f64 = cx
        .builder
        .ins()
        .bitcast(types::F64, ir::MemFlagsData::new(), rhs);
    let result = match op {
        BinaryOp::Add => cx.builder.ins().fadd(lhs_f64, rhs_f64),
        BinaryOp::Sub => cx.builder.ins().fsub(lhs_f64, rhs_f64),
        BinaryOp::Mul => cx.builder.ins().fmul(lhs_f64, rhs_f64),
        BinaryOp::Div => cx.builder.ins().fdiv(lhs_f64, rhs_f64),
        _ => unreachable!("guard restricts guarded binary operations"),
    };
    let result = box_f64_arithmetic(cx.builder, op, result);
    define_value_boxed(cx.builder, cx.variables, dest, result)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(non_number_block);
    cx.builder.seal_block(non_number_block);
    if op == BinaryOp::Add {
        let string_tag = cx.builder.ins().iconst(
            types::I64,
            i64::try_from(value::TAG_STRING).expect("string tag fits i64"),
        );
        let lhs_tag = emit_feedback_tag_code(cx.builder, lhs);
        let rhs_tag = emit_feedback_tag_code(cx.builder, rhs);
        let lhs_is_string = cx.builder.ins().icmp(
            cranelift_codegen::ir::condcodes::IntCC::Equal,
            lhs_tag,
            string_tag,
        );
        let rhs_is_string = cx.builder.ins().icmp(
            cranelift_codegen::ir::condcodes::IntCC::Equal,
            rhs_tag,
            string_tag,
        );
        let either_is_string = cx.builder.ins().bor(lhs_is_string, rhs_is_string);
        let string_block = cx.builder.create_block();
        let slow_block = cx.builder.create_block();
        cx.builder
            .ins()
            .brif(either_is_string, string_block, &[], slow_block, &[]);

        cx.builder.switch_to_block(string_block);
        cx.builder.seal_block(string_block);
        cx.flush()?;
        let call = cx.builder.ins().call(cx.string_add, &[cx.ctx, lhs, rhs]);
        let result = cx.builder.inst_results(call)[0];
        define_value_boxed(cx.builder, cx.variables, dest, result)?;
        cx.builder.ins().jump(merge_block, &[]);

        cx.builder.switch_to_block(slow_block);
        cx.builder.seal_block(slow_block);
    }
    let operation = DYNAMIC_BINARY_BASE + u32::from(binary_tag(op));
    let result = cx.call(operation, &[lhs, rhs], None)?;
    define_value_boxed(cx.builder, cx.variables, dest, result)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(merge_block);
    cx.builder.seal_block(merge_block);
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
fn lower_get_prop_ic(
    cx: &mut LoweringCx<'_, '_>,
    barrier_thunks: &BarrierThunks,
    access: PropAccess,
    roots: &[ValueId],
) -> Result<()> {
    let merge_block = cx.builder.create_block();
    lower_get_prop_ic_non_nullish(cx, barrier_thunks, access, roots, merge_block)?;
    cx.builder.switch_to_block(merge_block);
    cx.builder.seal_block(merge_block);
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
fn lower_get_prop_ic_non_nullish(
    cx: &mut LoweringCx<'_, '_>,
    barrier_thunks: &BarrierThunks,
    access: PropAccess,
    roots: &[ValueId],
    merge_block: ir::Block,
) -> Result<()> {
    let PropAccess {
        dest,
        object,
        key,
        slot,
        trio_field,
    } = access;
    let obj = use_value_boxed(cx.builder, cx.variables, object)?;
    let key_value = use_value_boxed(cx.builder, cx.variables, key)?;
    let box_base = i64::from_ne_bytes(value::BOX_BASE.to_ne_bytes());
    let pointer_type = cx.builder.func.dfg.value_type(cx.ctx);
    let ht_base = cx.ht_base;
    let ic_base = cx.ic_base;
    let barrier_state = cx.barrier_state;

    // 标签检查：仅 NaN-box 的 TAG_OBJECT 才可解句柄读 entry。
    let boxed_bits = cx.builder.ins().band_imm_s(obj, box_base);
    let is_boxed = cx
        .builder
        .ins()
        .icmp_imm_s(ir::condcodes::IntCC::Equal, boxed_bits, box_base);
    let tag = cx.builder.ins().ushr_imm_u(obj, 32);
    let tag = cx.builder.ins().band_imm_u(
        tag,
        i64::try_from(value::TAG_MASK).expect("tag mask fits i64"),
    );
    let is_obj = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        tag,
        i64::try_from(value::TAG_OBJECT).expect("object tag fits i64"),
    );
    let tag_ok = cx.builder.ins().band(is_boxed, is_obj);

    // IC 槽指针：基于 ic_base（当前 image 的 IC 区，始终映射），放在入口块计算
    // 以支配所有后续分支（miss 分支需要它作为 GetPropIc 的回填目标）。
    let ic_ptr = cx.builder.ins().iadd_imm_s(
        ic_base,
        i64::from(slot) * i64::from(constants::IC_SLOT_SIZE),
    );

    let entry_block = cx.builder.create_block();
    let legacy_entry_block = cx.builder.create_block();
    let zgc_kind_block = cx.builder.create_block();
    let zgc_entry_block = cx.builder.create_block();
    let zgc_fast_block = cx.builder.create_block();
    let receiver_assist_block = cx.builder.create_block();
    let shape_check_block = cx.builder.create_block();
    cx.builder.append_block_param(shape_check_block, types::I64);
    let shape_hit_block = cx.builder.create_block();
    let own_hit_block = cx.builder.create_block();
    let holder_check_block = cx.builder.create_block();
    let holder_block = cx.builder.create_block();
    let holder_resolve_block = cx.builder.create_block();
    let holder_legacy_block = cx.builder.create_block();
    let holder_zgc_block = cx.builder.create_block();
    let holder_fast_block = cx.builder.create_block();
    let holder_assist_block = cx.builder.create_block();
    let holder_addr_block = cx.builder.create_block();
    cx.builder.append_block_param(holder_addr_block, types::I64);
    let proto_hit_block = cx.builder.create_block();
    let accessor_hit_block = cx.builder.create_block();
    let miss_block = cx.builder.create_block();
    // 第一级：标签必须是 TAG_OBJECT。**句柄表 entry 读取必须放在此分支之后**：
    // `trusted()`（notrap）load 允许 Cranelift 块内投机提前，若 entry 读取与
    // tag 检查同块，非对象值（字符串等）的 handle 可能落在未提交的 block，
    // 投机读取直接段错误。条件分支隔离后跨块提升不合法，entry 只在
    // `tag_ok` 为真（对象句柄必然已分配提交）后才读取。
    cx.builder
        .ins()
        .brif(tag_ok, entry_block, &[], miss_block, &[]);

    // 第二级：读取接收者句柄 entry。Disabled 模式沿用稳定态快链；ZGC 只有偶数
    // access epoch 与稳定 entry 能直接使用地址，其余状态进入 no-GC load assist。
    cx.builder.switch_to_block(entry_block);
    cx.builder.seal_block(entry_block);
    let handle_idx = cx.builder.ins().band_imm_u(obj, i64::from(u32::MAX));
    let handle_i32 = cx.builder.ins().ireduce(types::I32, handle_idx);
    let entry_offset = cx.builder.ins().ishl_imm_u(handle_idx, 3);
    let entry_addr = cx.builder.ins().iadd(ht_base, entry_offset);
    let entry = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), entry_addr, 0);
    let entry_state = cx.builder.ins().band_imm_u(entry, 0xFFFF);
    let stable = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::UnsignedGreaterThanOrEqual,
        entry_state,
        i64::from(constants::HANDLE_STATE_STABLE_MIN),
    );
    let logical_addr = cx.builder.ins().ushr_imm_u(entry, 16);
    let heap_delta = cx.builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, heap_object_delta))?,
    );
    // IC 槽（32 字节）：
    // word0 = shape_id(lo32) | value_index(hi32)
    // word1 = kind(lo32) | proto_generation(hi32)
    // word2 = holder_handle(lo32) | expected_proto(hi32)
    let ic_word0 = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), ic_ptr, 0);
    let ic_shape = cx.builder.ins().band_imm_u(ic_word0, i64::from(u32::MAX));
    let ic_val_idx = load_ic_value_index(cx.builder, ic_ptr, ic_word0, trio_field);
    let ic_word1 = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), ic_ptr, 8);
    let ic_kind = cx.builder.ins().band_imm_u(ic_word1, i64::from(u32::MAX));
    let ic_generation = cx.builder.ins().ushr_imm_u(ic_word1, 32);
    let ic_word2 = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), ic_ptr, 16);
    let ic_holder = cx.builder.ins().band_imm_u(ic_word2, i64::from(u32::MAX));
    let ic_expected_proto = cx.builder.ins().ushr_imm_u(ic_word2, 32);
    let kind_own = ic_kind_is_own_hit(cx.builder, ic_kind, trio_field);
    let kind_proto = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        ic_kind,
        i64::from(constants::IC_KIND_PROTO_DATA),
    );
    let kind_accessor = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        ic_kind,
        i64::from(constants::IC_KIND_ACCESSOR),
    );
    let kind_holder = cx.builder.ins().bor(kind_proto, kind_accessor);
    let kind_supported = cx.builder.ins().bor(kind_own, kind_holder);
    let barrier_disabled =
        cx.builder
            .ins()
            .icmp_imm_u(ir::condcodes::IntCC::Equal, barrier_state, 0);
    cx.builder.ins().brif(
        barrier_disabled,
        legacy_entry_block,
        &[],
        zgc_kind_block,
        &[],
    );

    cx.builder.switch_to_block(legacy_entry_block);
    cx.builder.seal_block(legacy_entry_block);
    let legacy_ok = cx.builder.ins().band(stable, kind_supported);
    cx.builder.ins().brif(
        legacy_ok,
        shape_check_block,
        &[ir::BlockArg::Value(logical_addr)],
        miss_block,
        &[],
    );

    cx.builder.switch_to_block(zgc_kind_block);
    cx.builder.seal_block(zgc_kind_block);
    cx.builder
        .ins()
        .brif(kind_supported, zgc_entry_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(zgc_entry_block);
    cx.builder.seal_block(zgc_entry_block);
    let epoch_addr = cx.builder.ins().iadd_imm_s(
        barrier_state,
        i64::try_from(offset_of!(NativeBarrierState, access_epoch))
            .expect("access epoch offset fits i64"),
    );
    let access_epoch =
        cx.builder
            .ins()
            .atomic_load(types::I64, MemFlagsData::trusted(), epoch_addr);
    let epoch_bit = cx.builder.ins().band_imm_u(access_epoch, 1);
    let epoch_even = cx
        .builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::Equal, epoch_bit, 0);
    let direct = cx.builder.ins().band(stable, epoch_even);
    cx.builder
        .ins()
        .brif(direct, zgc_fast_block, &[], receiver_assist_block, &[]);

    cx.builder.switch_to_block(zgc_fast_block);
    cx.builder.seal_block(zgc_fast_block);
    increment_barrier_counter(
        cx.builder,
        barrier_state,
        offset_of!(NativeBarrierState, load_fast_events),
    );
    cx.builder
        .ins()
        .jump(shape_check_block, &[ir::BlockArg::Value(logical_addr)]);

    cx.builder.switch_to_block(receiver_assist_block);
    cx.builder.seal_block(receiver_assist_block);
    let call = cx
        .builder
        .ins()
        .call(barrier_thunks.load, &[cx.ctx, handle_i32]);
    let assisted = cx.builder.inst_results(call)[0];
    let assisted_ok = cx
        .builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::NotEqual, assisted, 0);
    cx.builder.ins().brif(
        assisted_ok,
        shape_check_block,
        &[ir::BlockArg::Value(assisted)],
        miss_block,
        &[],
    );

    // 第三级：对象地址已经过稳定态检查或 load assist，读取 shape 并与 IC 槽比对。
    cx.builder.switch_to_block(shape_check_block);
    cx.builder.seal_block(shape_check_block);
    let logical_addr = cx.builder.block_params(shape_check_block)[0];
    let addr = cx.builder.ins().iadd(logical_addr, heap_delta);
    let obj_word = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), addr, 8);
    let obj_shape = cx.builder.ins().ushr_imm_u(obj_word, 32);
    let shape_match = cx
        .builder
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, obj_shape, ic_shape);
    cx.builder
        .ins()
        .brif(shape_match, shape_hit_block, &[], miss_block, &[]);

    // shape 命中后按 kind 分派：OWN_DATA 直达自有值槽；PROTO_DATA / ACCESSOR 先校验直接原型与世代；其余走 miss。
    cx.builder.switch_to_block(shape_hit_block);
    cx.builder.seal_block(shape_hit_block);
    cx.builder
        .ins()
        .brif(kind_own, own_hit_block, &[], holder_check_block, &[]);

    cx.builder.switch_to_block(holder_check_block);
    cx.builder.seal_block(holder_check_block);
    cx.builder
        .ins()
        .brif(kind_holder, holder_block, &[], miss_block, &[]);

    // ProtoData / Accessor：同一 shape 的 receiver 可以有不同直接原型，故先比较
    // 对象头里的 proto handle；再比较原型世代以覆盖链上属性或原型变化。
    cx.builder.switch_to_block(holder_block);
    cx.builder.seal_block(holder_block);
    let receiver_header = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), addr, 0);
    let receiver_proto = cx
        .builder
        .ins()
        .band_imm_u(receiver_header, i64::from(u32::MAX));
    let proto_match = cx.builder.ins().icmp(
        ir::condcodes::IntCC::Equal,
        receiver_proto,
        ic_expected_proto,
    );
    let current_generation = cx.builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, proto_generation))?,
    );
    let current_generation = cx.builder.ins().uextend(types::I64, current_generation);
    let generation_match = cx.builder.ins().icmp(
        ir::condcodes::IntCC::Equal,
        current_generation,
        ic_generation,
    );
    let holder_valid = cx.builder.ins().band(proto_match, generation_match);
    cx.builder
        .ins()
        .brif(holder_valid, holder_resolve_block, &[], miss_block, &[]);

    // 解析 holder_handle → holder entry → holder 地址；ZGC holder 与 receiver 使用
    // 同一 access epoch 协议，odd epoch 或 relocating entry 必须进入 load assist。
    cx.builder.switch_to_block(holder_resolve_block);
    cx.builder.seal_block(holder_resolve_block);
    let holder_entry_offset = cx.builder.ins().ishl_imm_u(ic_holder, 3);
    let holder_entry_addr = cx.builder.ins().iadd(ht_base, holder_entry_offset);
    let holder_entry =
        cx.builder
            .ins()
            .load(types::I64, MemFlagsData::trusted(), holder_entry_addr, 0);
    let holder_state = cx.builder.ins().band_imm_u(holder_entry, 0xFFFF);
    let holder_stable = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::UnsignedGreaterThanOrEqual,
        holder_state,
        i64::from(constants::HANDLE_STATE_STABLE_MIN),
    );
    let holder_logical_addr = cx.builder.ins().ushr_imm_u(holder_entry, 16);
    cx.builder.ins().brif(
        barrier_disabled,
        holder_legacy_block,
        &[],
        holder_zgc_block,
        &[],
    );

    cx.builder.switch_to_block(holder_legacy_block);
    cx.builder.seal_block(holder_legacy_block);
    cx.builder.ins().brif(
        holder_stable,
        holder_addr_block,
        &[ir::BlockArg::Value(holder_logical_addr)],
        miss_block,
        &[],
    );

    cx.builder.switch_to_block(holder_zgc_block);
    cx.builder.seal_block(holder_zgc_block);
    let holder_epoch_addr = cx.builder.ins().iadd_imm_s(
        barrier_state,
        i64::try_from(offset_of!(NativeBarrierState, access_epoch))
            .expect("access epoch offset fits i64"),
    );
    let holder_epoch =
        cx.builder
            .ins()
            .atomic_load(types::I64, MemFlagsData::trusted(), holder_epoch_addr);
    let holder_epoch_bit = cx.builder.ins().band_imm_u(holder_epoch, 1);
    let holder_epoch_even =
        cx.builder
            .ins()
            .icmp_imm_u(ir::condcodes::IntCC::Equal, holder_epoch_bit, 0);
    let holder_direct = cx.builder.ins().band(holder_stable, holder_epoch_even);
    cx.builder.ins().brif(
        holder_direct,
        holder_fast_block,
        &[],
        holder_assist_block,
        &[],
    );

    cx.builder.switch_to_block(holder_fast_block);
    cx.builder.seal_block(holder_fast_block);
    increment_barrier_counter(
        cx.builder,
        barrier_state,
        offset_of!(NativeBarrierState, load_fast_events),
    );
    cx.builder.ins().jump(
        holder_addr_block,
        &[ir::BlockArg::Value(holder_logical_addr)],
    );

    cx.builder.switch_to_block(holder_assist_block);
    cx.builder.seal_block(holder_assist_block);
    let holder_i32 = cx.builder.ins().ireduce(types::I32, ic_holder);
    let call = cx
        .builder
        .ins()
        .call(barrier_thunks.load, &[cx.ctx, holder_i32]);
    let assisted_holder = cx.builder.inst_results(call)[0];
    let assisted_holder_ok =
        cx.builder
            .ins()
            .icmp_imm_u(ir::condcodes::IntCC::NotEqual, assisted_holder, 0);
    cx.builder.ins().brif(
        assisted_holder_ok,
        holder_addr_block,
        &[ir::BlockArg::Value(assisted_holder)],
        miss_block,
        &[],
    );

    cx.builder.switch_to_block(holder_addr_block);
    cx.builder.seal_block(holder_addr_block);
    let holder_logical_addr = cx.builder.block_params(holder_addr_block)[0];
    let holder_addr = cx.builder.ins().iadd(holder_logical_addr, heap_delta);
    cx.builder
        .ins()
        .brif(kind_accessor, accessor_hit_block, &[], proto_hit_block, &[]);

    // OWN_DATA 命中：`HEAP_OBJECT_HEADER_SIZE + value_index * 8` 处单 load。
    cx.builder.switch_to_block(own_hit_block);
    cx.builder.seal_block(own_hit_block);
    let value_shift = cx.builder.ins().ishl_imm_u(ic_val_idx, 3); // × 值槽 8 字节
    let value_offset = cx
        .builder
        .ins()
        .iadd_imm_s(value_shift, i64::from(constants::HEAP_OBJECT_HEADER_SIZE));
    let value_addr = cx.builder.ins().iadd(addr, value_offset);
    let value = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), value_addr, 0);
    define_value_boxed(cx.builder, cx.variables, dest, value)?;
    cx.builder.ins().jump(merge_block, &[]);

    // PROTO_DATA 命中：从 holder 对象的值槽 load。
    cx.builder.switch_to_block(proto_hit_block);
    cx.builder.seal_block(proto_hit_block);
    let proto_value_shift = cx.builder.ins().ishl_imm_u(ic_val_idx, 3);
    let proto_value_offset = cx.builder.ins().iadd_imm_s(
        proto_value_shift,
        i64::from(constants::HEAP_OBJECT_HEADER_SIZE),
    );
    let proto_value_addr = cx.builder.ins().iadd(holder_addr, proto_value_offset);
    let proto_value =
        cx.builder
            .ins()
            .load(types::I64, MemFlagsData::trusted(), proto_value_addr, 0);
    define_value_boxed(cx.builder, cx.variables, dest, proto_value)?;
    cx.builder.ins().jump(merge_block, &[]);

    // ACCESSOR 命中：load getter 后直接走宿主 invoke_callable。getter 是刚从
    // 堆里读出的临时句柄，必须作为临时 root 发布后再发起可能触发 GC 的调用。
    cx.builder.switch_to_block(accessor_hit_block);
    cx.builder.seal_block(accessor_hit_block);
    let getter_shift = cx.builder.ins().ishl_imm_u(ic_val_idx, 3);
    let getter_offset = cx
        .builder
        .ins()
        .iadd_imm_s(getter_shift, i64::from(constants::HEAP_OBJECT_HEADER_SIZE));
    let getter_addr = cx.builder.ins().iadd(holder_addr, getter_offset);
    let getter = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), getter_addr, 0);
    cx.publish_roots(roots, &[getter])?;
    let result = cx.call(NativeRuntimeOp::GetPropAccessor.id(), &[getter, obj], None)?;
    define_value_boxed(cx.builder, cx.variables, dest, result)?;
    cx.builder.ins().jump(merge_block, &[]);

    // miss：宿主完整 [[Get]] + IC 回填；`ic_ptr` 作为回填目标传入。
    cx.builder.switch_to_block(miss_block);
    cx.builder.seal_block(miss_block);
    let result = cx.call(
        NativeRuntimeOp::GetPropIc.id(),
        &[obj, key_value, ic_ptr],
        None,
    )?;
    define_value_boxed(cx.builder, cx.variables, dest, result)?;
    cx.builder.ins().jump(merge_block, &[]);

    Ok(())
}

fn lower_set_prop_ic(
    cx: &mut LoweringCx<'_, '_>,
    barrier_thunks: &BarrierThunks,
    access: PropAccess,
    value: ValueId,
) -> Result<()> {
    let PropAccess {
        dest,
        object,
        key,
        slot,
        trio_field,
    } = access;
    let obj = use_value_boxed(cx.builder, cx.variables, object)?;
    let key_value = use_value_boxed(cx.builder, cx.variables, key)?;
    let stored = use_value_boxed(cx.builder, cx.variables, value)?;
    let box_base = i64::from_ne_bytes(value::BOX_BASE.to_ne_bytes());
    let pointer_type = cx.builder.func.dfg.value_type(cx.ctx);
    let ht_base = cx.ht_base;
    let ic_base = cx.ic_base;
    let barrier_state = cx.barrier_state;

    // 标签检查：仅 NaN-box 的 TAG_OBJECT 才可解句柄读 entry。
    let boxed_bits = cx.builder.ins().band_imm_s(obj, box_base);
    let is_boxed = cx
        .builder
        .ins()
        .icmp_imm_s(ir::condcodes::IntCC::Equal, boxed_bits, box_base);
    let tag = cx.builder.ins().ushr_imm_u(obj, 32);
    let tag = cx.builder.ins().band_imm_u(
        tag,
        i64::try_from(value::TAG_MASK).expect("tag mask fits i64"),
    );
    let is_obj = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        tag,
        i64::try_from(value::TAG_OBJECT).expect("object tag fits i64"),
    );
    let tag_ok = cx.builder.ins().band(is_boxed, is_obj);

    // IC 槽指针：基于 ic_base（当前 image 的 IC 区，始终映射），放在本块计算
    // 以支配所有后续分支（miss 分支需要它作为 SetPropIc 的回填目标）。
    let ic_ptr = cx.builder.ins().iadd_imm_s(
        ic_base,
        i64::from(slot) * i64::from(constants::IC_SLOT_SIZE),
    );

    let entry_block = cx.builder.create_block();
    let legacy_entry_block = cx.builder.create_block();
    let zgc_kind_block = cx.builder.create_block();
    let zgc_entry_block = cx.builder.create_block();
    let zgc_fast_block = cx.builder.create_block();
    let receiver_assist_block = cx.builder.create_block();
    let shape_check_block = cx.builder.create_block();
    cx.builder.append_block_param(shape_check_block, types::I64);
    cx.builder.append_block_param(shape_check_block, types::I8);
    let hit_block = cx.builder.create_block();
    let zgc_store_mode_block = cx.builder.create_block();
    let legacy_store_block = cx.builder.create_block();
    let zgc_direct_store_block = cx.builder.create_block();
    let scalar_elide_block = cx.builder.create_block();
    let barrier_store_block = cx.builder.create_block();
    let store_done_block = cx.builder.create_block();
    let miss_block = cx.builder.create_block();
    let merge_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(tag_ok, entry_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(entry_block);
    cx.builder.seal_block(entry_block);
    let handle_idx = cx.builder.ins().band_imm_u(obj, i64::from(u32::MAX));
    let handle_i32 = cx.builder.ins().ireduce(types::I32, handle_idx);
    let entry_offset = cx.builder.ins().ishl_imm_u(handle_idx, 3);
    let entry_addr = cx.builder.ins().iadd(ht_base, entry_offset);
    let entry = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), entry_addr, 0);
    let state = cx.builder.ins().band_imm_u(entry, 0xFFFF);
    let stable = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::UnsignedGreaterThanOrEqual,
        state,
        i64::from(constants::HANDLE_STATE_STABLE_MIN),
    );
    let logical_addr = cx.builder.ins().ushr_imm_u(entry, 16);
    let heap_delta = cx.builder.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        cx.ctx,
        vmctx_offset(offset_of!(NativeVmContext, heap_object_delta))?,
    );

    let ic_word0 = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), ic_ptr, 0);
    let ic_shape = cx.builder.ins().band_imm_u(ic_word0, i64::from(u32::MAX));
    let ic_val_idx = load_ic_value_index(cx.builder, ic_ptr, ic_word0, trio_field);
    let ic_word1 = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), ic_ptr, 8);
    let ic_kind = cx.builder.ins().band_imm_u(ic_word1, i64::from(u32::MAX));
    let kind_own = ic_kind_is_own_hit(cx.builder, ic_kind, trio_field);
    let barrier_disabled =
        cx.builder
            .ins()
            .icmp_imm_u(ir::condcodes::IntCC::Equal, barrier_state, 0);
    cx.builder.ins().brif(
        barrier_disabled,
        legacy_entry_block,
        &[],
        zgc_kind_block,
        &[],
    );

    cx.builder.switch_to_block(legacy_entry_block);
    cx.builder.seal_block(legacy_entry_block);
    let legacy_ok = cx.builder.ins().band(stable, kind_own);
    let direct_store = cx.builder.ins().iconst(types::I8, 1);
    cx.builder.ins().brif(
        legacy_ok,
        shape_check_block,
        &[
            ir::BlockArg::Value(logical_addr),
            ir::BlockArg::Value(direct_store),
        ],
        miss_block,
        &[],
    );

    cx.builder.switch_to_block(zgc_kind_block);
    cx.builder.seal_block(zgc_kind_block);
    cx.builder
        .ins()
        .brif(kind_own, zgc_entry_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(zgc_entry_block);
    cx.builder.seal_block(zgc_entry_block);
    let epoch_addr = cx.builder.ins().iadd_imm_s(
        barrier_state,
        i64::try_from(offset_of!(NativeBarrierState, access_epoch))
            .expect("access epoch offset fits i64"),
    );
    let access_epoch =
        cx.builder
            .ins()
            .atomic_load(types::I64, MemFlagsData::trusted(), epoch_addr);
    let epoch_bit = cx.builder.ins().band_imm_u(access_epoch, 1);
    let epoch_even = cx
        .builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::Equal, epoch_bit, 0);
    let direct_resolve = cx.builder.ins().band(stable, epoch_even);
    cx.builder.ins().brif(
        direct_resolve,
        zgc_fast_block,
        &[],
        receiver_assist_block,
        &[],
    );

    // IC 命中且 access epoch 为偶：对象地址稳定，可尝试跳过 store barrier thunk。
    // 引用写入仍受 SATB / remset / 着色约束，因此这里只预计算「young + 未标记」
    // 直写；number 等非 box 槽在命中后再与旧 word 一起判定。
    cx.builder.switch_to_block(zgc_fast_block);
    cx.builder.seal_block(zgc_fast_block);
    let phase_addr = cx.builder.ins().iadd_imm_s(
        barrier_state,
        i64::try_from(offset_of!(NativeBarrierState, phase)).expect("phase offset fits i64"),
    );
    let phase = cx
        .builder
        .ins()
        .atomic_load(types::I64, MemFlagsData::trusted(), phase_addr);
    let marking = cx
        .builder
        .ins()
        .band_imm_u(phase, NATIVE_BARRIER_MARKING_MASK as i64);
    let marking_idle = cx
        .builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::Equal, marking, 0);
    let stable_young = cx.builder.ins().icmp_imm_u(
        ir::condcodes::IntCC::Equal,
        state,
        i64::from(constants::HANDLE_STATE_STABLE_YOUNG),
    );
    let direct_store = cx.builder.ins().band(marking_idle, stable_young);
    cx.builder.ins().jump(
        shape_check_block,
        &[
            ir::BlockArg::Value(logical_addr),
            ir::BlockArg::Value(direct_store),
        ],
    );

    cx.builder.switch_to_block(receiver_assist_block);
    cx.builder.seal_block(receiver_assist_block);
    let call = cx
        .builder
        .ins()
        .call(barrier_thunks.load, &[cx.ctx, handle_i32]);
    let assisted = cx.builder.inst_results(call)[0];
    let assisted_ok = cx
        .builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::NotEqual, assisted, 0);
    let no_direct_store = cx.builder.ins().iconst(types::I8, 0);
    cx.builder.ins().brif(
        assisted_ok,
        shape_check_block,
        &[
            ir::BlockArg::Value(assisted),
            ir::BlockArg::Value(no_direct_store),
        ],
        miss_block,
        &[],
    );

    cx.builder.switch_to_block(shape_check_block);
    cx.builder.seal_block(shape_check_block);
    let logical_addr = cx.builder.block_params(shape_check_block)[0];
    let direct_store = cx.builder.block_params(shape_check_block)[1];
    let addr = cx.builder.ins().iadd(logical_addr, heap_delta);
    let obj_word = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), addr, 8);
    let obj_shape = cx.builder.ins().ushr_imm_u(obj_word, 32);
    let shape_match = cx
        .builder
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, obj_shape, ic_shape);
    cx.builder
        .ins()
        .brif(shape_match, hit_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(hit_block);
    cx.builder.seal_block(hit_block);
    let value_shift = cx.builder.ins().ishl_imm_u(ic_val_idx, 3);
    let value_offset = cx
        .builder
        .ins()
        .iadd_imm_s(value_shift, i64::from(constants::HEAP_OBJECT_HEADER_SIZE));
    let logical_slot = cx.builder.ins().iadd(logical_addr, value_offset);
    let value_addr = cx.builder.ins().iadd(addr, value_offset);
    cx.builder.ins().brif(
        barrier_disabled,
        legacy_store_block,
        &[],
        zgc_store_mode_block,
        &[],
    );

    cx.builder.switch_to_block(zgc_store_mode_block);
    cx.builder.seal_block(zgc_store_mode_block);
    cx.builder.ins().brif(
        direct_store,
        zgc_direct_store_block,
        &[],
        scalar_elide_block,
        &[],
    );

    // 偶数 epoch 下的标量直写：新旧 word 都不是 NaN-box 时 SATB/Mark/remset
    // 均为空操作，晋升后的长寿对象（property-key 的 RECORD）也能跳过 thunk。
    cx.builder.switch_to_block(scalar_elide_block);
    cx.builder.seal_block(scalar_elide_block);
    let stored_unboxed = emit_unboxed_nanbox_predicate(cx.builder, stored);
    let old = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), value_addr, 0);
    let old_unboxed = emit_unboxed_nanbox_predicate(cx.builder, old);
    let scalar_direct = cx.builder.ins().band(stored_unboxed, old_unboxed);
    cx.builder.ins().brif(
        scalar_direct,
        zgc_direct_store_block,
        &[],
        barrier_store_block,
        &[],
    );

    cx.builder.switch_to_block(legacy_store_block);
    cx.builder.seal_block(legacy_store_block);
    cx.builder
        .ins()
        .store(MemFlagsData::trusted(), stored, value_addr, 0);
    cx.builder.ins().jump(store_done_block, &[]);

    cx.builder.switch_to_block(zgc_direct_store_block);
    cx.builder.seal_block(zgc_direct_store_block);
    cx.builder
        .ins()
        .atomic_store(MemFlagsData::trusted(), stored, value_addr);
    increment_barrier_counter(
        cx.builder,
        barrier_state,
        offset_of!(NativeBarrierState, store_fast_events),
    );
    cx.builder.ins().jump(store_done_block, &[]);

    cx.builder.switch_to_block(barrier_store_block);
    cx.builder.seal_block(barrier_store_block);
    let call = cx.builder.ins().call(
        barrier_thunks.store,
        &[cx.ctx, handle_i32, logical_slot, stored],
    );
    let status = cx.builder.inst_results(call)[0];
    let stored_ok = cx
        .builder
        .ins()
        .icmp_imm_u(ir::condcodes::IntCC::Equal, status, 0);
    cx.builder
        .ins()
        .brif(stored_ok, store_done_block, &[], miss_block, &[]);

    cx.builder.switch_to_block(store_done_block);
    cx.builder.seal_block(store_done_block);
    define_value_boxed(cx.builder, cx.variables, dest, stored)?;
    cx.builder.ins().jump(merge_block, &[]);

    // miss：宿主完整 [[Set]] + IC 回填；`ic_ptr` 作为回填目标传入。
    cx.builder.switch_to_block(miss_block);
    cx.builder.seal_block(miss_block);
    let result = cx.call(
        NativeRuntimeOp::SetPropIc.id(),
        &[obj, key_value, stored, ic_ptr],
        None,
    )?;
    define_value_boxed(cx.builder, cx.variables, dest, result)?;
    cx.builder.ins().jump(merge_block, &[]);

    cx.builder.switch_to_block(merge_block);
    cx.builder.seal_block(merge_block);
    Ok(())
}

fn lower_optional_get_prop_ic(
    cx: &mut LoweringCx<'_, '_>,
    barrier_thunks: &BarrierThunks,
    access: PropAccess,
    roots: &[ValueId],
) -> Result<()> {
    let PropAccess { dest, object, .. } = access;
    let obj = use_value_boxed(cx.builder, cx.variables, object)?;

    // 第零级：null / undefined 检查。
    let is_null =
        cx.builder
            .ins()
            .icmp_imm_s(ir::condcodes::IntCC::Equal, obj, value::encode_null());
    let is_undefined =
        cx.builder
            .ins()
            .icmp_imm_s(ir::condcodes::IntCC::Equal, obj, value::encode_undefined());
    let is_nullish = cx.builder.ins().bor(is_null, is_undefined);

    let nullish_block = cx.builder.create_block();
    let ic_entry_block = cx.builder.create_block();
    let merge_block = cx.builder.create_block();

    cx.builder
        .ins()
        .brif(is_nullish, nullish_block, &[], ic_entry_block, &[]);

    // nullish 分支：提前返回 undefined。
    cx.builder.switch_to_block(nullish_block);
    cx.builder.seal_block(nullish_block);
    let undefined = cx
        .builder
        .ins()
        .iconst(types::I64, value::encode_undefined());
    define_value_boxed(cx.builder, cx.variables, dest, undefined)?;
    cx.builder.ins().jump(merge_block, &[]);

    // IC 分支入口：非 nullish 值走与 GetProp 相同的共享核心。
    cx.builder.switch_to_block(ic_entry_block);
    cx.builder.seal_block(ic_entry_block);
    lower_get_prop_ic_non_nullish(cx, barrier_thunks, access, roots, merge_block)?;

    cx.builder.switch_to_block(merge_block);
    cx.builder.seal_block(merge_block);
    Ok(())
}

fn lower_value_operation(
    cx: &mut LoweringCx<'_, '_>,
    operation: NativeRuntimeOp,
    args: &[ValueId],
    destination: Option<ValueId>,
) -> Result<()> {
    let args = args
        .iter()
        .map(|value| use_value_boxed(cx.builder, cx.variables, *value))
        .collect::<Result<Vec<_>>>()?;
    let result = cx.call(operation.id(), &args, None)?;
    if let Some(destination) = destination {
        define_value_boxed(cx.builder, cx.variables, destination, result)?;
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
    frame: Option<&mut FrameLowering>,
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
        let frame = frame.context("host call arguments require a root frame")?;
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

fn lower_terminator(
    cx: &mut LoweringCx<'_, '_>,
    predecessor: BasicBlockId,
    terminator: &Terminator,
    constants: &[Constant],
    boolean_values: &HashSet<ValueId>,
    blocks: &HashMap<BasicBlockId, ir::Block>,
    phi_edges: &HashMap<(BasicBlockId, BasicBlockId), Vec<(ValueId, ValueId)>>,
) -> Result<()> {
    match terminator {
        Terminator::Return { value } => {
            let result = match value {
                Some(value) => use_value_boxed(cx.builder, cx.variables, *value)?,
                None => cx
                    .builder
                    .ins()
                    .iconst(types::I64, value::encode_undefined()),
            };
            cx.unlink_roots()?;
            cx.builder.ins().return_(&[result]);
        }
        Terminator::Jump { target } => {
            if target.0 <= predecessor.0 {
                cx.flush()?;
                lower_cooperative_poll(cx)?;
            }
            define_phi_edge(cx.builder, cx.variables, phi_edges, predecessor, *target)?;
            cx.builder.ins().jump(blocks[target], &[]);
        }
        Terminator::Branch {
            condition,
            true_block,
            false_block,
        } => {
            if true_block.0 <= predecessor.0 || false_block.0 <= predecessor.0 {
                cx.flush()?;
                lower_cooperative_poll(cx)?;
            }
            let condition_is_boolean = boolean_values.contains(condition);
            let condition = use_value_boxed(cx.builder, cx.variables, *condition)?;
            let condition = if condition_is_boolean {
                cx.builder.ins().icmp_imm_s(
                    ir::condcodes::IntCC::Equal,
                    condition,
                    value::encode_bool(true),
                )
            } else {
                let condition = cx.call(NativeRuntimeOp::IsTruthy.id(), &[condition], None)?;
                cx.builder.ins().icmp_imm_s(
                    ir::condcodes::IntCC::NotEqual,
                    condition,
                    value::encode_bool(false),
                )
            };
            define_phi_edge(
                cx.builder,
                cx.variables,
                phi_edges,
                predecessor,
                *true_block,
            )?;
            define_phi_edge(
                cx.builder,
                cx.variables,
                phi_edges,
                predecessor,
                *false_block,
            )?;
            cx.builder
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
                cx.flush()?;
                lower_cooperative_poll(cx)?;
            }
            let value = use_value_boxed(cx.builder, cx.variables, *value)?;
            if cases.is_empty() {
                define_phi_edge(
                    cx.builder,
                    cx.variables,
                    phi_edges,
                    predecessor,
                    *default_block,
                )?;
                cx.builder.ins().jump(blocks[default_block], &[]);
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
                        cx.builder
                            .ins()
                            .icmp_imm_u(ir::condcodes::IntCC::Equal, value, immediate);
                    define_phi_edge(
                        cx.builder,
                        cx.variables,
                        phi_edges,
                        predecessor,
                        case.target,
                    )?;
                    if index + 1 == cases.len() {
                        define_phi_edge(
                            cx.builder,
                            cx.variables,
                            phi_edges,
                            predecessor,
                            *default_block,
                        )?;
                        cx.builder.ins().brif(
                            condition,
                            blocks[&case.target],
                            &[],
                            blocks[default_block],
                            &[],
                        );
                    } else {
                        let next_case = cx.builder.create_block();
                        cx.builder
                            .ins()
                            .brif(condition, blocks[&case.target], &[], next_case, &[]);
                        cx.builder.switch_to_block(next_case);
                    }
                }
            }
        }
        Terminator::Throw { value } => {
            let value = use_value_boxed(cx.builder, cx.variables, *value)?;
            let exception = cx.call(NativeRuntimeOp::CreateException.id(), &[value], None)?;
            cx.unlink_roots()?;
            cx.builder.ins().return_(&[exception]);
        }
        Terminator::Unreachable => {
            let result = cx
                .builder
                .ins()
                .iconst(types::I64, value::encode_undefined());
            cx.unlink_roots()?;
            cx.builder.ins().return_(&[result]);
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

/// CLIF 版 `value::strip_gc_color`：仅对 handle-backed reference 清除 GC color。
/// number 与 inline SSO 的 payload 可能占用 bits 38–43，必须原样保留。
pub(crate) fn emit_strip_gc_color(
    builder: &mut FunctionBuilder<'_>,
    input: ir::Value,
) -> ir::Value {
    let is_number = emit_is_number(builder, input);
    let is_inline = emit_inline_string_predicate(builder, input);
    let color_mask = i64::from_ne_bytes((!value::GC_COLOR_MASK).to_ne_bytes());
    let stripped = builder.ins().band_imm_u(input, color_mask);
    let keep_raw = builder.ins().bor(is_number, is_inline);
    builder.ins().select(keep_raw, input, stripped)
}

/// CLIF 版 `value::is_exception`：与 `value::is_tagged` 一致，boxed 判定必须
/// 同时要求 SSO marker 位（48–50）为零。inline ASCII 字符串的 7-bit 码元载荷
/// 可覆盖 bits 32–36（如 `"[2,3]"` 第 5 个码元 `']'` 恰好落成 TAG_EXCEPTION），
/// 只查 BOX_BASE + tag 会把这类字符串误判为异常哨兵；标准 tagged handle 的
/// bits 44–50 恒为零，因此并入掩码不会漏判真实异常。
fn emit_is_exception(builder: &mut FunctionBuilder<'_>, input: ir::Value) -> ir::Value {
    let box_base = i64::from_ne_bytes(value::BOX_BASE.to_ne_bytes());
    let boxed_mask =
        i64::from_ne_bytes((value::BOX_BASE | value::INLINE_STRING_MARKER_MASK).to_ne_bytes());
    let boxed_bits = builder.ins().band_imm_s(input, boxed_mask);
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
    root_frame: Option<&mut FrameLowering>,
    ctx: ir::Value,
) -> Result<()> {
    let is_exception = emit_is_exception(builder, result);
    let exception_block = builder.create_block();
    let continue_block = builder.create_block();
    builder
        .ins()
        .brif(is_exception, exception_block, &[], continue_block, &[]);
    builder.switch_to_block(exception_block);
    if let Some(root_frame) = root_frame {
        root_frame.unlink(builder, ctx)?;
    }
    builder.ins().return_(&[result]);
    builder.switch_to_block(continue_block);
    Ok(())
}

fn lower_cooperative_poll(cx: &mut LoweringCx<'_, '_>) -> Result<()> {
    let budget_addr = cx.builder.ins().iadd_imm_s(
        cx.ctx,
        i64::from(vmctx_offset(offset_of!(
            NativeVmContext,
            stack_budget_bytes
        ))?),
    );
    let budget = cx
        .builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), budget_addr, 0);
    let step_i64 = i64::try_from(COOPERATIVE_POLL_STEP_BYTES).expect("poll step fits i64");
    // 预算已 ≤ 步长（含耗尽的 0）→ 慢路径：进 dispatcher 轮询并重置预算。
    let exhausted = cx.builder.ins().icmp_imm_s(
        ir::condcodes::IntCC::UnsignedLessThanOrEqual,
        budget,
        step_i64,
    );
    let slow_block = cx.builder.create_block();
    let fast_block = cx.builder.create_block();
    cx.builder
        .ins()
        .brif(exhausted, slow_block, &[], fast_block, &[]);

    // 快路径：预算充足，扣减步长后继续回边，不调用 dispatcher。
    cx.builder.switch_to_block(fast_block);
    cx.builder.seal_block(fast_block);
    let step = cx.builder.ins().iconst(types::I64, step_i64);
    let remaining = cx.builder.ins().isub(budget, step);
    cx.builder
        .ins()
        .store(MemFlagsData::trusted(), remaining, budget_addr, 0);
    let continue_block = cx.builder.create_block();
    cx.builder.ins().jump(continue_block, &[]);

    // 慢路径：预算耗尽，进 dispatcher 轮询（inspector / GC / 外部事件 / 期限）；
    // 宿主在 CooperativePoll 处理中把预算重置回初始值。
    cx.builder.switch_to_block(slow_block);
    cx.builder.seal_block(slow_block);
    let result = call_dispatcher(
        cx.builder,
        None,
        cx.dispatcher,
        cx.ctx,
        NativeRuntimeOp::CooperativePoll.id(),
        &[],
        None,
    )?;
    return_if_exception(cx.builder, result, cx.root_frame.as_deref_mut(), cx.ctx)?;
    cx.builder.ins().jump(continue_block, &[]);

    cx.builder.switch_to_block(continue_block);
    cx.builder.seal_block(continue_block);
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
        Constant::ArrayTemplate(_) => bail!("array templates are not valid switch keys"),
        Constant::ObjectTemplate { .. } => {
            bail!("object templates are not valid switch keys")
        }
        Constant::Uninitialized => bail!("uninitialized sentinel is not a valid switch key"),
    }
}

/// φ 边上的并行赋值：先按各自 dest 的表示读出全部 source，再统一写入。
///
/// 逐对选择表示，两端都是 typed 时不产出转换指令；循环回边因此不会把归纳变量
/// 打标再拆包。
fn define_phi_edge(
    builder: &mut FunctionBuilder<'_>,
    variables: &ValueRepr,
    phi_edges: &HashMap<(BasicBlockId, BasicBlockId), Vec<(ValueId, ValueId)>>,
    predecessor: BasicBlockId,
    target: BasicBlockId,
) -> Result<()> {
    if let Some(assignments) = phi_edges.get(&(predecessor, target)) {
        let values: Vec<_> = assignments
            .iter()
            .map(|(dest, source)| {
                use_value_as(builder, variables, variables.is_typed_value(*dest), *source)
            })
            .collect::<Result<_>>()?;
        for ((dest, _), value) in assignments.iter().zip(values) {
            define_value_as(builder, variables, *dest, value)?;
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

fn infer_boolean_values(function: &wjsm_ir::Function, constants: &[Constant]) -> HashSet<ValueId> {
    let mut booleans = HashSet::new();
    loop {
        let before = booleans.len();
        for block in function.blocks() {
            for instruction in block.instructions() {
                let destination = match instruction {
                    Instruction::Const { dest, constant }
                        if matches!(
                            constants.get(
                                usize::try_from(constant.0).expect("constant index fits usize"),
                            ),
                            Some(Constant::Bool(_))
                        ) =>
                    {
                        Some(*dest)
                    }
                    Instruction::Compare { dest, .. }
                    | Instruction::IsException { dest, .. }
                    | Instruction::GuardSameFunction { dest, .. } => Some(*dest),
                    Instruction::Unary {
                        dest,
                        op: UnaryOp::Not | UnaryOp::IsNullish,
                        ..
                    } => Some(*dest),
                    Instruction::CallBuiltin {
                        dest: Some(dest),
                        builtin:
                            Builtin::AbstractCompare
                            | Builtin::AbstractEq
                            | Builtin::StrictEq
                            | Builtin::ToBoolean
                            | Builtin::IsCallable
                            | Builtin::IsJsObject
                            | Builtin::ArrayHasElement
                            | Builtin::ObjectIs,
                        ..
                    } => Some(*dest),
                    Instruction::Phi { dest, sources }
                        if !sources.is_empty()
                            && sources
                                .iter()
                                .all(|source| booleans.contains(&source.value)) =>
                    {
                        Some(*dest)
                    }
                    _ => None,
                };
                if let Some(destination) = destination {
                    booleans.insert(destination);
                }
            }
        }
        if booleans.len() == before {
            return booleans;
        }
    }
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
    repr: &ValueRepr,
) {
    for (name, variable) in locals.iter_mut() {
        *variable = builder.declare_var(repr.local_type(name));
        // typed 局部的资格保证入口定义到不了任何 load（见 `ValueRepr::plan`），
        // 这里的 0.0 只是给 Cranelift 的 SSA 构造一个确定的支配定义。
        let initial = if repr.is_typed_local(name) {
            builder.ins().f64const(0.0)
        } else {
            builder.ins().iconst(types::I64, value::encode_undefined())
        };
        builder.def_var(*variable, initial);
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

fn box_f64_arithmetic(
    builder: &mut FunctionBuilder<'_>,
    op: BinaryOp,
    result: ir::Value,
) -> ir::Value {
    match op {
        // 有限数 +/- 不会产生 NaN；跳过 canonicalize，避免每次 add 的 unordered 比较。
        BinaryOp::Add | BinaryOp::Sub => {
            builder
                .ins()
                .bitcast(types::I64, ir::MemFlagsData::new(), result)
        }
        _ => box_f64_result(builder, result),
    }
}

fn emit_number_or_proven_f64(
    builder: &mut FunctionBuilder<'_>,
    encoded: ir::Value,
    id: ValueId,
    f64_values: &HashSet<ValueId>,
) -> ir::Value {
    if f64_values.contains(&id) {
        let zero = builder.ins().iconst(types::I64, 0);
        builder.ins().icmp(ir::condcodes::IntCC::Equal, zero, zero)
    } else {
        emit_is_number(builder, encoded)
    }
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
        CompareOp::Lt => 2,
        CompareOp::Gt => 3,
        CompareOp::LtEq => 4,
        CompareOp::GtEq => 5,
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
