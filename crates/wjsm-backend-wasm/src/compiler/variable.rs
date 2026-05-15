use anyhow::Result;
use wasm_encoder::{Instruction as WasmInstruction, MemArg};
use wjsm_ir::{Function as IrFunction, Instruction, Module as IrModule};

use super::state::{Compiler, EvalVarMapRecord, Region, RegionTree};
use super::cfg_analysis::{is_eval_memory_var_name, max_instruction_value_id};

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

    pub(crate) fn emit_eval_var_frame_enter(&mut self) {
        let frame_bytes = (self.var_memory_offsets.len() as u32) * 8;
        if frame_bytes == 0 {
            return;
        }

        self.emit(WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
        self.emit(WasmInstruction::LocalTee(self.eval_var_base_local_idx));
        self.emit(WasmInstruction::LocalSet(self.shadow_sp_scratch_idx));
        self.emit_shadow_stack_overflow_check(frame_bytes as i32);
        self.emit(WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
        self.emit(WasmInstruction::I32Const(frame_bytes as i32));
        self.emit(WasmInstruction::I32Add);
        self.emit(WasmInstruction::GlobalSet(self.shadow_sp_global_idx));
    }

    pub(crate) fn emit_eval_var_frame_exit(&mut self) {
        if self.var_memory_offsets.is_empty() {
            return;
        }
        self.emit(WasmInstruction::LocalGet(self.eval_var_base_local_idx));
        self.emit(WasmInstruction::GlobalSet(self.shadow_sp_global_idx));
    }

    pub(crate) fn emit_eval_var_address(&mut self, offset: u32) {
        self.emit(WasmInstruction::LocalGet(self.eval_var_base_local_idx));
        if offset != 0 {
            self.emit(WasmInstruction::I32Const(offset as i32));
            self.emit(WasmInstruction::I32Add);
        }
    }

    pub(crate) fn emit_store_stacked_binding(&mut self, memory_offset: Option<u32>, local_idx: Option<u32>) {
        if let Some(offset) = memory_offset {
            self.emit(WasmInstruction::LocalSet(self.string_concat_scratch_idx));
            self.emit_eval_var_address(offset);
            self.emit(WasmInstruction::LocalGet(self.string_concat_scratch_idx));
            self.emit(WasmInstruction::I64Store(MemArg {
                offset: 0,
                align: 3,
                memory_index: 0,
            }));
        } else if let Some(local_idx) = local_idx {
            self.emit(WasmInstruction::LocalSet(local_idx));
        }
    }
}
