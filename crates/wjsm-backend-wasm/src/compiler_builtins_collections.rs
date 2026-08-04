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
                // Step 3：fast path 双 plain f64 → 原生 f64.eq（== 对两个 number 即数值相等）；
                // slow path 内 spill（可能 ToPrimitive/分配）→ host abstract_eq。
                let lhs = args.first().context("AbstractEq expects 2 args")?;
                let rhs = args.get(1).context("AbstractEq expects 2 args")?;
                let lhs_l = self.local_idx(lhs.0);
                let rhs_l = self.local_idx(rhs.0);
                self.emit_compare_fast_slow(lhs_l, rhs_l, WasmInstruction::F64Eq, Builtin::AbstractEq)?;
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::StrictEq => {
                // strict_eq(a, b) -> bool
                // Step 3：fast path 双 plain f64 → 原生 f64.eq（0 === -0 true、NaN false）；
                // slow path 内 spill → host strict_eq。
                let lhs = args.first().context("StrictEq expects 2 args")?;
                let rhs = args.get(1).context("StrictEq expects 2 args")?;
                let lhs_l = self.local_idx(lhs.0);
                let rhs_l = self.local_idx(rhs.0);
                self.emit_compare_fast_slow(lhs_l, rhs_l, WasmInstruction::F64Eq, Builtin::StrictEq)?;
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::AbstractCompare => {
                // abstract_compare(a, b) -> bool (a < b)
                // 双 f64 → 原生 f64.lt（NaN → false，符合 ECMAScript 关系比较）；否则走 host。
                let lhs = args.first().context("AbstractCompare expects 2 args")?;
                let rhs = args.get(1).context("AbstractCompare expects 2 args")?;
                let lhs_l = self.local_idx(lhs.0);
                let rhs_l = self.local_idx(rhs.0);
                // Step 2d：双已知 f64 → 直接 f64.lt + bool box（无类型检查、无 host 调用、无 GC）。
                if self.value_known_f64(*lhs) && self.value_known_f64(*rhs) {
                    self.emit(WasmInstruction::LocalGet(lhs_l));
                    self.emit(WasmInstruction::F64ReinterpretI64);
                    self.emit(WasmInstruction::LocalGet(rhs_l));
                    self.emit(WasmInstruction::F64ReinterpretI64);
                    self.emit(WasmInstruction::F64Lt);
                    self.emit(WasmInstruction::I64ExtendI32U);
                    // truthiness-only（仅 branch 条件消费）：值存裸 0/1（i64），
                    // 消费端 emit_condition_to_bool_i32 直接 wrap——省去 box 构造。
                    // 否则 or 上 boxed bool 前缀（encode_bool，对所有消费者 sound）。
                    if dest.is_none_or(|d| !self.value_truthiness_only(d)) {
                        let box_base = value::BOX_BASE as i64;
                        let bool_tag_shifted = (value::TAG_BOOL as i64) << 32;
                        self.emit(WasmInstruction::I64Const(box_base | bool_tag_shifted));
                        self.emit(WasmInstruction::I64Or);
                    }
                } else {
                    // Step 3：类型未知 → fast/slow 分离——fast path 双 f64 无 host call/GC
                    // 不 spill；slow path（可能 ToPrimitive）内 spill 后 host 调用。
                    self.emit_compare_fast_slow(
                        lhs_l,
                        rhs_l,
                        WasmInstruction::F64Lt,
                        Builtin::AbstractCompare,
                    )?;
                }
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
            // ── Math.random / Date.now / performance.now: () -> i64 ──
            Builtin::MathRandom
            | Builtin::DateNow
            | Builtin::PerformanceNow
            | Builtin::AtomicsPause => {
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::MathMaxArray => {
                let array = args
                    .first()
                    .with_context(|| "Math.max array entry expects one array argument")?;
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit_value_args(std::slice::from_ref(array));
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(BuiltinDispatch::Handled)
            }
            // ── Math.hypot 固定二参：寄存器直传，免变参 shadow stack 打包/解包 ──
            Builtin::MathHypot if args.len() == 2 => {
                self.emit(WasmInstruction::LocalGet(self.local_idx(args[0].0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(args[1].0)));
                let func_idx = self.special_host_import_indices[&SpecialHostImport::MathHypot2];
                self.emit(WasmInstruction::Call(func_idx));
                self.store_or_drop_call_result(dest);
                Ok(BuiltinDispatch::Handled)
            }
            // ── Math variadic builtins (shadow stack) ──
            Builtin::MathMax | Builtin::MathMin | Builtin::MathHypot => self
                .compile_proto_method_call(dest, builtin, args)
                .map(|_| BuiltinDispatch::Handled),
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
            | Builtin::MapSetFirstKey
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
            // ── WeakMap multi-arg builtins ──
            | Builtin::WeakMapProtoSet
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
            // ── Map/Set 2-arg builtins（this, key/value；宿主签名为 2 参直传，不补 undefined）──
            Builtin::MapProtoGet
            | Builtin::MapSetHas
            | Builtin::MapSetDelete
            | Builtin::SetProtoAdd
            | Builtin::SetProtoHas
            | Builtin::SetProtoDelete
            // ── WeakMap/WeakSet 2-arg builtins ──
            | Builtin::WeakMapProtoGet
            | Builtin::WeakMapProtoHas
            | Builtin::WeakMapProtoDelete
            | Builtin::WeakSetProtoAdd
            | Builtin::WeakSetProtoHas
            | Builtin::WeakSetProtoDelete => {
                for arg in args.iter().take(2) {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(arg.0)));
                }
                for _ in args.len()..2 {
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

    /// Step 3：比较类 builtin（AbstractCompare/StrictEq/AbstractEq）的 fast/slow 分派。
    /// 结果（encode_bool i64）留在栈上。
    ///
    /// - fast path：双 plain f64 → 原生 f64 比较 + bool box——无 host call、无 GC → 不 spill。
    /// - slow path：可能 ToPrimitive/分配（字符串/对象比较）→ 在 slow path 内
    ///   spill live handles（同一指令位置的 liveness，静态不变）后 host 调用。
    fn emit_compare_fast_slow(
        &mut self,
        lhs_l: u32,
        rhs_l: u32,
        f64_op: WasmInstruction,
        host_builtin: Builtin,
    ) -> Result<()> {
        let box_base = value::BOX_BASE as i64;
        // is_f64(lhs) && is_f64(rhs)
        self.emit(WasmInstruction::LocalGet(lhs_l));
        self.emit(WasmInstruction::I64Const(box_base));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(box_base));
        self.emit(WasmInstruction::I64Ne);
        self.emit(WasmInstruction::LocalGet(rhs_l));
        self.emit(WasmInstruction::I64Const(box_base));
        self.emit(WasmInstruction::I64And);
        self.emit(WasmInstruction::I64Const(box_base));
        self.emit(WasmInstruction::I64Ne);
        self.emit(WasmInstruction::I32And);
        self.emit(WasmInstruction::If(BlockType::Result(ValType::I64)));
        // 双 f64：原生比较，结果包装为 encode_bool（i64）
        self.emit(WasmInstruction::LocalGet(lhs_l));
        self.emit(WasmInstruction::F64ReinterpretI64);
        self.emit(WasmInstruction::LocalGet(rhs_l));
        self.emit(WasmInstruction::F64ReinterpretI64);
        self.emit(f64_op);
        self.emit(WasmInstruction::If(BlockType::Result(ValType::I64)));
        self.emit(WasmInstruction::I64Const(value::encode_bool(true)));
        self.emit(WasmInstruction::Else);
        self.emit(WasmInstruction::I64Const(value::encode_bool(false)));
        self.emit(WasmInstruction::End);
        self.emit(WasmInstruction::Else);
        // slow path：spill → host call → 恢复 spill。
        let spill = self.current_spill_locals();
        self.emit_safepoint_spill_prologue(&spill);
        self.emit(WasmInstruction::LocalGet(lhs_l));
        self.emit(WasmInstruction::LocalGet(rhs_l));
        let func_idx = self.builtin_func_idx(&host_builtin)?;
        self.emit(WasmInstruction::Call(func_idx));
        self.emit_safepoint_spill_epilogue(spill.len());
        self.emit(WasmInstruction::End);
        Ok(())
    }
}
