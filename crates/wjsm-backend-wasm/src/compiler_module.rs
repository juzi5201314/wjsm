use super::*;
use crate::host_import_registry::{host_import_specs, SpecialHostImport};

impl Compiler {
    /// Convert an IR ValueId to a WASM local index, accounting for ssa_local_base.
    pub(crate) fn local_idx(&self, val_id: u32) -> u32 {
        val_id + self.ssa_local_base
    }

    /// call_func_idx scratch local (i32) — 存放解析后的函数表索引
    pub(crate) fn call_func_idx_scratch(&self) -> u32 {
        self.shadow_sp_scratch_idx + 1
    }

    /// call_env_obj scratch local (i64) — 存放解析后的闭包环境对象
    pub(crate) fn call_env_obj_scratch(&self) -> u32 {
        self.string_concat_scratch_idx + 1
    }

    pub(crate) fn emit_resolve_callable_for_helper(
        &self,
        func: &mut Function,
        callee_local: u32,
        func_idx_local: u32,
        env_obj_local: u32,
    ) {
        func.instruction(&WasmInstruction::LocalGet(callee_local));
        func.instruction(&WasmInstruction::I64Const(32));
        func.instruction(&WasmInstruction::I64ShrU);
        func.instruction(&WasmInstruction::I64Const(value::TAG_MASK as i64));
        func.instruction(&WasmInstruction::I64And);
        func.instruction(&WasmInstruction::I64Const(value::TAG_CLOSURE as i64));
        func.instruction(&WasmInstruction::I64Eq);
        func.instruction(&WasmInstruction::If(BlockType::Empty));

        func.instruction(&WasmInstruction::LocalGet(callee_local));
        func.instruction(&WasmInstruction::I32WrapI64);
        func.instruction(&WasmInstruction::Call(self.special_host_import_indices[&SpecialHostImport::ClosureGetFunc]));
        func.instruction(&WasmInstruction::LocalSet(func_idx_local));
        func.instruction(&WasmInstruction::LocalGet(callee_local));
        func.instruction(&WasmInstruction::I32WrapI64);
        func.instruction(&WasmInstruction::Call(self.special_host_import_indices[&SpecialHostImport::ClosureGetEnv]));
        func.instruction(&WasmInstruction::LocalSet(env_obj_local));

        func.instruction(&WasmInstruction::Else);
        func.instruction(&WasmInstruction::LocalGet(callee_local));
        func.instruction(&WasmInstruction::I32WrapI64);
        func.instruction(&WasmInstruction::LocalSet(func_idx_local));
        func.instruction(&WasmInstruction::I64Const(value::encode_undefined()));
        func.instruction(&WasmInstruction::LocalSet(env_obj_local));
        func.instruction(&WasmInstruction::End);
    }

