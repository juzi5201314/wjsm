//! 宿主环境主 trait：组合各 sub-trait。
//!
//! 后端实现 `HostRuntime` 即获得完整宿主能力。sub-trait 的拆分让后端可以
//! 先实现能力子集（如只 `ConsoleHost + ObjectHost`），再逐步补全。

use crate::{AsyncHost, ConsoleHost, GcHost, ObjectHost};

/// 完整宿主环境：console + 对象分配 + GC + async hooks。
///
/// 本 trait 是 marker/组合 trait，无自身方法；所有行为来自 sub-trait。
/// 为任何同时实现全部 sub-trait 的类型自动提供 blanket impl。
pub trait HostRuntime: ConsoleHost + ObjectHost + GcHost + AsyncHost {}

impl<T> HostRuntime for T where T: ConsoleHost + ObjectHost + GcHost + AsyncHost {}
