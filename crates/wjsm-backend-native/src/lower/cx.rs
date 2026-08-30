//! 栈帧、LoweringCx 与编译入口类型。

#![allow(unused_imports)]
use super::*;
use anyhow::{Context, Result, anyhow, bail};
use cranelift_codegen::ir::{self, InstBuilder, MemFlagsData, types};
use cranelift_frontend::{FunctionBuilder, Variable};
use cranelift_module::{DataId, FuncId, Module};
use cranelift_object::ObjectModule;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::mem::{offset_of, size_of};
use wjsm_ir::{FunctionId, Instruction, Program, ValueId, constants, value};
use wjsm_native_abi::{NativeVmContext, native_variable_names};

/// 一个 generated function 栈帧上的固定资源：GC root frame 与 host 调用参数区。
///
/// 三个 base 指针在入口块一次性物化；入口块支配其余所有块，因此它们可以在任意块里
/// 直接以 `store base + 常量 offset` 使用，无需每次重算 `stack_addr`。
pub(crate) struct FrameLowering {
    pub(crate) bitmap_by_root_count: Vec<ir::GlobalValue>,
    pub(crate) capacity: usize,
    /// 块内各 root 槽当前持有的 ValueId；跨块必须清空（前驱可能发布了不同内容）。
    /// 暂存但尚未落地的 root 集合。发布推迟到下一个可 GC 调用点，
    /// 非安全点之间的 root frame 内容对 GC 不可见，无需维护。
    pub(crate) staged_roots: Vec<ValueId>,
    pub(crate) staged_dirty: bool,
    /// 上一次真正落地 root frame 时 builder 所在的 CLIF block。同一条 IR 指令
    /// 的 lowering 会分裂出互斥的兄弟块（如动态加法的 string 快路径与 dispatcher
    /// 慢路径），先 lower 的兄弟块清掉 dirty 后，后 lower 的兄弟块若照旧跳过
    /// 发布，运行时走到它就会带着陈旧 root frame 进宿主——正在构造、仅存于
    /// SSA 的对象对 GC 根快照不可见，会被并发标记误判为死。只有仍在同一
    /// block（先前发布点支配当前点）时才允许跳过。
    pub(crate) flushed_block: Option<ir::Block>,
    /// 入口块一次性物化的基址，被所有块支配后可跨块复用。
    pub(crate) frame_base: ir::Value,
    pub(crate) roots_base: ir::Value,
    /// 全函数共用的 host 调用参数区：参数在写入前已全部物化，写完立即被同一条
    /// `call` 消费，调用返回后即为死数据，因此各调用点不会重叠使用。
    pub(crate) arena_slot: StackSlot,
    pub(crate) arena_base: ir::Value,
    pub(crate) arena_bytes: u32,
    /// 提升到 SSA 的 boxed 局部占用 root 槽的尾部，跨 safepoint 常驻。
    pub(crate) pinned_local_count: usize,
}