    pub(crate) fn compile_module(&mut self, module: &IrModule) -> Result<()> {
        // Pass 1: Register all IR functions as WASM functions.
        let mut main_wasm_idx: Option<u32> = None;
        for (i, function) in module.functions().iter().enumerate() {
            let wasm_idx = self._next_import_func;
            self.function_name_to_wasm_idx
                .insert(function.name().to_string(), wasm_idx);

            let declared_param_count = function
                .params()
                .iter()
                .filter(|p| {
                    let s = p.as_str();
                    s != "$env" && s != "$this" && !s.ends_with(".$env") && !s.ends_with(".$this")
                })
                .count() as u32;
            self.function_param_counts.push(declared_param_count);
            self.function_names.push(function.name().to_string());

            if function.name() == "main" {
                if self.mode == CompileMode::Eval {
                    // eval entry: Type 3 = (scope_env: i64) -> i64 completion value
                    self.functions.function(3);
                } else {
                    // main: Type 4 = () -> i64 (返回异常值或 undefined)
                    self.functions.function(4);
                }
                main_wasm_idx = Some(wasm_idx);
            } else {
                // JS functions: Type 12 = (i64, i64, i32, i32) -> i64 (含 env_obj)
                self.functions.function(12);
            }

            self.push_func_table(wasm_idx);
            self.function_id_to_wasm_idx.insert(i as u32, wasm_idx);
            self._next_import_func += 1;
        }

        // Add main export (must be known now).
        let main_idx = main_wasm_idx.context("backend-wasm expects lowered `main` function")?;
        if self.mode == CompileMode::Eval {
            self.exports
                .export("__eval_entry", ExportKind::Func, main_idx);
        } else {
            self.exports.export("main", ExportKind::Func, main_idx);
        }

        // Reserve indices for object helper functions (so they're known during user function compilation).
        self.obj_new_func_idx = self._next_import_func;
        self.functions.function(7);
        self.push_func_table(self._next_import_func);
        self._next_import_func += 1;

        self.obj_get_func_idx = self._next_import_func;
        self.functions.function(8);
        self.push_func_table(self._next_import_func);
        self._next_import_func += 1;

        self.obj_set_func_idx = self._next_import_func;
        self.functions.function(9);
        self.push_func_table(self._next_import_func);
        self._next_import_func += 1;

        self.obj_delete_func_idx = self._next_import_func;
        self.functions.function(8); // Type 8: (i64, i32) -> (i64)
        self.push_func_table(self._next_import_func);
        self._next_import_func += 1;

        self.to_int32_func_idx = self._next_import_func;
        self.functions.function(10); // Type 10: (i64) -> (i32)
        self.push_func_table(self._next_import_func);
        self._next_import_func += 1;

        self.string_eq_func_idx = self._next_import_func;
        self.functions.function(26); // Type 26: (i32, i32) -> i32
        self.push_func_table(self._next_import_func);
        self._next_import_func += 1;

        self.arr_new_func_idx = self._next_import_func;
        self.functions.function(7); // Type 7: (i32) -> i32
        self.push_func_table(self._next_import_func);
        self._next_import_func += 1;

        self.elem_get_func_idx = self._next_import_func;
        self.functions.function(8); // Type 8: (i64, i32) -> i64
        self.push_func_table(self._next_import_func);
        self._next_import_func += 1;

        self.elem_set_func_idx = self._next_import_func;
        self.functions.function(9); // Type 9: (i64, i32, i64) -> ()
        self.push_func_table(self._next_import_func);
        self._next_import_func += 1;

        self.get_proto_from_ctor_func_idx = self._next_import_func;
        self.functions.function(3); // Type 3: (i64) -> (i64)
        self.push_func_table(self._next_import_func);
        self._next_import_func += 1;
        // Register array prototype method imports in function table
        let arr_proto_base = self.function_table.len() as u32;
        for (idx, spec) in host_import_specs().iter().enumerate() {
            if spec.group == Some(crate::host_import_registry::HostImportGroup::ArrayPrototypeMethod) {
                self.push_func_table(idx as u32);
            }
        }
        self.arr_proto_table_base = arr_proto_base;

        // Pre-write typeof type strings to data segment start (nul-terminated)
        // 必须在编译用户函数之前设置，否则 encode_constant 会从 offset 0 开始分配字符串，
        // 随后 typeof 字符串会覆盖用户字符串数据。
        let typeof_strings: &[(u32, &str)] = &[
            (constants::TYPEOF_UNDEFINED_OFFSET, "undefined"),
            (constants::TYPEOF_OBJECT_OFFSET, "object"),
            (constants::TYPEOF_BOOLEAN_OFFSET, "boolean"),
            (constants::TYPEOF_STRING_OFFSET, "string"),
            (constants::TYPEOF_FUNCTION_OFFSET, "function"),
            (constants::TYPEOF_NUMBER_OFFSET, "number"),
            (constants::TYPEOF_SYMBOL_OFFSET, "symbol"),
            (constants::TYPEOF_BIGINT_OFFSET, "bigint"),
        ];
        for &(offset, s) in typeof_strings {
            let end = offset as usize + s.len() + 1;
            if self.string_data.len() < end {
                self.string_data.resize(end, 0);
            }
            self.string_data[offset as usize..offset as usize + s.len()]
                .copy_from_slice(s.as_bytes());
            self.string_data[offset as usize + s.len()] = 0;
            self.string_ptr_cache
                .insert(s.to_string(), self.data_base + offset);
        }

        // Pre-write property descriptor strings after typeof strings
        // 用于 Object.getOwnPropertyDescriptor 返回的描述符对象
        let prop_desc_strings: &[(u32, &str)] = &[
            (constants::PROP_DESC_VALUE_OFFSET, "value"),
            (constants::PROP_DESC_WRITABLE_OFFSET, "writable"),
            (constants::PROP_DESC_ENUMERABLE_OFFSET, "enumerable"),
            (constants::PROP_DESC_CONFIGURABLE_OFFSET, "configurable"),
            (constants::PROP_DESC_GET_OFFSET, "get"),
            (constants::PROP_DESC_SET_OFFSET, "set"),
        ];
        for &(offset, s) in prop_desc_strings {
            let end = offset as usize + s.len() + 1;
            if self.string_data.len() < end {
                self.string_data.resize(end, 0);
            }
            self.string_data[offset as usize..offset as usize + s.len()]
                .copy_from_slice(s.as_bytes());
            self.string_data[offset as usize + s.len()] = 0;
            self.string_ptr_cache
                .insert(s.to_string(), self.data_base + offset);
        }

        let promise_strings: &[(u32, &str)] = &[
            (constants::PROMISE_STATE_PENDING_OFFSET, "pending"),
            (constants::PROMISE_STATE_FULFILLED_OFFSET, "fulfilled"),
            (constants::PROMISE_STATE_REJECTED_OFFSET, "rejected"),
            (constants::PROMISE_THEN_OFFSET, "then"),
            (constants::PROMISE_CATCH_OFFSET, "catch"),
            (constants::PROMISE_FINALLY_OFFSET, "finally"),
            (constants::PROMISE_RESOLVE_OFFSET, "resolve"),
            (constants::PROMISE_REJECT_OFFSET, "reject"),
            (constants::PROMISE_ALL_OFFSET, "all"),
            (constants::PROMISE_RACE_OFFSET, "race"),
            (constants::PROMISE_ALLSETTLED_OFFSET, "allSettled"),
            (constants::PROMISE_ANY_OFFSET, "any"),
            (constants::PROMISE_CONSTRUCTOR_OFFSET, "constructor"),
            (constants::ASYNC_ITERATOR_OFFSET, "asyncIterator"),
        ];
        for &(offset, s) in promise_strings {
            let end = offset as usize + s.len() + 1;
            if self.string_data.len() < end {
                self.string_data.resize(end, 0);
            }
            self.string_data[offset as usize..offset as usize + s.len()]
                .copy_from_slice(s.as_bytes());
            self.string_data[offset as usize + s.len()] = 0;
            self.string_ptr_cache
                .insert(s.to_string(), self.data_base + offset);
        }

        self.data_offset = constants::USER_STRING_START;
        // 填充 string_data 到 data_offset，确保后续用户字符串追加到正确偏移量
        self.string_data.resize(self.data_offset as usize, 0);

        // Assign global indices before compile_object_helpers needs them.
        self.func_props_global_idx = 0;
        self.heap_ptr_global_idx = 1;
        self.obj_table_global_idx = 2;
        self.obj_table_count_global_idx = 3;
        self.num_ir_functions = module.functions().len() as u32;
        self.shadow_sp_global_idx = 4;
        self.alloc_counter_global_idx = 5;
        self.array_proto_handle_global_idx = 9;
        self.object_proto_handle_global_idx = 10;
        self.eval_var_map_ptr_global_idx = 11;
        self.eval_var_map_count_global_idx = 12;

        // Record user function base index (after all imports + helpers)
        self.user_func_base_idx = self._next_import_func;
        for function in module.functions() {
            if function.name() == "main" {
                self.compile_function(module, function)?;
            } else {
                self.compile_js_function(module, function)?;
            }
        }

        // Pass 3: Compile object helper functions.
        self.compile_object_helpers();
        // 编译数组辅助函数
        self.compile_array_helpers();
        self.table.table(TableType {
            element_type: RefType::FUNCREF,
            minimum: self.function_table.len() as u64,
            maximum: None,
            table64: false,
            shared: false,
        });
        self.exports.export("__table", ExportKind::Table, 0);

        self.elements.active(
            Some(0),
            &ConstExpr::i32_const(0),
            Elements::Functions(std::borrow::Cow::Borrowed(&self.function_table)),
        );

        self.finalize_eval_var_map_data();

        // Allocate handle table at start of heap.
        // Handle table replaces func_props: maps handle_index → object ptr (i32).
        // Function property objects are stored at indices 0..num_functions-1.
        // Runtime objects are stored at indices num_functions..capacity.
        let heap_start = (self.data_offset + 7) & !7; // align to 8 bytes
        let num_functions = self.num_ir_functions;
        let handle_table_entries = std::cmp::max(256, num_functions * 2);
        let handle_table_size = handle_table_entries * 4;

        let shadow_stack_base = heap_start + handle_table_size;
        let object_heap_start = shadow_stack_base + SHADOW_STACK_SIZE;
        let shadow_stack_end = shadow_stack_base + SHADOW_STACK_SIZE;
        if self.mode == CompileMode::Normal {
            // Global 0: func_props_ptr (deprecated, set to 0)
            self.globals.global(
                GlobalType {
                    val_type: ValType::I32,
                    mutable: false,
                    shared: false,
                },
                &ConstExpr::i32_const(0),
            );
            self.exports.export("__func_props", ExportKind::Global, 0);
            // Global 1: heap_ptr (starts after handle table + shadow stack, mutable)
            self.globals.global(
                GlobalType {
                    val_type: ValType::I32,
                    mutable: true,
                    shared: false,
                },
                &ConstExpr::i32_const(object_heap_start as i32),
            );
            self.heap_ptr_global_idx = 1;
            // Global 2: obj_table_ptr (immutable, points to handle table base)
            self.globals.global(
                GlobalType {
                    val_type: ValType::I32,
                    mutable: false,
                    shared: false,
                },
                &ConstExpr::i32_const(heap_start as i32),
            );
            self.obj_table_global_idx = 2;
            // Global 3: obj_table_count (mutable, starts at 0, incremented by $obj_new)
            self.globals.global(
                GlobalType {
                    val_type: ValType::I32,
                    mutable: true,
                    shared: false,
                },
                &ConstExpr::i32_const(0),
            );
            self.obj_table_count_global_idx = 3;
            // Global 4: shadow_sp (mutable, starts at shadow_stack_base)
            self.globals.global(
                GlobalType {
                    val_type: ValType::I32,
                    mutable: true,
                    shared: false,
                },
                &ConstExpr::i32_const(shadow_stack_base as i32),
            );
            self.shadow_sp_global_idx = 4;
            // Global 5: alloc_counter (mutable i32, initial 0, for GC heuristic)
            self.globals.global(
                GlobalType {
                    val_type: ValType::I32,
                    mutable: true,
                    shared: false,
                },
                &ConstExpr::i32_const(0),
            );
            self.alloc_counter_global_idx = 5;
            // Export alloc_counter for runtime debugging
            self.exports
                .export("__alloc_counter", ExportKind::Global, 5);
            // Export globals for runtime access
            self.exports
                .export("__obj_table_ptr", ExportKind::Global, 2);
            self.exports.export("__heap_ptr", ExportKind::Global, 1);
            self.exports
                .export("__obj_table_count", ExportKind::Global, 3);
            self.exports.export("__shadow_sp", ExportKind::Global, 4);
            // Global 6: __object_heap_start (immutable, for runtime GC heap base)
            self.globals.global(
                GlobalType {
                    val_type: ValType::I32,
                    mutable: false,
                    shared: false,
                },
                &ConstExpr::i32_const(object_heap_start as i32),
            );
            // Global 7: __num_ir_functions (immutable, for runtime GC root set)
            self.globals.global(
                GlobalType {
                    val_type: ValType::I32,
                    mutable: false,
                    shared: false,
                },
                &ConstExpr::i32_const(num_functions as i32),
            );
            self.object_heap_start_global_idx = 6;
            self.num_ir_functions_global_idx = 7;
            self.exports
                .export("__object_heap_start", ExportKind::Global, 6);
            self.exports
                .export("__num_ir_functions", ExportKind::Global, 7);
            // Global 8: __shadow_stack_end (immutable, for shadow stack bounds check)
            self.globals.global(
                GlobalType {
                    val_type: ValType::I32,
                    mutable: false,
                    shared: false,
                },
                &ConstExpr::i32_const(shadow_stack_end as i32),
            );
            self.exports
                .export("__shadow_stack_end", ExportKind::Global, 8);
            // Global 9: array_proto_handle (mutable, starts at -1 for uninitialized)
            self.globals.global(
                GlobalType {
                    val_type: ValType::I32,
                    mutable: true,
                    shared: false,
                },
                &ConstExpr::i32_const(-1),
            );
            self.exports
                .export("__array_proto_handle", ExportKind::Global, 9);
            // Global 10: object_proto_handle (mutable, starts at -1 for uninitialized)
            self.globals.global(
                GlobalType {
                    val_type: ValType::I32,
                    mutable: true,
                    shared: false,
                },
                &ConstExpr::i32_const(-1),
            );
            self.exports
                .export("__object_proto_handle", ExportKind::Global, 10);
            // Global 11/12: eval variable map metadata.
            self.globals.global(
                GlobalType {
                    val_type: ValType::I32,
                    mutable: false,
                    shared: false,
                },
                &ConstExpr::i32_const(self.eval_var_map_ptr as i32),
            );
            self.exports.export(
                "__eval_var_map_ptr",
                ExportKind::Global,
                self.eval_var_map_ptr_global_idx,
            );
            self.globals.global(
                GlobalType {
                    val_type: ValType::I32,
                    mutable: false,
                    shared: false,
                },
                &ConstExpr::i32_const(self.eval_var_map_count as i32),
            );
            self.exports.export(
                "__eval_var_map_count",
                ExportKind::Global,
                self.eval_var_map_count_global_idx,
            );
        } else {
            self.exports.export("__func_props", ExportKind::Global, 0);
            self.exports.export("__heap_ptr", ExportKind::Global, 1);
            self.exports
                .export("__obj_table_ptr", ExportKind::Global, 2);
            self.exports
                .export("__obj_table_count", ExportKind::Global, 3);
            self.exports.export("__shadow_sp", ExportKind::Global, 4);
            self.exports
                .export("__alloc_counter", ExportKind::Global, 5);
            self.exports
                .export("__object_heap_start", ExportKind::Global, 6);
            self.exports
                .export("__num_ir_functions", ExportKind::Global, 7);
            self.exports
                .export("__shadow_stack_end", ExportKind::Global, 8);
            self.exports
                .export("__array_proto_handle", ExportKind::Global, 9);
            self.exports
                .export("__object_proto_handle", ExportKind::Global, 10);
            self.exports.export(
                "__eval_var_map_ptr",
                ExportKind::Global,
                self.eval_var_map_ptr_global_idx,
            );
            self.exports.export(
                "__eval_var_map_count",
                ExportKind::Global,
                self.eval_var_map_count_global_idx,
            );
        }
        if !self.string_data.is_empty() {
            self.data.active(
                0,
                &ConstExpr::i32_const(self.data_base as i32),
                self.string_data.clone(),
            );
        }
        Ok(())
    }

