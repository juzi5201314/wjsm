//! Inline cache 快链发射（R2）。
//!
//! # 缓存什么
//!
//! IC 只缓存**位置信息**：`shape_id → 值槽下标`。属性语义（accessor、proxy、
//! 原型链、字典 shape）全部仍由宿主 `obj_get` 承担。这条边界是本模块的安全根基：
//! 缓存命中只是「跳过一次宿主往返，直接按下标读值槽」，命中条件不满足就退回宿主，
//! 因此 IC 无论怎么陈旧都不可能让语义走偏。
//!
//! # 命中路径没有调用、没有 spill
//!
//! 快链全程无 call、无分配，因此**不是 GC 安全点**——不需要 safepoint spill
//! （spill prologue/epilogue 是原属性读的主要体积来源）。这要求快链内绝不能
//! 出现任何可能触发 GC 的操作，新增指令时必须保持这一点。
//!
//! # 为什么读地址是安全的
//!
//! 句柄 entry 的低 16 位是状态；只有稳定态（>= `HANDLE_STATE_STABLE_MIN`）的
//! entry 地址可被直接使用。GC 并发搬迁时状态会先转为 Relocating，快链的单次
//! `i64.lt_u` 判定即失败并退回宿主，宿主走完整的转发协议。Free entry 地址为 0，
//! 状态同样不满足，因此快链永不会对野地址发 load。
//!
//! # 失效
//!
//! 命中要求 `obj_shape == ic_shape` 精确相等，所以接收者的任何形状变化（加属性、
//! delete 导致字典退化）都会自动使缓存失效，无需任何全局清扫。

use wasm_encoder::{BlockType, Instruction as WasmInstruction, MemArg, ValType};
use wjsm_ir::constants;
use wjsm_ir::value;

use crate::Compiler;
use crate::host_import_registry::SpecialHostImport;

/// memory0（IC 槽区）的 u32 字段访问。
fn ic_mem_arg(offset: u32) -> MemArg {
    MemArg {
        offset: u64::from(offset),
        align: 2,
        memory_index: 0,
    }
}

/// memory2（对象堆，memory64）的 i64 访问。
fn heap_mem_arg(offset: u32) -> MemArg {
    MemArg {
        offset: u64::from(offset),
        align: 3,
        memory_index: wjsm_ir::HEAP_MEMORY_INDEX,
    }
}

/// memory2 的窄宽度访问：wasm 要求 align 不得超过访问宽度的自然对齐。
/// `align` 是 log2(字节数)：0 = 1 字节，2 = 4 字节，3 = 8 字节。
fn heap_mem_arg_narrow(offset: u32, align: u32) -> MemArg {
    MemArg {
        offset: u64::from(offset),
        align,
        memory_index: wjsm_ir::HEAP_MEMORY_INDEX,
    }
}

impl Compiler {
    /// 属性读的 inline cache 快链。
    ///
    /// 结构：
    /// ```text
    /// block $exit (result i64)
    ///   block $miss
    ///     obj 是普通对象？          否 → br $miss
    ///     句柄 entry 处于稳定态？   否 → br $miss
    ///     obj_shape == ic_shape？   否 → br $miss
    ///     kind == OwnData？         否 → br $miss
    ///     读值槽 → br $exit
    ///   end
    ///   宿主 obj_get（带 safepoint spill）+ 按需回填 IC
    /// end
    /// ```
    pub(crate) fn emit_get_prop_ic(
        &mut self,
        object_local: u32,
        name_id_ptr: u32,
        ic_slot_addr: u32,
        dest_local: u32,
    ) {
        let addr_scratch = self.ic_addr_scratch_idx;
        // 「是普通对象」= 装箱位与 tag 位同时匹配，一次 and + 一次比较。
        let tag_field_mask = (value::BOX_BASE | (value::TAG_MASK << 32)) as i64;
        let tag_object_bits = (value::BOX_BASE | (value::TAG_OBJECT << 32)) as i64;

        self.emit(WasmInstruction::Block(BlockType::Result(ValType::I64)));
        self.emit(WasmInstruction::Block(BlockType::Empty));

        // ── 1. obj 必须是普通对象 ──
        // 函数/闭包的属性对象走 function_props 偏移，数组命名属性在宿主侧表，
        // 都不按 shape 缓存，交给宿主。
        self.emit(WasmInstruction::LocalGet(object_local));
        self.emit(WasmInstruction::I64Const(tag_field_mask));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(tag_object_bits));
        self.emit(WasmInstruction::I64Ne);
        self.emit(WasmInstruction::BrIf(0));

