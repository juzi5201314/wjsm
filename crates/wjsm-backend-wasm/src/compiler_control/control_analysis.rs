use super::*;

pub(super) fn chain_jumps_to(blocks: &[BasicBlock], start: usize, target: usize) -> bool {
    let mut current = start;
    for _ in 0..10 {
        if current == target {
            return true;
        }
        let block = match blocks.get(current) {
            Some(b) => b,
            None => return false,
        };
        match block.terminator() {
            Terminator::Jump { target: t } => current = t.0 as usize,
            _ => return false,
        }
    }
    false
}

/// 沿 Jump 链追溯，找到最终跳转目标（最多 10 跳）
pub(super) fn resolve_jump_chain(blocks: &[BasicBlock], start: usize) -> usize {
    let mut current = start;
    for _ in 0..10 {
        let block = match blocks.get(current) {
            Some(b) => b,
            None => return current,
        };
        match block.terminator() {
            Terminator::Jump { target } => current = target.0 as usize,
            _ => return current,
        }
    }
    current
}

pub(super) fn count_predecessors(blocks: &[BasicBlock], target: usize) -> usize {
    blocks
        .iter()
        .filter(|b| match b.terminator() {
            Terminator::Jump { target: t } => t.0 as usize == target,
            Terminator::Branch {
                true_block,
                false_block,
                ..
            } => true_block.0 as usize == target || false_block.0 as usize == target,
            _ => false,
        })
        .count()
}

pub(super) fn block_successors(block: &BasicBlock) -> impl Iterator<Item = usize> + '_ {
    let mut targets = [None, None];
    let mut extra: Option<Vec<usize>> = None;
    match block.terminator() {
        Terminator::Jump { target } => targets[0] = Some(target.0 as usize),
        Terminator::Branch {
            true_block,
            false_block,
            ..
        } => {
            targets[0] = Some(true_block.0 as usize);
            targets[1] = Some(false_block.0 as usize);
        }
        Terminator::Switch {
            cases,
            default_block,
            exit_block,
            ..
        } => {
            let mut switch_targets = Vec::with_capacity(cases.len() + 2);
            switch_targets.extend(cases.iter().map(|case| case.target.0 as usize));
            switch_targets.push(default_block.0 as usize);
            switch_targets.push(exit_block.0 as usize);
            extra = Some(switch_targets);
        }
        Terminator::Return { .. } | Terminator::Throw { .. } | Terminator::Unreachable => {}
    }
    targets
        .into_iter()
        .flatten()
        .chain(extra.into_iter().flatten())
}

impl Compiler {
    pub(crate) fn compile_control_flow(
        &mut self,
        module: &IrModule,
        function: &IrFunction,
        start_idx: usize,
    ) -> Result<()> {
        if self.needs_cfg_dispatch(function) {
            self.compile_cfg_dispatch(module, function, start_idx)
        } else {
            self.compile_structured(module, function, start_idx)
        }
    }

    fn needs_cfg_dispatch(&self, function: &IrFunction) -> bool {
        let blocks = function.blocks();
        let loops = detect_loops(blocks);
        blocks.iter().enumerate().any(|(idx, block)| {
            block_successors(block).any(|target_idx| {
                target_idx < idx
                    && !loops
                        .iter()
                        .any(|loop_info| loop_info.header_idx == target_idx)
            })
        })
    }

    fn compile_cfg_dispatch(
        &mut self,
        module: &IrModule,
        function: &IrFunction,
        start_idx: usize,
    ) -> Result<()> {
        let blocks = function.blocks();
        let pc = self.computed_idx_scratch_idx;
        self.emit(WasmInstruction::I32Const(start_idx as i32));
        self.emit(WasmInstruction::LocalSet(pc));
        self.emit(WasmInstruction::Block(BlockType::Empty));
        self.emit(WasmInstruction::Loop(BlockType::Empty));

        for (idx, block) in blocks.iter().enumerate() {
            // 死异常块（is_exception 恒 false 分支的异常目标）不生成 dispatch case。
            if self.current_function_id.is_some_and(|f| {
                self.f64_analysis
                    .as_ref()
                    .is_some_and(|a| a.is_dead_exception_block(f, idx))
            }) {
                continue;
            }
            self.emit(WasmInstruction::LocalGet(pc));
            self.emit(WasmInstruction::I32Const(idx as i32));
            self.emit(WasmInstruction::I32Eq);
            self.emit(WasmInstruction::If(BlockType::Empty));

            let mut suspended = false;
            for (instr_idx, instruction) in block.instructions().iter().enumerate() {
                self.set_emit_cursor(idx, instr_idx);
                if self.compile_instruction(module, instruction)? {
                    suspended = true;
                    break;
                }
            }

            if !suspended {
                self.compile_dispatch_terminator(module, blocks, idx, pc)?;
            }

            self.emit(WasmInstruction::End);
        }

        self.emit(WasmInstruction::Unreachable);
        self.emit(WasmInstruction::End);
        self.emit(WasmInstruction::End);
        self.emit_return(&None);
        Ok(())
    }