    pub(crate) fn compile_function(
        &mut self,
        module: &IrModule,
        function: &IrFunction,
    ) -> Result<()> {
        self.current_func_is_main = function.name() == "main";
        self.current_func_returns_value =
            self.mode == CompileMode::Eval || self.current_func_is_main;
        self.ssa_local_base = if self.mode == CompileMode::Eval {
            function.params().len() as u32
        } else {
            0
        };
        // Pass 1: direct eval 函数的变量改由 shadow stack frame 承载。
        self.assign_eval_var_memory(function);
        // Pass 2: assign WASM local indices to non-eval variable names.
        self.assign_var_locals(function);

        // Pass 3: lower Phi to dedicated locals after variable locals to avoid index overlap.
        self.lower_phi_to_locals(function);

        let local_count = self.required_local_count(function);
        // scratch locals: i64 在前, i32 在后
        // string_concat (i64) at local_count
        // call_env_obj (i64) at local_count+1
        // shadow_sp (i32) at local_count+2
        // call_func_idx (i32) at local_count+3
        self.string_concat_scratch_idx = local_count;
        self.shadow_sp_scratch_idx = local_count + 2;
        self.eval_var_base_local_idx = self.shadow_sp_scratch_idx + 2;
        let param_i64_count = self.ssa_local_base;
        let total_i64_locals = local_count.saturating_sub(param_i64_count) + 2; // string_concat + call_env_obj
        let total_i32_locals = 2 + u32::from(!self.var_memory_offsets.is_empty());
        let locals = if total_i64_locals == 0 && total_i32_locals == 0 {
            Vec::new()
        } else {
            vec![
                (total_i64_locals, ValType::I64),
                (total_i32_locals, ValType::I32),
            ]
        };
        self.current_func = Some(Function::new(locals));
        self.emit_eval_var_frame_enter();

        // 预分配函数属性对象：为每个 IR 函数调用 $obj_new(8)，将返回的 handle_idx
        // 对应 obj_table[0..num_functions-1]，存储函数属性对象的 ptr。
        // 这样后续 GetProp/SetProp 可以通过 obj_table 统一查找。
        if function.name() == "main" {
            let length_name_id = self.intern_data_string("length");
            let name_name_id = self.intern_data_string("name");
            let box_base = value::BOX_BASE as i64;
            let tag_object = (value::TAG_OBJECT << 32) as i64;
            for i in 0..self.num_ir_functions as usize {
                self.emit(WasmInstruction::I32Const(8));
                self.emit(WasmInstruction::Call(self.obj_new_func_idx));
                self.emit(WasmInstruction::LocalTee(self.shadow_sp_scratch_idx));
                self.emit(WasmInstruction::I64ExtendI32U);
                self.emit(WasmInstruction::I64Const(box_base | tag_object));
                self.emit(WasmInstruction::I64Or);
                self.emit(WasmInstruction::I32Const(length_name_id as i32));
                let param_count = self.function_param_counts[i];
                self.emit(WasmInstruction::I64Const(value::encode_f64(
                    param_count as f64,
                )));
                self.emit(WasmInstruction::Call(self.obj_set_func_idx));
                self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
                self.emit(WasmInstruction::I64ExtendI32U);
                self.emit(WasmInstruction::I64Const(box_base | tag_object));
                self.emit(WasmInstruction::I64Or);
                self.emit(WasmInstruction::I32Const(name_name_id as i32));
                let func_name = self.function_names[i].clone();
                let name_ptr = self.intern_data_string(&func_name);
                self.emit(WasmInstruction::I64Const(value::encode_string_ptr(
                    name_ptr,
                )));
                self.emit(WasmInstruction::Call(self.obj_set_func_idx));
            }
            // ── 初始化 Array.prototype ──
            // 复用 shadow_sp_scratch_idx 作为 proto handle 的临时存储（proto_init_scratch）。
            // 创建 Array.prototype 对象（容量 64），存储 handle 到 Global 9
            self.emit(WasmInstruction::I32Const(64));
            self.emit(WasmInstruction::Call(self.obj_new_func_idx));
            self.emit(WasmInstruction::LocalTee(self.shadow_sp_scratch_idx));
            self.emit(WasmInstruction::GlobalSet(
                self.array_proto_handle_global_idx,
            ));
            // 为每个原型方法在 Array.prototype 上设置属性
            let method_names: [(u32, &str); 27] = [
                (0, "push"),
                (1, "pop"),
                (2, "includes"),
                (3, "indexOf"),
                (4, "join"),
                (5, "concat"),
                (6, "slice"),
                (7, "fill"),
                (8, "reverse"),
                (9, "flat"),
                (10, "shift"),
                (11, "unshift"),
                (12, "sort"),
                (13, "at"),
                (14, "copyWithin"),
                (15, "forEach"),
                (16, "map"),
                (17, "filter"),
                (18, "reduce"),
                (19, "reduceRight"),
                (20, "find"),
                (21, "findIndex"),
                (22, "some"),
                (23, "every"),
                (24, "flatMap"),
                (25, "splice"),
                (26, "isArray"),
            ];
            for (offset, name) in &method_names {
                let name_id = self.intern_data_string(name);
                let table_idx = self.arr_proto_table_base + offset;
                // 推入 boxed proto handle (i64)
                self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
                self.emit(WasmInstruction::I64ExtendI32U);
                let box_base = value::BOX_BASE as i64;
                let tag_object = (value::TAG_OBJECT << 32) as i64;
                self.emit(WasmInstruction::I64Const(box_base | tag_object));
                self.emit(WasmInstruction::I64Or);
                // 推入 name_id (i32)
                self.emit(WasmInstruction::I32Const(name_id as i32));
                // 推入编码后的函数表索引 (i64)
                self.emit(WasmInstruction::I64Const(value::encode_function_idx(
                    table_idx,
                )));
                // 调用 $obj_set(proto, name_id, func_value)
                self.emit(WasmInstruction::Call(self.obj_set_func_idx));
            }

            // ── 初始化 Object.prototype ──
            // 创建空对象（容量 64），存储 handle 到 Global 10
            self.emit(WasmInstruction::I32Const(64));
            self.emit(WasmInstruction::Call(self.obj_new_func_idx));
            self.emit(WasmInstruction::GlobalSet(
                self.object_proto_handle_global_idx,
            ));
        }

        let cfg = Cfg::from_function(function);
        let region_tree = RegionTree::build(function, &cfg)
            .map_err(|error| anyhow::anyhow!("failed to build region tree: {error:?}"))?;

        self.compiled_blocks.clear();
        self.loop_stack.clear();
        self.if_depth = 0;

        if cfg.successors.is_empty() {
            // Empty function body — emit end directly.
            self.emit(WasmInstruction::End);
        } else {
            self.compile_region_tree(module, function, &region_tree)?;
            self.emit(WasmInstruction::End);
        }

        self.codes.function(
            self.current_func
                .as_ref()
                .context("current function missing after compile")?,
        );

        // Clean up per-function state.
        self.var_locals.clear();
        self.var_memory_offsets.clear();
        self.phi_locals.clear();
        self.current_function_has_eval = false;

        Ok(())
    }

