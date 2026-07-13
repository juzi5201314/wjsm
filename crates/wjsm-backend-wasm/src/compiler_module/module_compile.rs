use super::*;

impl Compiler {
    pub(crate) fn compile_module(&mut self, module: &IrModule) -> Result<()> {
        // Pass 0: 模块级 GC 分析（Layer 3c）
        self.gc_analysis = Some(GcAnalysis::analyze(module));
        // 收集源文件路径和函数源码位置映射（供运行时错误堆栈映射）。
        self.source_file = module.source_file().map(|s| s.to_string());

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
            self.function_needs_prototype
                .push(function.needs_prototype());

            if is_module_entry_ir_function(function.name()) {
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
            if let Some(span) = function.source_span() {
                self.source_map_entries
                    .push((wasm_idx, span.line, span.col));
            }
            self._next_import_func += 1;
        }

        // Add main export (must be known now).
        let main_idx =
            main_wasm_idx.context("backend-wasm expects lowered module entry function")?;
        if self.mode == CompileMode::Eval {
            self.exports
                .export("__eval_entry", ExportKind::Func, main_idx);
        } else {
            self.exports.export("main", ExportKind::Func, main_idx);
        }

        // Reserve indices for object helper functions (so they're known during user function compilation).
        if self.mode == CompileMode::Normal {
            let support_import_base = host_import_specs().len() as u32;
            self.obj_new_func_idx = support_import_base;
            self.obj_get_func_idx = support_import_base + 1;
            self.obj_set_func_idx = support_import_base + 2;
            self.obj_delete_func_idx = support_import_base + 3;
            self.arr_new_func_idx = support_import_base + 4;
            self.elem_get_func_idx = support_import_base + 5;
            self.elem_set_func_idx = support_import_base + 6;
            self.string_eq_func_idx = support_import_base + 7;
            self.to_int32_func_idx = support_import_base + 8;
            self.get_proto_from_ctor_func_idx = support_import_base + 9;
            for i in 0..10u32 {
                self.push_func_table(support_import_base + i);
            }
        } else {
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
            self.functions.function(8);
            self.push_func_table(self._next_import_func);
            self._next_import_func += 1;
            self.to_int32_func_idx = self._next_import_func;
            self.functions.function(10);
            self.push_func_table(self._next_import_func);
            self._next_import_func += 1;
            self.string_eq_func_idx = self._next_import_func;
            self.functions.function(26);
            self.push_func_table(self._next_import_func);
            self._next_import_func += 1;
            self.arr_new_func_idx = self._next_import_func;
            self.functions.function(7);
            self.push_func_table(self._next_import_func);
            self._next_import_func += 1;
            self.elem_get_func_idx = self._next_import_func;
            self.functions.function(8);
            self.push_func_table(self._next_import_func);
            self._next_import_func += 1;
            self.elem_set_func_idx = self._next_import_func;
            self.functions.function(9);
            self.push_func_table(self._next_import_func);
            self._next_import_func += 1;
            self.get_proto_from_ctor_func_idx = self._next_import_func;
            self.functions.function(3);
            self.push_func_table(self._next_import_func);
            self._next_import_func += 1;
        }
        let arr_proto_base = self.table_base + self.function_table.len() as u32;
        for (idx, _) in array_proto_method_specs() {
            self.push_func_table(idx as u32);
        }
        self.arr_proto_table_base = arr_proto_base;

        if self.mode == CompileMode::Normal {
            // P2.2: __wjsm_init_globals — 在 bootstrap 之前由 runtime 调用，
            // 设置所有 imported globals 的初始值（heap 布局等编译期计算值）。
            // 必须在 initialize_host_post_bootstrap 之前执行，因为 host 函数
            // 依赖 heap_ptr/obj_table_ptr 等全局的正确值。
            self.init_globals_func_idx = self._next_import_func;
            self.functions.function(4); // () -> i64
            self._next_import_func += 1;
            self.exports.export(
                "__wjsm_init_globals",
                ExportKind::Func,
                self.init_globals_func_idx,
            );

            // Startup snapshot 边界：把 primordial bootstrap 与当前模块函数属性初始化拆成可单独调用的阶段。
            self.bootstrap_func_idx = self._next_import_func;
            self.functions.function(4); // () -> i64
            self._next_import_func += 1;

            self.init_function_props_func_idx = self._next_import_func;
            self.functions.function(4); // () -> i64
            self._next_import_func += 1;
            self.exports.export(
                "__wjsm_bootstrap_once",
                ExportKind::Func,
                self.bootstrap_func_idx,
            );
            self.exports.export(
                "__wjsm_init_function_props",
                ExportKind::Func,
                self.init_function_props_func_idx,
            );
        }

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

        // Pre-write primordial property names used by bootstrap, function-props,
        // and host post-bootstrap (Array.prototype methods, length, name,
        // toStringTag, etc.). Fixed offsets ensure name_ids are consistent
        // across different user source compilations — required for snapshot ABI.
        for (offset, s) in constants::primordial_string_offsets() {
            let end = *offset as usize + s.len() + 1;
            if self.string_data.len() < end {
                self.string_data.resize(end, 0);
            }
            self.string_data[*offset as usize..*offset as usize + s.len()]
                .copy_from_slice(s.as_bytes());
            self.string_data[*offset as usize + s.len()] = 0;
            self.string_ptr_cache
                .insert(s.to_string(), self.data_base + *offset);
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
        self.object_heap_start_global_idx = 5;
        self.num_ir_functions_global_idx = 6;
        self.shadow_stack_end_global_idx = 7;
        self.array_proto_handle_global_idx = 8;
        self.object_proto_handle_global_idx = 9;
        self.eval_var_map_ptr_global_idx = 10;
        self.eval_var_map_count_global_idx = 11;
        self.bootstrap_done_global_idx = 12;
        self.function_props_done_global_idx = 13;
        self.function_props_base_global_idx = 14;
        self.arr_proto_table_base_global_idx = 15;
        self.arr_proto_table_len_global_idx = 16;
        self.arr_proto_table_hash_global_idx = 17;
        self.heap_limit_global_idx = 18;
        self.alloc_ptr_global_idx = 19;
        self.alloc_end_global_idx = 20;
        self.gc_alloc_bytes_global_idx = 21;
        self.gc_trigger_bytes_global_idx = 22;
        self.gc_phase_global_idx = 23;
        self.good_color_global_idx = 24;
        self.barrier_buf_ptr_global_idx = 25;
        self.barrier_buf_end_global_idx = 26;

        // Record user function base index (after all imports + helpers)
        self.user_func_base_idx = self._next_import_func;
        for (function_id, function) in module.functions().iter().enumerate() {
            if is_module_entry_ir_function(function.name()) {
                self.compile_function(module, function)?;
            } else {
                self.compile_js_function(
                    module,
                    function,
                    wjsm_ir::FunctionId(function_id as u32),
                )?;
            }
        }

        self.compile_number_proto_wrappers();

        // P2.2 后 heap 布局由 imported globals 显式初始化。计算 heap_start 之前
        // 必须先固化全部 data segment；否则后续追加的函数名字符串或 eval metadata
        // 会落进 object heap，被分配/GC 覆盖。
        self.finalize_eval_var_map_data();
        self.intern_data_string("length");
        self.intern_data_string("name");
        for function_name in self.function_names.clone() {
            self.intern_data_string(&function_name);
        }

        // P2.2: 提前计算 heap 布局，供 bootstrap 函数中的 emit_globals_init 使用。
        // 这些值原本在 globals 定义段中计算，现在 globals 是 import 的，
        // 需要在编译 bootstrap 之前确定初始值。
        let heap_start = (self.data_offset + (constants::HEAP_ALLOCATION_ALIGNMENT - 1))
            & !(constants::HEAP_ALLOCATION_ALIGNMENT - 1);
        let num_functions = self.num_ir_functions;
        let handle_table_entries = std::cmp::max(
            constants::HANDLE_TABLE_MIN_ENTRIES,
            num_functions * constants::HANDLE_TABLE_FUNCTION_ENTRY_FACTOR,
        );
        let handle_table_size = handle_table_entries * constants::HANDLE_TABLE_ENTRY_SIZE;
        // 独立 shadow memory：主内存布局为 handle table → barrier → object heap。
        let barrier_event_buf_base = heap_start + handle_table_size;
        let barrier_event_buf_end =
            barrier_event_buf_base + constants::GC_BARRIER_EVENT_BUFFER_SIZE;
        let object_heap_start = (barrier_event_buf_end + (constants::GC_REGION_SIZE - 1))
            & !(constants::GC_REGION_SIZE - 1);
        if self.mode == CompileMode::Normal {
            let needed_len = object_heap_start as usize;
            if self.string_data.len() < needed_len {
                self.string_data.resize(needed_len, 0);
            }
            self.data_offset = self.data_offset.max(object_heap_start);
            self.normal_init_values = Some(NormalGlobalsInit {
                heap_ptr: object_heap_start as i32,
                obj_table_ptr: heap_start as i32,
                // 影子栈在独立 memory：sp 从 0 增长，end 为当前已提交容量。
                shadow_sp: 0,
                object_heap_start: object_heap_start as i32,
                num_ir_functions: num_functions as i32,
                shadow_stack_end: SHADOW_STACK_INITIAL_SIZE as i32,
                eval_var_map_ptr: self.eval_var_map_ptr as i32,
                eval_var_map_count: self.eval_var_map_count as i32,
                arr_proto_table_base: self.arr_proto_table_base as i32,
                arr_proto_table_len: array_proto_table_len() as i32,
                arr_proto_table_hash: array_proto_table_hash() as i64,
                alloc_ptr: object_heap_start as i32,
                alloc_end: object_heap_start as i32,
                gc_alloc_bytes: 0,
                gc_trigger_bytes: constants::GC_INITIAL_TRIGGER_BYTES as i32,
                gc_phase: 0,
                good_color: 0,
                barrier_buf_ptr: barrier_event_buf_base as i32,
                barrier_buf_end: barrier_event_buf_end as i32,
            });
        }

        // Pass 3: Compile helper functions.
        if self.mode == CompileMode::Eval {
            self.compile_object_helpers();
        }
        if self.mode == CompileMode::Eval {
            self.compile_array_helpers();
        }
        if self.mode == CompileMode::Eval {
            self.compile_get_proto_from_ctor();
        }
        if self.mode == CompileMode::Normal {
            self.compile_init_globals_function();
            self.compile_bootstrap_once_function();
            self.compile_init_function_props_function();
        }
        // Eval / Normal 均把函数填入父模块 __table（eval 现在 import 同一张表）。
        // 私有 table + element 会在临时 Instance 销毁后让 FunctionRef 失效。
        self.elements.active(
            Some(0),
            &ConstExpr::i32_const(self.table_base as i32),
            Elements::Functions(std::borrow::Cow::Borrowed(&self.function_table)),
        );

        if self.mode == CompileMode::Eval {
            let globals = [
                ("__func_props", self.func_props_global_idx),
                ("__heap_ptr", self.heap_ptr_global_idx),
                ("__obj_table_ptr", self.obj_table_global_idx),
                ("__obj_table_count", self.obj_table_count_global_idx),
                ("__shadow_sp", self.shadow_sp_global_idx),
                ("__object_heap_start", self.object_heap_start_global_idx),
                ("__num_ir_functions", self.num_ir_functions_global_idx),
                ("__shadow_stack_end", self.shadow_stack_end_global_idx),
                ("__array_proto_handle", self.array_proto_handle_global_idx),
                ("__object_proto_handle", self.object_proto_handle_global_idx),
                ("__eval_var_map_ptr", self.eval_var_map_ptr_global_idx),
                ("__eval_var_map_count", self.eval_var_map_count_global_idx),
                ("__bootstrap_done", self.bootstrap_done_global_idx),
                ("__function_props_done", self.function_props_done_global_idx),
                ("__function_props_base", self.function_props_base_global_idx),
                (
                    "__arr_proto_table_base",
                    self.arr_proto_table_base_global_idx,
                ),
                ("__arr_proto_table_len", self.arr_proto_table_len_global_idx),
                (
                    "__arr_proto_table_hash",
                    self.arr_proto_table_hash_global_idx,
                ),
                ("__heap_limit", self.heap_limit_global_idx),
                ("__alloc_ptr", self.alloc_ptr_global_idx),
                ("__alloc_end", self.alloc_end_global_idx),
                ("__gc_alloc_bytes", self.gc_alloc_bytes_global_idx),
                ("__gc_trigger_bytes", self.gc_trigger_bytes_global_idx),
                ("__gc_phase", self.gc_phase_global_idx),
                ("__good_color", self.good_color_global_idx),
                ("__barrier_buf_ptr", self.barrier_buf_ptr_global_idx),
                ("__barrier_buf_end", self.barrier_buf_end_global_idx),
            ];
            for (name, index) in globals {
                self.exports.export(name, ExportKind::Global, index);
            }
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

    /// P2.2: 在 main prologue 中初始化所有 imported globals。
    /// 这些值原本通过 ConstExpr 在 global 定义时设置，改为 import 后必须显式 global.set。
    /// 只在 main 函数开始时调用一次，在任何 helper 调用之前。
    fn emit_globals_init(&mut self) {
        let init = match &self.normal_init_values {
            Some(v) => *v,
            None => return,
        };
        // global 0: __func_props = 0 (deprecated)
        self.emit(WasmInstruction::I32Const(0));
        self.emit(WasmInstruction::GlobalSet(0));
        // global 1: __heap_ptr
        self.emit(WasmInstruction::I32Const(init.heap_ptr));
        self.emit(WasmInstruction::GlobalSet(1));
        // global 2: __obj_table_ptr
        self.emit(WasmInstruction::I32Const(init.obj_table_ptr));
        self.emit(WasmInstruction::GlobalSet(2));
        // global 3: __obj_table_count = 0
        self.emit(WasmInstruction::I32Const(0));
        self.emit(WasmInstruction::GlobalSet(3));
        // global 4: __shadow_sp
        self.emit(WasmInstruction::I32Const(init.shadow_sp));
        self.emit(WasmInstruction::GlobalSet(4));
        // global 5: __object_heap_start
        self.emit(WasmInstruction::I32Const(init.object_heap_start));
        self.emit(WasmInstruction::GlobalSet(5));
        // global 6: __num_ir_functions
        self.emit(WasmInstruction::I32Const(init.num_ir_functions));
        self.emit(WasmInstruction::GlobalSet(6));
        // global 7: __shadow_stack_end
        self.emit(WasmInstruction::I32Const(init.shadow_stack_end));
        self.emit(WasmInstruction::GlobalSet(7));
        // global 8: __array_proto_handle = -1 (uninitialized)
        self.emit(WasmInstruction::I32Const(-1));
        self.emit(WasmInstruction::GlobalSet(8));
        // global 9: __object_proto_handle = -1 (uninitialized)
        self.emit(WasmInstruction::I32Const(-1));
        self.emit(WasmInstruction::GlobalSet(9));
        // global 10: __eval_var_map_ptr
        self.emit(WasmInstruction::I32Const(init.eval_var_map_ptr));
        self.emit(WasmInstruction::GlobalSet(10));
        // global 11: __eval_var_map_count
        self.emit(WasmInstruction::I32Const(init.eval_var_map_count));
        self.emit(WasmInstruction::GlobalSet(11));
        // global 12: __bootstrap_done = 0
        self.emit(WasmInstruction::I32Const(0));
        self.emit(WasmInstruction::GlobalSet(12));
        // global 13: __function_props_done = 0
        self.emit(WasmInstruction::I32Const(0));
        self.emit(WasmInstruction::GlobalSet(13));
        // global 14: __function_props_base = 0
        self.emit(WasmInstruction::I32Const(0));
        self.emit(WasmInstruction::GlobalSet(14));
        // global 15: __arr_proto_table_base
        self.emit(WasmInstruction::I32Const(init.arr_proto_table_base));
        self.emit(WasmInstruction::GlobalSet(15));
        // global 16: __arr_proto_table_len
        self.emit(WasmInstruction::I32Const(init.arr_proto_table_len));
        self.emit(WasmInstruction::GlobalSet(16));
        // global 17: __arr_proto_table_hash
        self.emit(WasmInstruction::I64Const(init.arr_proto_table_hash));
        self.emit(WasmInstruction::GlobalSet(17));
        // global 18: __heap_limit = u32::MAX (runtime overrides when max_heap_size is configured)
        self.emit(WasmInstruction::I32Const(-1));
        self.emit(WasmInstruction::GlobalSet(18));
        // global 19: __alloc_ptr
        self.emit(WasmInstruction::I32Const(init.alloc_ptr));
        self.emit(WasmInstruction::GlobalSet(19));
        // global 20: __alloc_end
        self.emit(WasmInstruction::I32Const(init.alloc_end));
        self.emit(WasmInstruction::GlobalSet(20));
        // global 21: __gc_alloc_bytes
        self.emit(WasmInstruction::I32Const(init.gc_alloc_bytes));
        self.emit(WasmInstruction::GlobalSet(21));
        // global 22: __gc_trigger_bytes
        self.emit(WasmInstruction::I32Const(init.gc_trigger_bytes));
        self.emit(WasmInstruction::GlobalSet(22));
        // global 23: __gc_phase
        self.emit(WasmInstruction::I32Const(init.gc_phase));
        self.emit(WasmInstruction::GlobalSet(23));
        // global 24: __good_color
        self.emit(WasmInstruction::I32Const(init.good_color));
        self.emit(WasmInstruction::GlobalSet(24));
        // global 25: __barrier_buf_ptr
        self.emit(WasmInstruction::I32Const(init.barrier_buf_ptr));
        self.emit(WasmInstruction::GlobalSet(25));
        // global 26: __barrier_buf_end
        self.emit(WasmInstruction::I32Const(init.barrier_buf_end));
        self.emit(WasmInstruction::GlobalSet(26));
    }

    fn compile_init_globals_function(&mut self) {
        let previous_shadow_sp_scratch_idx = self.shadow_sp_scratch_idx;
        self.shadow_sp_scratch_idx = 0;
        self.current_func = Some(Function::new(vec![(1, ValType::I32)]));

        // 设置所有 imported globals 的初始值
        self.emit_globals_init();

        self.emit(WasmInstruction::I64Const(value::encode_undefined()));
        self.emit(WasmInstruction::End);

        self.codes.function(
            self.current_func
                .as_ref()
                .expect("init_globals function should be initialized"),
        );
        self.current_func = None;
        self.shadow_sp_scratch_idx = previous_shadow_sp_scratch_idx;
    }
}
