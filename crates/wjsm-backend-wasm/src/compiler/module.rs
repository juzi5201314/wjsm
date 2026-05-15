use anyhow::{Context, Result};
use wasm_encoder::{
    ConstExpr, Elements, ExportKind, GlobalType, RefType, TableType, ValType,
};
use wjsm_ir::{constants, Module as IrModule};

use super::state::{CompileMode, Compiler, SHADOW_STACK_SIZE};

impl Compiler {
    pub(crate) fn compile_module(&mut self, module: &IrModule) -> Result<()> {
        // Pass 1: Register all IR functions as WASM functions.
        let mut main_wasm_idx: Option<u32> = None;
        for function in module.functions() {
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
                    // main: Type 1 = () -> ()
                    self.functions.function(1);
                }
                main_wasm_idx = Some(wasm_idx);
            } else {
                // JS functions: Type 12 = (i64, i64, i32, i32) -> i64 (含 env_obj)
                self.functions.function(12);
            }

            self.push_func_table(wasm_idx);
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
        // 设置 obj_spread_func_idx 为 import index 82
        self.obj_spread_func_idx = 82;

        self.get_proto_from_ctor_func_idx = self._next_import_func;
        self.functions.function(3); // Type 3: (i64) -> (i64)
        self.push_func_table(self._next_import_func);
        self._next_import_func += 1;
        // Register array prototype method imports in function table (imports 50-76)
        let arr_proto_base = self.function_table.len() as u32;
        for import_idx in 50u32..=76u32 {
            self.push_func_table(import_idx);
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

}
