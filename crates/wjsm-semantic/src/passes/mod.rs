//! semantic lowering 后的 IR 优化 pass。

pub(crate) mod array_inline;
pub(crate) mod cfg_fold;
pub(crate) mod direct_call;
pub(crate) mod escape_scalar;
pub(crate) mod inline_for_ea;
