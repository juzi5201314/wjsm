use crate::Compiler;

pub(super) fn bind(compiler: &mut Compiler, support_import_base: u32) {
    compiler.arr_new_func_idx = support_import_base + 4;
    compiler.elem_get_func_idx = support_import_base + 5;
    compiler.elem_set_func_idx = support_import_base + 6;
}
