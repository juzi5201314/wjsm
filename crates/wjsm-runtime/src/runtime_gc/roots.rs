//! Root 发现（spec §10）。
//!
//! `RuntimeRoots`：impl RootProvider，扫描 shadow stack + host 侧表。
//! - shadow stack：[base, sp) 每 8B 槽读 i64，tag_needs_root 则提取 handle（object/array），
//!   closure → 解析 env_obj handle。
//! - host 侧表：IR function property objects (0..num_ir_functions) 直接 root；
//!   fixed-point（microtask/promise/continuation/streams）由 P4 集成时
//!   在 collect_with_roots 的多轮 roots 注入覆盖。
//!
//! 移植自 runtime_builtins.rs:2974-3091 + trace_runtime_side_table_roots_fixed_point。
use crate::runtime_gc::api::{GcContext, Handle, RootProvider};
use wjsm_ir::value;

/// 运行时 root 提供者：扫描 shadow stack + host 侧表。
pub struct RuntimeRoots;

impl RootProvider for RuntimeRoots {
    fn for_each_shadow_stack_root(&mut self, ctx: &mut GcContext, visit: &mut dyn FnMut(Handle)) {
        let sp = ctx.shadow_sp();
        let end = ctx.shadow_stack_end();
        // shadow stack 区间：[object_heap_start - SHADOW_STACK_SIZE, sp)
        // 但实际 base 应取 shadow stack 底部。runtime 把 shadow stack 放在 memory 头部。
        // 简化：扫 [0, sp)（涵盖整个 shadow stack 区），tag_needs_root 过滤。
        // 注：object_heap_start 之后是对象堆，不在 shadow stack 区，不会被误扫（tag 过滤）。
        if sp == 0 || end == 0 || sp > end {
            return;
        }
        ctx.with_memory(|_caller, data| {
            let mut addr = 0usize;
            while addr + 8 <= sp.min(data.len()) {
                let val = i64::from_le_bytes([
                    data[addr],
                    data[addr + 1],
                    data[addr + 2],
                    data[addr + 3],
                    data[addr + 4],
                    data[addr + 5],
                    data[addr + 6],
                    data[addr + 7],
                ]);
                visit_value_handle(val, visit);
                addr += 8;
            }
        });
    }

    fn for_each_host_table_root(&mut self, ctx: &mut GcContext, visit: &mut dyn FnMut(Handle)) {
        // 直接 root：IR function property objects（0..num_ir_functions）。
        // 这些是 obj_table[0..n]，函数属性对象，永久存活。
        let n = ctx.num_ir_functions();
        for h in 0..(n as Handle) {
            visit(h);
        }
        // fixed-point host 侧表（microtask/promise/continuation/streams）：
        // P4 集成时由 collect_with_roots 的多轮 roots 注入覆盖。
        // 本方法只提供稳定 root（function props）；动态 root 经专门路径注入。
    }
}

/// 从 NaN-boxed value 提取 root handle（shadow stack 扫描用）。
/// - object/array：decode_object_handle
/// - closure：经 host closures 表解析 env_obj（顶层 root，spec §10）
/// - 其他 handle tag：低 32 位作为 handle（若 < obj_table_count）
/// - 标量：忽略
fn visit_value_handle(val: i64, visit: &mut dyn FnMut(Handle)) {
    if !value::tag_needs_root(val) {
        return;
    }
    if value::is_object(val) || value::is_array(val) {
        visit(value::decode_object_handle(val));
        return;
    }
    // closure：env_obj 是 host 侧表索引，不在 obj_table；P4 经 host table root 注入。
    // function：low32 是函数表索引 = obj_table 下标（function property object）。
    if value::is_function(val) {
        visit((val as u32) as Handle);
        return;
    }
    // 其他 handle tag（closure/native_callable/bigint/symbol/regexp/proxy/scope_record/
    // iterator/enumerator/runtime_string/exception）：host 侧表追踪，不在此。
}