    fn compile_dispatch_terminator(
        &mut self,
        module: &IrModule,
        blocks: &[BasicBlock],
        idx: usize,
        pc: u32,
    ) -> Result<()> {
        match blocks[idx].terminator() {
            Terminator::Return { value } => self.emit_return(value),
            Terminator::Throw { value } => {
                self.emit_eval_var_frame_exit();
                self.emit(WasmInstruction::LocalGet(self.local_idx(value.0)));
                let func_idx = self
                    .builtin_func_indices
                    .get(&Builtin::CreateException)
                    .copied()
                    .expect("CreateException import must be registered");
                self.emit(WasmInstruction::Call(func_idx));
                self.emit(WasmInstruction::Return);
            }
            Terminator::Unreachable => self.emit(WasmInstruction::Unreachable),
            Terminator::Jump { target } => {
                self.emit_dispatch_jump(blocks, idx, target.0 as usize, pc, 1);
            }
            Terminator::Branch {
                condition,
                true_block,
                false_block,
            } => {
                // 恒 false（is_exception 且操作数必非异常）→ 异常目标（true）是
                // 死代码：直接跳正常路径，不发射条件求值与 if/else。
                let condition_constant_false = self.current_function_id.is_some_and(|f| {
                    self.f64_analysis.as_ref().is_some_and(|a| {
                        a.condition_constant_false(f, *condition)
                    })
                });
                let true_terminates = matches!(
                    blocks[true_block.0 as usize].terminator(),
                    Terminator::Throw { .. } | Terminator::Unreachable
                );
                if condition_constant_false && true_terminates {
                    self.emit_dispatch_jump(blocks, idx, false_block.0 as usize, pc, 1);
                    return Ok(());
                }
                self.emit_condition_to_bool_i32(*condition);
                self.emit(WasmInstruction::If(BlockType::Empty));
                self.emit_dispatch_jump(blocks, idx, true_block.0 as usize, pc, 2);
                self.emit(WasmInstruction::Else);
                self.emit_dispatch_jump(blocks, idx, false_block.0 as usize, pc, 2);
                self.emit(WasmInstruction::End);
            }
            Terminator::Switch {
                value,
                cases,
                default_block,
                ..
            } => {
                for case in cases {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(value.0)));
                    let const_val = self
                        .encode_constant(&module.constants()[case.constant.0 as usize], module)?;
                    self.emit(WasmInstruction::I64Const(const_val));
                    self.emit(WasmInstruction::I64Eq);
                    self.emit(WasmInstruction::If(BlockType::Empty));
                    self.emit_dispatch_jump(blocks, idx, case.target.0 as usize, pc, 2);
                    self.emit(WasmInstruction::End);
                }
                self.emit_dispatch_jump(blocks, idx, default_block.0 as usize, pc, 1);
            }
        }
        Ok(())
    }

    fn emit_dispatch_jump(
        &mut self,
        blocks: &[BasicBlock],
        from_idx: usize,
        target_idx: usize,
        pc: u32,
        br_depth: u32,
    ) {
        self.emit_phi_moves(blocks, from_idx, target_idx);
        self.emit(WasmInstruction::I32Const(target_idx as i32));
        self.emit(WasmInstruction::LocalSet(pc));
        self.emit(WasmInstruction::Br(br_depth));
    }

    /// Phi lowering pass: for each Phi instruction, allocate a WASM local for its dest,
    /// and schedule moves from source values in predecessor blocks.
    pub(crate) fn lower_phi_to_locals(&mut self, function: &IrFunction) {
        self.phi_locals.clear();
        let mut next_local = self.next_var_local;

        for block in function.blocks() {
            for instruction in block.instructions() {
                if let Instruction::Phi { dest, .. } = instruction {
                    self.phi_locals.insert(dest.0, next_local);
                    next_local += 1;
                }
            }
        }
        self.next_var_local = next_local;
    }

    pub(crate) fn assign_eval_var_memory(&mut self, function: &IrFunction) {
        self.var_memory_offsets.clear();
        self.current_function_has_eval = function.has_eval();
        if !function.has_eval() {
            return;
        }

        let mut names = Vec::new();
        for block in function.blocks() {
            for instruction in block.instructions() {
                let name = match instruction {
                    Instruction::LoadVar { name, .. } | Instruction::StoreVar { name, .. } => name,
                    _ => continue,
                };
                if is_eval_memory_var_name(name) {
                    names.push(name.clone());
                }
            }
        }
        names.sort();
        names.dedup();

        for (index, name) in names.into_iter().enumerate() {
            let offset = index as u32 * 8;
            self.var_memory_offsets.insert(name.clone(), offset);
            self.eval_var_map_records.push(EvalVarMapRecord {
                function_name: function.name().to_string(),
                var_name: name,
                offset,
            });
        }
    }

    pub(crate) fn assign_var_locals(&mut self, function: &IrFunction) {
        self.var_locals.clear();
        if self.ssa_local_base > 0 {
            for (index, param) in function.params().iter().enumerate() {
                if !self.is_eval_memory_var(param) {
                    self.var_locals.insert(param.clone(), index as u32);
                }
            }
        }
        let max_ssa = function
            .blocks()
            .iter()
            .flat_map(|block| block.instructions())
            .map(max_instruction_value_id)
            .max()
            .map_or(0, |max| max + 1);

        self.next_var_local = self.ssa_local_base + max_ssa;
        for block in function.blocks() {
            for instruction in block.instructions() {
                let name = match instruction {
                    Instruction::LoadVar { name, .. } | Instruction::StoreVar { name, .. } => name,
                    _ => continue,
                };
                if self.is_eval_memory_var(name) {
                    continue;
                }
                self.var_locals.entry(name.clone()).or_insert_with(|| {
                    let idx = self.next_var_local;
                    self.next_var_local += 1;
                    idx
                });
            }
        }
    }

    pub(crate) fn is_eval_memory_var(&self, name: &str) -> bool {
        self.current_function_has_eval && self.var_memory_offsets.contains_key(name)
    }
}

