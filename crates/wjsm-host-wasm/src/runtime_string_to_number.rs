//! ECMAScript §7.1.4 String 的 ToNumber（StringToNumber）。
//!
//! 纯实现已迁至 `wjsm-builtins`；本模块再导出以保持调用路径。

pub(crate) use wjsm_builtins::js_string_content_to_f64;
