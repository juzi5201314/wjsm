//! semantic lowering 后的 IR 优化 pass。

pub(crate) mod array_inline;
pub(crate) mod direct_call;
pub(crate) mod string_concat;
pub(crate) mod string_fold;
pub(crate) mod tail_self_loop;
