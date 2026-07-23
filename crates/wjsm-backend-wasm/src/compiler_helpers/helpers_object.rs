#[path = "helpers_object/alloc.rs"]
mod alloc;
#[path = "helpers_object/array.rs"]
mod array;
#[path = "helpers_object/property.rs"]
mod property;
#[path = "helpers_object/resolve.rs"]
mod resolve;

use crate::Compiler;

impl Compiler {
    /// V2 的 object/array calls 均绑定到 memory64 support ABI，避免 inline static helper。
    pub(crate) fn bind_v2_support_helpers(&mut self, support_import_base: u32) {
        alloc::bind(self, support_import_base);
        resolve::bind(self, support_import_base);
        property::bind(self, support_import_base);
        array::bind(self, support_import_base);
        self.string_eq_func_idx = support_import_base + 7;
        self.to_int32_func_idx = support_import_base + 8;
        self.get_proto_from_ctor_func_idx = support_import_base + 9;
        for offset in 0..10 {
            self.push_func_table(support_import_base + offset);
        }
    }
}
