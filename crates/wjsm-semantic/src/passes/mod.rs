//! semantic lowering 后的 IR 优化 pass。

pub(crate) mod cfg_fold;
pub(crate) mod direct_call;
pub(crate) mod inline_for_ea;
pub(crate) mod escape_scalar;
