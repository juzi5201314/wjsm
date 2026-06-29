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
            } => {
                true_block.0 as usize == target || false_block.0 as usize == target
            }
            _ => false,
        })
        .count()
}


fn block_successors(block: &BasicBlock) -> impl Iterator<Item = usize> + '_ {
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
    pub(crate) fn compile_region_tree(
        &mut self,
        module: &IrModule,
        function: &IrFunction,
        region_tree: &RegionTree,
    ) -> Result<()> {
        match &region_tree.root {
            Region::Linear { start_idx } => {
                if self.needs_cfg_dispatch(function) {
                    self.compile_cfg_dispatch(module, function, *start_idx)
                } else {
                    self.compile_structured(module, function, *start_idx)
                }
            }
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
                self.emit_to_bool_i32(condition.0);
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
                    let const_val = self.encode_constant(
                        &module.constants()[case.constant.0 as usize],
                        module,
                    )?;
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