        // ── 2. 读句柄 entry（handle * 8 是 entry 在句柄表中的字节偏移）──
        self.emit(WasmInstruction::LocalGet(object_local));
        self.emit(WasmInstruction::I64Const(0xFFFF_FFFF));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(3));
        self.emit(WasmInstruction::I64Shl);
        self.emit(WasmInstruction::I64AtomicLoad(heap_mem_arg(0)));
        self.emit(WasmInstruction::LocalTee(addr_scratch));

        // ── 3. 稳定态单比较：state < STABLE_MIN → 退回宿主 ──
        self.emit(WasmInstruction::I64Const(0xFFFF));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(i64::from(
            constants::HANDLE_STATE_STABLE_MIN,
        )));
        self.emit(WasmInstruction::I64LtU);
        self.emit(WasmInstruction::BrIf(0));

        // ── 4. 对象地址 = entry >> 16 ──
        self.emit(WasmInstruction::LocalGet(addr_scratch));
        self.emit(WasmInstruction::I64Const(16));
        self.emit(WasmInstruction::I64ShrU);
        self.emit(WasmInstruction::LocalSet(addr_scratch));

        // ── 5. shape 比较：对象头 +8 的高 32 位是 shape_id ──
        self.emit(WasmInstruction::LocalGet(addr_scratch));
        self.emit(WasmInstruction::I64Load(heap_mem_arg(
            constants::HEAP_OBJECT_VALUE_CAPACITY_OFFSET,
        )));
        self.emit(WasmInstruction::I64Const(32));
        self.emit(WasmInstruction::I64ShrU);
        self.emit(WasmInstruction::I32WrapI64);
        self.emit(WasmInstruction::I32Const(ic_slot_addr as i32));
        self.emit(WasmInstruction::I32Load(ic_mem_arg(
            constants::IC_SLOT_SHAPE_ID_OFFSET,
        )));
        self.emit(WasmInstruction::I32Ne);
        self.emit(WasmInstruction::BrIf(0));

        // ── 6. kind 必须是 OwnData ──
        // 空槽（kind=Empty）与退化槽（kind=Megamorphic）都在此退回宿主；
        // 用 kind 而非 shape_id 判空，因为 SHAPE_ID_EMPTY 是合法 shape。
        self.emit(WasmInstruction::I32Const(ic_slot_addr as i32));
        self.emit(WasmInstruction::I32Load(ic_mem_arg(
            constants::IC_SLOT_KIND_OFFSET,
        )));
        self.emit(WasmInstruction::I32Const(
            constants::IC_KIND_OWN_DATA as i32,
        ));
        self.emit(WasmInstruction::I32Ne);
        self.emit(WasmInstruction::BrIf(0));

        // ── 7. 命中：读值槽 addr + 16 + value_index * 8 ──
        self.emit(WasmInstruction::LocalGet(addr_scratch));
        self.emit(WasmInstruction::I32Const(ic_slot_addr as i32));
        self.emit(WasmInstruction::I32Load(ic_mem_arg(
            constants::IC_SLOT_VALUE_INDEX_OFFSET,
        )));
        self.emit(WasmInstruction::I64ExtendI32U);
        self.emit(WasmInstruction::I64Const(3));
        self.emit(WasmInstruction::I64Shl);
        self.emit(WasmInstruction::I64Add);
        self.emit(WasmInstruction::I64Load(heap_mem_arg(
            constants::HEAP_OBJECT_HEADER_SIZE,
        )));
        self.emit(WasmInstruction::Br(1));

        self.emit(WasmInstruction::End); // $miss

        // ── miss：宿主慢路径（完整语义）+ 按需回填缓存 ──
        let spill = self.current_spill_locals();
        self.emit_safepoint_spill_prologue(&spill);
        self.emit(WasmInstruction::LocalGet(object_local));
        self.emit(WasmInstruction::I32Const(name_id_ptr as i32));
        self.emit(WasmInstruction::Call(self.obj_get_func_idx));

        // 回填放在 spill 窗口内：ic_backfill 会 intern 属性名，保守视为可能触发 GC。
        // 已退化的槽不再回填，否则 megamorphic 站点每次都要多付一次宿主往返。
        self.emit(WasmInstruction::I32Const(ic_slot_addr as i32));
        self.emit(WasmInstruction::I32Load(ic_mem_arg(
            constants::IC_SLOT_KIND_OFFSET,
        )));
        self.emit(WasmInstruction::I32Const(
            constants::IC_KIND_MEGAMORPHIC as i32,
        ));
        self.emit(WasmInstruction::I32Ne);
        self.emit(WasmInstruction::If(BlockType::Empty));
        self.emit(WasmInstruction::LocalGet(object_local));
        self.emit(WasmInstruction::I32Const(name_id_ptr as i32));
        self.emit(WasmInstruction::I32Const(ic_slot_addr as i32));
        self.emit(WasmInstruction::Call(
            self.special_host_import_indices[&SpecialHostImport::IcBackfill],
        ));
        self.emit(WasmInstruction::End); // if

        self.emit_safepoint_spill_epilogue(spill.len());
        self.emit(WasmInstruction::End); // $exit

        self.emit(WasmInstruction::LocalSet(dest_local));
    }

    /// 数组元素读的内联快链（R4）。
    ///
    /// 命中条件（任一不满足即退回宿主 `emit_computed_get` 的完整语义）：
    /// 1. 接收者是 `TAG_ARRAY`
    /// 2. 键是 f64，且为 `[0, 2^31)` 内的整数值
    /// 3. 句柄处于稳定态
    /// 4. ElementsKind == PACKED（HOLEY 的洞、DICTIONARY 的索引 accessor 都必须
    ///    走完整 `[[Get]]`；见 constants::ARRAY_KIND_*）
    /// 5. 下标 < length（越界是「缺失自有属性」，须沿原型链查找）
    ///
    /// 命中路径无 call、无分配，故不是安全点，不需要 safepoint spill。
    pub(crate) fn emit_get_elem_fast(
        &mut self,
        object: wjsm_ir::ValueId,
        index: wjsm_ir::ValueId,
        dest_local: u32,
    ) {
        let object_local = self.local_idx(object.0);
        let index_local = self.local_idx(index.0);
        let addr_scratch = self.ic_addr_scratch_idx;
        let idx_scratch = self.computed_idx_scratch_idx;
        let tag_field_mask = (value::BOX_BASE | (value::TAG_MASK << 32)) as i64;
        let tag_array_bits = (value::BOX_BASE | (value::TAG_ARRAY << 32)) as i64;

        self.emit(WasmInstruction::Block(BlockType::Result(ValType::I64)));
        self.emit(WasmInstruction::Block(BlockType::Empty));

        // ── 1. 接收者是数组 ──
        self.emit(WasmInstruction::LocalGet(object_local));
        self.emit(WasmInstruction::I64Const(tag_field_mask));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(tag_array_bits));
        self.emit(WasmInstruction::I64Ne);
        self.emit(WasmInstruction::BrIf(0));

        // ── 2. 键必须是 f64（未装箱）──
        // NaN-boxed 值的装箱位全置；f64 数值不满足该模式。
        self.emit(WasmInstruction::LocalGet(index_local));
        self.emit(WasmInstruction::I64Const(value::BOX_BASE as i64));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(value::BOX_BASE as i64));
        self.emit(WasmInstruction::I64Eq);
        self.emit(WasmInstruction::BrIf(0));

        // ── 2b. 键须是 [0, 2^31) 内的整数：0 <= f < 2^31 且 trunc(f) == f ──
        // 非整数（1.5）、负数、超范围都退回宿主：它们要么是命名属性，要么需要
        // ToString 规范化，快链无法表达。
        self.emit(WasmInstruction::LocalGet(index_local));
        self.emit(WasmInstruction::F64ReinterpretI64);
        self.emit(WasmInstruction::F64Const(0.0.into()));
        self.emit(WasmInstruction::F64Lt);
        self.emit(WasmInstruction::BrIf(0));
        self.emit(WasmInstruction::LocalGet(index_local));
        self.emit(WasmInstruction::F64ReinterpretI64);
        self.emit(WasmInstruction::F64Const(2_147_483_648.0.into()));
        self.emit(WasmInstruction::F64Ge);
        self.emit(WasmInstruction::BrIf(0));
        // trunc 后转回 f64 与原值比较，排除小数（NaN 亦在此被排除：NaN != NaN）。
        self.emit(WasmInstruction::LocalGet(index_local));
        self.emit(WasmInstruction::F64ReinterpretI64);
        self.emit(WasmInstruction::I32TruncF64S);
        self.emit(WasmInstruction::LocalTee(idx_scratch));
        self.emit(WasmInstruction::F64ConvertI32S);
        self.emit(WasmInstruction::LocalGet(index_local));
        self.emit(WasmInstruction::F64ReinterpretI64);
        self.emit(WasmInstruction::F64Ne);
        self.emit(WasmInstruction::BrIf(0));

        // ── 3. 句柄稳定态（单比较，见 R3 的状态重编号）──
        self.emit(WasmInstruction::LocalGet(object_local));
        self.emit(WasmInstruction::I64Const(0xFFFF_FFFF));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(3));
        self.emit(WasmInstruction::I64Shl);
        self.emit(WasmInstruction::I64AtomicLoad(heap_mem_arg(0)));
        self.emit(WasmInstruction::LocalTee(addr_scratch));
        self.emit(WasmInstruction::I64Const(0xFFFF));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(i64::from(
            constants::HANDLE_STATE_STABLE_MIN,
        )));
        self.emit(WasmInstruction::I64LtU);
        self.emit(WasmInstruction::BrIf(0));

        // 对象地址 = entry >> 16
        self.emit(WasmInstruction::LocalGet(addr_scratch));
        self.emit(WasmInstruction::I64Const(16));
        self.emit(WasmInstruction::I64ShrU);
        self.emit(WasmInstruction::LocalSet(addr_scratch));

        // ── 4. kind 必须不是 DICTIONARY ──
        // DICTIONARY 表示某些索引位置有 accessor，快链无法表达「调 getter」，
        // 一律退回宿主（宿主按索引精确判断）。PACKED/HOLEY 都继续走快链：
        // 洞由第 6 步逐元素判定，不依赖 kind。
        self.emit(WasmInstruction::LocalGet(addr_scratch));
        self.emit(WasmInstruction::I64Load8U(heap_mem_arg_narrow(
            constants::HEAP_ARRAY_KIND_OFFSET,
            0,
        )));
        self.emit(WasmInstruction::I64Const(i64::from(
            constants::ARRAY_KIND_DICTIONARY,
        )));
        self.emit(WasmInstruction::I64Eq);
        self.emit(WasmInstruction::BrIf(0));

        // ── 5. 下标 < length（数组头 +8 的低 32 位）──
        self.emit(WasmInstruction::LocalGet(idx_scratch));
        self.emit(WasmInstruction::LocalGet(addr_scratch));
        self.emit(WasmInstruction::I64Load32U(heap_mem_arg_narrow(
            constants::HEAP_ARRAY_LENGTH_OFFSET,
            2,
        )));
        self.emit(WasmInstruction::I32WrapI64);
        self.emit(WasmInstruction::I32GeU);
        self.emit(WasmInstruction::BrIf(0));

        // ── 6. 读元素槽 addr + 16 + idx * 8，并**逐元素判洞** ──
        //
        // 不能靠 `kind == PACKED` 推断「无洞」：洞还会由字面量空位 `[1,,3]`、
        // `new Array(n)`、`length = n` 等路径产生，这些路径不经过 set_element，
        // 因此不会升级 kind。漏判会让洞哨兵（NaN-boxed tag 0x12）作为 NaN 泄漏
        // 给用户代码。内联判洞既正确，又让 HOLEY 数组的非洞元素仍走快链。
        self.emit(WasmInstruction::LocalGet(addr_scratch));
        self.emit(WasmInstruction::LocalGet(idx_scratch));
        self.emit(WasmInstruction::I64ExtendI32U);
        self.emit(WasmInstruction::I64Const(3));
        self.emit(WasmInstruction::I64Shl);
        self.emit(WasmInstruction::I64Add);
        self.emit(WasmInstruction::I64Load(heap_mem_arg(
            constants::HEAP_OBJECT_HEADER_SIZE,
        )));
        // 元素值暂存到 addr_scratch（地址已用完）。
        self.emit(WasmInstruction::LocalTee(addr_scratch));
        self.emit(WasmInstruction::I64Const(
            (value::BOX_BASE | (value::TAG_MASK << 32)) as i64,
        ));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(
            (value::BOX_BASE | (value::TAG_ARRAY_HOLE << 32)) as i64,
        ));
        self.emit(WasmInstruction::I64Eq);
        self.emit(WasmInstruction::BrIf(0));

        // ── 命中 ──
        self.emit(WasmInstruction::LocalGet(addr_scratch));
        self.emit(WasmInstruction::Br(1));

        self.emit(WasmInstruction::End); // 慢路径入口

        // ── 慢路径：宿主完整语义（带 safepoint spill）──
        let spill = self.current_spill_locals();
        self.emit_safepoint_spill_prologue(&spill);
        self.emit_computed_get(object, index);
        self.emit_safepoint_spill_epilogue(spill.len());

        self.emit(WasmInstruction::End); // $exit
        self.emit(WasmInstruction::LocalSet(dest_local));
    }

    /// 当前 GetProp 站点的 IC 槽地址；无缓存槽（计算键 / Eval 模式）返回 None。
    pub(crate) fn current_ic_slot(&self) -> Option<u32> {
        let function_id = self.current_function_id?;
        self.ic_sites
            .get(&(
                function_id.0,
                self.current_emit_block_idx,
                self.current_emit_instr_idx,
            ))
            .copied()
    }
}
