//! builtin 编译：AtomicsCompareExchange ~ SymbolCreate

use super::*;

impl Compiler {
    /// 处理 AtomicsCompareExchange ~ SymbolCreate 等 builtin。
    pub(crate) fn compile_builtin_string_math(
        &mut self,
        dest: Option<ValueId>,
        builtin: &Builtin,
        args: &[ValueId],
    ) -> Result<Option<()>> {
        match builtin {
            Builtin::AtomicsCompareExchange
            | Builtin::AtomicsWait
            | Builtin::AtomicsWaitAsync => {
                for arg in args.iter().take(4) {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(arg.0)));
                }
                for _ in args.len()..4 {
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
                Ok(Some(()))
            }
            // ── SharedArrayBuffer 2-arg builtins ──
            Builtin::SharedArrayBufferProtoGrow => {
                for arg in args.iter().take(2) {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(arg.0)));
                }
                for _ in args.len()..2 {
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
                Ok(Some(()))
            }
            // ── DataView get methods: Type 2 (this, byteOffset) ──
            Builtin::DataViewProtoGetFloat64
            | Builtin::DataViewProtoGetFloat32
            | Builtin::DataViewProtoGetInt32
            | Builtin::DataViewProtoGetUint32
            | Builtin::DataViewProtoGetInt16
            | Builtin::DataViewProtoGetUint16
            | Builtin::DataViewProtoGetInt8
            | Builtin::DataViewProtoGetUint8
            // ── TypedArray 新增原型方法: Type 2 (2-arg: this, arg1) ──
            // join 的 separator 参数是可选的，缺省时用 undefined 填充。
            | Builtin::TypedArrayProtoJoin
            | Builtin::TypedArrayProtoAt => {
                for arg in args.iter().take(2) {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(arg.0)));
                }
                for _ in args.len()..2 {
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
                Ok(Some(()))
            }
            // ── TypedArray 新增原型方法: Type 17 (4-arg) ──
            Builtin::TypedArrayProtoCopyWithin
            | Builtin::TypedArrayProtoFill => {
                // fill 最多 4 个参数 (this, value, start, end)，缺失的用 undefined 填充
                for arg in args.iter().take(4) {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(arg.0)));
                }
                for _ in args.len()..4 {
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
                Ok(Some(()))
            }
            // ── TypedArray 新增原型方法: Type 3 (1-arg) ──
            Builtin::TypedArrayProtoReverse
            | Builtin::TypedArrayProtoToString
            | Builtin::TypedArrayProtoEntries
            | Builtin::TypedArrayProtoKeys
            | Builtin::TypedArrayProtoValues => {
                let val = args.first().with_context(|| format!("{builtin} expects 1 argument"))?;
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
                Ok(Some(()))
            }
            // ── TypedArray 新增原型方法: Type 12 shadow stack (回调方法) ──
            Builtin::TypedArrayProtoForEach
            | Builtin::TypedArrayProtoMap
            | Builtin::TypedArrayProtoFilter
            | Builtin::TypedArrayProtoReduce
            | Builtin::TypedArrayProtoReduceRight
            | Builtin::TypedArrayProtoFind
            | Builtin::TypedArrayProtoFindIndex
            | Builtin::TypedArrayProtoSome
            | Builtin::TypedArrayProtoEvery
            | Builtin::TypedArrayProtoSort => self.compile_proto_method_call(dest, builtin, args).map(Some),
            // ── Array prototype method calls (Type 12 imports) ─────────────
            Builtin::ArrayShift
            | Builtin::ArrayUnshiftVa
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
            | Builtin::ArraySpliceVa
            | Builtin::ArrayConcatVa
            | Builtin::ArrayFlat
            | Builtin::ArrayIsArray
            | Builtin::ArrayFrom
            | Builtin::DateConstructor => self.compile_proto_method_call(dest, builtin, args).map(Some),
            Builtin::DateConstructorNew => {
                self.compile_date_constructor_new(dest, args).map(Some)
            }
            Builtin::AbortShadowStackOverflow => {
                bail!("AbortShadowStackOverflow should not appear in compile_builtin_call");
            }
            Builtin::FuncCall | Builtin::FuncBind => {
                // These use shadow stack: compile like array proto methods
                self.compile_proto_method_call(dest, builtin, args).map(Some)
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
                Ok(Some(()))
            }
            Builtin::GetPrototypeFromConstructor => {
                let func_idx = self.get_proto_from_ctor_func_idx;
                self.emit(WasmInstruction::LocalGet(self.local_idx(args[0].0)));
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(Some(()))
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
                Ok(Some(()))
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
                Ok(Some(()))
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
                    .expect("builtin import must be registered");
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(Some(()))
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
                Ok(Some(()))
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
                Ok(Some(()))
            }
            Builtin::PrivateAccessorBind => {
                let obj_arg = args
                    .first()
                    .context("PrivateAccessorBind expects 4 args (obj, key, get, set)")?;
                let key_arg = args
                    .get(1)
                    .context("PrivateAccessorBind expects 4 args (obj, key, get, set)")?;
                let get_arg = args
                    .get(2)
                    .context("PrivateAccessorBind expects 4 args (obj, key, get, set)")?;
                let set_arg = args
                    .get(3)
                    .context("PrivateAccessorBind expects 4 args (obj, key, get, set)")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(obj_arg.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(key_arg.0)));
                self.emit(WasmInstruction::I32WrapI64);
                self.emit(WasmInstruction::LocalGet(self.local_idx(get_arg.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(set_arg.0)));
                let func_idx = self
                    .builtin_func_indices
                    .get(builtin)
                    .copied()
                    .expect("builtin import must be registered");
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(Some(()))
            }
            Builtin::ObjectKeys
            | Builtin::ObjectValues
            | Builtin::ObjectEntries
            | Builtin::ObjectFromEntries
            | Builtin::ObjectGetPrototypeOf
            | Builtin::ObjectGetOwnPropertyNames
            | Builtin::ObjectGetOwnPropertySymbols
            | Builtin::ObjectGetOwnPropertyDescriptors
            | Builtin::ObjectFreeze
            | Builtin::ObjectSeal
            | Builtin::ObjectIsFrozen
            | Builtin::ObjectIsSealed
            | Builtin::ObjectIsExtensible
            | Builtin::ObjectPreventExtensions
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
                Ok(Some(()))
            }
            Builtin::ObjectHasOwn
            | Builtin::ObjectSetPrototypeOf
            | Builtin::ObjectIs
            | Builtin::ObjectGroupBy
            | Builtin::ObjectDefineProperties
            | Builtin::MapGroupBy => {
                let name = builtin.to_string();
                let a = args.first().context(format!("{name} expects 2 args"))?;
                let b = args.get(1).context(format!("{name} expects 2 args"))?;
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
                Ok(Some(()))
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
                Ok(Some(()))
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
                Ok(Some(()))
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
            | Builtin::BigIntBitAnd
            | Builtin::BigIntBitOr
            | Builtin::BigIntBitXor
            | Builtin::BigIntShl
            | Builtin::BigIntShr
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
                Ok(Some(()))
            }
            Builtin::BigIntNeg | Builtin::BigIntBitNot => {
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
                Ok(Some(()))
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
                Ok(Some(()))
            }
            _ => Ok(None),
        }
    }
}
