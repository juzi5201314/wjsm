// ExecContext 方法片段：strings
macro_rules! exec_ctx_strings {
    () => {
    fn store_string(&mut self, s: &str) -> Value {
        crate::runtime_render::store_runtime_string(self.caller, s.to_string())
    }
    fn store_string_owned(&mut self, s: String) -> Value {
        crate::runtime_render::store_runtime_string(self.caller, s)
    }
    fn read_string_bytes(&mut self, val: Value) -> Option<Vec<u8>> {
        crate::runtime_render::read_value_string_bytes(self.caller, val)
    }
    fn read_string_utf8_lossy(&mut self, val: Value) -> String {
        crate::runtime_render::read_runtime_string_utf8_lossy(self.caller, val)
    }
    fn canonicalize_name_id(&mut self, name_id: u32) -> Option<u32> {
        crate::property_key::canonicalize_v2_name_id(self.caller, name_id)
    }
    fn intern_property_key(&mut self, s: &str) -> u32 {
        let index = crate::property_key::intern_runtime_property_key(
            self.caller.data(),
            RuntimeString::from_utf8_str(s),
        );
        crate::property_key::encode_runtime_string_name_id(index)
    }
    fn property_key_string(&mut self, name_id: u32) -> Option<String> {
        match crate::property_key::decode_name_id(name_id) {
            crate::property_key::DecodedNameId::RuntimeString(index) => {
                crate::property_key::runtime_property_key_units(self.caller.data(), index)
                    .map(|rs| rs.to_utf8_lossy())
            }
            crate::property_key::DecodedNameId::MemoryString(index) => {
                let env = self.env()?;
                let bytes =
                    crate::runtime_render::read_string_bytes_mem(self.caller, &env.memory, index);
                Some(String::from_utf8_lossy(&bytes).into_owned())
            }
            crate::property_key::DecodedNameId::Symbol(_) => None,
        }
    }
    fn name_id_matches(&mut self, name_id: u32, expected: &str) -> bool {
        let key = RuntimeString::from_utf8_str(expected);
        let Some(env) = self.env() else {
            return false;
        };
        crate::property_key::name_id_matches_runtime_string(self.caller, &env, name_id, &key)
    }
    fn name_id_to_property_key_value(&mut self, name_id: u32) -> Option<Value> {
        use wjsm_host::property_key::{DecodedNameId, decode_name_id};
        match decode_name_id(name_id) {
            DecodedNameId::MemoryString(index) => Some(value::encode_string_ptr(index)),
            DecodedNameId::Symbol(index) => Some(value::encode_symbol_handle(index)),
            DecodedNameId::RuntimeString(index) => {
                // 从 runtime property key 表取回 RuntimeString，再编码为 runtime string handle
                let key =
                    crate::property_key::runtime_property_key_units(self.caller.data(), index)?;
                Some(crate::runtime_render::store_runtime_string(
                    self.caller,
                    key,
                ))
            }
        }
    }
    fn property_value_to_name_id(&mut self, prop: Value, allow_symbol: bool) -> Option<u32> {
        if !allow_symbol && value::is_symbol(prop) {
            return None;
        }
        crate::property_key::property_key_value_to_name_id(self.caller, prop, true)
    }
    fn read_memory_string(&mut self, ptr: u32, len: Option<u32>) -> String {
        let Some(env) = self.env() else {
            return String::new();
        };
        match len {
            Some(n) => {
                let data = env.memory.data(&*self.caller);
                let start = ptr as usize;
                let end = start.saturating_add(n as usize);
                if end > data.len() {
                    return String::new();
                }
                let bytes = &data[start..end];
                let bytes = if bytes.ends_with(&[0]) {
                    &bytes[..bytes.len() - 1]
                } else {
                    bytes
                };
                String::from_utf8_lossy(bytes).into_owned()
            }
            None => crate::runtime_render::read_string(self.caller, ptr).unwrap_or_default(),
        }
    }
    fn read_memory_string_bytes(&mut self, ptr: u32) -> Vec<u8> {
        crate::runtime_render::read_string_bytes(self.caller, ptr)
    }
    fn string_values_equal(&mut self, a: Value, b: Value) -> bool {
        let a_str = crate::runtime_values::get_string_value(self.caller, a);
        let b_str = crate::runtime_values::get_string_value(self.caller, b);
        a_str == b_str
    }
    fn string_lt(&mut self, a: Value, b: Value) -> bool {
        let a_str = crate::runtime_values::get_string_value(self.caller, a);
        let b_str = crate::runtime_values::get_string_value(self.caller, b);
        a_str.cmp_utf16(&b_str).is_lt()
    }
    fn string_utf16_len(&mut self, val: Value) -> Option<u32> {
        if !value::is_string(val) {
            return None;
        }
        Some(crate::runtime_values::get_string_value(self.caller, val).utf16_len() as u32)
    }
    fn get_runtime_string(&mut self, val: Value) -> wjsm_host::RuntimeString {
        crate::get_string_value(self.caller, val)
    }
    fn concat_utf16_va(&mut self, parts: &[Value]) -> Option<wjsm_host::RuntimeString> {
        crate::runtime_render::concat_utf16_va(self.caller, parts)
    }
    fn store_runtime_string(&mut self, s: wjsm_host::RuntimeString) -> Value {
        crate::runtime_render::store_runtime_string(self.caller, s)
    }
    fn create_symbol(&mut self, description: Option<String>, global_key: Option<String>) -> Value {
        let mut table = self
            .caller
            .data()
            .symbol_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let handle = table.len() as u32;
        table.push(crate::types::SymbolEntry {
            description,
            global_key,
        });
        value::encode_symbol_handle(handle)
    }
    fn symbol_entry(&mut self, val: Value) -> Option<(Option<String>, Option<String>)> {
        if !value::is_symbol(val) {
            return None;
        }
        let handle = value::decode_symbol_handle(val) as usize;
        let table = self
            .caller
            .data()
            .symbol_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        table
            .get(handle)
            .map(|e| (e.description.clone(), e.global_key.clone()))
    }
    fn symbol_well_known(&mut self, id: i32) -> Value {
        if id < 0 {
            return value::encode_undefined();
        }
        let table = self
            .caller
            .data()
            .symbol_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if (id as usize) < table.len() {
            value::encode_symbol_handle(id as u32)
        } else {
            value::encode_undefined()
        }
    }
    fn find_global_symbol(&mut self, key: &str) -> Option<Value> {
        let table = self
            .caller
            .data()
            .symbol_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for (idx, entry) in table.iter().enumerate() {
            if entry.global_key.as_deref() == Some(key) {
                return Some(value::encode_symbol_handle(idx as u32));
            }
        }
        None
    }
    fn install_well_known_symbols_on_symbol_constructor(&mut self, ctor: Value) {
        crate::symbol_well_known::install_well_known_symbols_on_symbol_constructor(
            self.caller,
            ctor,
        );
    }
    };
}
