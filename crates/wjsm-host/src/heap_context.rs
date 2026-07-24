//! 后端无关的堆/侧表上下文。
//!
//! 这是多后端解耦的**核心接缝**：`HostRuntime` 的各能力域（console/object/gc/async）
//! 不直接操作后端状态，而是经 [`HeapContext`] 的最小操作集完成。各后端用自身的
//! 运行时上下文实现本 trait——wasmtime 后端用 `Caller<RuntimeState>`，
//! native 后端用其原生堆/侧表。
//!
//! # 设计约束
//!
//! - **后端无关**：签名只出现 `Value`/`Handle`/字节/`usize`，禁止 `Caller`/`Store`/`Extern`。
//! - **最小集**：只覆盖 HostRuntime 能力域真正需要的操作，不做全量 builtins 抽象。
//! - **对象安全**：方法不泛型，可经 `&mut dyn HeapContext` 使用（HostRuntime 据此委托）。
//! - **全 `&mut self`**：读操作（如 `read_shadow_arg`/`array_elem`）也取 `&mut self`。
//!   后端读取堆可能需可变上下文（wasmtime 的 `Memory::data`/`Caller`、或读取触发惰性
//!   加载/GC），统一 `&mut self` 让后端无需 unsafe/内部可变性即可实现。

use crate::{GcOutcome, Handle, Value};

/// async_hooks 生命周期事件类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsyncHookEvent {
    Init,
    Before,
    After,
    Destroy,
    PromiseResolve,
}

/// 后端无关的堆/侧表操作上下文。
pub trait HeapContext {
    // ── console / 通用读 ──
    /// 从影子栈读取第 `index` 个 vararg（`args_base` 为槽基址）。
    fn read_shadow_arg(&mut self, args_base: i32, index: u32) -> Value;
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
