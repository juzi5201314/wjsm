//! 后端无关的堆/侧表上下文。
//!
//! builtins 的语义算法不直接操作后端状态，而是经 [`HeapContext`] 的最小操作集完成；
//! native 后端用其原生堆/侧表实现本 trait。
//!
//! # 设计约束
//!
//! - **后端无关**：签名只出现 `CallArgs`、`Value`/`Handle`/字节/`usize`，禁止后端特化类型。
//! - **最小集**：只覆盖 builtins 真正需要的操作，不做全量抽象。
//! - **对象安全**：方法不泛型，可经 `&mut dyn HeapContext` 使用（`ExecContext` 继承本 trait）。
//! - **全 `&mut self`**：堆读取可能触发惰性加载或 GC，统一 `&mut self` 让后端无需
//!   unsafe/内部可变性即可实现。

use crate::{CallArgs, Handle, Value};

/// async_hooks 生命周期事件类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsyncHookEvent {
    Init,
    Before,
    After,
    Destroy,
    PromiseResolve,
}

/// 一轮 GC 的后端无关结果投影。
///
/// GC 算法内部的完整可观测性结构（碎片、暂停、迁移等指标）与具体堆布局耦合；
/// 本类型是**有意简化**的后端无关投影，只保留跨后端可比的指标，后端实现时把
/// 内部统计投影为该结果。
#[derive(Debug, Clone, Copy, Default)]
pub struct GcOutcome {
    /// 累计完成的 GC 轮数（含本轮）。
    pub cycle_count: u64,
    /// 本轮回收的字节数。
    pub bytes_collected: usize,
    /// 本轮耗时（微秒）。
    pub duration_us: u64,
}

/// 后端无关的堆/侧表操作上下文。
pub trait HeapContext {
    // ── console / 通用读 ──
    /// 从 native call arena 读取第 `index` 个实参；越界返回 `undefined`。
    fn read_call_arg(&mut self, args: CallArgs, index: u32) -> Value;
    /// 把字符串值渲染为 UTF-8（非字符串值由实现决定回退）。
    fn read_string_utf8(&mut self, val: Value) -> String;
    /// 追加字节到 stdout 输出缓冲。
    fn write_output(&mut self, bytes: &[u8]);

    // ── handle / 容器读 ──
    /// handle 是否指向存活对象。
    fn resolve_handle(&mut self, handle: Handle) -> bool;
    /// 数组长度；非数组返回 None。
    fn array_length(&mut self, handle: Handle) -> Option<u32>;
    /// 数组元素；越界或 hole 返回 None。
    fn array_elem(&mut self, handle: Handle, index: u32) -> Option<Value>;
    /// 读对象属性（沿原型链）；不存在返回 None。
    fn get_property(&mut self, handle: Handle, key: &str) -> Option<Value>;

    // ── object 写/分配 ──
    /// 分配对象，返回其 NaN-boxed 值。
    fn alloc_object(&mut self, capacity: u32) -> Value;
    /// 分配数组，返回其 NaN-boxed 值。
    fn alloc_array(&mut self, capacity: u32) -> Value;
    /// 写对象属性。
    fn set_property(&mut self, handle: Handle, key: &str, value: Value);
    /// 删除对象属性，返回是否成功。
    fn delete_property(&mut self, handle: Handle, key: &str) -> bool;

    // ── gc ──
    /// 触发一轮 GC，返回后端无关结果投影。
    fn gc_collect(&mut self) -> GcOutcome;
    /// 当前堆已用字节数。
    fn heap_used_bytes(&mut self) -> usize;

    // ── async hooks 状态 + GC 临时根 ──
    /// 开始一轮 async hook 派发（emit_depth+1）。
    fn async_emit_begin(&mut self);
    /// 查询某事件当前启用的回调值列表（`promise` 区分 promise 事件通道）。
    fn async_hook_callbacks(&mut self, event: AsyncHookEvent, promise: bool) -> Vec<Value>;
    /// 结束一轮 async hook 派发（emit_depth-1）。
    fn async_emit_end(&mut self);
    /// 压入 GC 临时根，返回压入前栈长（供后续 truncate 恢复）。
    fn push_temp_roots(&mut self, roots: &[Value]) -> usize;
    /// 恢复 GC 临时根栈到指定长度。
    fn truncate_temp_roots(&mut self, len: usize);
}
