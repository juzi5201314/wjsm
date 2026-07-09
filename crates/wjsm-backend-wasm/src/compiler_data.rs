use super::*;

impl Compiler {
    pub(crate) fn encode_constant(
        &mut self,
        constant: &Constant,
        _module: &IrModule,
    ) -> Result<i64> {
        match constant {
            Constant::Number(value) => Ok(value.to_bits() as i64),
            Constant::String(value) => {
                if let Some(&ptr) = self.string_ptr_cache.get(value) {
                    return Ok(value::encode_string_ptr(ptr));
                }
                let ptr = self.data_base + self.data_offset;
                let mut bytes = value.as_bytes().to_vec();
                bytes.push(0);
                let len = bytes.len() as u32;

                self.string_data.extend(bytes);
                self.data_offset += len;
                self.string_ptr_cache.insert(value.clone(), ptr);

                Ok(value::encode_string_ptr(ptr))
            }
            Constant::Bool(b) => Ok(value::encode_bool(*b)),
            Constant::Null => Ok(value::encode_null()),
            Constant::Undefined => Ok(value::encode_undefined()),
            Constant::FunctionRef(function_id) => self.encode_function_ref_id(*function_id),
            Constant::NativeCallableEval => Ok(value::encode_native_callable_idx(0)),
            Constant::BigInt(_) => {
                bail!("BigInt constants should be handled in compile_instruction::Const")
            }
            Constant::RegExp { .. } => {
                bail!("RegExp constants should be handled in compile_instruction::Const")
            }
            Constant::ModuleId(module_id) => {
                // 模块 ID 直接编码为 i64 整数
                Ok(module_id.0 as i64)
            }
        }
    }

    pub(crate) fn encode_function_ref_id(&self, function_id: wjsm_ir::FunctionId) -> Result<i64> {
        let wasm_idx = *self
            .function_id_to_wasm_idx
            .get(&function_id.0)
            .with_context(|| format!("no WASM index for function id {}", function_id.0))?;
        let table_idx = *self
            .function_table_reverse
            .get(&wasm_idx)
            .with_context(|| format!("no table index for WASM function index {wasm_idx}"))?;
        Ok(value::encode_function_idx(table_idx))
    }
    /// Intern a nul-terminated string in the data section and return its offset.
    /// 如果字符串已缓存，直接返回已有偏移量。
    /// 与 encode_constant 中的字符串处理逻辑相同。
    pub(crate) fn intern_data_string(&mut self, s: &str) -> u32 {
        if let Some(&ptr) = self.string_ptr_cache.get(s) {
            return ptr;
        }
        let ptr = self.data_base + self.data_offset;
        let mut bytes = s.as_bytes().to_vec();
        bytes.push(0);
        let len = bytes.len() as u32;
        self.string_data.extend(bytes);
        self.data_offset += len;
        self.string_ptr_cache.insert(s.to_string(), ptr);
        ptr
    }

    pub(crate) fn finalize_eval_var_map_data(&mut self) {
        self.eval_var_map_records.sort_by(|a, b| {
            a.function_name
                .cmp(&b.function_name)
                .then_with(|| a.var_name.cmp(&b.var_name))
                .then_with(|| a.offset.cmp(&b.offset))
        });
        self.eval_var_map_records.dedup();

        if self.eval_var_map_records.is_empty() {
            self.eval_var_map_ptr = 0;
            self.eval_var_map_count = 0;
            return;
        }

        let records = self.eval_var_map_records.clone();
        let mut table = Vec::with_capacity(records.len() * EVAL_VAR_MAP_RECORD_SIZE as usize);
        for record in records {
            let function_ptr = self.intern_data_string(&record.function_name);
            let var_ptr = self.intern_data_string(&record.var_name);
            table.extend_from_slice(&function_ptr.to_le_bytes());
            table.extend_from_slice(&(record.function_name.len() as u32).to_le_bytes());
            table.extend_from_slice(&var_ptr.to_le_bytes());
            table.extend_from_slice(&(record.var_name.len() as u32).to_le_bytes());
            table.extend_from_slice(&record.offset.to_le_bytes());
        }

        let table_ptr = (self.data_offset + 3) & !3;
        if self.string_data.len() < table_ptr as usize {
            self.string_data.resize(table_ptr as usize, 0);
        }
        self.string_data.extend_from_slice(&table);
        self.data_offset = table_ptr + table.len() as u32;
        self.eval_var_map_ptr = self.data_base + table_ptr;
        self.eval_var_map_count = (table.len() as u32) / EVAL_VAR_MAP_RECORD_SIZE;
    }

