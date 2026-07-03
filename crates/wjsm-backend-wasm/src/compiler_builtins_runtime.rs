//! builtin 编译：StringEndsWith ~ FinalizationRegistryProtoUnregister

use super::*;
use crate::compiler_builtins::BuiltinDispatch;

impl Compiler {
    /// 处理 StringEndsWith ~ FinalizationRegistryProtoUnregister 等 builtin。
    pub(crate) fn compile_builtin_runtime(
        &mut self,
        dest: Option<ValueId>,
        builtin: &Builtin,
        args: &[ValueId],
    ) -> Result<BuiltinDispatch> {
        match builtin {
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
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::StringNormalize => {
                let receiver = args.first().context("String method expects receiver")?;
                let form = args.get(1).copied();
                self.emit(WasmInstruction::LocalGet(self.local_idx(receiver.0)));
                if let Some(arg) = form {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(arg.0)));
                } else {
                    self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                }
                self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(BuiltinDispatch::Handled)
            }
            // ── String varargs builtins (shadow stack) ──
            Builtin::StringConcatVa
            | Builtin::StringFromCharCode
            | Builtin::StringFromCodePoint
            | Builtin::StringMatchAll => self
                .compile_proto_method_call(dest, builtin, args)
                .map(|_| BuiltinDispatch::Handled),
            // ── Proxy / Reflect builtins ──────────────────────────────────────────
            Builtin::ProxyCreate
            | Builtin::ProxyRevocable
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
                self.emit_value_args(args);
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(BuiltinDispatch::Handled)
            }
            // Reflect.get(target, prop[, receiver]) — 省略 receiver 时按规范使用 target。
            Builtin::ReflectGet => {
                if let Some(target) = args.first() {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(target.0)));
                } else {
                    self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                }
                if let Some(property_key) = args.get(1) {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(property_key.0)));
                } else {
                    self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                }
                if let Some(receiver) = args.get(2).or_else(|| args.first()) {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(receiver.0)));
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
            // Reflect.set(target, prop, val[, receiver]) — 省略 receiver 时按规范使用 target（与 ReflectGet 一致）。
            Builtin::ReflectSet => {
                if let Some(target) = args.first() {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(target.0)));
                } else {
                    self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                }
                if let Some(property_key) = args.get(1) {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(property_key.0)));
                } else {
                    self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                }
                if let Some(value_arg) = args.get(2) {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(value_arg.0)));
                } else {
                    self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                }
                if let Some(receiver) = args.get(3).or_else(|| args.first()) {
                    self.emit(WasmInstruction::LocalGet(self.local_idx(receiver.0)));
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
            Builtin::ReflectConstruct => {
                self.emit_value_args(args);
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
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::GetBuiltinGlobal => {
                let name_val = args.first().context("GetBuiltinGlobal expects 1 arg")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(name_val.0)));
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::CreateGlobalObject => {
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::CreateException => {
                let value = args.first().context("CreateException expects 1 arg")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(value.0)));
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::ExceptionValue => {
                let handle = args.first().context("ExceptionValue expects 1 arg")?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(handle.0)));
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::CreateUnmappedArgumentsObject => {
                let args_array = args.first().with_context(|| {
                    format!("{builtin} expects 2 arguments (args_array, param_count)")
                })?;
                let param_count = args.get(1).with_context(|| {
                    format!("{builtin} expects 2 arguments (args_array, param_count)")
                })?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(args_array.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(param_count.0)));
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::CreateMappedArgumentsObject => {
                let args_array = args.first().with_context(|| {
                    format!("{builtin} expects 3 arguments (args_array, param_count, func_ref)")
                })?;
                let param_count = args.get(1).with_context(|| {
                    format!("{builtin} expects 3 arguments (args_array, param_count, func_ref)")
                })?;
                let func_ref = args.get(2).with_context(|| {
                    format!("{builtin} expects 3 arguments (args_array, param_count, func_ref)")
                })?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(args_array.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(param_count.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(func_ref.0)));
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(BuiltinDispatch::Handled)
            }
            // ── WeakRef / FinalizationRegistry builtins ──
            Builtin::WeakRefConstructor
            | Builtin::HeadersConstructor
            | Builtin::RequestConstructor
            | Builtin::ResponseConstructor
            | Builtin::AbortControllerConstructor
            | Builtin::ReadableStreamConstructor
            | Builtin::WritableStreamConstructor
            | Builtin::TransformStreamConstructor
            | Builtin::CountQueuingStrategyConstructor
            | Builtin::ByteLengthQueuingStrategyConstructor => {
                self.emit(WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
                self.emit(WasmInstruction::LocalSet(self.shadow_sp_scratch_idx));
                self.emit_shadow_stack_overflow_check((args.len() * 8) as i32);
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
                self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
                self.emit(WasmInstruction::I32Const(args.len() as i32));
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
                self.emit(WasmInstruction::GlobalSet(self.shadow_sp_global_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::WeakRefProtoDeref => {
                // Type 3: direct call (this_val) -> i64
                let this_val = args
                    .first()
                    .with_context(|| format!("{builtin} expects 1 argument (this_val)"))?;
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(this_val.0)));
                self.emit(WasmInstruction::Call(func_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::FinalizationRegistryConstructor => {
                // Type 12: constructor - env=undefined, this=undefined, shadow_args=all args
                self.emit(WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
                self.emit(WasmInstruction::LocalSet(self.shadow_sp_scratch_idx));
                self.emit_shadow_stack_overflow_check((args.len() * 8) as i32);
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
                self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
                self.emit(WasmInstruction::I32Const(args.len() as i32));
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
                self.emit(WasmInstruction::GlobalSet(self.shadow_sp_global_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::FinalizationRegistryProtoRegister => {
                // Type 12: method - env=undefined, this=args[0], shadow_args=args[1..]
                let this_val = args
                    .first()
                    .with_context(|| format!("{builtin} expects 4 arguments"))?;
                let shadow_args = &args[1..];
                self.emit(WasmInstruction::GlobalGet(self.shadow_sp_global_idx));
                self.emit(WasmInstruction::LocalSet(self.shadow_sp_scratch_idx));
                self.emit_shadow_stack_overflow_check((shadow_args.len() * 8) as i32);
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
                self.emit(WasmInstruction::I64Const(value::encode_undefined()));
                self.emit(WasmInstruction::LocalGet(self.local_idx(this_val.0)));
                self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
                self.emit(WasmInstruction::I32Const(shadow_args.len() as i32));
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::Call(func_idx));
                self.emit(WasmInstruction::LocalGet(self.shadow_sp_scratch_idx));
                self.emit(WasmInstruction::GlobalSet(self.shadow_sp_global_idx));
                if let Some(d) = dest {
                    self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
                }
                Ok(BuiltinDispatch::Handled)
            }
            Builtin::FinalizationRegistryProtoUnregister => {
                // Type 2: direct call (this_val, token) -> i64
                let this_val = args
                    .first()
                    .with_context(|| format!("{builtin} expects 2 arguments"))?;
                let token = args
                    .get(1)
                    .with_context(|| format!("{builtin} expects 2 arguments: this_val, token"))?;
                let func_idx = self.builtin_func_idx(builtin)?;
                self.emit(WasmInstruction::LocalGet(self.local_idx(this_val.0)));
                self.emit(WasmInstruction::LocalGet(self.local_idx(token.0)));
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
