use crate::Compiler;

pub(super) fn bind(compiler: &mut Compiler, support_import_base: u32) {
    compiler.obj_get_func_idx = support_import_base + 1;
    compiler.obj_delete_func_idx = support_import_base + 3;
}
