//! builtin 编译：IteratorDone ~ TypedArrayProtoLastIndexOf

use super::*;
use crate::compiler_builtins::BuiltinDispatch;
use crate::host_import_registry::SpecialHostImport;

impl Compiler {
    /// 处理 IteratorDone ~ TypedArrayProtoLastIndexOf 等 builtin。
    pub(crate) fn compile_builtin_collections(
        &mut self,
        dest: Option<ValueId>,
        builtin: &Builtin,
        args: &[ValueId],
    ) -> Result<BuiltinDispatch> {
        match builtin {
            Builtin::IteratorDone | Builtin::EnumeratorDone => {
                let handle = args
                    .first()
                    .context("IteratorDone/EnumeratorDone expects 1 arg")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(handle.0)));
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::TypeOf => {
                // typeof(value) -> 返回类型名称字符串指针
                let val = args.first().context("TypeOf expects 1 arg")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(val.0)));
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::In => {
                // prop in object -> bool
                let object = args.first().context("In expects 2 args (object, prop)")?;
                let prop = args.get(1).context("In expects 2 args (object, prop)")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(object.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(prop.0)));
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(BuiltinDispatch::Handled)
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
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::AbstractEq => {
                // abstract_eq(a, b) -> bool
                let lhs = args.first().context("AbstractEq expects 2 args")?;
                let rhs = args.get(1).context("AbstractEq expects 2 args")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(lhs.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(rhs.0)));
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::StrictEq => {
                let lhs = args.first().context("StrictEq expects 2 args")?;
                let rhs = args.get(1).context("StrictEq expects 2 args")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(lhs.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(rhs.0)));
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::AbstractCompare => {
                // abstract_compare(a, b) -> bool (a < b)
                let lhs = args.first().context("AbstractCompare expects 2 args")?;
                let rhs = args.get(1).context("AbstractCompare expects 2 args")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(lhs.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(rhs.0)));
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::DefineProperty => {
                // define_property(obj: i64, key: i32, desc: i64) -> i64
                // 成功返回该对象，失败返回可捕获 TAG_EXCEPTION（由语句级 IsException 分叉抛出）。
                let obj_arg = args.first().context("DefineProperty expects 3 args")?;
                let key_arg = args.get(1).context("DefineProperty expects 3 args")?;
                let desc_arg = args.get(2).context("DefineProperty expects 3 args")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(obj_arg.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(key_arg.0)));
                self.emit(WasmInstruction::Call(
                    self.special_host_import_indices[&SpecialHostImport::SymbolPropertyKey],
                ));
                self.emit(WasmInstruction::LocalGet(self.local_idx(desc_arg.0)));
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                self.store_or_drop_call_result(dest);
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::GetOwnPropDesc => {
                // get_own_prop_desc(obj: i64, key: i64) -> i64
                let obj_arg = args.first().context("GetOwnPropDesc expects 2 args")?;
                let key_arg = args.get(1).context("GetOwnPropDesc expects 2 args")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(obj_arg.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(key_arg.0)));
                self.emit(WasmInstruction::Call(
                    self.special_host_import_indices[&SpecialHostImport::SymbolPropertyKey],
                ));
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(BuiltinDispatch::Handled)
            }
            // ── Array method builtins ─────────────────────────────────────
            Builtin::ArrayPush
            | Builtin::ArrayPushHole
            | Builtin::ArrayPushSpread
            | Builtin::ArrayPop
            | Builtin::ArrayIncludes
            | Builtin::ArrayJoin
            | Builtin::ArrayConcat
            | Builtin::ArrayReverse
            | Builtin::ArrayInitLength
            | Builtin::ArrayGetLength => {
                self.emit_value_args(args);
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                self.store_or_drop_call_result(dest);
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::ArrayIndexOf | Builtin::ArraySlice => {
                for arg in args.iter().take(3) {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(arg.0)));
                }
                for _ in args.len()..3 {
                    self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                }
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::ArrayFill => {
                for arg in args.iter().take(4) {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(arg.0)));
                }
                for _ in args.len()..4 {
                    self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                }
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(BuiltinDispatch::Handled)
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
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(BuiltinDispatch::Handled)
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
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(BuiltinDispatch::Handled)
            }
            // ── Math.random: () -> i64 ──
            Builtin::MathRandom
            | Builtin::DateNow
            | Builtin::AtomicsPause => {
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(BuiltinDispatch::Handled)
            }
            // ── Math variadic builtins (shadow stack) ──
            Builtin::MathMax | Builtin::MathMin | Builtin::MathHypot => {
                self.compile_proto_method_call(dest, builtin, args).map(|_| BuiltinDispatch::Handled)
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
                } else if matches!(
                    builtin,
                    Builtin::NumberProtoToString
                        | Builtin::NumberProtoToFixed
                        | Builtin::NumberProtoToExponential
                        | Builtin::NumberProtoToPrecision
                ) {
                    self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                }
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(BuiltinDispatch::Handled)
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
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(BuiltinDispatch::Handled)
            }
            // ── Error constructors: Type 2 (i64, i64) -> i64 — 接受 (message, options) ──
            // options 用于 ES2022 Error.cause；缺失时补 undefined。
            Builtin::ErrorConstructor
            | Builtin::TypeErrorConstructor
            | Builtin::RangeErrorConstructor
            | Builtin::SyntaxErrorConstructor
            | Builtin::ReferenceErrorConstructor
            | Builtin::URIErrorConstructor
            | Builtin::EvalErrorConstructor => {
                let val = args
                    .first()
                    .with_context(|| format!("{builtin} expects at least 1 argument"))?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(val.0)));
                // 第二参数 options（缺失时补 undefined）
                if args.len() >= 2 {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(args[1].0)));
                } else {
                    self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                }
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(BuiltinDispatch::Handled)
            }
            // ── Boolean + remaining single-arg builtins ──
            Builtin::BooleanConstructor
            | Builtin::BooleanProtoToString
            | Builtin::BooleanProtoValueOf
            | Builtin::ErrorProtoToString
            // ── Map single-arg builtins ──
            | Builtin::MapConstructor
            | Builtin::MapSetClear
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
            // ── SharedArrayBuffer builtins ──
            | Builtin::SharedArrayBufferProtoByteLength
            | Builtin::SharedArrayBufferProtoGrowable
            | Builtin::SharedArrayBufferProtoMaxByteLength
            | Builtin::SharedArrayBufferSpecies
            // ── Atomics single-arg builtins ──
            | Builtin::AtomicsIsLockFree
            // ── TypedArray prototype single-arg builtins ──
            | Builtin::TypedArrayProtoLength
            | Builtin::TypedArrayProtoByteLength
            | Builtin::TypedArrayProtoByteOffset
            // ── Date single-arg builtins (not constructor) ──
            | Builtin::DateParse => {
                let val = args
                    .first()
                    .with_context(|| format!("{builtin} expects at least 1 argument"))?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(val.0)));
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(BuiltinDispatch::Handled)
            }
            // Date.UTC / MapSet.forEach：多参数经影子栈。
            Builtin::DateUTC | Builtin::MapSetForEach => {
                self.compile_proto_method_call(dest, builtin, args).map(|_| BuiltinDispatch::Handled)
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
            // ── SharedArrayBuffer builtins (2-arg / 3-arg) ──
            | Builtin::SharedArrayBufferProtoSlice
            // ── Atomics multi-arg builtins (2 args padded to 3) ──
            | Builtin::AtomicsLoad
            // ── Atomics multi-arg builtins (3 args) ──
            | Builtin::AtomicsStore
            | Builtin::AtomicsAdd
            | Builtin::AtomicsSub
            | Builtin::AtomicsAnd
            | Builtin::AtomicsOr
            | Builtin::AtomicsXor
            | Builtin::AtomicsExchange
            | Builtin::AtomicsNotify
            // ── DataView constructor ──
            | Builtin::SharedArrayBufferConstructor
            | Builtin::DataViewConstructor
            // ── DataView set methods ──
            | Builtin::DataViewProtoSetFloat64
            | Builtin::DataViewProtoSetFloat32
            | Builtin::DataViewProtoSetInt32
            | Builtin::DataViewProtoSetUint32
            | Builtin::DataViewProtoSetInt16
            | Builtin::DataViewProtoSetUint16
            | Builtin::DataViewProtoSetInt8
            | Builtin::DataViewProtoSetUint8
            // ── TypedArray 新增构造器 ──
            | Builtin::BigInt64ArrayConstructor
            | Builtin::BigUint64ArrayConstructor
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
            | Builtin::TypedArrayProtoSubarray
            // ── TypedArray 新增原型方法: Type 16 (3-arg: this, arg1, fromIndex) ──
            // indexOf/lastIndexOf/includes 的第三个参数是可选的，缺省时用 undefined 填充。
            | Builtin::TypedArrayProtoIndexOf
            | Builtin::TypedArrayProtoLastIndexOf
            | Builtin::TypedArrayProtoIncludes => {
                for arg in args.iter().take(3) {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(arg.0)));
                }
                for _ in args.len()..3 {
                    self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                }
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(BuiltinDispatch::Handled)
            }
            // ── Atomics 4-arg builtins (compareExchange, wait, waitAsync) ──
            _ => Ok(BuiltinDispatch::NotHandled),
        }
    }
}
