//! builtin 编译：ConsoleLog ~ EnumeratorKey

use super::*;
use crate::compiler_builtins::BuiltinDispatch;
use crate::host_import_registry::SpecialHostImport;

impl Compiler {
    /// 处理 ConsoleLog ~ EnumeratorKey 等 builtin。
    pub(crate) fn compile_builtin_core(
        &mut self,
        dest: Option<ValueId>,
        builtin: &Builtin,
        args: &[ValueId],
    ) -> Result<BuiltinDispatch> {
        match builtin {
            Builtin::ConsoleLog
            | Builtin::ConsoleError
            | Builtin::ConsoleWarn
            | Builtin::ConsoleInfo
            | Builtin::ConsoleDebug
            | Builtin::ConsoleTrace => {
                // 使用影子栈传递所有参数，调用 console varargs (i32, i32) -> ()
                // 保存 shadow_sp 基址
                self.emit(WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
                self.emit(WasmInstruction::LocalSet(self.shadow_sp_scratch_idx));
                // 影子栈边界检查
                self.emit_shadow_stack_overflow_check((args.len() * 8) as i32);
                // 将所有参数写入影子栈
                for arg in args {
                    self.emit(WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
                    self.emit(WasmInstruction::LocalGet(self.local_idx(arg.0)));
                    self.emit(WasmInstruction::I64Store(MemArg {
                        offset: 0,
                        align: 3,
                        memory_index: 0,
                    }));
                    self.emit(WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
                    self.emit(WasmInstruction::I32Const(8));
                    self.emit(WasmInstruction::I32Add);
                    self.emit(WasmInstruction::GlobalSet(self.shadow_sp_global_idx));
                }
                // Type 33: (i32, i32) -> (): args_base, args_count
                self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
                self.emit(WasmInstruction::I32Const(args.len() as i32));
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                // 恢复 shadow_sp
                self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
                self.emit(WasmInstruction::GlobalSet(self.shadow_sp_global_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::Debugger => {
                // No-op in Phase 3
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::F64Mod | Builtin::F64Exp => {
                // f64_mod(a, b) / f64_pow(a, b) — call runtime host function
                let lhs = args.first().context("F64Mod/Exp expects 2 arguments")?;
                let rhs = args.get(1).context("F64Mod/Exp expects 2 arguments")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(lhs.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(rhs.0)));
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::SetTimeout | Builtin::SetInterval => {
                let callback = args
                    .first()
                    .with_context(|| format!("{builtin} expects 2 arguments"))?;
                let delay = args
                    .get(1)
                    .with_context(|| format!("{builtin} expects 2 arguments"))?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(callback.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(delay.0)));
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                self.store_or_drop_call_result(dest);
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::ClearTimeout | Builtin::ClearInterval => {
                let timer_id = args
                    .first()
                    .with_context(|| format!("{builtin} expects 1 argument"))?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(timer_id.0)));
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::CreateClosure => {
                // args: [func_ref_val, env_obj_val]
                // func_ref_val 是 NaN-boxed 函数值 → 提取 table_idx (i32.wrap_i64)
                // env_obj_val 是 NaN-boxed 环境对象 (i64)
                // 调用 closure_create(table_idx, env_obj) → i64 (TAG_CLOSURE 编码)
                let func_ref_val = args
                    .first()
                    .with_context(|| "CreateClosure expects func_ref arg")?;
                let env_obj_val = args
                    .get(1)
                    .with_context(|| "CreateClosure expects env_obj arg")?;
                // 推入 func_ref (i64) 与 env_obj (i64)；运行时 decode_function_idx
                self.emit(WasmInstruction::LocalGet(self.local_idx(func_ref_val.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(env_obj_val.0)));
                // 调用 closure_create
                self.emit(WasmInstruction::Call(
                    self.special_host_import_indices[&SpecialHostImport::ClosureCreate],
                ));
                self.store_or_drop_call_result(dest);
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::Fetch => {
                let input = args
                    .first()
                    .with_context(|| format!("{builtin} expects at least 1 argument"))?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(input.0)));
                let init = args.get(1).copied();
                if let Some(init_val) = init {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(init_val.0)));
                } else {
                    self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                }
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                self.store_or_drop_call_result(dest);
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::JsonStringify => {
                for arg in args.iter().take(3) {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(arg.0)));
                }
                for _ in args.len()..3 {
                    self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                }
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                self.store_or_drop_call_result(dest);
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::JsonParse => {
                for arg in args.iter().take(2) {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(arg.0)));
                }
                for _ in args.len()..2 {
                    self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                }
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                self.store_or_drop_call_result(dest);
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::Eval => {
                let code = args.first().context("eval expects code arg")?;
                let env = args.get(1).context("eval expects scope env arg")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(code.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(env.0)));
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                self.store_or_drop_call_result(dest);
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::EvalIndirect => {
                let code = args.first().context("indirect eval expects code arg")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(code.0)));
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                self.store_or_drop_call_result(dest);
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::EvalResult => {
                let value = args.first().context("eval.result expects value arg")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(value.0)));
                self.store_or_drop_call_result(dest);
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::ScopeRecordCreate => {
                let capacity = args
                    .first()
                    .context("scope_record_create expects capacity")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(capacity.0)));
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                self.store_or_drop_call_result(dest);
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::ScopeRecordAddBinding => {
                let rec = args
                    .first()
                    .context("scope_record_add_binding expects record")?;
                let name = args
                    .get(1)
                    .context("scope_record_add_binding expects name")?;
                let val = args
                    .get(2)
                    .context("scope_record_add_binding expects value")?;
                let is_tdz = args
                    .get(3)
                    .context("scope_record_add_binding expects is_tdz")?;
                let is_const = args
                    .get(4)
                    .context("scope_record_add_binding expects is_const")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(rec.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(name.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(val.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(is_tdz.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(is_const.0)));
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                self.emit(WasmInstruction::Drop);
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::EvalGetBinding => {
                let rec = args.first().context("eval_get_binding expects record")?;
                let name = args.get(1).context("eval_get_binding expects name")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(rec.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(name.0)));
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    let local = self.local_idx(d.0);
                    self.emit(WasmInstruction::LocalSet(local));
                    if self.mode == CompileMode::Eval {
                        self.emit(WasmInstruction::LocalGet(local));
                        self.emit(WasmInstruction::I64Const(32));
                        self.emit(WasmInstruction::I64ShrU);
                        self.emit(WasmInstruction::I32WrapI64);
                        self.emit(WasmInstruction::I32Const(value::TAG_EXCEPTION as i32));
                        self.emit(WasmInstruction::I32Eq);
                        self.emit(WasmInstruction::If(BlockType::Empty));
                        self.emit(WasmInstruction::LocalGet(local));
                        self.emit_eval_var_frame_exit();
                        self.emit(WasmInstruction::Return);
                        self.emit(WasmInstruction::End);
                    }
                } else {
                    self.emit(WasmInstruction::Drop);
                }
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::EvalSetBinding => {
                let rec = args.first().context("eval_set_binding expects record")?;
                let name = args.get(1).context("eval_set_binding expects name")?;
                let val = args.get(2).context("eval_set_binding expects value")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(rec.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(name.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(val.0)));
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                self.store_or_drop_call_result(dest);
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::EvalHasBinding => {
                let rec = args.first().context("eval_has_binding expects record")?;
                let name = args.get(1).context("eval_has_binding expects name")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(rec.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(name.0)));
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                self.store_or_drop_call_result(dest);
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::EvalSuperBase => {
                let rec = args.first().context("eval_super_base expects record")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(rec.0)));
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                self.store_or_drop_call_result(dest);
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::ScopeRecordSetMeta => {
                let rec = args
                    .first()
                    .context("scope_record_set_meta expects record")?;
                let key = args.get(1).context("scope_record_set_meta expects key")?;
                let val = args.get(2).context("scope_record_set_meta expects value")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(rec.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(key.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(val.0)));
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                self.emit(WasmInstruction::Drop);
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::ScopeRecordDestroy => {
                let rec = args
                    .first()
                    .context("scope_record_destroy expects record")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(rec.0)));
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::IsException => {
                let value = args.first().context("is_exception expects value arg")?;
                let value_local = self.local_idx(value.0);
                // Check if value is TAG_EXCEPTION
                self.emit(WasmInstruction::LocalGet(value_local));
                self.emit(WasmInstruction::I64Const(32));
                self.emit(WasmInstruction::I64ShrU);
                self.emit(WasmInstruction::I32WrapI64);
                self.emit(WasmInstruction::I32Const(value::TAG_EXCEPTION as i32));
                self.emit(WasmInstruction::I32Eq);
                // Result is i32 (0 or 1). Convert to NaN-boxed bool
                self.emit(WasmInstruction::I64ExtendI32U);
                let box_base = value::BOX_BASE as i64;
                let bool_tag_shifted = (value::TAG_BOOL as i64) << 32;
                self.emit(WasmInstruction::I64Const(box_base | bool_tag_shifted));
                self.emit(WasmInstruction::I64Or);
                self.store_or_drop_call_result(dest);
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::NewTarget => {
                // new.target meta property: (i64 dummy) -> i64
                let arg = args.first().context("new.target expects 1 dummy arg")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(arg.0)));
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                self.store_or_drop_call_result(dest);
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::Throw => {
                if let Some(val) = args.first() {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(val.0)));
                } else {
                    self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                }
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                self.emit(WasmInstruction::Unreachable);
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::IteratorFrom | Builtin::EnumeratorFrom | Builtin::IteratorStepValue => {
                let val = args
                    .first()
                    .context("IteratorFrom/EnumeratorFrom/IteratorStepValue expects 1 arg")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(val.0)));
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                self.store_or_drop_call_result(dest);
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::IteratorNext | Builtin::EnumeratorNext => {
                let handle = args
                    .first()
                    .context("IteratorNext/EnumeratorNext expects 1 arg")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(handle.0)));
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::IteratorClose => {
                let handle = args.first().context("IteratorClose expects handle arg")?;
                let completion = args
                    .get(1)
                    .context("IteratorClose expects completion arg")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(handle.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(completion.0)));
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                self.store_or_drop_call_result(dest);
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::AsyncIteratorFrom => {
                let val = args.first().context("AsyncIteratorFrom expects 1 arg")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(val.0)));
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::IteratorValue | Builtin::EnumeratorKey => {
                let handle = args
                    .first()
                    .context("IteratorValue/EnumeratorKey expects 1 arg")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(handle.0)));
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(BuiltinDispatch::Handled)
            }
            _ => Ok(BuiltinDispatch::NotHandled),
        }
    }
}
