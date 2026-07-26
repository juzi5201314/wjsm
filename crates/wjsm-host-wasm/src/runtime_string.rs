//! RuntimeString：UTF-16 字符串内部表示。
//!
//! 类型本体已迁至 `wjsm-host`（纯数据，后端无关）；本模块仅再导出，
//! 保持 `crate::runtime_string::RuntimeString` 调用路径稳定。

pub(crate) use wjsm_host::RuntimeString;
