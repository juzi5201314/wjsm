use super::*;
use crate::host_import_registry::{
    SpecialHostImport, array_proto_method_specs, array_proto_property_name, array_proto_table_hash,
    array_proto_table_len, host_import_specs,
};

mod module_bootstrap;
mod module_compile;
mod module_setup;

/// direct_call fast 入口的类型段起始索引（shared_types 中 Type 39 起）。
/// 类型索引 = `FAST_ENTRY_TYPE_BASE + N`（N = 声明形参数）。
pub(crate) const FAST_ENTRY_TYPE_BASE: u32 = 39;
/// fast 入口支持的声明形参数上限（超过走 slow 入口）。
pub(crate) const MAX_FAST_PARAMS: u32 = 8;
