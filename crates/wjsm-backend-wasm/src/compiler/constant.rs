use anyhow::{Result, bail};
use wjsm_ir::{Constant, Module as IrModule, value};

use super::state::{Compiler, EVAL_VAR_MAP_RECORD_SIZE};

impl Compiler {
    pub(crate) fn encode_constant(&mut self, constant: &Constant, _module: &IrModule) -> Result<i64> {
        match constant {
            Constant::Number(value) => Ok(value.to_bits() as i64),
            Constant::String(value) => {
                if let Some(&ptr) = self.string_ptr_cache.get(value) {
                    return Ok(value::encode_string_ptr(ptr));
                }
                let ptr = self.data_base + self.data_offset;
                let mut bytes = value.as_bytes().to_vec();
                bytes.push(0);
                let len = bytes.len() as u32;

                self.string_data.extend(bytes);
                self.data_offset += len;
                self.string_ptr_cache.insert(value.clone(), ptr);

                Ok(value::encode_string_ptr(ptr))
            }
            Constant::Bool(b) => Ok(value::encode_bool(*b)),
            Constant::Null => Ok(value::encode_null()),
            Constant::Undefined => Ok(value::encode_undefined()),
            Constant::FunctionRef(function_id) => {
                let wasm_idx = function_id.0;
                let table_idx = self
                    .function_table_reverse
                    .get(&wasm_idx)
                    .copied()
                    .unwrap_or(wasm_idx);
                Ok(value::encode_function_idx(table_idx))
            }
            Constant::NativeCallableEval => Ok(value::encode_native_callable_idx(0)),
            Constant::BigInt(_) => {
                bail!("BigInt constants should be handled in compile_instruction::Const")
            }
            Constant::RegExp { .. } => {
                bail!("RegExp constants should be handled in compile_instruction::Const")
            }
            Constant::ModuleId(module_id) => {
                // 模块 ID 直接编码为 i64 整数
                Ok(module_id.0 as i64)
            }
        }
    }
    /// Intern a nul-terminated string in the data section and return its offset.
    /// 如果字符串已缓存，直接返回已有偏移量。
    /// 与 encode_constant 中的字符串处理逻辑相同。
    pub(crate) fn intern_data_string(&mut self, s: &str) -> u32 {
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

    pub(crate) fn finalize_eval_var_map_data(&mut self) {
        self.eval_var_map_records.sort_by(|a, b| {
            a.function_name
                .cmp(&b.function_name)
                .then_with(|| a.var_name.cmp(&b.var_name))
                .then_with(|| a.offset.cmp(&b.offset))
        });
        self.eval_var_map_records.dedup();

        if self.eval_var_map_records.is_empty() {
            self.eval_var_map_ptr = 0;
            self.eval_var_map_count = 0;
            return;
        }

        let records = self.eval_var_map_records.clone();
        let mut table = Vec::with_capacity(records.len() * EVAL_VAR_MAP_RECORD_SIZE as usize);
        for record in records {
            let function_ptr = self.intern_data_string(&record.function_name);
            let var_ptr = self.intern_data_string(&record.var_name);
            table.extend_from_slice(&function_ptr.to_le_bytes());
            table.extend_from_slice(&(record.function_name.len() as u32).to_le_bytes());
            table.extend_from_slice(&var_ptr.to_le_bytes());
            table.extend_from_slice(&(record.var_name.len() as u32).to_le_bytes());
            table.extend_from_slice(&record.offset.to_le_bytes());
        }

        let table_ptr = (self.data_offset + 3) & !3;
        if self.string_data.len() < table_ptr as usize {
            self.string_data.resize(table_ptr as usize, 0);
        }
        self.string_data.extend_from_slice(&table);
        self.data_offset = table_ptr + table.len() as u32;
        self.eval_var_map_ptr = self.data_base + table_ptr;
        self.eval_var_map_count = (table.len() as u32) / EVAL_VAR_MAP_RECORD_SIZE;
    }
}
