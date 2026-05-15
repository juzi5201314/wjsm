use anyhow::{Context, Result, bail};
use wasm_encoder::{Instruction as WasmInstruction, MemArg};
use wjsm_ir::{Builtin, ValueId, value};

use super::state::Compiler;

impl Compiler {
    pub(crate) fn compile_proto_method_call(
        &mut self,
        dest: Option<ValueId>,
        builtin: &Builtin,
        args: &[ValueId],
    ) -> Result<()> {
        let import_idx = self
            .builtin_func_indices
            .get(builtin)
            .copied()
            .with_context(|| format!("no WASM func index for builtin {builtin}"))?;
        // 确定 this_val 和影子栈参数
        // ArrayIsArray: this_val=undefined, 所有 args 走影子栈
        // 其他方法: this_val=args[0], args[1..] 走影子栈
        let (this_val_idx, shadow_args) = if matches!(builtin, Builtin::ArrayIsArray | Builtin::StringFromCharCode | Builtin::StringFromCodePoint | Builtin::MathMax | Builtin::MathMin | Builtin::MathHypot | Builtin::DateConstructor) {
            (None, args)
        } else {
            let this = args
                .first()
                .with_context(|| format!("{builtin} expects at least 1 argument (this_val)"))?;
            (Some(this.0), &args[1..])
        };
        // 保存 shadow_sp 基址
        self.emit(WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
        self.emit(WasmInstruction::LocalSet(self.shadow_sp_scratch_idx));
        // 影子栈边界检查
        self.emit_shadow_stack_overflow_check((shadow_args.len() * 8) as i32);

        // 将 shadow_args 写入影子栈
        for arg in shadow_args {
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
        // 推入 Type 12 调用参数: env_obj, this_val, args_base, args_count
        // env_obj = undefined
        self.emit(WasmInstruction::I64Const(value::encode_undefined()));
        // this_val
        if let Some(val_idx) = this_val_idx {
            self.emit(WasmInstruction::LocalGet(self.local_idx(val_idx)));
        } else {
            self.emit(WasmInstruction::I64Const(value::encode_undefined()));
        }
        // args_base
        self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
        // args_count
        self.emit(WasmInstruction::I32Const(shadow_args.len() as i32));
        // 调用 Type 12 宿主函数
        self.emit(WasmInstruction::Call(import_idx));
        // 恢复 shadow_sp
        self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
        self.emit(WasmInstruction::GlobalSet(self.shadow_sp_global_idx));
        // 处理返回值
        if let Some(d) = dest {
            self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
        } else {
            self.emit(WasmInstruction::Drop);
        }
        Ok(())
    }


    pub(crate) fn compile_builtin_call(
        &mut self,
        dest: Option<ValueId>,
        builtin: &Builtin,
        args: &[ValueId],
    ) -> Result<()> {
        match builtin {
            Builtin::ConsoleLog
            | Builtin::ConsoleError
            | Builtin::ConsoleWarn
            | Builtin::ConsoleInfo
            | Builtin::ConsoleDebug
            | Builtin::ConsoleTrace => {
                let first_arg = args
                    .first()
                    .with_context(|| format!("{builtin} expects at least one argument"))?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(first_arg.0)));
                let func_idx = self.builtin_func_indices.get(builtin).copied().unwrap_or(0);
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::Debugger => {
                // No-op in Phase 3
                Ok(())
            }
            Builtin::F64Mod | Builtin::F64Exp => {
                // f64_mod(a, b) / f64_pow(a, b) — call runtime host function
                let lhs = args.first().context("F64Mod/Exp expects 2 arguments")?;
                let rhs = args.get(1).context("F64Mod/Exp expects 2 arguments")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(lhs.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(rhs.0)));
                let func_idx = self.builtin_func_indices.get(builtin).copied().unwrap_or(0);
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
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
                let func_idx = self.builtin_func_indices.get(builtin).copied().unwrap_or(0);
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                } else {
                    self.emit(WasmInstruction::Drop);
                }
                Ok(())
            }
            Builtin::ClearTimeout | Builtin::ClearInterval => {
                let timer_id = args
                    .first()
                    .with_context(|| format!("{builtin} expects 1 argument"))?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(timer_id.0)));
                let func_idx = self.builtin_func_indices.get(builtin).copied().unwrap_or(0);
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::CreateClosure => {
                // args: [func_ref_val, env_obj_val]
                // func_ref_val 是 NaN-boxed 函数值 → 提取 table_idx (i32.wrap_i64)
                // env_obj_val 是 NaN-boxed 环境对象 (i64)
                // 调用 closure_create(table_idx, env_obj) → i64 (TAG_CLOSURE 编码)
                let func_ref_val = args
                    .get(0)
                    .with_context(|| "CreateClosure expects func_ref arg")?;
                let env_obj_val = args
                    .get(1)
                    .with_context(|| "CreateClosure expects env_obj arg")?;
                // 推入 func_idx (i32): 从 NaN-boxed 函数值提取
                self.emit(WasmInstruction::LocalGet(self.local_idx(func_ref_val.0)));
                self.emit(WasmInstruction::I32WrapI64);
                // 推入 env_obj (i64)
                self.emit(WasmInstruction::LocalGet(self.local_idx(env_obj_val.0)));
                // 调用 closure_create
                self.emit(WasmInstruction::Call(self.closure_create_func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                } else {
                    self.emit(WasmInstruction::Drop);
                }
                Ok(())
            }
            Builtin::Fetch | Builtin::JsonStringify | Builtin::JsonParse => {
                let val = args
                    .first()
                    .with_context(|| format!("{builtin} expects 1 argument"))?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(val.0)));
                let func_idx = self.builtin_func_indices.get(builtin).copied().unwrap_or(0);
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                } else {
                    self.emit(WasmInstruction::Drop);
                }
                Ok(())
            }
            Builtin::Eval => {
                let code = args.first().context("eval expects code arg")?;
                let env = args.get(1).context("eval expects scope env arg")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(code.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(env.0)));
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                } else {
                    self.emit(WasmInstruction::Drop);
                }
                Ok(())
            }
            Builtin::EvalIndirect => {
                let code = args.first().context("indirect eval expects code arg")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(code.0)));
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                } else {
                    self.emit(WasmInstruction::Drop);
                }
                Ok(())
            }
            Builtin::EvalResult => {
                let value = args.first().context("eval.result expects value arg")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(value.0)));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                } else {
                    self.emit(WasmInstruction::Drop);
                }
                Ok(())
            }
            Builtin::Throw => {
                if let Some(val) = args.first() {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(val.0)));
                } else {
                    self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                }
                let func_idx = self.builtin_func_indices.get(builtin).copied().unwrap_or(3);
                self.emit(WasmInstruction::Call(func_idx));
                self.emit(WasmInstruction::Unreachable);
                Ok(())
            }
            Builtin::IteratorFrom | Builtin::EnumeratorFrom => {
                let val = args
                    .first()
                    .context("IteratorFrom/EnumeratorFrom expects 1 arg")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(val.0)));
                let func_idx = self.builtin_func_indices.get(builtin).copied().unwrap_or(0);
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::IteratorNext | Builtin::EnumeratorNext => {
                let handle = args
                    .first()
                    .context("IteratorNext/EnumeratorNext expects 1 arg")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(handle.0)));
                let func_idx = self.builtin_func_indices.get(builtin).copied().unwrap_or(0);
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::IteratorClose => {
                let handle = args.first().context("IteratorClose expects 1 arg")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(handle.0)));
                let func_idx = self.builtin_func_indices.get(builtin).copied().unwrap_or(0);
                self.emit(WasmInstruction::Call(func_idx));
                Ok(())
            }
            Builtin::IteratorValue | Builtin::EnumeratorKey => {
                let handle = args
                    .first()
                    .context("IteratorValue/EnumeratorKey expects 1 arg")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(handle.0)));
                let func_idx = self.builtin_func_indices.get(builtin).copied().unwrap_or(0);
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::IteratorDone | Builtin::EnumeratorDone => {
                let handle = args
                    .first()
                    .context("IteratorDone/EnumeratorDone expects 1 arg")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(handle.0)));
                let func_idx = self.builtin_func_indices.get(builtin).copied().unwrap_or(0);
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::TypeOf => {
                // typeof(value) -> 返回类型名称字符串指针
                let val = args.first().context("TypeOf expects 1 arg")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(val.0)));
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .unwrap_or(13);
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::In => {
                // prop in object -> bool
                let object = args.first().context("In expects 2 args (object, prop)")?;
                let prop = args.get(1).context("In expects 2 args (object, prop)")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(object.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(prop.0)));
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .unwrap_or(14);
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::InstanceOf => {
                // value instanceof constructor -> bool
                let value = args
                    .first()
                    .context("InstanceOf expects 2 args (value, constructor)")?;
                let constructor = args
                    .get(1)
                    .context("InstanceOf expects 2 args (value, constructor)")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(value.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(constructor.0)));
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .unwrap_or(15);
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::AbstractEq => {
                // abstract_eq(a, b) -> bool
                let lhs = args.first().context("AbstractEq expects 2 args")?;
                let rhs = args.get(1).context("AbstractEq expects 2 args")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(lhs.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(rhs.0)));
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .unwrap_or(19);
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::AbstractCompare => {
                // abstract_compare(a, b) -> bool (a < b)
                let lhs = args.first().context("AbstractCompare expects 2 args")?;
                let rhs = args.get(1).context("AbstractCompare expects 2 args")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(lhs.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(rhs.0)));
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .unwrap_or(20);
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::DefineProperty => {
                // define_property(obj: i64, key: i64, desc: i64) -> ()
                let obj_arg = args.first().context("DefineProperty expects 3 args")?;
                let key_arg = args.get(1).context("DefineProperty expects 3 args")?;
                let desc_arg = args.get(2).context("DefineProperty expects 3 args")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(obj_arg.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(key_arg.0)));
                self.emit(WasmInstruction::I32WrapI64);
                self.emit(WasmInstruction::LocalGet(self.local_idx(desc_arg.0)));
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .unwrap_or(17);
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(obj_arg.0)));
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::GetOwnPropDesc => {
                // get_own_prop_desc(obj: i64, key: i64) -> i64
                let obj_arg = args.first().context("GetOwnPropDesc expects 2 args")?;
                let key_arg = args.get(1).context("GetOwnPropDesc expects 2 args")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(obj_arg.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(key_arg.0)));
                self.emit(WasmInstruction::I32WrapI64);
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .unwrap_or(18);
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            // ── Array method builtins ─────────────────────────────────────
            Builtin::ArrayPush
            | Builtin::ArrayPop
            | Builtin::ArrayIncludes
            | Builtin::ArrayJoin
            | Builtin::ArrayConcat
            | Builtin::ArrayReverse
            | Builtin::ArrayInitLength
            | Builtin::ArrayGetLength => {
                // Single arg: (i64) -> i64 or Two arg: (i64, i64) -> i64
                // These all take the array as the first arg
                for arg in args {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(arg.0)));
                }
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for builtin {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            // ── Math unary builtins (i64) -> i64 ──
            Builtin::MathAbs
            | Builtin::MathAcos
            | Builtin::MathAcosh
            | Builtin::MathAsin
            | Builtin::MathAsinh
            | Builtin::MathAtan
            | Builtin::MathAtanh
            | Builtin::MathCbrt
            | Builtin::MathCeil
            | Builtin::MathClz32
            | Builtin::MathCos
            | Builtin::MathCosh
            | Builtin::MathExp
            | Builtin::MathExpm1
            | Builtin::MathFloor
            | Builtin::MathFround
            | Builtin::MathLog
            | Builtin::MathLog1p
            | Builtin::MathLog10
            | Builtin::MathLog2
            | Builtin::MathRound
            | Builtin::MathSign
            | Builtin::MathSin
            | Builtin::MathSinh
            | Builtin::MathSqrt
            | Builtin::MathTan
            | Builtin::MathTanh
            | Builtin::MathTrunc => {
                let val = args
                    .first()
                    .with_context(|| format!("{builtin} expects 1 argument"))?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(val.0)));
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for builtin {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            // ── Math binary builtins (i64, i64) -> i64 ──
            Builtin::MathAtan2 | Builtin::MathImul | Builtin::MathPow => {
                let lhs = args
                    .first()
                    .with_context(|| format!("{builtin} expects 2 arguments"))?;
                let rhs = args
                    .get(1)
                    .with_context(|| format!("{builtin} expects 2 arguments"))?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(lhs.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(rhs.0)));
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for builtin {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            // ── Math.random: () -> i64 ──
            Builtin::MathRandom
            | Builtin::DateNow => {
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for builtin {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            // ── Math variadic builtins (shadow stack) ──
            Builtin::MathMax | Builtin::MathMin | Builtin::MathHypot => {
                self.compile_proto_method_call(dest, builtin, args)
            }
            // ── Number builtins ──
            Builtin::NumberConstructor
            | Builtin::NumberIsNaN
            | Builtin::NumberIsFinite
            | Builtin::NumberIsInteger
            | Builtin::NumberIsSafeInteger
            | Builtin::NumberParseFloat
            | Builtin::NumberProtoToString
            | Builtin::NumberProtoValueOf
            | Builtin::NumberProtoToFixed
            | Builtin::NumberProtoToExponential
            | Builtin::NumberProtoToPrecision => {
                let val = args
                    .first()
                    .with_context(|| format!("{builtin} expects at least 1 argument"))?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(val.0)));
                if let Some(second) = args.get(1) {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(second.0)));
                }
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for builtin {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::NumberParseInt => {
                let val = args
                    .first()
                    .with_context(|| "Number.parseInt expects at least 1 argument")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(val.0)));
                if let Some(second) = args.get(1) {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(second.0)));
                } else {
                    self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                }
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for builtin {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            // ── Boolean builtins ──
            Builtin::BooleanConstructor
            | Builtin::BooleanProtoToString
            | Builtin::BooleanProtoValueOf
            // ── Error builtins ──
            | Builtin::ErrorConstructor
            | Builtin::TypeErrorConstructor
            | Builtin::RangeErrorConstructor
            | Builtin::SyntaxErrorConstructor
            | Builtin::ReferenceErrorConstructor
            | Builtin::URIErrorConstructor
            | Builtin::EvalErrorConstructor
            | Builtin::ErrorProtoToString
            // ── Map single-arg builtins ──
            | Builtin::MapConstructor
            | Builtin::MapSetClear
            | Builtin::MapSetForEach
            | Builtin::MapSetKeys
            | Builtin::MapSetValues
            | Builtin::MapSetEntries
            // ── Set single-arg builtins ──
            | Builtin::SetConstructor
            // ── WeakMap single-arg builtins ──
            | Builtin::WeakMapConstructor
            // ── WeakSet single-arg builtins ──
            | Builtin::WeakSetConstructor
            // ── ArrayBuffer single-arg builtins ──
            | Builtin::ArrayBufferConstructor
            | Builtin::ArrayBufferProtoByteLength
            // ── TypedArray prototype single-arg builtins ──
            | Builtin::TypedArrayProtoLength
            | Builtin::TypedArrayProtoByteLength
            | Builtin::TypedArrayProtoByteOffset
            // ── Date single-arg builtins (not constructor) ──
            | Builtin::DateParse
            | Builtin::DateUTC => {
                let val = args
                    .first()
                    .with_context(|| format!("{builtin} expects at least 1 argument"))?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(val.0)));
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for builtin {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::ArrayIndexOf | Builtin::ArraySlice | Builtin::ArrayFill => {
                // 3+ arg functions: (i64, i64, i64) -> i64 etc
                for arg in args {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(arg.0)));
                }
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for builtin {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            // ── Map/Set multi-arg builtins ──
            Builtin::MapProtoSet
            | Builtin::MapProtoGet
            | Builtin::MapSetHas
            | Builtin::MapSetDelete
            | Builtin::SetProtoAdd
            // ── WeakMap multi-arg builtins ──
            | Builtin::WeakMapProtoSet
            | Builtin::WeakMapProtoGet
            | Builtin::WeakMapProtoHas
            | Builtin::WeakMapProtoDelete
            // ── WeakSet multi-arg builtins ──
            | Builtin::WeakSetProtoAdd
            | Builtin::WeakSetProtoHas
            | Builtin::WeakSetProtoDelete
            // ── ArrayBuffer multi-arg builtins ──
            | Builtin::ArrayBufferProtoSlice
            // ── DataView constructor ──
            | Builtin::DataViewConstructor
            // ── DataView get methods ──
            | Builtin::DataViewProtoGetFloat64
            | Builtin::DataViewProtoGetFloat32
            | Builtin::DataViewProtoGetInt32
            | Builtin::DataViewProtoGetUint32
            | Builtin::DataViewProtoGetInt16
            | Builtin::DataViewProtoGetUint16
            | Builtin::DataViewProtoGetInt8
            | Builtin::DataViewProtoGetUint8
            // ── DataView set methods ──
            | Builtin::DataViewProtoSetFloat64
            | Builtin::DataViewProtoSetFloat32
            | Builtin::DataViewProtoSetInt32
            | Builtin::DataViewProtoSetUint32
            | Builtin::DataViewProtoSetInt16
            | Builtin::DataViewProtoSetUint16
            | Builtin::DataViewProtoSetInt8
            | Builtin::DataViewProtoSetUint8
            // ── TypedArray constructors ──
            | Builtin::Int8ArrayConstructor
            | Builtin::Uint8ArrayConstructor
            | Builtin::Uint8ClampedArrayConstructor
            | Builtin::Int16ArrayConstructor
            | Builtin::Uint16ArrayConstructor
            | Builtin::Int32ArrayConstructor
            | Builtin::Uint32ArrayConstructor
            | Builtin::Float32ArrayConstructor
            | Builtin::Float64ArrayConstructor
            // ── TypedArray prototype multi-arg methods ──
            | Builtin::TypedArrayProtoSet
            | Builtin::TypedArrayProtoSlice
            | Builtin::TypedArrayProtoSubarray => {
                for arg in args {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(arg.0)));
                }
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for builtin {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            // ── Date constructor (shadow stack, variadic args) ──
            Builtin::DateConstructor => {
                self.compile_proto_method_call(dest, builtin, args)
            }
            // ── Array prototype method calls (Type 12 imports) ─────────────
            Builtin::ArrayShift
            | Builtin::ArraySort
            | Builtin::ArrayAt
            | Builtin::ArrayCopyWithin
            | Builtin::ArrayForEach
            | Builtin::ArrayMap
            | Builtin::ArrayFilter
            | Builtin::ArrayReduce
            | Builtin::ArrayReduceRight
            | Builtin::ArrayFind
            | Builtin::ArrayFindIndex
            | Builtin::ArraySome
            | Builtin::ArrayEvery
            | Builtin::ArrayFlatMap
            | Builtin::ArrayFlat
            | Builtin::ArraySpliceVa
            | Builtin::ArrayConcatVa
            | Builtin::ArrayUnshiftVa => self.compile_proto_method_call(dest, builtin, args),
            Builtin::ArrayIsArray => self.compile_proto_method_call(dest, builtin, args),
            Builtin::AbortShadowStackOverflow => {
                bail!("AbortShadowStackOverflow should not appear in compile_builtin_call");
            }
            Builtin::FuncCall | Builtin::FuncBind => {
                // These use shadow stack: compile like array proto methods
                self.compile_proto_method_call(dest, builtin, args)
            }
            Builtin::MapSetGetSize => {
                let val = args
                    .first()
                    .with_context(|| format!("{builtin} expects at least 1 argument"))?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(val.0)));
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .unwrap_or_else(|| panic!("no func idx for {builtin}"));
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::GetPrototypeFromConstructor => {
                let func_idx = self.get_proto_from_ctor_func_idx;
                self.emit(WasmInstruction::LocalGet(self.local_idx(args[0].0)));
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::ObjectRest | Builtin::FuncApply => {
                let func_idx = self.builtin_func_indices.get(builtin).copied().unwrap_or(0);
                for arg in args.iter() {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(arg.0)));
                }
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            // ── Object builtin methods ─────────────────────────────────
            Builtin::HasOwnProperty => {
                let obj_arg = args
                    .first()
                    .context("HasOwnProperty expects 2 args (obj, key)")?;
                let key_arg = args
                    .get(1)
                    .context("HasOwnProperty expects 2 args (obj, key)")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(obj_arg.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(key_arg.0)));
                self.emit(WasmInstruction::I32WrapI64);
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .unwrap_or(83);
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::PrivateGet => {
                let obj_arg = args
                    .first()
                    .context("PrivateGet expects 2 args (obj, key)")?;
                let key_arg = args
                    .get(1)
                    .context("PrivateGet expects 2 args (obj, key)")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(obj_arg.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(key_arg.0)));
                self.emit(WasmInstruction::I32WrapI64);
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .unwrap_or(313);
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::PrivateSet => {
                let obj_arg = args
                    .first()
                    .context("PrivateSet expects 3 args (obj, key, value)")?;
                let key_arg = args
                    .get(1)
                    .context("PrivateSet expects 3 args (obj, key, value)")?;
                let val_arg = args
                    .get(2)
                    .context("PrivateSet expects 3 args (obj, key, value)")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(obj_arg.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(key_arg.0)));
                self.emit(WasmInstruction::I32WrapI64);
                self.emit(WasmInstruction::LocalGet(self.local_idx(val_arg.0)));
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .unwrap_or(314);
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::PrivateHas => {
                let obj_arg = args
                    .first()
                    .context("PrivateHas expects 2 args (obj, key)")?;
                let key_arg = args
                    .get(1)
                    .context("PrivateHas expects 2 args (obj, key)")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(obj_arg.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(key_arg.0)));
                self.emit(WasmInstruction::I32WrapI64);
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .unwrap_or(315);
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::ObjectKeys
            | Builtin::ObjectValues
            | Builtin::ObjectEntries
            | Builtin::ObjectGetPrototypeOf
            | Builtin::ObjectGetOwnPropertyNames
            | Builtin::ObjectProtoToString
            | Builtin::ObjectProtoValueOf => {
                let val = args.first().context("Object method expects 1 arg")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(val.0)));
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for builtin {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::ObjectSetPrototypeOf | Builtin::ObjectIs => {
                let a = args.first().context("Object method expects 2 args")?;
                let b = args.get(1).context("Object method expects 2 args")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(a.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(b.0)));
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for builtin {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::ObjectCreate => {
                // Object.create(proto, properties?) → 第2个参数可省略
                let a = args.first().context("Object.create expects proto arg")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(a.0)));
                if args.len() >= 2 {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(args[1].0)));
                } else {
                    self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                }
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for builtin {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::ObjectAssign => {
                // Type 12 shadow stack: (env, target, args_base, args_count) -> i64
                let target = args.first().context("Object.assign expects target")?;
                // shadow_args = sources (args[1..])
                let shadow_args = &args[1..];
                // 保存 shadow_sp 基址
                self.emit(WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
                self.emit(WasmInstruction::LocalSet(self.shadow_sp_scratch_idx));
                // 影子栈边界检查
                self.emit_shadow_stack_overflow_check((shadow_args.len() * 8) as i32);
                // 将 shadow_args 写入影子栈
                for arg in shadow_args {
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
                // Type 12 call: env_obj=undefined, this_val=target, args_base, args_count
                self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                self.emit(WasmInstruction::LocalGet(self.local_idx(target.0)));
                self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
                self.emit(WasmInstruction::I32Const(shadow_args.len() as i32));
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .unwrap_or(87);
                self.emit(WasmInstruction::Call(func_idx));
                // 恢复 shadow_sp
                self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
                self.emit(WasmInstruction::GlobalSet(self.shadow_sp_global_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            // ── BigInt builtins ──
            Builtin::BigIntFromLiteral => {
                // Handled in compile_instruction (Const)
                bail!("BigIntFromLiteral should not reach compile_builtin_call");
            }
            Builtin::BigIntAdd
            | Builtin::BigIntSub
            | Builtin::BigIntMul
            | Builtin::BigIntDiv
            | Builtin::BigIntMod
            | Builtin::BigIntPow
            | Builtin::BigIntEq
            | Builtin::BigIntCmp => {
                let a = args.first().context("BigInt binary op expects 2 args")?;
                let b = args.get(1).context("BigInt binary op expects 2 args")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(a.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(b.0)));
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::BigIntNeg => {
                let a = args.first().context("BigIntNeg expects 1 arg")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(a.0)));
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            // ── Symbol builtins ──
            Builtin::SymbolCreate => {
                // Symbol(desc?) — desc 可选，缺省为 undefined
                if let Some(desc) = args.first() {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(desc.0)));
                } else {
                    self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                }
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::SymbolFor | Builtin::SymbolKeyFor => {
                let arg = args.first().context("Symbol for/keyFor expects 1 arg")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(arg.0)));
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::SymbolWellKnown => {
                let arg = args.first().context("SymbolWellKnown expects 1 arg")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(arg.0)));
                self.emit(WasmInstruction::F64ReinterpretI64);
                self.emit(WasmInstruction::I32TruncF64S);
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            // ── RegExp builtins ──
            Builtin::RegExpTest | Builtin::RegExpExec => {
                // regex.test(str) / regex.exec(str) - str 参数可选（默认 undefined）
                let regex = args.first().context("RegExp test/exec expects receiver")?;
                let str_arg = args.get(1);
                self.emit(WasmInstruction::LocalGet(self.local_idx(regex.0)));
                if let Some(s) = str_arg {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(s.0)));
                } else {
                    // 缺失参数默认为 undefined
                    self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                }
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            // ── RegExp internal builtin (not called directly from user code) ──
            Builtin::RegExpCreate => {
                bail!("RegExpCreate should only be called internally for RegExp literals")
            }
            // ── String prototype builtins (2-arg) ──
            Builtin::StringMatch | Builtin::StringSearch => {
                // str.match(regexp) / str.search(regexp) - regexp 参数可选（默认 undefined）
                let str_arg = args
                    .first()
                    .context("String match/search expects receiver")?;
                let regexp = args.get(1);
                self.emit(WasmInstruction::LocalGet(self.local_idx(str_arg.0)));
                if let Some(re) = regexp {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(re.0)));
                } else {
                    // 缺失参数默认为 undefined
                    self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                }
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            // ── String prototype builtins (3-arg) ──
            Builtin::StringReplace | Builtin::StringSplit => {
                // str.replace(search, replace) / str.split(sep, limit) - 3 args
                let str_arg = args
                    .first()
                    .context("String replace/split expects at least 2 arguments")?;
                let second = args
                    .get(1)
                    .context("String replace/split expects at least 2 arguments")?;
                // For StringSplit, limit is optional; for StringReplace, both are required
                let third = args.get(2);

                self.emit(WasmInstruction::LocalGet(self.local_idx(str_arg.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(second.0)));
                if let Some(third_arg) = third {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(third_arg.0)));
                } else {
                    // Push undefined as default for missing optional argument
                    self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                }
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            // ── Promise builtins ──
            Builtin::PromiseCreate => {
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for {builtin}"))?;
                self.emit(WasmInstruction::I64Const(0));
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::PromiseInstanceResolve | Builtin::PromiseInstanceReject => {
                let promise = args
                    .first()
                    .context("promise instance resolve/reject expects 2 args")?;
                let val = args
                    .get(1)
                    .context("promise instance resolve/reject expects 2 args")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(promise.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(val.0)));
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::PromiseThen => {
                let promise = args.first().context("promise.then expects 3 args")?;
                let on_fulfilled = args.get(1).context("promise.then expects 3 args")?;
                let on_rejected = args.get(2).context("promise.then expects 3 args")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(promise.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(on_fulfilled.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(on_rejected.0)));
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::PromiseCatch
            | Builtin::PromiseFinally
            | Builtin::PromiseResolveStatic
            | Builtin::PromiseRejectStatic
            | Builtin::PromiseAll
            | Builtin::PromiseRace
            | Builtin::PromiseAllSettled
            | Builtin::PromiseAny => {
                let promise = args
                    .first()
                    .context("promise catch/finally expects 2 args")?;
                let callback = args
                    .get(1)
                    .context("promise catch/finally expects 2 args")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(promise.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(callback.0)));
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::PromiseWithResolvers
            | Builtin::PromiseCreateResolveFunction
            | Builtin::PromiseCreateRejectFunction
            | Builtin::IsCallable
            | Builtin::IsPromise
            | Builtin::AsyncGeneratorStart => {
                let val = args.first().context("expects 1 arg")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(val.0)));
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::QueueMicrotask => {
                let callback = args.first().context("queue_microtask expects 1 arg")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(callback.0)));
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::DrainMicrotasks => {
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::AsyncFunctionStart => {
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for {builtin}"))?;
                for arg in args.iter().take(1) {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(arg.0)));
                }
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::AsyncFunctionResume => {
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for {builtin}"))?;
                for arg in args.iter().take(5) {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(arg.0)));
                }
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::AsyncFunctionSuspend => {
                bail!("AsyncFunctionSuspend should be handled in compile_instruction (Suspend)");
            }
            Builtin::ContinuationCreate => {
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for {builtin}"))?;
                for arg in args.iter().take(3) {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(arg.0)));
                }
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::ContinuationSaveVar => {
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for {builtin}"))?;
                for arg in args.iter().take(3) {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(arg.0)));
                }
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::ContinuationLoadVar => {
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for {builtin}"))?;
                for arg in args.iter().take(2) {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(arg.0)));
                }
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::AsyncGeneratorNext
            | Builtin::AsyncGeneratorReturn
            | Builtin::AsyncGeneratorThrow => {
                let generator = args
                    .first()
                    .context("async generator method expects 2 args")?;
                let val = args
                    .get(1)
                    .context("async generator method expects 2 args")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(generator.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(val.0)));
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            // ── 动态 import builtins ─────────────────────────────────────────────
            Builtin::RegisterModuleNamespace => {
                let module_id = args
                    .first()
                    .context("register_module_namespace expects 2 args")?;
                let namespace_obj = args
                    .get(1)
                    .context("register_module_namespace expects 2 args")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(module_id.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(namespace_obj.0)));
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                Ok(())
            }
            Builtin::DynamicImport => {
                let module_id = args.first().context("dynamic_import expects 1 arg")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(module_id.0)));
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            // ── JSX builtins ───────────────────────────────────────────────────
            Builtin::JsxCreateElement => {
                // jsx_create_element(tag: i64, props: i64, children: i64) -> i64
                let a_tag = args.first().context("JsxCreateElement expects tag arg")?;
                let a_props = args.get(1).context("JsxCreateElement expects props arg")?;
                let a_children = args.get(2).context("JsxCreateElement expects children arg")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(a_tag.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(a_props.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(a_children.0)));
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for builtin {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            // ── String prototype builtins (1-arg receiver) ──
            Builtin::StringCharAt
            | Builtin::StringCharCodeAt
            | Builtin::StringCodePointAt
            | Builtin::StringToLowerCase
            | Builtin::StringToUpperCase
            | Builtin::StringTrim
            | Builtin::StringTrimEnd
            | Builtin::StringTrimStart
            | Builtin::StringToString
            | Builtin::StringValueOf
            | Builtin::StringIterator => {
                let receiver = args.first().context("String method expects receiver")?;
                let second = args.get(1);
                self.emit(WasmInstruction::LocalGet(self.local_idx(receiver.0)));
                if let Some(arg) = second {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(arg.0)));
                }
                let func_idx = self.builtin_func_indices.get(builtin).copied()
                    .with_context(|| format!("no WASM func index for {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            // ── String prototype builtins (receiver + 1 arg) ──
            Builtin::StringRepeat
            | Builtin::StringAt => {
                let receiver = args.first().context("String method expects receiver")?;
                let first = args.get(1).context("String method expects argument")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(receiver.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(first.0)));
                let func_idx = self.builtin_func_indices.get(builtin).copied()
                    .with_context(|| format!("no WASM func index for {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            // ── String prototype builtins (receiver + 2 args, second optional) ──
            Builtin::StringEndsWith
            | Builtin::StringIncludes
            | Builtin::StringIndexOf
            | Builtin::StringLastIndexOf
            | Builtin::StringPadEnd
            | Builtin::StringPadStart
            | Builtin::StringReplaceAll
            | Builtin::StringSlice
            | Builtin::StringStartsWith
            | Builtin::StringSubstring => {
                let receiver = args.first().context("String method expects receiver")?;
                let first = args.get(1).context("String method expects argument")?;
                let second = args.get(2);
                self.emit(WasmInstruction::LocalGet(self.local_idx(receiver.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(first.0)));
                if let Some(arg) = second {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(arg.0)));
                } else {
                    self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                }
                let func_idx = self.builtin_func_indices.get(builtin).copied()
                    .with_context(|| format!("no WASM func index for {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            // ── String varargs builtins (shadow stack) ──
            Builtin::StringConcatVa
            | Builtin::StringFromCharCode
            | Builtin::StringFromCodePoint
            | Builtin::StringMatchAll => {
                self.compile_proto_method_call(dest, builtin, args)
            }
            // ── Proxy / Reflect builtins ──────────────────────────────────────────
            Builtin::ProxyCreate
            | Builtin::ProxyRevocable
            | Builtin::ReflectGet
            | Builtin::ReflectSet
            | Builtin::ReflectHas
            | Builtin::ReflectDeleteProperty
            | Builtin::ReflectApply
            | Builtin::ReflectGetPrototypeOf
            | Builtin::ReflectSetPrototypeOf
            | Builtin::ReflectIsExtensible
            | Builtin::ReflectPreventExtensions
            | Builtin::ReflectGetOwnPropertyDescriptor
            | Builtin::ReflectDefineProperty
            | Builtin::ReflectOwnKeys => {
                for arg in args {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(arg.0)));
                }
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for builtin {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::ReflectConstruct => {
                for arg in args {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(arg.0)));
                }
                if args.len() < 3 {
                    if let Some(target) = args.first() {
                        self.emit(WasmInstruction::LocalGet(self.local_idx(target.0)));
                    } else {
                        self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                    }
                    if args.len() < 2 {
                        self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                    }
                }
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for builtin {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
            Builtin::GetBuiltinGlobal => {
                let name_val = args.first().context("GetBuiltinGlobal expects 1 arg")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(name_val.0)));
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .with_context(|| format!("no WASM func index for builtin {builtin}"))?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(())
            }
        }
    }

}
