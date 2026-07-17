use crate::Compiler;

pub(super) fn bind(compiler: &mut Compiler, support_import_base: u32) {
    compiler.obj_set_func_idx = support_import_base + 2;
}