impl FrameLowering {
    /// 必须在入口块内调用：本方法物化的基址值被其余所有块支配，供跨块复用。
    pub(crate) fn new(
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
            flushed_block: None,
            frame_base,
            roots_base,
            arena_slot,
            arena_base,
            arena_bytes: ARENA_MIN_BYTES,
            pinned_local_count: 0,
        })
    }

    /// 为一次 host 调用预留 `bytes` 字节参数区，返回共享参数区基址。
    pub(crate) fn reserve_arena(&mut self, bytes: u32) -> ir::Value {
        self.arena_bytes = self.arena_bytes.max(bytes);
        self.arena_base
    }

    /// lower 结束、`finalize` 之前写回参数区实际尺寸。
    pub(crate) fn finish(&self, builder: &mut FunctionBuilder<'_>) {
        builder.func.sized_stack_slots[self.arena_slot].size = self.arena_bytes;
    }

    pub(crate) fn link(&self, builder: &mut FunctionBuilder<'_>, ctx: ir::Value) -> Result<()> {
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
    pub(crate) fn stage(&mut self, roots: &[ValueId]) {
        self.staged_roots.clear();
        self.staged_roots.extend_from_slice(roots);
        self.staged_dirty = true;
    }

    /// 在可 GC / 可重入调用之前把暂存的 root 集合真正写入 root frame。
    ///
    /// 只有「暂存集自上次发布未变、且上次发布落在当前 CLIF block（顺序执行
    /// 必先经过它，即支配当前点）」时才可跳过；跨块一律重新发布——兄弟块
    /// 之间互不支配，复用彼此的发布会让 GC 扫到缺槽的陈旧 root frame。
    pub(crate) fn flush(
        &mut self,
        builder: &mut FunctionBuilder<'_>,
        variables: &ValueRepr,
    ) -> Result<()> {
        if !self.staged_dirty
            && self.flushed_block.is_some()
            && self.flushed_block == builder.current_block()
        {
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
    pub(crate) fn publish(
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
        self.flushed_block = builder.current_block();
        Ok(())
    }

    pub(crate) fn pin_frame_locals(
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

    pub(crate) fn update_pinned_local(
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
    pub(crate) fn enter_block(&mut self) {
        self.staged_roots.clear();
        self.staged_dirty = false;
        self.flushed_block = None;
    }

    pub(crate) fn unlink(&self, builder: &mut FunctionBuilder<'_>, ctx: ir::Value) -> Result<()> {
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

pub(crate) fn slot_offset(index: usize, context: &'static str) -> Result<i32> {
    index
        .checked_mul(size_of::<i64>())
        .and_then(|offset| i32::try_from(offset).ok())
        .with_context(|| format!("{context} offset exceeds i32"))
}
pub(crate) fn vmctx_offset(offset: usize) -> Result<i32> {
    i32::try_from(offset).context("native vmctx field offset exceeds i32")
}

pub(crate) fn barrier_state_offset(offset: usize) -> i32 {
    i32::try_from(offset).expect("native barrier state field offset fits i32")
}

pub(crate) fn increment_barrier_counter(
    builder: &mut FunctionBuilder<'_>,
    barrier: ir::Value,
    offset: usize,
) {
    let offset = barrier_state_offset(offset);
    let current = builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), barrier, offset);
    let next = builder.ins().iadd_imm_u(current, 1);
    builder
        .ins()
        .store(MemFlagsData::trusted(), next, barrier, offset);
}

pub(crate) fn frame_offset(offset: usize) -> Result<i32> {
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
            | Instruction::ConstructCall { .. } => 1,
            // IC accessor 命中时会把刚 load 出的 getter 作为临时 root 发布后再
            // 调用宿主 invoke_callable；保守起见所有 GetProp 都预留一个临时槽
            // 所有 GetProp 都预留一个临时槽（多预留不影响正确性）。闩锁快路径复用同一 IC 核心，
            // 同样预留。
            Instruction::GetProp { .. } => 1,
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

/// 为「常量字符串键的 GetProp / SetProp」分配全局 IC 槽。
pub(crate) fn allocate_ic_slots(program: &Program) -> (Vec<HashMap<ValueId, u32>>, u32) {
    let plan = plan_ic_slots(program);
    (plan.per_function, plan.total)
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
    pub(crate) id: FuncId,
    pub(crate) signature: Signature,
    pub(crate) colocated: bool,
}

pub(crate) struct DeclaredBarrierThunks {
    pub(crate) load: DeclaredFunction,
    pub(crate) store: DeclaredFunction,
}

pub(crate) struct BarrierThunks {
    pub(crate) load: ir::FuncRef,
    pub(crate) store: ir::FuncRef,
}

pub(crate) struct DeclaredData {
    pub(crate) id: DataId,
    pub(crate) colocated: bool,
    pub(crate) tls: bool,
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

    pub(crate) fn import(&self, function: &mut Function) -> BarrierThunks {
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
    pub(crate) fn import(&self, func: &mut Function) -> ir::GlobalValue {
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
    pub slot_map: Option<&'a wjsm_optimize::SlotMap>,
    pub function_decls: &'a [DeclaredFunction],
    pub direct_callable_functions: &'a HashSet<FunctionId>,
    pub safepoint_free: bool,
    pub collect_diagnostics: bool,
    /// 程序级 hypot getter 属性名；空集则 ACCESSOR IC 不发 hypot 快路径。
    pub hypot_property_names: &'a HashSet<String>,
}

/// 指令级 lowering 的共享可变上下文。
pub(crate) struct LoweringCx<'a, 'f> {
    pub(crate) builder: &'a mut FunctionBuilder<'f>,
    pub(crate) variables: &'a ValueRepr,
    pub(crate) root_frame: Option<&'a mut FrameLowering>,
    pub(crate) dispatcher: ir::FuncRef,
    pub(crate) string_add: ir::FuncRef,
    pub(crate) string_builder_finish: ir::FuncRef,
    pub(crate) ctx: ir::Value,
    pub(crate) env: ir::Value,
    pub(crate) this_value: ir::Value,
    pub(crate) function_index: u32,
    pub(crate) current_block: BasicBlockId,
    pub(crate) target_config: cranelift_codegen::isa::TargetFrontendConfig,
    /// 入口块缓存的 handle / IC / barrier 基址，函数内 IC 命中路径复用。
    pub(crate) ht_base: ir::Value,
    pub(crate) ic_base: ir::Value,
    pub(crate) barrier_state: ir::Value,
    pub(crate) current_instruction: u32,
    pub(crate) feedback_ptr: Option<ir::Value>,
}

impl LoweringCx<'_, '_> {
    pub(crate) fn stage_roots(&mut self, roots: &[ValueId]) {
        if let Some(root_frame) = self.root_frame.as_mut() {
            root_frame.stage(roots);
        }
    }

    pub(crate) fn flush_roots(&mut self) -> Result<()> {
        if let Some(root_frame) = self.root_frame.as_mut() {
            root_frame.flush(self.builder, self.variables)?;
        }
        Ok(())
    }

    pub(crate) fn unlink_roots(&mut self) -> Result<()> {
        if let Some(root_frame) = self.root_frame.as_mut() {
            root_frame.unlink(self.builder, self.ctx)?;
        }
        Ok(())
    }

    pub(crate) fn enter_root_block(&mut self) {
        if let Some(root_frame) = self.root_frame.as_mut() {
            root_frame.enter_block();
        }
    }

    pub(crate) fn finish_roots(&mut self) {
        if let Some(root_frame) = self.root_frame.as_mut() {
            root_frame.finish(self.builder);
        }
    }

    pub(crate) fn publish_roots(
        &mut self,
        roots: &[ValueId],
        temporaries: &[ir::Value],
    ) -> Result<()> {
        if let Some(root_frame) = self.root_frame.as_mut() {
            root_frame.publish(self.builder, self.variables, roots, temporaries)?;
        }
        Ok(())
    }

    pub(crate) fn update_pinned_local(&mut self, index: usize, value: ir::Value) -> Result<()> {
        if let Some(root_frame) = self.root_frame.as_mut() {
            root_frame.update_pinned_local(self.builder, index, value)?;
        }
        Ok(())
    }

    /// 统一的宿主分派入口：dispatcher 可能触发 GC 与重入，调用前必须落地 root。
    pub(crate) fn call(
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

    pub(crate) fn stage(&mut self, roots: &[ValueId]) {
        self.stage_roots(roots);
    }

    /// 在可 GC / 可重入调用之前落地暂存的 root 集合。
    pub(crate) fn flush(&mut self) -> Result<()> {
        self.flush_roots()
    }
}

/// 指令 lowering 所需的只读/半可变表。
pub(crate) struct InstructionTables<'a> {
    pub(crate) program: &'a Program,
    pub(crate) ir_function: &'a wjsm_ir::Function,
    pub(crate) has_env_layout: bool,
    pub(crate) constants: &'a [Constant],
    pub(crate) function_index: u32,
    pub(crate) barrier_thunks: &'a BarrierThunks,
    pub(crate) f64_values: &'a HashSet<ValueId>,
    pub(crate) int32_values: &'a HashSet<ValueId>,
    pub(crate) speculative: bool,
    pub(crate) constant_defs: &'a HashMap<ValueId, ConstantId>,
    pub(crate) math_thunks: &'a HashMap<Builtin, DeclaredFunction>,
    pub(crate) hoisted_constants: &'a HashMap<ConstantId, ir::Value>,
    pub(crate) imported_math_thunks: &'a mut HashMap<Builtin, ir::FuncRef>,
    pub(crate) slow_call_signature: ir::SigRef,
    pub(crate) variable_slots: &'a HashMap<String, u32>,
    pub(crate) frame_locals: &'a HashMap<String, Variable>,
    pub(crate) frame_local_indices: &'a HashMap<String, usize>,
    pub(crate) ic_slots: &'a HashMap<ValueId, u32>,
    pub(crate) template_origins: &'a TemplateOriginMap,
    pub(crate) function_decls: &'a [DeclaredFunction],
    pub(crate) imported_function_decls: &'a mut HashMap<FunctionId, ir::FuncRef>,
    pub(crate) direct_callable_functions: &'a HashSet<FunctionId>,
    /// 程序内 hypot getter 的属性名（如 `"norm"`）；空集则 ACCESSOR IC 不发 hypot 快路径。
    pub(crate) hypot_property_names: &'a HashSet<String>,
}

/// 调用类指令的操作数。
pub(crate) struct CallLowering<'a> {
    pub(crate) destination: Option<ValueId>,
    pub(crate) callee: ValueId,
    pub(crate) this_value: ValueId,
    pub(crate) args: &'a [ValueId],
    pub(crate) operation: NativeRuntimeOp,
    pub(crate) forward_args: bool,
}

/// 属性访问 IC 的操作数。
#[derive(Clone, Copy)]
pub(crate) struct PropAccess {
    pub(crate) dest: ValueId,
    pub(crate) object: ValueId,
    pub(crate) key: ValueId,
    pub(crate) slot: u32,
    pub(crate) trio_field: Option<TrioField>,
}

pub(crate) fn prop_access(
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

pub(crate) fn load_ic_value_index(
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

pub(crate) fn ic_kind_is_own_hit(
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