// ── 空跳转块消除（CFG 清洗）──────────────────────────────────────────────
//
// 语句级 is_exception 分叉的正常路径往往落到一个"空 continue 跳板"：无指令、
// 无 phi、terminator 仅为无条件 Jump 的块。该块对循环更新块产生一条非循环头
// 后向边（needs_cfg_dispatch 只豁免 detect_loops 认定的循环头），于是整个函数
// 被降级为 cfg 状态机（每迭代约 28 次分派，性能约 2 倍损失）。消除这些空块后
// CFG 恢复为结构化可编译形态（真实 wasm loop）。
//
// 正确性论证：
// - 空块 E 无任何指令 → 不定义/改写值；E → T 边上流动的值与各 P → E 边流入的
//   值相同（E 是透明转发）。把每条 P → E 边压缩为 P → T 后，T 的 phi 源 (E, v)
//   改写为 (P, v)：任何执行路径上的值序列不变。
// - 支配性：v 的定义块支配 E → 支配所有 P（P → E 是边，所有经 P 的路径都经 E）
//   → 改写后 phi 源值仍在支配位置定义，无 dominance 违例。
// - 变换后 E 不可达（所有前驱已重定向），清空指令 + terminator 置 Unreachable，
//   保持"BasicBlockId == 向量下标"不变式（不删除块、不移位）。
//
// 保守判据（不满足即跳过，绝不冒险）：
// 1. E 不是函数入口；
// 2. E 不自环（Jump 到自己 = 死循环，保留）；
// 3. 任一前驱 P 已有一条直连 T 的边 → 跳过。此时两条路径（P→E→T 与 P→T）
//    合并，phi 可能收到同一前驱的不同值（改写会产生重复前驱源）；且 branch
//    两目标退化为同一块的形态编译器处理不可靠；
// 4. 前驱 P 的 Branch 两目标都为 E（基线退化形态）→ 跳过。