    pub(crate) fn compile_js_function(
        &mut self,
        module: &IrModule,
        function: &IrFunction,
    ) -> Result<()> {
        self.current_func_returns_value = true;
        // Type 12 signature: (i64 env_obj, i64 this_val, i32 args_base, i32 args_count) -> i64
        // WASM params: local 0 = env_obj (i64), local 1 = this_val (i64),
        //              local 2 = args_base_ptr (i32), local 3 = args_count (i32)
        self.assign_eval_var_memory(function);

        // Map $env/$this to WASM params (both bare and scoped names)
        self.var_locals.clear();
        self.var_locals.insert("$env".to_string(), 0);
        self.var_locals.insert("$this".to_string(), 1);

        // Count declared params (excluding $env/$this in both bare and scoped forms)
        let declared_params: Vec<&String> = function
            .params()
            .iter()
            .filter(|p| {
                let s = p.as_str();
                s != "$env" && s != "$this" && !s.ends_with(".$env") && !s.ends_with(".$this")
            })
            .collect();

        // Allocate locals for declared params starting at local 4 (after env, this, args_base, args_count)
        // These will be loaded from shadow stack in the prologue
        let mut param_local_idx = 4;
        for param_name in &declared_params {
            if self.is_eval_memory_var(param_name) {
                continue;
            }
            self.var_locals
                .insert((*param_name).clone(), param_local_idx);
            param_local_idx += 1;
        }
        // Map scoped $env/$this param names to the same locals as bare names
        for p in function.params() {
            if p.ends_with(".$env") {
                self.var_locals.insert(p.clone(), 0);
            } else if p.ends_with(".$this") {
                self.var_locals.insert(p.clone(), 1);
            }
        }
        self.ssa_local_base = param_local_idx;
        // Variable locals start after param locals
        self.next_var_local = param_local_idx;
        // Assign variable locals for LoadVar/StoreVar.
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
        self.lower_phi_to_locals(function);

        // 计算实际需要的 local 数量
        // SSA 值从 ssa_local_base 开始分配，需要 ssa_local_base + max_ssa 个 locals
        // 但 var_locals 已经包含了声明的参数，其索引也是从 ssa_local_base 开始
        // 所以实际需要的 locals 数量 = max_ssa (SSA 值数量)
        // 而不是 ssa_local_base + max_ssa (因为 params 是 WASM 参数，不是声明的 locals)
        let max_ssa = function
            .blocks()
            .iter()
            .flat_map(|block| block.instructions())
            .map(max_instruction_value_id)
            .max()
            .map_or(0, |max| max + 1);

        // 总 local 数量
        // 为避免 SSA locals 和 var locals 索引重叠（SSA 值可能需要跨 StoreVar 保持活性，如解构），
        // 将 var locals 偏移到 SSA 最大值之后。
        let ssa_max = max_ssa + self.ssa_local_base;
        let var_rebase_start = self.ssa_local_base;
        // rebase: 所有 >= ssa_local_base 的 var/phi local 索引偏移到 ssa_max 之后
        let offset = ssa_max.saturating_sub(var_rebase_start);
        for idx in self.var_locals.values_mut() {
            if *idx >= var_rebase_start {
                *idx += offset;
            }
        }
        let total_var_locals = self.next_var_local + offset;
        for idx in self.phi_locals.values_mut() {
            if *idx >= var_rebase_start {
                *idx += offset;
            }
        }
        let total_locals = ssa_max
            .max(total_var_locals)
            .max(self.phi_locals.values().copied().max().map_or(0, |m| m + 1));

        // scratch locals: 所有 i64 在前，然后所有 i32（WASM locals 按 type 分组）
        // string_concat (i64) at total_locals
        // call_env_obj (i64) at total_locals+1
        // shadow_sp (i32) at total_locals+2
        // call_func_idx (i32) at total_locals+3
        self.string_concat_scratch_idx = total_locals;
        // call_env_obj scratch = string_concat + 1 (i64), computed by call_env_obj_scratch()
        self.shadow_sp_scratch_idx = total_locals + 2;
        self.eval_var_base_local_idx = self.shadow_sp_scratch_idx + 2;
        // call_func_idx = shadow_sp + 1 (i32), computed by call_func_idx_scratch()
        let total_i64_locals = total_locals.saturating_sub(4) + 2; // string_concat + call_env_obj
        let total_i32_locals = 2 + u32::from(!self.var_memory_offsets.is_empty());

        let locals = if total_i64_locals == 0 && total_i32_locals == 0 {
            Vec::new()
        } else {
            vec![
                (total_i64_locals, ValType::I64),
                (total_i32_locals, ValType::I32),
            ]
        };
        self.current_func = Some(Function::new(locals));
        self.emit_eval_var_frame_enter();

        // ── Prologue: Load declared params from shadow stack ──
        // args_base_ptr is at local 2, args_count is at local 3
        for (i, param_name) in declared_params.iter().enumerate() {
            let param_memory_offset = self.var_memory_offsets.get(*param_name).copied();
            let param_local = self.var_locals.get(*param_name).copied();

            // if i < args_count: load from shadow stack
            // else: set to undefined
            self.emit(WasmInstruction::I32Const(i as i32)); // i
            self.emit(WasmInstruction::LocalGet(3)); // args_count
            self.emit(WasmInstruction::I32LtU); // i < args_count (unsigned)

            self.emit(WasmInstruction::If(BlockType::Empty));
            // Load from shadow stack: memory[args_base_ptr + i*8]
            self.emit(WasmInstruction::LocalGet(2)); // args_base_ptr
            self.emit(WasmInstruction::I32Const((i * 8) as i32));
            self.emit(WasmInstruction::I32Add);
            self.emit(WasmInstruction::I64Load(MemArg {
                offset: 0,
                align: 3,
                memory_index: 0,
            }));
            self.emit_store_stacked_binding(param_memory_offset, param_local);
            self.emit(WasmInstruction::Else);
            // Out of bounds: set to undefined
            self.emit(WasmInstruction::I64Const(value::encode_undefined()));
            self.emit_store_stacked_binding(param_memory_offset, param_local);
            self.emit(WasmInstruction::End);
        }

        let cfg = Cfg::from_function(function);
        let region_tree = RegionTree::build(function, &cfg)
            .map_err(|error| anyhow::anyhow!("failed to build region tree: {error:?}"))?;

        self.compiled_blocks.clear();
        self.loop_stack.clear();
        self.if_depth = 0;

        if cfg.successors.is_empty() {
            // Empty function — return undefined.
            self.emit(WasmInstruction::I64Const(value::encode_undefined()));
            self.emit(WasmInstruction::Return);
            self.emit(WasmInstruction::End);
        } else {
            self.compile_region_tree(module, function, &region_tree)?;
            self.emit(WasmInstruction::End);
        }

        self.codes.function(
            self.current_func
                .as_ref()
                .context("current function missing after compile")?,
        );

        // Clean up per-function state.
        self.var_locals.clear();
        self.var_memory_offsets.clear();
        self.phi_locals.clear();
        self.current_function_has_eval = false;

        Ok(())
    }
}
