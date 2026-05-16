use super::*;

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

    pub(crate) fn emit_store_stacked_binding(
        &mut self,
        memory_offset: Option<u32>,
        local_idx: Option<u32>,
    ) {
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

    /// 结构化编译：按顺序处理 block，处理 Branch 为 WASM if/else，处理循环为 block/loop。
    pub(crate) fn compile_structured(
        &mut self,
        module: &IrModule,
        function: &IrFunction,
        start_idx: usize,
    ) -> Result<()> {
        let blocks = function.blocks();
        let loops = detect_loops(blocks);
        self.loop_stack.clear();
        self.if_depth = 0;
        let mut idx = start_idx;

        while idx < blocks.len() {
            while let Some(top) = self.loop_stack.last() {
                if idx >= top.exit_idx {
                    self.emit(WasmInstruction::End);
                    self.emit(WasmInstruction::End);
                    self.loop_stack.pop();
                } else {
                    break;
                }
            }

            if self.compiled_blocks.contains(&idx) {
                break;
            }

            if let Some(loop_info) = loops.iter().find(|l| l.header_idx == idx) {
                self.emit(WasmInstruction::Block(BlockType::Empty));
                self.emit(WasmInstruction::Loop(BlockType::Empty));
                self.loop_stack.push(loop_info.clone());
            }

            self.compiled_blocks.insert(idx);

            let block = &blocks[idx];

            let mut suspended = false;
            for instruction in block.instructions() {
                if self.compile_instruction(module, instruction)? {
                    suspended = true;
                    break;
                }
            }

            if suspended {
                idx += 1;
                continue;
            }

            match block.terminator() {
                Terminator::Return { value } => {
                    self.emit_return(value);
                    idx += 1;
                }
                Terminator::Unreachable => {
                    // 死代码块 — 跳过
                    idx += 1;
                }
                Terminator::Jump { target } => {
                    let target_idx = target.0 as usize;
                    if let Some(depth) = self.loop_continue_depth(target_idx) {
                        // back-edge：continue 循环
                        self.emit_phi_moves(blocks, idx, target_idx);
                        self.emit(WasmInstruction::Br(depth));
                        idx += 1;
                    } else if let Some(depth) = self.loop_break_depth(target_idx) {
                        // 跳到循环出口：break
                        self.emit_phi_moves(blocks, idx, target_idx);
                        self.emit(WasmInstruction::Br(depth));
                        idx += 1;
                    } else if target_idx == idx + 1 {
                        // 自然 fall-through
                        idx = target_idx;
                    } else if target_idx > idx {
                        // 前向跳转到非相邻 block（如 try/catch 跳过 finally_entry）
                        // 中间的 block 是不可达的，直接跳到目标
                        self.emit_phi_moves(blocks, idx, target_idx);
                        idx = target_idx;
                    } else {
                        // 后向跳转但不是循环 — 不应发生
                        self.emit_phi_moves(blocks, idx, target_idx);
                        idx = target_idx;
                    }
                }
                Terminator::Branch {
                    condition,
                    true_block,
                    false_block,
                } => {
                    let true_idx = true_block.0 as usize;
                    let false_idx = false_block.0 as usize;

                    // 循环头条件（while/for 模式）：
                    // true → body, false → exit
                    // 发射：condition → i32.eqz → br_if (break if falsy)
                    if self
                        .loop_stack
                        .last()
                        .map_or(false, |l| l.header_idx == idx)
                    {
                        self.emit_to_bool_i32(condition.0);
                        self.emit(WasmInstruction::I32Eqz);
                        let break_depth = self.loop_break_depth(false_idx).unwrap_or(1);
                        self.emit(WasmInstruction::BrIf(break_depth));
                        idx = true_idx;
                        continue;
                    }

                    // do-while 条件（true 目标是循环头）：
                    // true → header (continue), false → exit
                    // 发射：condition → br_if (continue if truthy)
                    if let Some(depth) = self.loop_continue_depth(true_idx) {
                        self.emit_to_bool_i32(condition.0);
                        self.emit(WasmInstruction::BrIf(depth));
                        idx = false_idx;
                        continue;
                    }

                    // 普通 if/else
                    self.emit_to_bool_i32(condition.0);
                    self.if_depth += 1;
                    self.emit(WasmInstruction::If(BlockType::Empty));

                    let true_is_merge = self.is_merge_block(blocks, false_idx, true_idx);
                    let false_is_merge = self.is_merge_block(blocks, true_idx, false_idx);

                    let true_terminates = if true_is_merge {
                        self.emit_phi_moves(blocks, idx, true_idx);
                        false
                    } else {
                        self.compiled_blocks.insert(true_idx);
                        self.compile_branch_body(module, blocks, true_idx)?
                    };

                    self.emit(WasmInstruction::Else);
                    let false_terminates = if false_is_merge {
                        self.emit_phi_moves(blocks, idx, false_idx);
                        false
                    } else {
                        self.compiled_blocks.insert(false_idx);
                        self.compile_branch_body(module, blocks, false_idx)?
                    };

                    self.emit(WasmInstruction::End);
                    self.if_depth -= 1;

                    if true_terminates && false_terminates {
                        self.emit(WasmInstruction::Unreachable);
                    }

                    // 继续到 merge block
                    let merge = if false_is_merge {
                        false_idx
                    } else if true_is_merge {
                        true_idx
                    } else {
                        self.find_merge(blocks, true_idx, false_idx)
                    };

                    // 如果 merge 已编译（循环 back-edge 情况），跳到下一个未编译块
                    if self.compiled_blocks.contains(&merge) {
                        let mut next = true_idx.max(false_idx) + 1;
                        while next < blocks.len() && self.compiled_blocks.contains(&next) {
                            next += 1;
                        }
                        idx = next;
                    } else {
                        idx = merge;
                    }
                
                    // 当 merge 已被编译（作为某分支主体的一部分），而另一个分支未终结时，
                    // 需要为 fall-through 路径重新发射 merge 块的终止器。
                    if self.compiled_blocks.contains(&merge) && !(true_terminates && false_terminates) {
                        if let Some(merge_block) = blocks.get(merge) {
                            // 重新发射 phi 指令：将 phi_local 复制到 SSA local
                            for instruction in merge_block.instructions() {
                                if let Instruction::Phi { dest, .. } = instruction {
                                    if let Some(&phi_local) = self.phi_locals.get(&dest.0) {
                                        self.emit(WasmInstruction::LocalGet(phi_local));
                                        self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
                                    }
                                }
                            }
                            // 重新发射 merge 块的 return
                            if let Terminator::Return { value } = merge_block.terminator() {
                                self.emit_return(value);
                            }
                        }
                    }
                }
                Terminator::Switch {
                    value,
                    cases,
                    default_block,
                    exit_block,
                } => {
                    let exit_idx = exit_block.0 as usize;
                    self.compiled_blocks.insert(idx);
                    let default_target_idx = default_block.0 as usize;

                    struct SwitchEntry {
                        is_default: bool,
                        constant_idx: Option<u32>,
                        target_idx: usize,
                    }

                    let mut entries: Vec<SwitchEntry> = Vec::new();
                    for case in cases.iter() {
                        entries.push(SwitchEntry {
                            is_default: false,
                            constant_idx: Some(case.constant.0),
                            target_idx: case.target.0 as usize,
                        });
                    }
                    entries.push(SwitchEntry {
                        is_default: true,
                        constant_idx: None,
                        target_idx: default_target_idx,
                    });

                    entries.sort_by_key(|e| e.target_idx);

                    let num_entries = entries.len();
                    let default_pos = entries.iter().position(|e| e.is_default).unwrap();

                    self.compiled_blocks.insert(default_target_idx);
                    self.compiled_blocks.insert(exit_idx);

                    self.emit(WasmInstruction::Block(BlockType::Empty));

                    for _ in 0..num_entries {
                        self.emit(WasmInstruction::Block(BlockType::Empty));
                    }

                    for (i, entry) in entries.iter().enumerate() {
                        if entry.is_default {
                            continue;
                        }
                        self.emit(WasmInstruction::LocalGet(self.local_idx(value.0)));
                        let const_val = self.encode_constant(
                            &module.constants()[entry.constant_idx.unwrap() as usize],
                            module,
                        )?;
                        self.emit(WasmInstruction::I64Const(const_val));
                        self.emit(WasmInstruction::I64Eq);
                        self.emit(WasmInstruction::BrIf(i as u32));
                    }
                    self.emit(WasmInstruction::Br(default_pos as u32));

                    for i in 0..num_entries {
                        if i == default_pos {
                            self.compiled_blocks.remove(&default_target_idx);
                        }
                        self.emit(WasmInstruction::End);
                        let entry_target = entries[i].target_idx;
                        let switch_break_depth = (num_entries - i - 1) as u32;
                        let extra_depth = (num_entries - i) as u32;
                        self.compile_switch_case(
                            module,
                            blocks,
                            entry_target,
                            exit_idx,
                            switch_break_depth,
                            extra_depth,
                            &loops,
                        )?;
                    }

                    self.emit(WasmInstruction::End);

                    if self.current_func_returns_value {
                        self.emit(WasmInstruction::Unreachable);
                    }

                    idx = exit_idx;
                }
                Terminator::Throw { value } => {
                    // 调用 runtime throw host function，然后 trap
                    self.emit_eval_var_frame_exit();
                    self.emit(WasmInstruction::LocalGet(self.local_idx(value.0)));
                    let func_idx = self
                        .builtin_func_indices
                        .get(&Builtin::Throw)
                        .copied()
                        .unwrap_or(3);
                    self.emit(WasmInstruction::Call(func_idx));
                    self.emit(WasmInstruction::Unreachable);
                    idx += 1;
                }
            }
        }

        // 关闭所有剩余的循环
        while self.loop_stack.pop().is_some() {
            self.emit(WasmInstruction::End); // loop end
            self.emit(WasmInstruction::End); // block end
        }

        // 函数返回 i64 时，确保所有控制流路径末尾都有值在栈上。
        // 到达此处意味着没有任何块以 Return 结束——应被视为 unreachable。
        if self.current_func_returns_value {
            self.emit(WasmInstruction::Unreachable);
        }

        Ok(())
    }

    /// 编译 switch case body。支持嵌套控制流（if/else、循环、嵌套 switch）。
    /// 从 case_idx 开始，跟随控制流编译所有属于 case body 的 block。
    pub(crate) fn compile_switch_case(
        &mut self,
        module: &IrModule,
        blocks: &[BasicBlock],
        case_start: usize,
        exit_idx: usize,
        switch_break_depth: u32,
        extra_depth: u32,
        loops: &[LoopInfo],
    ) -> Result<()> {
        let initial_loop_depth = self.loop_stack.len();
        let mut idx = case_start;

        loop {
            if idx >= blocks.len() {
                break;
            }

            // 关闭已到达出口的循环
            while let Some(top) = self.loop_stack.last() {
                if idx >= top.exit_idx && self.loop_stack.len() > initial_loop_depth {
                    self.emit(WasmInstruction::End);
                    self.emit(WasmInstruction::End);
                    self.loop_stack.pop();
                } else {
                    break;
                }
            }

            // 在循环头打开 block/loop
            if let Some(loop_info) = loops.iter().find(|l| l.header_idx == idx) {
                self.emit(WasmInstruction::Block(BlockType::Empty));
                self.emit(WasmInstruction::Loop(BlockType::Empty));
                self.loop_stack.push(loop_info.clone());
            }

            if self.compiled_blocks.contains(&idx) {
                break;
            }
            self.compiled_blocks.insert(idx);

            let block = &blocks[idx];

            let mut suspended = false;
            for instruction in block.instructions() {
                if self.compile_instruction(module, instruction)? {
                    suspended = true;
                    break;
                }
            }

            if suspended {
                break;
            }

            match block.terminator() {
                Terminator::Return { value } => {
                    self.emit_return(value);
                    break;
                }
                Terminator::Unreachable => {
                    break;
                }
                Terminator::Jump { target } => {
                    let target_idx = target.0 as usize;
                    if target_idx == exit_idx {
                        // switch break
                        self.emit_phi_moves(blocks, idx, target_idx);
                        self.emit(WasmInstruction::Br(switch_break_depth));
                        break;
                    } else if let Some(depth) = self.loop_continue_depth(target_idx) {
                        // loop continue（仅当循环在 case body 外部时加 extra_depth）
                        self.emit_phi_moves(blocks, idx, target_idx);
                        let adj = if target_idx >= case_start {
                            depth
                        } else {
                            depth + extra_depth
                        };
                        self.emit(WasmInstruction::Br(adj));
                        if target_idx >= case_start {
                            idx += 1; // 循环在 case body 内部，继续编译下一个 block（循环出口）
                        } else {
                            break; // 循环在 case body 外部（switch 在循环内），退出 case body 编译
                        }
                    } else if let Some(depth) = self.loop_break_depth(target_idx) {
                        // loop break（仅当循环在 case body 外部时加 extra_depth）
                        self.emit_phi_moves(blocks, idx, target_idx);
                        let adj = if target_idx >= case_start {
                            depth
                        } else {
                            depth + extra_depth
                        };
                        self.emit(WasmInstruction::Br(adj));
                        if target_idx >= case_start {
                            idx = target_idx; // 循环在 case body 内部，继续到循环出口
                        } else {
                            break; // 循环在 case body 外部，退出 case body 编译
                        }
                    } else if target_idx == idx + 1 {
                        // 自然 fall-through
                        idx = target_idx;
                    } else if target_idx > idx {
                        // 前向跳转
                        self.emit_phi_moves(blocks, idx, target_idx);
                        idx = target_idx;
                    } else {
                        // 后向跳转
                        self.emit_phi_moves(blocks, idx, target_idx);
                        idx = target_idx;
                    }
                }
                Terminator::Branch {
                    condition,
                    true_block,
                    false_block,
                } => {
                    let true_idx = true_block.0 as usize;
                    let false_idx = false_block.0 as usize;

                    // 循环头条件（while/for 模式）：
                    if self
                        .loop_stack
                        .last()
                        .map_or(false, |l| l.header_idx == idx)
                    {
                        self.emit_to_bool_i32(condition.0);
                        self.emit(WasmInstruction::I32Eqz);
                        let break_depth = self.loop_break_depth(false_idx).unwrap_or(1);
                        let adj = if false_idx >= case_start {
                            break_depth
                        } else {
                            break_depth + extra_depth
                        };
                        self.emit(WasmInstruction::BrIf(adj));
                        idx = true_idx;
                        continue;
                    }

                    // do-while 条件（true 目标是循环头）
                    if let Some(depth) = self.loop_continue_depth(true_idx) {
                        self.emit_to_bool_i32(condition.0);
                        let adj = if true_idx >= case_start {
                            depth
                        } else {
                            depth + extra_depth
                        };
                        self.emit(WasmInstruction::BrIf(adj));
                        idx = false_idx;
                        continue;
                    }

                    // 普通 if/else
                    self.emit_to_bool_i32(condition.0);
                    self.if_depth += 1;
                    self.emit(WasmInstruction::If(BlockType::Empty));

                    let true_is_merge = self.is_merge_block(blocks, false_idx, true_idx);
                    let false_is_merge = self.is_merge_block(blocks, true_idx, false_idx);

                    let true_terminates = if true_is_merge {
                        self.emit_phi_moves(blocks, idx, true_idx);
                        self.emit(WasmInstruction::Nop);
                        false
                    } else {
                        self.compiled_blocks.insert(true_idx);
                        self.compile_branch_body(module, blocks, true_idx)?
                    };

                    self.emit(WasmInstruction::Else);
                    let false_terminates = if false_is_merge {
                        self.emit_phi_moves(blocks, idx, false_idx);
                        self.emit(WasmInstruction::Nop);
                        false
                    } else {
                        self.compiled_blocks.insert(false_idx);
                        self.compile_branch_body(module, blocks, false_idx)?
                    };

                    self.emit(WasmInstruction::End);
                    self.if_depth -= 1;

                    if true_terminates && false_terminates {
                        self.emit(WasmInstruction::Unreachable);
                    }

                    // 继续到 merge block
                    let merge = if false_is_merge {
                        false_idx
                    } else if true_is_merge {
                        true_idx
                    } else {
                        self.find_merge(blocks, true_idx, false_idx)
                    };

                    if self.compiled_blocks.contains(&merge) {
                        let mut next = true_idx.max(false_idx) + 1;
                        while next < blocks.len() && self.compiled_blocks.contains(&next) {
                            next += 1;
                        }
                        idx = next;
                    } else {
                        idx = merge;
                    }
                }
                Terminator::Switch {
                    value,
                    cases,
                    default_block,
                    exit_block: nested_exit,
                } => {
                    // case body 内嵌套的 switch
                    let num_cases = cases.len();
                    let nested_default_idx = default_block.0 as usize;
                    let nested_exit_idx = nested_exit.0 as usize;
                    // 发射嵌套 switch 的 WASM blocks
                    self.emit(WasmInstruction::Block(BlockType::Empty));
                    self.emit(WasmInstruction::Block(BlockType::Empty));
                    for _ in 0..num_cases {
                        self.emit(WasmInstruction::Block(BlockType::Empty));
                    }

                    // 发射比较链
                    for (i, case) in cases.iter().enumerate() {
                        self.emit(WasmInstruction::LocalGet(self.local_idx(value.0)));
                        let const_val = self.encode_constant(
                            &module.constants()[case.constant.0 as usize],
                            module,
                        )?;
                        self.emit(WasmInstruction::I64Const(const_val));
                        self.emit(WasmInstruction::I64Eq);
                        self.emit(WasmInstruction::BrIf(i as u32));
                    }
                    self.emit(WasmInstruction::Br(num_cases as u32));

                    // 编译嵌套 case bodies
                    // 性能优化：直接迭代 cases，避免创建中间向量。
                    for (i, case) in cases.iter().enumerate() {
                        self.emit(WasmInstruction::End);
                        let cidx = case.target.0 as usize;
                        self.compiled_blocks.insert(cidx);
                        let nested_break = (num_cases - i) as u32;
                        let nested_extra = extra_depth + (num_cases - i) as u32 + 1;
                        self.compile_switch_case(
                            module,
                            blocks,
                            cidx,
                            nested_exit_idx,
                            nested_break,
                            nested_extra,
                            loops,
                        )?;
                    }

                    // 编译嵌套 default body
                    self.emit(WasmInstruction::End);
                    self.compiled_blocks.insert(nested_default_idx);
                    self.compile_switch_case(
                        module,
                        blocks,
                        nested_default_idx,
                        nested_exit_idx,
                        0,
                        extra_depth + 1,
                        loops,
                    )?;

                    // 关闭 nested exit block
                    self.emit(WasmInstruction::End);
                    self.compiled_blocks.insert(nested_exit_idx);

                    idx = nested_exit_idx;
                }
                Terminator::Throw { value } => {
                    self.emit_eval_var_frame_exit();
                    self.emit(WasmInstruction::LocalGet(self.local_idx(value.0)));
                    let func_idx = self
                        .builtin_func_indices
                        .get(&Builtin::Throw)
                        .copied()
                        .unwrap_or(3);
                    self.emit(WasmInstruction::Call(func_idx));
                    self.emit(WasmInstruction::Unreachable);
                    break;
                }
            }
        }

        // 关闭在 case body 内打开的循环
        while self.loop_stack.len() > initial_loop_depth {
            self.loop_stack.pop();
            self.emit(WasmInstruction::End);
            self.emit(WasmInstruction::End);
        }

        Ok(())
    }

    /// 编译分支体（if/else 的 true 或 false block）。
    /// 处理 Jump 到 merge block（no-op）、Return（发射）、循环 continue/break（发射 br）。
    /// 返回 `Ok(true)` 表示该分支以终止指令（Return/Unreachable/br）结束，
    /// 调用者可据此判断是否需要发射 `Unreachable` 以避免 WASM 验证错误；
    /// 返回 `Ok(false)` 表示分支正常落入（fall through）后续代码。
    pub(crate) fn compile_branch_body(
        &mut self,
        module: &IrModule,
        blocks: &[BasicBlock],
        idx: usize,
    ) -> Result<bool> {
        if idx >= blocks.len() {
            return Ok(false);
        }
        let block = &blocks[idx];

        let mut suspended = false;
        for instruction in block.instructions() {
            if self.compile_instruction(module, instruction)? {
                suspended = true;
                break;
            }
        }

        if suspended {
            return Ok(false);
        }

        match block.terminator() {
            Terminator::Return { value } => {
                self.emit_return(value);
                Ok(true)
            }
            Terminator::Jump { target } => {
                let target_idx = target.0 as usize;
                if let Some(depth) = self.loop_continue_depth(target_idx) {
                    // back-edge：continue 循环
                    self.emit_phi_moves(blocks, idx, target_idx);
                    self.emit(WasmInstruction::Br(depth));
                    Ok(true)
                } else if let Some(depth) = self.loop_break_depth(target_idx) {
                    // 跳到循环出口：break
                    self.emit_phi_moves(blocks, idx, target_idx);
                    self.emit(WasmInstruction::Br(depth));
                    Ok(true)
                } else if target_idx < idx && block_has_suspend(&blocks[target_idx]) {
                    // async 状态机的循环头可能位于另一个 switch case 中，不能用当前 case 的 label 回跳；
                    // 这里内联到下一个 suspend，让循环体能够调度下一轮 resume。
                    self.emit_phi_moves(blocks, idx, target_idx);
                    self.compile_branch_body(module, blocks, target_idx)
                } else {
                    // 普通 merge 跳转
                    self.emit_phi_moves(blocks, idx, target_idx);
                    Ok(false)
                }
            }
            Terminator::Unreachable => Ok(true),
            Terminator::Branch {
                condition,
                true_block,
                false_block,
            } => {
                // 分支体内嵌套的 if/else
                let true_idx = true_block.0 as usize;
                let false_idx = false_block.0 as usize;

                self.emit_to_bool_i32(condition.0);
                self.if_depth += 1;
                self.emit(WasmInstruction::If(BlockType::Empty));

                let true_is_merge = self.is_merge_block(blocks, false_idx, true_idx);
                let false_is_merge = self.is_merge_block(blocks, true_idx, false_idx);

                let true_terminates = if true_is_merge {
                    self.emit_phi_moves(blocks, idx, true_idx);
                    self.emit(WasmInstruction::Nop);
                    false
                } else {
                    self.compiled_blocks.insert(true_idx);
                    self.compile_branch_body(module, blocks, true_idx)?
                };

                self.emit(WasmInstruction::Else);
                let false_terminates = if false_is_merge {
                    self.emit_phi_moves(blocks, idx, false_idx);
                    self.emit(WasmInstruction::Nop);
                    false
                } else {
                    self.compiled_blocks.insert(false_idx);
                    self.compile_branch_body(module, blocks, false_idx)?
                };

                self.emit(WasmInstruction::End);
                self.if_depth -= 1;

                if true_terminates && false_terminates {
                    self.emit(WasmInstruction::Unreachable);
                }

                Ok(true_terminates && false_terminates)
            }
            _ => {
                self.emit(WasmInstruction::Unreachable);
                Ok(true)
            }
        }
    }

    /// Emit Phi moves: for each Phi instruction in the target block that references
    /// the current predecessor block, emit a move from the source value to the Phi local.
    pub(crate) fn emit_phi_moves(
        &mut self,
        blocks: &[BasicBlock],
        pred_idx: usize,
        target_idx: usize,
    ) {
        if target_idx >= blocks.len() {
            return;
        }
        let target_block = &blocks[target_idx];
        for instruction in target_block.instructions() {
            if let Instruction::Phi { dest, sources } = instruction {
                for source in sources {
                    if source.predecessor.0 as usize == pred_idx {
                        if let Some(&phi_local) = self.phi_locals.get(&dest.0) {
                            self.emit(WasmInstruction::LocalGet(self.local_idx(source.value.0)));
                            self.emit(WasmInstruction::LocalSet(phi_local));
                        }
                    }
                }
            }
        }
    }

    /// Check if `false_idx` is the natural merge block for a branch where
    /// true path is at `true_idx` and false path is at `false_idx`.
    pub(crate) fn is_merge_block(
        &self,
        blocks: &[BasicBlock],
        true_idx: usize,
        false_idx: usize,
    ) -> bool {
        if let Some(true_block) = blocks.get(true_idx) {
            match true_block.terminator() {
                Terminator::Jump { target } if target.0 as usize == false_idx => return true,
                _ => {}
            }
        }
        if let Some(false_block) = blocks.get(false_idx) {
            for instruction in false_block.instructions() {
                if let Instruction::Phi { sources, .. } = instruction {
                    if sources.len() > 1
                        && sources.iter().any(|s| s.predecessor.0 as usize == true_idx)
                    {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Find the merge block where true and false paths converge.
    pub(crate) fn find_merge(
        &self,
        blocks: &[BasicBlock],
        true_idx: usize,
        false_idx: usize,
    ) -> usize {
        // Check where the true block jumps to
        if let Some(true_block) = blocks.get(true_idx) {
            match true_block.terminator() {
                Terminator::Jump { target } => return target.0 as usize,
                _ => {}
            }
        }
        // Check where the false block jumps to
        if let Some(false_block) = blocks.get(false_idx) {
            match false_block.terminator() {
                Terminator::Jump { target } => return target.0 as usize,
                _ => {}
            }
        }
        // Default: the block after the false block
        false_idx + 1
    }

    pub(crate) fn emit_return(&mut self, value: &Option<ValueId>) {
        if let Some(v) = value {
            self.emit(WasmInstruction::LocalGet(self.local_idx(v.0)));
        } else if self.current_func_returns_value {
            self.emit(WasmInstruction::I64Const(value::encode_undefined()));
        }
        self.emit_eval_var_frame_exit();
        self.emit(WasmInstruction::Return);
    }

    pub(crate) fn loop_continue_depth(&self, target_idx: usize) -> Option<u32> {
        let len = self.loop_stack.len();
        for (i, l) in self.loop_stack.iter().enumerate().rev() {
            if l.header_idx == target_idx {
                return Some(2 * (len - 1 - i) as u32 + self.if_depth);
            }
        }
        None
    }

    pub(crate) fn loop_break_depth(&self, target_idx: usize) -> Option<u32> {
        let len = self.loop_stack.len();
        for (i, l) in self.loop_stack.iter().enumerate().rev() {
            if l.exit_idx == target_idx {
                return Some(2 * (len - 1 - i) as u32 + 1 + self.if_depth);
            }
        }
        None
    }

    // ── Instruction compilation ─────────────────────────────────────────────
}