/// 消除所有函数中的空跳转块，返回消除总数。
pub(crate) fn eliminate_empty_jump_blocks(module: &mut IrModule) -> usize {
    let mut total = 0;
    // 逐轮迭代：消除一个块可能暴露新的空跳转块（链 E1→E2→T 逐轮塌缩）；
    // 每轮只取索引最小的可消除候选，保证同一轮内不并发消除共享前驱的块。
    loop {
        let mut round = 0;
        for function_id in 0..module.functions().len() {
            round += eliminate_empty_jump_blocks_in_function(module, function_id);
        }
        if round == 0 {
            break;
        }
        total += round;
        // 防呆上限：正常 CFG 远小于此；达到即停，保证终止。
        if total > 8192 {
            break;
        }
    }
    total
}

/// usize → BasicBlockId（块 id 由 u32 承载，索引必然可转换）。
fn block_id(idx: usize) -> wjsm_ir::BasicBlockId {
    wjsm_ir::BasicBlockId(u32::try_from(idx).expect("block index must fit u32"))
}

/// 消除单个函数中的空跳转块（每轮最多消除一个，返回是否消除）。
fn eliminate_empty_jump_blocks_in_function(module: &mut IrModule, function_id: usize) -> usize {
    let Some(function) = module
        .function_mut(wjsm_ir::FunctionId(
            u32::try_from(function_id).expect("function id must fit u32"),
        ))
    else {
        return 0;
    };
    let entry = function.entry().0 as usize;
    let n = function.blocks().len();

    // 重新计算前驱（上一轮消除可能已改变 CFG）。
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); n];
    let loops = crate::detect_loops(function.blocks());
    {
        let blocks = function.blocks();
        for (i, block) in blocks.iter().enumerate() {
            for succ in block_successors(block) {
                preds[succ].push(i);
            }
        }
    }

    // 寻找本轮可消除的候选（索引最小者）。
    let mut chosen: Option<(usize, usize)> = None; // (e, target)
    {
        let blocks = function.blocks();
        for e in 0..n {
            if e == entry {
                continue;
            }
            let block = &blocks[e];
            if !block.instructions().is_empty() {
                continue;
            }
            let Terminator::Jump { target } = block.terminator() else {
                continue;
            };
            let t = target.0 as usize;
            if t == e || t >= e {
                // 自环或非后向跳转（前向跳板不制造非循环头后向边，无性能收益）。
                continue;
            }
            // 守卫：只消除"continue 跳板"形态——空块 E 的后向跳转目标 T 必须是
            // Jump 终止（循环增量/latch 块）且直接跳向某真实循环头。这是
            // 制造非循环头后向边（needs_cfg_dispatch 降级状态机）的唯一形态，
            // 且消除后分支目标 T 的链编译是安全的（T 终止于 Jump → 循环头，
            // compile_branch_body 直接 loop_continue br）。
            // 其余形态（T 是 Branch 延续的 if 链、phi 合并等）消除后会改变
            // 结构化编译器的嵌套续体解析路径（frame pop 可能被 br 跳过），
            // 风险大于收益，保守保留。
            let loops = &loops;
            let t_is_update_to_header = match blocks.get(t).map(|b| b.terminator()) {
                Some(Terminator::Jump { target: jt }) => loops
                    .iter()
                    .any(|l| l.header_idx == jt.0 as usize),
                _ => false,
            };
            if !t_is_update_to_header {
                continue;
            }
            // 守卫：任何前驱 P 已直连 T，或 P 的 Branch 两目标均为 E。
            let safe = preds[e].iter().all(|&p| {
                let p_block = &blocks[p];
                if block_successors(p_block).any(|s| s == t) {
                    return false;
                }
                match p_block.terminator() {
                    Terminator::Branch {
                        true_block,
                        false_block,
                        ..
                    } => true_block.0 as usize != e || false_block.0 as usize != e,
                    _ => true,
                }
            });
            if !safe {
                continue;
            }
            // 守卫：目标 T 含 phi → 跳过。消除会把"分支直接指向 phi 合并块"
            // 的形态交给结构化编译器（其 merge/续体逻辑对该形态存在多个已知
            // 错码：false_is_merge 跳过 phi move、嵌套 merge 续体错位）。
            // 目标无 phi 的消除（arithmetic 的增量块、continue 跳板）已覆盖
            // 状态机降级的性能场景，phi 目标保留原样即可。
            let target_has_phi = blocks[t]
                .instructions()
                .iter()
                .any(|ins| matches!(ins, Instruction::Phi { .. }));
            if target_has_phi {
                continue;
            }
            chosen = Some((e, t));
            break;
        }
    }

    let Some((e, t)) = chosen else {
        return 0;
    };
    let e_preds = preds[e].clone();

    let blocks = function.blocks_mut();
    // 1) 重定向所有前驱 P 的 E 边为 T 边。
    for &p in &e_preds {
        match blocks[p].terminator_mut() {
            Terminator::Jump { target } => {
                debug_assert_eq!(target.0 as usize, e);
                *target = block_id(t);
            }
            Terminator::Branch {
                true_block,
                false_block,
                ..
            } => {
                if true_block.0 as usize == e {
                    *true_block = block_id(t);
                }
                if false_block.0 as usize == e {
                    *false_block = block_id(t);
                }
            }
            Terminator::Switch {
                cases,
                default_block,
                exit_block,
                ..
            } => {
                for case in cases {
                    if case.target.0 as usize == e {
                        case.target = block_id(t);
                    }
                }
                if default_block.0 as usize == e {
                    *default_block = block_id(t);
                }
                if exit_block.0 as usize == e {
                    *exit_block = block_id(t);
                }
            }
            // Return/Throw/Unreachable 无后继，不可能在 preds(e) 中。
            _ => {}
        }
    }

    // 2) 改写 T 的 phi：源 (E, v) → 每个前驱 P 一份 (P, v)。
    //    守卫已保证 P 原本无直连 T 的边，故 (P, v) 不会与既有源冲突；
    //    防御性去重，满足 IR 校验器"phi 前驱源唯一"约束。
    for instruction in blocks[t].instructions_mut() {
        let Instruction::Phi { sources, .. } = instruction else {
            continue;
        };
        let mut e_value: Option<wjsm_ir::ValueId> = None;
        for source in sources.iter() {
            if source.predecessor.0 as usize == e {
                e_value = Some(source.value);
            }
        }
        sources.retain(|s| s.predecessor.0 as usize != e);
        if let Some(v) = e_value {
            for &p in &e_preds {
                if !sources
                    .iter()
                    .any(|s| s.predecessor.0 as usize == p && s.value == v)
                {
                    sources.push(wjsm_ir::PhiSource {
                        predecessor: block_id(p),
                        value: v,
                    });
                }
            }
        }
    }

    // 3) 中和 E：清空指令、terminator 置 Unreachable（块索引不变）。
    blocks[e].instructions_mut().clear();
    blocks[e].set_terminator(Terminator::Unreachable);

    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use wjsm_ir::{
        BasicBlock, BasicBlockId, Constant, ConstantId, Function, Module, PhiSource, Terminator,
        ValueId,
    };

    fn empty_jump(id: u32, target: u32) -> BasicBlock {
        let mut b = BasicBlock::new(BasicBlockId(id));
        b.set_terminator(Terminator::Jump {
            target: BasicBlockId(target),
        });
        b
    }

    fn branch(id: u32, cond: u32, t: u32, f: u32) -> BasicBlock {
        let mut b = BasicBlock::new(BasicBlockId(id));
        b.set_terminator(Terminator::Branch {
            condition: ValueId(cond),
            true_block: BasicBlockId(t),
            false_block: BasicBlockId(f),
        });
        b
    }

    /// 复刻 arithmetic 场景的循环形状（与真实 IR 一致）：
    ///   bb1(循环头) 条件 → 循环体 bb2 / 出口 bb4(return)；
    ///   bb2 内 is_exception 分叉 → 异常 bb6 / 空 continue 跳板 bb5；bb5 → 增量块 bb3 → 回边 bb1。
    /// 消除前 bb5→bb3 是"非循环头后向边"（bb3 不是循环头）→ needs_cfg_dispatch 降级
    /// 状态机；消除后仅剩 bb3→bb1 一条指向循环头的回边 → 可结构化编译。
    #[test]
    fn eliminates_continue_trampoline_restoring_loop_shape() {
        let mut module = Module::new();
        module.add_constant(Constant::Number(0.0));
        let mut f = Function::new("work", BasicBlockId(0));
        // bb0: 入口（定义 %0 条件、%1 异常条件）→ 循环头
        let mut bb0 = BasicBlock::new(BasicBlockId(0));
        bb0.push_instruction(Instruction::Const {
            dest: ValueId(0),
            constant: ConstantId(0),
        });
        bb0.push_instruction(Instruction::Const {
            dest: ValueId(1),
            constant: ConstantId(0),
        });
        bb0.set_terminator(Terminator::Jump {
            target: BasicBlockId(1),
        });
        f.push_block(bb0);
        f.push_block(branch(1, 0, 2, 4)); // 循环头条件：true→体，false→出口
        f.push_block(branch(2, 1, 6, 5)); // 体：is_exception → 异常/跳板
        let mut bb3 = BasicBlock::new(BasicBlockId(3));
        bb3.push_instruction(Instruction::Const {
            dest: ValueId(2),
            constant: ConstantId(0),
        });
        bb3.set_terminator(Terminator::Jump {
            target: BasicBlockId(1),
        });
        f.push_block(bb3); // 增量块（有指令，非空）
        let mut bb4 = BasicBlock::new(BasicBlockId(4));
        bb4.set_terminator(Terminator::Return { value: None });
        f.push_block(bb4); // 循环出口
        f.push_block(empty_jump(5, 3)); // 空 continue 跳板（待消除）
        let mut bb6 = BasicBlock::new(BasicBlockId(6));
        bb6.set_terminator(Terminator::Throw { value: ValueId(1) });
        f.push_block(bb6); // 异常路径
        module.push_function(f);

        // 变换前：存在非循环头后向边 → needs_cfg_dispatch 判定为真。
        let before = has_non_loop_header_back_edge(&module.functions()[0]);
        assert!(before, "空跳板应制造非循环头后向边");

        let eliminated = eliminate_empty_jump_blocks(&mut module);
        assert_eq!(eliminated, 1, "仅空跳板 bb5 可消除");

        let blocks = module.functions()[0].blocks();
        match blocks[2].terminator() {
            Terminator::Branch { false_block, .. } => assert_eq!(false_block.0, 3),
            other => panic!("expected branch, got {other:?}"),
        }
        assert!(blocks[5].instructions().is_empty());
        assert!(matches!(
            blocks[5].terminator(),
            Terminator::Unreachable
        ));
        // 变换后：无非循环头后向边 → 可结构化编译。
        assert!(!has_non_loop_header_back_edge(&module.functions()[0]));
        module.verify().expect("transformed IR must verify");
    }

    /// 复刻 `x = a || b` 形态：bb0 的 true 分支直达 phi 合并块 bb2，false 分支 bb1
    /// 跳入 bb2。bb1 非空（有 const）本就不应消除；关键是守卫也拒绝消除
    /// "前驱已直连目标"的空块（phi 前驱源会重复）。
    #[test]
    fn skips_block_when_predecessor_already_targets_merge() {
        let mut module = Module::new();
        module.add_constant(Constant::Number(1.0));
        let mut f = Function::new("or", BasicBlockId(0));
        // bb0: 条件 → bb2(合并) / bb1（定义 %0 条件与 %2 跳板源值）
        let mut bb0 = BasicBlock::new(BasicBlockId(0));
        bb0.push_instruction(Instruction::Const {
            dest: ValueId(0),
            constant: ConstantId(0),
        });
        bb0.push_instruction(Instruction::Const {
            dest: ValueId(2),
            constant: ConstantId(0),
        });
        bb0.set_terminator(Terminator::Branch {
            condition: ValueId(0),
            true_block: BasicBlockId(2),
            false_block: BasicBlockId(1),
        });
        f.push_block(bb0);
        // bb1: 空跳板 → bb2（前驱 bb0 已直连 bb2 → 守卫拒绝）
        f.push_block(empty_jump(1, 2));
        // bb2: phi 合并块
        let mut bb2 = BasicBlock::new(BasicBlockId(2));
        bb2.push_instruction(Instruction::Phi {
            dest: ValueId(1),
            sources: vec![
                PhiSource {
                    predecessor: BasicBlockId(0),
                    value: ValueId(0),
                },
                PhiSource {
                    predecessor: BasicBlockId(1),
                    value: ValueId(2),
                },
            ],
        });
        bb2.set_terminator(Terminator::Return {
            value: Some(ValueId(1)),
        });
        f.push_block(bb2);
        module.push_function(f);

        let eliminated = eliminate_empty_jump_blocks(&mut module);
        assert_eq!(eliminated, 0, "守卫应拒绝消除（前驱已直连合并块）");
        module.verify().expect("untouched IR must verify");
    }

    /// 目标块含 phi 时保守跳过（即使后向且可达循环头——phi 目标的消除会把
    /// "分支直接指向 phi 合并块"的形态交给结构化编译器）。
    /// bb4(空,后向) → bb2(phi 合并,循环 latch)。
    #[test]
    fn skips_elimination_when_target_has_phi() {
        let mut module = Module::new();
        module.add_constant(Constant::Number(0.0));
        let mut f = Function::new("phi", BasicBlockId(0));
        // bb0: 入口（定义 %0 条件、%2 源值）→ bb1
        let mut bb0 = BasicBlock::new(BasicBlockId(0));
        bb0.push_instruction(Instruction::Const {
            dest: ValueId(0),
            constant: ConstantId(0),
        });
        bb0.push_instruction(Instruction::Const {
            dest: ValueId(2),
            constant: ConstantId(0),
        });
        bb0.set_terminator(Terminator::Jump {
            target: BasicBlockId(1),
        });
        f.push_block(bb0);
        // bb1: 循环头：true → bb4(体)，false → bb5(出口)
        f.push_block(branch(1, 0, 4, 5));
        // bb2: phi 合并块（循环 latch，回边 bb1）
        let mut bb2 = BasicBlock::new(BasicBlockId(2));
        bb2.push_instruction(Instruction::Phi {
            dest: ValueId(1),
            sources: vec![PhiSource {
                predecessor: BasicBlockId(4),
                value: ValueId(2),
            }],
        });
        bb2.set_terminator(Terminator::Jump {
            target: BasicBlockId(1),
        });
        f.push_block(bb2);
        // bb3: 不可达占位
        let mut bb3 = BasicBlock::new(BasicBlockId(3));
        bb3.set_terminator(Terminator::Unreachable);
        f.push_block(bb3);
        // bb4: 后向空跳板 → bb2（目标含 phi → 跳过）
        f.push_block(empty_jump(4, 2));
        // bb5: 循环出口
        let mut bb5 = BasicBlock::new(BasicBlockId(5));
        bb5.set_terminator(Terminator::Return { value: None });
        f.push_block(bb5);
        module.push_function(f);

        let eliminated = eliminate_empty_jump_blocks(&mut module);
        assert_eq!(eliminated, 0, "目标含 phi 时必须保守跳过");
        let blocks = module.functions()[0].blocks();
        assert!(matches!(
            blocks[4].terminator(),
            Terminator::Jump { .. }
        ));
        module.verify().expect("untouched IR must verify");
    }

    /// 循环内后向空跳转链逐轮塌缩（bb4→bb3→bb2，bb2 回边到循环头 bb1）。
    /// latch bb2 保留（其目标 bb1 是 Branch 终止的循环头，非 Jump 终止的更新块）。
    #[test]
    fn collapses_chains_of_empty_blocks() {
        let mut module = Module::new();
        module.add_constant(Constant::Number(0.0));
        let mut f = Function::new("chain", BasicBlockId(0));
        // bb0: 入口（定义 %0 条件）→ 循环头 bb1
        let mut bb0 = BasicBlock::new(BasicBlockId(0));
        bb0.push_instruction(Instruction::Const {
            dest: ValueId(0),
            constant: ConstantId(0),
        });
        bb0.set_terminator(Terminator::Jump {
            target: BasicBlockId(1),
        });
        f.push_block(bb0);
        // bb1: 循环头：true → bb4(体)，false → bb5(出口)
        f.push_block(branch(1, 0, 4, 5));
        // bb2: 循环 latch（空跳回边 bb1）
        f.push_block(empty_jump(2, 1));
        // bb3: 后向空跳板 → bb2（bb2 Jump 直达循环头 ✓）
        f.push_block(empty_jump(3, 2));
        // bb4: 后向空跳板 → bb3
        f.push_block(empty_jump(4, 3));
        // bb5: 循环出口
        let mut bb5 = BasicBlock::new(BasicBlockId(5));
        bb5.set_terminator(Terminator::Return { value: None });
        f.push_block(bb5);
        module.push_function(f);

        let eliminated = eliminate_empty_jump_blocks(&mut module);
        assert_eq!(eliminated, 2, "bb4/bb3 逐轮塌缩至 latch bb2；bb2 与入口 bb0 保留");

        let blocks = module.functions()[0].blocks();
        // 循环头 bb1 最终直达 latch bb2
        match blocks[1].terminator() {
            Terminator::Branch { true_block, .. } => assert_eq!(true_block.0, 2),
            other => panic!("expected branch, got {other:?}"),
        }
        module.verify().expect("transformed IR must verify");
    }

    /// 保守判据：自环空块（死循环）与函数入口块不消除。
    #[test]
    fn keeps_self_loop_and_entry_blocks() {
        let mut module = Module::new();
        let mut f = Function::new("spin", BasicBlockId(0));
        // bb0: 入口 = 空跳块（不应消除，否则入口丢失）
        f.push_block(empty_jump(0, 1));
        // bb1: 空自环死循环
        f.push_block(empty_jump(1, 1));
        // bb2: 返回
        let mut bb2 = BasicBlock::new(BasicBlockId(2));
        bb2.set_terminator(Terminator::Return { value: None });
        f.push_block(bb2);
        module.push_function(f);

        let eliminated = eliminate_empty_jump_blocks(&mut module);
        assert_eq!(eliminated, 0, "入口与自环块必须保留");
        module.verify().expect("untouched IR must verify");
    }

    /// 真实 JS 回归：arithmetic 场景的 work() 循环消除空 continue 跳板后
    /// 无非循环头后向边（结构化编译的前提）。
    #[test]
    fn real_arithmetic_loop_becomes_structured_compilable() {
        let mut module = lower(
            "function work() { let s = 0.0; for (let i = 0; i < 100000; i++) s += i * 1.0001; return s; } console.log(work());",
        );
        let work_idx = module
            .functions()
            .iter()
            .position(|f| f.name() == "work")
            .expect("work function");

        let before = has_non_loop_header_back_edge(&module.functions()[work_idx]);
        let eliminated = eliminate_empty_jump_blocks(&mut module);
        let after = has_non_loop_header_back_edge(&module.functions()[work_idx]);

        assert!(before, "arithmetic work() 基线上应存在非循环头后向边");
        assert!(eliminated > 0, "work() 应有空跳板可消除");
        assert!(!after, "消除后应恢复为结构化可编译 CFG");
        module.verify().expect("transformed IR must verify");
    }

    // ── helpers ────────────────────────────────────────────────────────────

    /// 是否存在"非循环头后向边"（needs_cfg_dispatch 的判定）：
    /// 目标索引 < 源索引且目标不是 detect_loops 认定的循环头。
    fn has_non_loop_header_back_edge(function: &Function) -> bool {
        let blocks = function.blocks();
        let loops = crate::detect_loops(blocks);
        blocks.iter().enumerate().any(|(idx, block)| {
            block_successors(block).any(|target_idx| {
                target_idx < idx
                    && !loops
                        .iter()
                        .any(|loop_info| loop_info.header_idx == target_idx)
            })
        })
    }

    fn lower(source: &str) -> Program {
        use wjsm_parser::parse_module;
        use wjsm_semantic::lower_module;
        lower_module(parse_module(source).expect("parse"), false).expect("lower")
    }
}

