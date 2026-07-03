use super::*;

pub(crate) enum BuiltinDispatch {
    Handled,
    NotHandled,
}

impl Compiler {
    pub(crate) fn ensure_string_ptr_const(&mut self, s: &str) -> u32 {
        if let Some(&ptr) = self.string_ptr_cache.get(s) {
            return ptr;
        }
        let ptr = self.data_base + self.data_offset;
        let mut bytes = s.as_bytes().to_vec();
        bytes.push(0);
        let len = bytes.len() as u32;
        self.string_data.extend(bytes);
        self.data_offset += len;
        self.string_ptr_cache.insert(s.to_string(), ptr);
        ptr
    }

    /// 分发 builtin 编译到各类别方法。每个方法返回 `Handled` 表示已处理，
    /// `NotHandled` 表示不属于该类别。
    pub(crate) fn compile_builtin_call(
        &mut self,
        dest: Option<ValueId>,
        builtin: &Builtin,
        args: &[ValueId],
    ) -> Result<()> {
        if matches!(
            self.compile_builtin_core(dest, builtin, args)?,
            BuiltinDispatch::Handled
        ) {
            return Ok(());
        }
        if matches!(
            self.compile_builtin_collections(dest, builtin, args)?,
            BuiltinDispatch::Handled
        ) {
            return Ok(());
        }
        if matches!(
            self.compile_builtin_string_math(dest, builtin, args)?,
            BuiltinDispatch::Handled
        ) {
            return Ok(());
        }
        if matches!(
            self.compile_builtin_async_proxy(dest, builtin, args)?,
            BuiltinDispatch::Handled
        ) {
            return Ok(());
        }
        if matches!(
            self.compile_builtin_runtime(dest, builtin, args)?,
            BuiltinDispatch::Handled
        ) {
            return Ok(());
        }
        bail!("unhandled builtin: {builtin:?}")
    }

    pub(crate) fn builtin_func_idx(&self, builtin: &Builtin) -> Result<u32> {
        self.builtin_func_indices
            .get(builtin)
            .copied()
            .ok_or_else(|| anyhow::anyhow!("missing builtin function index for {builtin:?}"))
    }

    pub(crate) fn emit_value_args(&mut self, args: &[ValueId]) {
        for arg in args {
            self.emit(WasmInstruction::LocalGet(self.local_idx(arg.0)));
        }
    }

    pub(crate) fn emit_padded_value_args(&mut self, args: &[ValueId], width: usize) {
        for index in 0..width {
            if let Some(arg) = args.get(index) {
                self.emit(WasmInstruction::LocalGet(self.local_idx(arg.0)));
            } else {
                self.emit(WasmInstruction::I64Const(value::encode_undefined()));
            }
        }
    }

    pub(crate) fn store_or_drop_call_result(&mut self, dest: Option<ValueId>) {
        if let Some(dest) = dest {
            self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
        } else {
            self.emit(WasmInstruction::Drop);
        }
    }
}
