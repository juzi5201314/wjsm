use super::*;
use crate::host_import_registry::{HostImportGroup, host_import_specs};

impl Compiler {
    /// 将 Number.prototype 宿主导入登记到函数表（供 encode_function_idx 字面量快路径等）。
    pub(crate) fn compile_number_proto_wrappers(&mut self) {
        for (import_idx, spec) in host_import_specs().iter().enumerate() {
            if spec.group != Some(HostImportGroup::NumberPrototypeMethod) {
                continue;
            }
            self.push_func_table(import_idx as u32);
        }
    }
}