    /// Emit WASM instructions that test whether a NaN-boxed i64 value is null or undefined.
    pub(crate) fn emit_is_nullish_i32(&mut self, val_id: u32) {
        let val_local = self.local_idx(val_id);
        let box_base = value::BOX_BASE as i64;

        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::I64Const(box_base));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(box_base));
        self.emit(WasmInstruction::I64Eq);
        self.emit(WasmInstruction::If(BlockType::Result(ValType::I32)));

        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::I64Const(32));
        self.emit(WasmInstruction::I64ShrU);
        self.emit(WasmInstruction::I64Const(value::TAG_MASK as i64));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(value::TAG_UNDEFINED as i64));
        self.emit(WasmInstruction::I64Eq);

        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::I64Const(32));
        self.emit(WasmInstruction::I64ShrU);
        self.emit(WasmInstruction::I64Const(value::TAG_MASK as i64));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(value::TAG_NULL as i64));
        self.emit(WasmInstruction::I64Eq);
        self.emit(WasmInstruction::I32Or);

        self.emit(WasmInstruction::Else);
        self.emit(WasmInstruction::I32Const(0));
        self.emit(WasmInstruction::End);
    }

    // ── Truthiness helpers ───────────────────────────────────────────────────

    /// Emit WASM instructions that convert a NaN-boxed i64 value to an i32 boolean
    /// (1 = truthy, 0 = falsy).
    ///
    /// This is the unified truthiness check for all control flow conditions.
    pub(crate) fn emit_to_bool_i32(&mut self, val_id: u32) {
        let val_local = self.local_idx(val_id);
        let to_bool_idx = self.special_host_import_indices
            [&crate::host_import_registry::SpecialHostImport::ToBool];
        // Strategy:
        // 1. Check if it's undefined (TAG_UNDEFINED) → falsy
        // 2. Check if it's null (TAG_NULL) → falsy
        // 3. Check if it's bool (TAG_BOOL) → decode payload bit
        // 4. Check if it's string → compile期首字节或宿主 to_bool（运行时串）
        // 5. Check if it's bigint → 宿主 to_bool（0n falsy）
        // 6. Check if it's f64 (no tag) → check 0.0 and NaN
        // 7. Otherwise (object, etc.) → truthy
        //
        // Implementation using a series of nested if/else:

        let box_base = value::BOX_BASE as i64;

        // Check if the value is NaN-boxed (has BOX_BASE pattern)
        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::I64Const(box_base));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(box_base));
        self.emit(WasmInstruction::I64Eq);

        // If NaN-boxed, check the tag
        self.emit(WasmInstruction::If(BlockType::Result(ValType::I32)));
        // NaN-boxed path: check tag
        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::I64Const(32));
        self.emit(WasmInstruction::I64ShrU);
        self.emit(WasmInstruction::I64Const(0x1F));
        self.emit(WasmInstruction::I64And);

        // Check TAG_UNDEFINED (0x2)
        self.emit(WasmInstruction::I64Const(value::TAG_UNDEFINED as i64));
        self.emit(WasmInstruction::I64Eq);
        self.emit(WasmInstruction::If(BlockType::Result(ValType::I32)));
        self.emit(WasmInstruction::I32Const(0)); // undefined is falsy
        self.emit(WasmInstruction::Else);

        // Check TAG_NULL (0x3)
        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::I64Const(32));
        self.emit(WasmInstruction::I64ShrU);
        self.emit(WasmInstruction::I64Const(0x1F));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(value::TAG_NULL as i64));
        self.emit(WasmInstruction::I64Eq);
        self.emit(WasmInstruction::If(BlockType::Result(ValType::I32)));
        self.emit(WasmInstruction::I32Const(0)); // null is falsy
        self.emit(WasmInstruction::Else);

        // Check TAG_BOOL (0x4)
        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::I64Const(32));
        self.emit(WasmInstruction::I64ShrU);
        self.emit(WasmInstruction::I64Const(0x1F));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(value::TAG_BOOL as i64));
        self.emit(WasmInstruction::I64Eq);
        self.emit(WasmInstruction::If(BlockType::Result(ValType::I32)));
        // Bool: extract payload bit (val & 1)
        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::I64Const(1));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I32WrapI64);
        self.emit(WasmInstruction::Else);
        // Check TAG_STRING (0x1): load first byte from memory to detect empty string
        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::I64Const(32));
        self.emit(WasmInstruction::I64ShrU);
        self.emit(WasmInstruction::I64Const(0x1F));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(value::TAG_STRING as i64));
        self.emit(WasmInstruction::I64Eq);
        self.emit(WasmInstruction::If(BlockType::Result(ValType::I32)));
        // 运行时字符串句柄：宿主 to_bool 检查是否为空
        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::I64Const(
            (value::STRING_RUNTIME_HANDLE_FLAG << 32) as i64,
        ));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(
            (value::STRING_RUNTIME_HANDLE_FLAG << 32) as i64,
        ));
        self.emit(WasmInstruction::I64Eq);
        self.emit(WasmInstruction::If(BlockType::Result(ValType::I32)));
        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::Call(to_bool_idx));
        self.emit(WasmInstruction::Else);
        // 编译期字符串：提取低 32 位作为内存指针，读取首字节
        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::I32WrapI64);
        self.emit(WasmInstruction::I32Load8U(MemArg {
            offset: 0,
            align: 0,
            memory_index: 0,
        }));
        // 如果首字节 == 0（nul-terminated 空串）则 falsy，否则 truthy
        self.emit(WasmInstruction::I32Eqz);
        self.emit(WasmInstruction::If(BlockType::Result(ValType::I32)));
        self.emit(WasmInstruction::I32Const(0)); // 空串 falsy
        self.emit(WasmInstruction::Else);
        self.emit(WasmInstruction::I32Const(1)); // 非空串 truthy
        self.emit(WasmInstruction::End); // end empty string check
        self.emit(WasmInstruction::End); // end runtime string check
        self.emit(WasmInstruction::Else);
        // Check TAG_BIGINT (0xD)：0n falsy，非零 truthy
        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::I64Const(32));
        self.emit(WasmInstruction::I64ShrU);
        self.emit(WasmInstruction::I64Const(0x1F));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(value::TAG_BIGINT as i64));
        self.emit(WasmInstruction::I64Eq);
        self.emit(WasmInstruction::If(BlockType::Result(ValType::I32)));
        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::Call(to_bool_idx));
        self.emit(WasmInstruction::Else);
        // Other NaN-boxed types (object, symbol, etc.) → truthy
        self.emit(WasmInstruction::I32Const(1));
        self.emit(WasmInstruction::End); // end TAG_BIGINT check
        self.emit(WasmInstruction::End); // end TAG_STRING check

        self.emit(WasmInstruction::End); // end TAG_BOOL check

        self.emit(WasmInstruction::End); // end TAG_NULL check

        self.emit(WasmInstruction::End); // end TAG_UNDEFINED check

        self.emit(WasmInstruction::Else);
        // Not NaN-boxed → it's a raw f64
        // Check for +0, -0, and NaN
        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::F64ReinterpretI64);
        self.emit(WasmInstruction::F64Const(0.0.into()));
        self.emit(WasmInstruction::F64Eq);
        // If equal to 0.0, it's falsy (+0 or -0)
        // Also need to check NaN (NaN != NaN, so NaN is falsy too)
        self.emit(WasmInstruction::If(BlockType::Result(ValType::I32)));
        self.emit(WasmInstruction::I32Const(0)); // 0 is falsy
        self.emit(WasmInstruction::Else);
        // Check for NaN: x != x
        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::F64ReinterpretI64);
        self.emit(WasmInstruction::LocalGet(val_local));
        self.emit(WasmInstruction::F64ReinterpretI64);
        self.emit(WasmInstruction::F64Ne);
        // f64.ne returns 1 if NaN (since NaN != NaN)
        self.emit(WasmInstruction::If(BlockType::Result(ValType::I32)));
        self.emit(WasmInstruction::I32Const(0)); // NaN is falsy
        self.emit(WasmInstruction::Else);
        self.emit(WasmInstruction::I32Const(1)); // non-zero number is truthy
        self.emit(WasmInstruction::End); // end NaN check
        self.emit(WasmInstruction::End); // end == 0 check

        self.emit(WasmInstruction::End); // end NaN-boxed check
    }

    // ── Local management ────────────────────────────────────────────────────

    pub(crate) fn required_local_count(&self, function: &IrFunction) -> u32 {
        let max_ssa = function
            .blocks()
            .iter()
            .flat_map(|block| block.instructions())
            .map(max_instruction_value_id)
            .max()
            .map_or(0, |max| max + 1);

        (max_ssa + self.ssa_local_base)
            .max(self.next_var_local)
            .max(self.phi_locals.values().copied().max().map_or(0, |m| m + 1))
    }

    pub(crate) fn emit_shadow_stack_overflow_check(&mut self, arg_count_bytes: i32) {
        self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
        self.emit(WasmInstruction::I32Const(arg_count_bytes));
        self.emit(WasmInstruction::I32Add);
        self.emit(WasmInstruction::GlobalGet(self.shadow_stack_end_global_idx));
        self.emit(WasmInstruction::I32GtU);
        self.emit(WasmInstruction::If(BlockType::Empty));
        let func_idx = self
            .builtin_func_indices
            .get(&Builtin::AbortShadowStackOverflow)
            .copied()
            .expect("AbortShadowStackOverflow import must be registered");
        self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
        self.emit(WasmInstruction::I32Const(arg_count_bytes));
        self.emit(WasmInstruction::GlobalGet(self.shadow_stack_end_global_idx));
        self.emit(WasmInstruction::Call(func_idx));
        self.emit(WasmInstruction::Unreachable);
        self.emit(WasmInstruction::End);
    }

    pub(crate) fn emit(&mut self, instruction: WasmInstruction<'_>) {
        if self.debug {
            self.debug_emit_counter = self.debug_emit_counter.saturating_add(1);
        }
        self.current_func
            .as_mut()
            .expect("compiler function should be initialized before emission")
            .instruction(&instruction);
    }

    /// 记录当前函数 var_locals 到 debug_local_entries（finish 前调用）。
    pub(crate) fn collect_debug_locals(&mut self) {
        if !self.debug {
            return;
        }
        let func_idx = self.current_wasm_func_idx;
        for (name, &local_idx) in &self.var_locals {
            self.debug_local_entries
                .push((func_idx, local_idx, name.clone()));
        }
    }

    /// 进入函数编译时重置 debug 状态并绑定 wasm 函数索引。
    pub(crate) fn begin_function_debug(&mut self, function_name: &str) {
        if !self.debug {
            return;
        }
        self.current_wasm_func_idx = self
            .function_name_to_wasm_idx
            .get(function_name)
            .copied()
            .unwrap_or(0);
        self.debug_emit_counter = 0;
    }

    pub(crate) fn finish(mut self) -> Vec<u8> {
        // WASM section order: type, import, function, table, memory, global, export, element, code, data.
        self.module.section(&self.types);
        self.module.section(&self.imports);
        self.module.section(&self.functions);
        self.module.section(&self.table);
        if self.mode == CompileMode::Normal {
            self.module.section(&self.memory);
            self.module.section(&self.globals);
        }
        self.module.section(&self.exports);
        self.module.section(&self.elements);
        self.module.section(&self.codes);

        if !self.string_data.is_empty() {
            self.module.section(&self.data);
        }

        // 发射 WASM name 自定义段（函数名），供 dump-wat/disasm 生成可读输出。
        // 仅 Normal 模式：Eval 模式是运行时 eval()，无 CLI 调试路径会编译 Eval wasm。
        // function_names[i] 对应 FunctionId(i)，function_id_to_wasm_idx 映射到真实 wasm 索引。
        // 合成的 bootstrap/helper 函数不在 function_names 中，保持未命名。
        if self.mode == CompileMode::Normal && !self.function_names.is_empty() {
            let mut func_names = NameMap::new();
            for (i, name) in self.function_names.iter().enumerate() {
                if let Some(&idx) = self.function_id_to_wasm_idx.get(&(i as u32)) {
                    func_names.append(idx, name);
                }
            }
            let mut names = NameSection::new();
            names.functions(&func_names);
            self.module.section(&names);
        }

        // 发射 "wjsm_sourcemap" 自定义段（函数源码位置映射），供运行时错误堆栈映射。
        // 格式：source_file_len(u32 LE) + source_file_bytes + num_entries(u32 LE)
        //       + [func_idx(u32 LE), line(u32 LE), col(u32 LE)] * num_entries
        if self.mode == CompileMode::Normal
            && (self.source_file.is_some() || !self.source_map_entries.is_empty())
        {
            let mut data = Vec::new();
            if let Some(file) = &self.source_file {
                let bytes = file.as_bytes();
                data.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
                data.extend_from_slice(bytes);
            } else {
                data.extend_from_slice(&0u32.to_le_bytes());
            }
            data.extend_from_slice(&(self.source_map_entries.len() as u32).to_le_bytes());
            for &(func_idx, line, col) in &self.source_map_entries {
                data.extend_from_slice(&func_idx.to_le_bytes());
                data.extend_from_slice(&line.to_le_bytes());
                data.extend_from_slice(&col.to_le_bytes());
            }
            self.module.section(&wasm_encoder::CustomSection {
                name: "wjsm_sourcemap".into(),
                data: data.into(),
            });
        }

        // 发射 "wjsm_debug" 自定义段（Inspector 行映射 / 局部变量名 / debugger PC）。
        // 格式（version=1）：
        //   version u32 LE
        //   source_file_len u32 + bytes
        //   num_line_entries u32 + [func, wasm_pc, line, col] * N
        //   num_local_entries u32 + [func, local_idx, name_len, name_utf8] * M
        //   num_debugger_pcs u32 + [func, wasm_pc] * K
        if self.debug && self.mode == CompileMode::Normal {
            let mut data = Vec::new();
            data.extend_from_slice(&1u32.to_le_bytes());
            if let Some(file) = &self.source_file {
                let bytes = file.as_bytes();
                data.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
                data.extend_from_slice(bytes);
            } else {
                data.extend_from_slice(&0u32.to_le_bytes());
            }
            data.extend_from_slice(&(self.debug_line_entries.len() as u32).to_le_bytes());
            for &(func_idx, wasm_pc, line, col) in &self.debug_line_entries {
                data.extend_from_slice(&func_idx.to_le_bytes());
                data.extend_from_slice(&wasm_pc.to_le_bytes());
                data.extend_from_slice(&line.to_le_bytes());
                data.extend_from_slice(&col.to_le_bytes());
            }
            data.extend_from_slice(&(self.debug_local_entries.len() as u32).to_le_bytes());
            for (func_idx, local_idx, name) in &self.debug_local_entries {
                data.extend_from_slice(&func_idx.to_le_bytes());
                data.extend_from_slice(&local_idx.to_le_bytes());
                let bytes = name.as_bytes();
                data.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
                data.extend_from_slice(bytes);
            }
            data.extend_from_slice(&(self.debug_debugger_pcs.len() as u32).to_le_bytes());
            for &(func_idx, wasm_pc) in &self.debug_debugger_pcs {
                data.extend_from_slice(&func_idx.to_le_bytes());
                data.extend_from_slice(&wasm_pc.to_le_bytes());
            }
            self.module.section(&wasm_encoder::CustomSection {
                name: "wjsm_debug".into(),
                data: data.into(),
            });
        }

        self.module.finish()
    }
}
