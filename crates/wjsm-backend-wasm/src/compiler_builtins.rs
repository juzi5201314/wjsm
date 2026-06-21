use super::*;

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

    /// 分发 builtin 编译到各类别方法。每个方法返回 `Ok(Some(()))` 表示已处理，
    /// `Ok(None)` 表示不属于该类别。
    pub(crate) fn compile_builtin_call(
        &mut self,
        dest: Option<ValueId>,
        builtin: &Builtin,
        args: &[ValueId],
    ) -> Result<()> {
        if let Some(()) = self.compile_builtin_core(dest, builtin, args)? {
            return Ok(());
        }
        if let Some(()) = self.compile_builtin_collections(dest, builtin, args)? {
            return Ok(());
        }
        if let Some(()) = self.compile_builtin_string_math(dest, builtin, args)? {
            return Ok(());
        }
        if let Some(()) = self.compile_builtin_async_proxy(dest, builtin, args)? {
            return Ok(());
        }
        if let Some(()) = self.compile_builtin_runtime(dest, builtin, args)? {
            return Ok(());
        }
        bail!("unhandled builtin: {builtin:?}")
    }
}
