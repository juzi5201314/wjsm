//! builtin 编译：SymbolFor ~ StringRepeat

use super::*;

impl Compiler {
    /// 处理 SymbolFor ~ StringRepeat 等 builtin。
    pub(crate) fn compile_builtin_async_proxy(
        &mut self,
        dest: Option<ValueId>,
        builtin: &Builtin,
        args: &[ValueId],
    ) -> Result<Option<()>> {
        match builtin {
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
                Ok(Some(()))
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
                Ok(Some(()))
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
                Ok(Some(()))
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
                Ok(Some(()))
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
                Ok(Some(()))
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
                Ok(Some(()))
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
                Ok(Some(()))
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
                Ok(Some(()))
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
                Ok(Some(()))
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
                Ok(Some(()))
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
                Ok(Some(()))
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
                Ok(Some(()))
            }
            Builtin::AsyncFunctionStart => {
                if args.len() < 1 {
                    bail!(
                        "AsyncFunctionStart requires at least 1 argument, got {}",
                        args.len()
                    );
                }
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
                Ok(Some(()))
            }
            Builtin::AsyncFunctionResume => {
                if args.len() < 5 {
                    bail!(
                        "AsyncFunctionResume requires at least 5 arguments, got {}",
                        args.len()
                    );
                }
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
                Ok(Some(()))
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
                Ok(Some(()))
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
                Ok(Some(()))
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
                Ok(Some(()))
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
                Ok(Some(()))
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
                Ok(Some(()))
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
                Ok(Some(()))
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
                Ok(Some(()))
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
                Ok(Some(()))
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
                Ok(Some(()))
            }
            // ── String prototype builtins (receiver + 2 args, second optional) ──
            _ => Ok(None),
        }
    }
}
