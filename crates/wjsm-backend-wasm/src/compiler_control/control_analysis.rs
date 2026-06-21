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


impl Compiler {
    pub(crate) fn compile_region_tree(
        &mut self,
        module: &IrModule,
        function: &IrFunction,
        region_tree: &RegionTree,
    ) -> Result<()> {
        match &region_tree.root {
            Region::Linear { start_idx } => self.compile_structured(module, function, *start_idx),
        }
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
