use super::*;
use crate::host_import_registry::{SpecialHostImport, host_import_specs};

impl Compiler {
    /// Convert an IR ValueId to a WASM local index, accounting for ssa_local_base.
    pub(crate) fn local_idx(&self, val_id: u32) -> u32 {
        val_id + self.ssa_local_base
    }

    /// call_func_idx scratch local (i32) — 存放解析后的函数表索引
    pub(crate) fn call_func_idx_scratch(&self) -> u32 {
        self.shadow_sp_scratch_idx + 1
    }

    /// GC safepoint 容量检查（P2 T2.3，spec IMPL-13/R2）。
    /// 函数 prologue 一次性检查：当前 shadow_sp + 本函数 spill_upper_bound
    /// 是否超出 shadow_stack_end。若超出，trap（防止 spill 区溢出覆盖对象堆）。
    ///
    /// spill_upper_bound = 本函数所有 safepoint 处 live handle local 数的最大值 × 8。
    /// 编译期静态计算；运行期只发一个比较。
    fn emit_safepoint_capacity_check(&mut self, _module: &IrModule, function: &IrFunction) {
        let spill_upper_bound = self.compute_max_spill_bytes(function);
        if spill_upper_bound == 0 {
            return;
        }
        // if (shadow_sp + spill_upper_bound) > shadow_stack_end: unreachable
        self.emit(WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
        self.emit(WasmInstruction::I32Const(spill_upper_bound as i32));
        self.emit(WasmInstruction::I32Add);
        self.emit(WasmInstruction::GlobalGet(self.shadow_stack_end_global_idx));
        self.emit(WasmInstruction::I32GtU);
        self.emit(WasmInstruction::If(BlockType::Empty));
        self.emit(WasmInstruction::Unreachable);
        self.emit(WasmInstruction::End);
    }

    /// 计算本函数所有 safepoint 处 live handle local 数的最大值 × 8（字节）。
    fn compute_max_spill_bytes(&self, function: &IrFunction) -> usize {
        let Some(ref liveness) = self.current_fn_liveness else {
            return 0;
        };
        let value_ty = self.current_fn_value_ty.as_ref();
        let var_liveness = self.current_fn_var_liveness.as_ref();
        let var_ty = self.current_fn_var_ty.as_ref();
        let mut max = 0usize;
        for (bid, instr_map) in liveness {
            let block = match function.block_by_id(*bid) {
                Some(b) => b,
                None => continue,
            };
            let instrs = block.instructions();
            for (i, ins) in instrs.iter().enumerate() {
                if !Self::is_safepoint(ins) {
                    continue;
                }
                let mut cnt = 0usize;
                if let Some(live) = instr_map.get(&i) {
                    cnt += live
                        .iter()
                        .filter(|v| {
                            value_ty
                                .and_then(|m| m.get(v))
                                .is_none_or(|t| *t == ValueTy::Handle)
                        })
                        .count();
                }
                // 变量 spill 上界：与 current_spill_locals 一致——存活且可能持有 handle 的变量 local。
                // 变量 local 与 SSA 值 local 索引不相交，故直接相加即精确上界。
                if let Some(names) = var_liveness.and_then(|m| m.get(bid)).and_then(|m| m.get(&i)) {
                    cnt += names
                        .iter()
                        .filter(|name| {
                            self.var_locals.contains_key(*name)
                                && var_ty
                                    .and_then(|m| m.get(*name))
                                    .is_none_or(|t| *t == ValueTy::Handle)
                        })
                        .count();
                }
                max = max.max(cnt);
            }
        }
        max * 8
    }

    /// 计算并缓存当前函数的 GC safepoint 分析：per-ValueId liveness + 变量 liveness +
    /// 两者的 ValueTy。compile_function / compile_eval 入口各调用一次。
    fn setup_gc_safepoint_analysis(&mut self, module: &IrModule, function: &IrFunction) {
        // per-ValueId liveness（扁平 → 嵌套便于查询）。
        let flat = crate::analysis_liveness::compute_liveness(function);
        let mut nested: HashMap<
            wjsm_ir::BasicBlockId,
            HashMap<usize, std::collections::HashSet<wjsm_ir::ValueId>>,
        > = HashMap::new();
        for ((bid, i), set) in flat {
            nested.entry(bid).or_default().insert(i, set);
        }
        self.current_fn_liveness = Some(nested);

        // 变量 liveness（弥补 per-ValueId liveness 看不到变量存活的空洞，供变量 spill）。
        let var_flat = crate::analysis_liveness::compute_var_liveness(function);
        let mut var_nested: HashMap<
            wjsm_ir::BasicBlockId,
            HashMap<usize, std::collections::HashSet<String>>,
        > = HashMap::new();
        for ((bid, i), set) in var_flat {
            var_nested.entry(bid).or_default().insert(i, set);
        }
        self.current_fn_var_liveness = Some(var_nested);

        let (value_ty, var_ty) = crate::analysis_value_ty::infer_value_and_var_ty(module, function);
        self.current_fn_value_ty = Some(value_ty);
        self.current_fn_var_ty = Some(var_ty);
    }

    /// call_env_obj scratch local (i64) — 存放解析后的闭包环境对象
    pub(crate) fn call_env_obj_scratch(&self) -> u32 {
        self.string_concat_scratch_idx + 1
    }
    /// Nested JS functions may LoadVar `$0.$global` (builtin globals like `$262`); only `main` stores it at init.
    fn emit_init_module_global_for_js_function(&mut self, function: &IrFunction) {
        let needs = function
            .blocks()
            .iter()
            .flat_map(|b| b.instructions())
            .any(|inst| {
                matches!(
                    inst,
                    Instruction::LoadVar { name, .. } | Instruction::StoreVar { name, .. }
                        if name == "$0.$global"
                )
            });
        if !needs {
            return;
        }
        let Some(&local_idx) = self.var_locals.get("$0.$global") else {
            return;
        };
        let func_idx = self
            .builtin_func_indices
            .get(&Builtin::CreateGlobalObject)
            .copied()
            .expect("create_global_object builtin");
        self.emit(WasmInstruction::Call(func_idx));
        self.emit(WasmInstruction::LocalSet(local_idx));
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
        func.instruction(&WasmInstruction::Call(
            self.special_host_import_indices[&SpecialHostImport::ClosureGetFunc],
        ));
        func.instruction(&WasmInstruction::LocalSet(func_idx_local));
        func.instruction(&WasmInstruction::LocalGet(callee_local));
        func.instruction(&WasmInstruction::I32WrapI64);
        func.instruction(&WasmInstruction::Call(
            self.special_host_import_indices[&SpecialHostImport::ClosureGetEnv],
        ));
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
        // Pass 0: 模块级 GC 分析（Layer 3c）
        self.gc_analysis = Some(GcAnalysis::analyze(module));

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
            if spec.group
                == Some(crate::host_import_registry::HostImportGroup::ArrayPrototypeMethod)
            {
                self.push_func_table(idx as u32);
            }
        }
        self.arr_proto_table_base = arr_proto_base;

        if self.mode == CompileMode::Normal {
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
        self.alloc_counter_global_idx = 5;
        self.array_proto_handle_global_idx = 9;
        self.object_proto_handle_global_idx = 10;
        self.eval_var_map_ptr_global_idx = 11;
        self.eval_var_map_count_global_idx = 12;
        self.bootstrap_done_global_idx = 13;
        self.function_props_done_global_idx = 14;
        self.function_props_base_global_idx = 15;

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
        // Pass 3: Compile object helper functions.
        self.compile_object_helpers();
        // 编译数组辅助函数
        self.compile_array_helpers();
        if self.mode == CompileMode::Normal {
            self.compile_bootstrap_once_function();
            self.compile_init_function_props_function();
        }
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
        // P4 GC：obj_table 必须容纳 GC 阈值前的峰值分配数。
        // GC 默认阈值 1000，故 obj_table 至少 2048（覆盖阈值 + 临时对象缓冲）。
        // 旧值 max(256, num_functions*2) 在 GC 接通后不够（count 超 256 → obj_table 越界读垃圾）。
        let handle_table_entries = std::cmp::max(2048, num_functions * 2);
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
            // Global 13/14/15: startup snapshot bootstrap/function-property phase state.
            self.globals.global(
                GlobalType {
                    val_type: ValType::I32,
                    mutable: true,
                    shared: false,
                },
                &ConstExpr::i32_const(0),
            );
            self.exports.export(
                "__bootstrap_done",
                ExportKind::Global,
                self.bootstrap_done_global_idx,
            );
            self.globals.global(
                GlobalType {
                    val_type: ValType::I32,
                    mutable: true,
                    shared: false,
                },
                &ConstExpr::i32_const(0),
            );
            self.exports.export(
                "__function_props_done",
                ExportKind::Global,
                self.function_props_done_global_idx,
            );
            self.globals.global(
                GlobalType {
                    val_type: ValType::I32,
                    mutable: true,
                    shared: false,
                },
                &ConstExpr::i32_const(0),
            );
            self.exports.export(
                "__function_props_base",
                ExportKind::Global,
                self.function_props_base_global_idx,
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

    fn emit_startup_phase_call(&mut self, func_idx: u32) {
        self.emit(WasmInstruction::Call(func_idx));
        self.emit(WasmInstruction::LocalTee(self.string_concat_scratch_idx));
        self.emit(WasmInstruction::LocalGet(self.string_concat_scratch_idx));
        self.emit(WasmInstruction::I64Const(32));
        self.emit(WasmInstruction::I64ShrU);
        self.emit(WasmInstruction::I32WrapI64);
        self.emit(WasmInstruction::I32Const(value::TAG_EXCEPTION as i32));
        self.emit(WasmInstruction::I32Eq);
        self.emit(WasmInstruction::If(BlockType::Empty));
        self.emit(WasmInstruction::LocalGet(self.string_concat_scratch_idx));
        self.emit_eval_var_frame_exit();
        self.emit(WasmInstruction::Return);
        self.emit(WasmInstruction::End);
    }

    fn compile_bootstrap_once_function(&mut self) {
        let previous_shadow_sp_scratch_idx = self.shadow_sp_scratch_idx;
        self.shadow_sp_scratch_idx = 0;
        self.current_func = Some(Function::new(vec![(1, ValType::I32)]));

        self.emit(WasmInstruction::GlobalGet(self.bootstrap_done_global_idx));
        self.emit(WasmInstruction::I32Const(0));
        self.emit(WasmInstruction::I32Ne);
        self.emit(WasmInstruction::If(BlockType::Empty));
        self.emit(WasmInstruction::I64Const(value::encode_undefined()));
        self.emit(WasmInstruction::Return);
        self.emit(WasmInstruction::End);

        // ── 初始化 Array.prototype ──
        self.emit(WasmInstruction::I32Const(64));
        self.emit(WasmInstruction::Call(self.obj_new_func_idx));
        self.emit(WasmInstruction::LocalTee(self.shadow_sp_scratch_idx));
        self.emit(WasmInstruction::GlobalSet(
            self.array_proto_handle_global_idx,
        ));
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
            self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
            self.emit(WasmInstruction::I64ExtendI32U);
            let box_base = value::BOX_BASE as i64;
            let tag_object = (value::TAG_OBJECT << 32) as i64;
            self.emit(WasmInstruction::I64Const(box_base | tag_object));
            self.emit(WasmInstruction::I64Or);
            self.emit(WasmInstruction::I32Const(name_id as i32));
            self.emit(WasmInstruction::I64Const(value::encode_function_idx(
                table_idx,
            )));
            self.emit(WasmInstruction::Call(self.obj_set_func_idx));
        }

        // ── 初始化 Object.prototype ──
        self.emit(WasmInstruction::GlobalGet(
            self.object_proto_handle_global_idx,
        ));
        self.emit(WasmInstruction::I32Const(-1));
        self.emit(WasmInstruction::I32Eq);
        self.emit(WasmInstruction::If(BlockType::Empty));
        self.emit(WasmInstruction::I32Const(64));
        self.emit(WasmInstruction::Call(self.obj_new_func_idx));
        self.emit(WasmInstruction::LocalTee(self.shadow_sp_scratch_idx));
        self.emit(WasmInstruction::GlobalSet(
            self.object_proto_handle_global_idx,
        ));
        self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
        self.emit(WasmInstruction::I64ExtendI32U);
        let object_tag = value::BOX_BASE as i64 | ((value::TAG_OBJECT << 32) as i64);
        self.emit(WasmInstruction::I64Const(object_tag));
        self.emit(WasmInstruction::I64Or);
        self.emit(WasmInstruction::Call(
            self.special_host_import_indices[&SpecialHostImport::ObjectProtoInit],
        ));
        self.emit(WasmInstruction::Drop);
        self.emit(WasmInstruction::End);

        self.emit(WasmInstruction::GlobalGet(self.obj_table_count_global_idx));
        self.emit(WasmInstruction::GlobalSet(self.function_props_base_global_idx));
        self.emit(WasmInstruction::I32Const(1));
        self.emit(WasmInstruction::GlobalSet(self.bootstrap_done_global_idx));
        self.emit(WasmInstruction::I64Const(value::encode_undefined()));
        self.emit(WasmInstruction::End);

        self.codes.function(
            self.current_func
                .as_ref()
                .expect("bootstrap function should be initialized"),
        );
        self.current_func = None;
        self.shadow_sp_scratch_idx = previous_shadow_sp_scratch_idx;
    }

    fn compile_init_function_props_function(&mut self) {
        let previous_shadow_sp_scratch_idx = self.shadow_sp_scratch_idx;
        self.shadow_sp_scratch_idx = 0;
        self.current_func = Some(Function::new(vec![(1, ValType::I32)]));

        if self.mode == CompileMode::Normal {
            self.emit(WasmInstruction::GlobalGet(self.function_props_done_global_idx));
            self.emit(WasmInstruction::I32Const(0));
            self.emit(WasmInstruction::I32Ne);
            self.emit(WasmInstruction::If(BlockType::Empty));
            self.emit(WasmInstruction::I64Const(value::encode_undefined()));
            self.emit(WasmInstruction::Return);
            self.emit(WasmInstruction::End);

            self.emit(WasmInstruction::GlobalGet(self.function_props_base_global_idx));
            self.emit(WasmInstruction::GlobalSet(self.obj_table_count_global_idx));
        }

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
            self.emit(WasmInstruction::I64Const(value::encode_string_ptr(name_ptr)));
            self.emit(WasmInstruction::Call(self.obj_set_func_idx));
        }

        if self.mode == CompileMode::Normal {
            self.emit(WasmInstruction::I32Const(1));
            self.emit(WasmInstruction::GlobalSet(self.function_props_done_global_idx));
        }
        self.emit(WasmInstruction::I64Const(value::encode_undefined()));
        self.emit(WasmInstruction::End);

        self.codes.function(
            self.current_func
                .as_ref()
                .expect("function props initializer should be initialized"),
        );
        self.current_func = None;
        self.shadow_sp_scratch_idx = previous_shadow_sp_scratch_idx;
    }

    pub(crate) fn compile_function(
        &mut self,
        module: &IrModule,
        function: &IrFunction,
    ) -> Result<()> {
        self.current_func_is_main = is_module_entry_ir_function(function.name());
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

        // ── GC safepoint（P2）：计算 liveness + ValueTy（per-ValueId + 变量）──
        // 供 NewObject/NewArray/Call/CallBuiltin/SuperCall/ConstructCall 的 safepoint spill。
        self.setup_gc_safepoint_analysis(module, function);
        self.current_emit_block_idx = 0;
        self.current_emit_instr_idx = 0;

        let local_count = self.required_local_count(function);
        // string_concat (i64) at local_count
        // call_env_obj (i64) at local_count+1
        // shadow_sp (i32) at local_count+2
        // call_func_idx (i32) at local_count+3
        // safepoint_sp_saved (i32) at local_count+4  (P2: safepoint spill save/restore)
        self.string_concat_scratch_idx = local_count;
        self.shadow_sp_scratch_idx = local_count + 2;
        self.safepoint_sp_saved_idx = local_count + 4;
        self.eval_var_base_local_idx = local_count + 5;
        let param_i64_count = self.ssa_local_base;
        let total_i64_locals = local_count.saturating_sub(param_i64_count) + 2; // string_concat + call_env_obj
        let total_i32_locals = 3 + u32::from(!self.var_memory_offsets.is_empty());
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

        // ── GC safepoint 容量检查（P2 T2.3，spec IMPL-13/R2）──
        // 防 spill 区溢出覆盖对象堆：shadow_sp + frame_size + spill_upper_bound <= shadow_stack_end。
        // spill_upper_bound = 本函数所有 safepoint 的最大 live handle local 数 × 8。
        self.emit_safepoint_capacity_check(module, function);

        if is_module_entry_ir_function(function.name()) && self.mode == CompileMode::Normal {
            self.emit_startup_phase_call(self.bootstrap_func_idx);
            self.emit_startup_phase_call(self.init_function_props_func_idx);
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
        self.current_fn_liveness = None;
        self.current_fn_value_ty = None;
        self.current_fn_var_liveness = None;
        self.current_fn_var_ty = None;

        Ok(())
    }

    pub(crate) fn compile_js_function(
        &mut self,
        module: &IrModule,
        function: &IrFunction,
        function_id: wjsm_ir::FunctionId,
    ) -> Result<()> {
        self.current_func_returns_value = true;
        self.current_home_object = function.home_object;
        self.current_function_id = Some(function_id);
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

        // ── GC safepoint（P2）：计算 liveness + ValueTy（per-ValueId + 变量）──
        self.setup_gc_safepoint_analysis(module, function);
        self.current_emit_block_idx = 0;
        self.current_emit_instr_idx = 0;

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
        // safepoint_sp_saved (i32) at total_locals+4  (P2: safepoint spill save/restore)
        self.string_concat_scratch_idx = total_locals;
        // call_env_obj scratch = string_concat + 1 (i64), computed by call_env_obj_scratch()
        self.shadow_sp_scratch_idx = total_locals + 2;
        self.safepoint_sp_saved_idx = total_locals + 4;
        self.eval_var_base_local_idx = total_locals + 5;
        // call_func_idx = shadow_sp + 1 (i32), computed by call_func_idx_scratch()
        let total_i64_locals = total_locals.saturating_sub(4) + 2; // string_concat + call_env_obj
        let total_i32_locals = 3 + u32::from(!self.var_memory_offsets.is_empty());

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

        // ── GC safepoint 容量检查（P2 T2.3，spec IMPL-13/R2）──
        self.emit_safepoint_capacity_check(module, function);

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

        self.emit_init_module_global_for_js_function(function);
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
        self.current_home_object = None;
        self.current_function_id = None;
        self.current_fn_liveness = None;
        self.current_fn_value_ty = None;
        self.current_fn_var_liveness = None;
        self.current_fn_var_ty = None;

        Ok(())
    }
}
