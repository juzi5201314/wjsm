//! 运行时字符串表（`runtime_strings`）的保守清扫。
//!
//! 背景：字符串 handle 是 host 表索引（NaN-boxed `TAG_STRING | STRING_RUNTIME_HANDLE_FLAG`），
//! 被 wasm 线性内存中的值引用；字符串不参与 wasm 对象图，`wjsm-gc` 只回收堆对象，
//! 因此 `runtime_strings` 原本只增不减——字符串密集负载（拼接/模板串/split/slice）RSS 线性爆炸。
//!
//! 本模块在字符串表估算字节超过阈值时，从以下来源**保守**收集存活字符串 handle：
//! 1. shadow stack `[0, sp)`（编译器在 string_concat / StringConcatVa 等产字符串的
//!    host 调用前 safepoint spill 所有存活 handle，见 `compiler_gc_analysis.rs` 与
//!    `instr_main.rs` 的 Add 分支——这是清扫正确性的前提）；
//! 2. V2 堆全部存活对象的槽位值（原型 + 数组元素 + 属性槽，经 `HeapAccessV2`；
//!    与 zgc 构图同源，闭包 env 等对象才能被覆盖）；
//! 3. host 侧表（microtask/promise/回调/Map-Set/数组命名属性等）与 side-table-backed 值
//!    （`is_marked` 恒 true → 保守）。
//!
//! 然后把未命中的表项替换为空串（释放 UTF-16 数据），并把空槽推进 free list，
//! `store_runtime_string` 优先复用空槽（handle 索引稳定，无需重写内存中的值）。
//!
//! **安全论证**：清扫只可能多留（把死字符串当活），不可能误释放活字符串——
//! 三个来源覆盖了所有活字符串 handle 可能存在的位置；wasm 在 host 调用内暂停，
//! 影子栈/堆/表均静止。阈值检查发生在 push 之前，刚创建的新字符串不在表内，天然存活。

use std::collections::HashSet;
use std::sync::atomic::Ordering;

use wjsm_ir::value;
use wasmtime::Caller;

use crate::runtime_gc::roots::{collect_host_table_values, collect_side_table_backed_host_values};
use crate::runtime_gc::GcContext;
use crate::runtime_string::RuntimeString;
use crate::RuntimeState;

/// 字符串表清扫阈值（估算字节）。首次触发后按存活量翻倍，避免合法大活集反复清扫。
pub(crate) const SWEEP_THRESHOLD_BYTES: usize = 16 * 1024 * 1024;

/// 每项固定开销估算（Vec header + 分配器元数据）。
pub(crate) const PER_ENTRY_OVERHEAD: usize = 64;

/// 若字符串表估算字节达到下次清扫阈值，执行保守清扫。
///
/// 必须在**存字符串之前**调用：新字符串尚未入表，天然属于存活集。
pub(crate) fn maybe_sweep_runtime_strings(caller: &mut Caller<'_, RuntimeState>) {
    let approx = caller
        .data()
        .runtime_string_approx_bytes
        .load(Ordering::Relaxed);
    let next = caller
        .data()
        .runtime_string_next_sweep
        .load(Ordering::Relaxed);
    if approx < next {
        return;
    }
    let Some(env) = crate::wasm_env::WasmEnv::from_caller(caller) else {
        return;
    };
    sweep_runtime_strings(caller, &env);
}

fn collect_string_handle(val: i64, live: &mut HashSet<u32>) {
    if value::is_runtime_string_handle(val) {
        live.insert(value::decode_runtime_string_handle(val));
    }
}

/// 保守清扫：见模块文档。
fn sweep_runtime_strings(caller: &mut Caller<'_, RuntimeState>, env: &crate::wasm_env::WasmEnv) {
    let mut gc_ctx = GcContext::new(caller, env, "string-sweep");
    let obj_table_count = gc_ctx.obj_table_count();
    let mut live: HashSet<u32> = HashSet::new();

    // 1. shadow stack [0, sp)：编译器在产字符串的 host 调用前 spill 存活 handle。
    let sp = gc_ctx.shadow_sp();
    let shadow_vals: Vec<i64> = gc_ctx.with_shadow_memory(|data| {
        let mut out = Vec::new();
        let mut addr = 0usize;
        let limit = sp.min(data.len());
        while addr + 8 <= limit {
            out.push(i64::from_le_bytes([
                data[addr],
                data[addr + 1],
                data[addr + 2],
                data[addr + 3],
                data[addr + 4],
                data[addr + 5],
                data[addr + 6],
                data[addr + 7],
            ]));
            addr += 8;
        }
        out
    });
    for v in shadow_vals {
        collect_string_handle(v, &mut live);
    }

    let access = gc_ctx.with_state(|st| st.heap_access_v2().clone());
    // 2. V2 堆所有存活对象的槽位值（原型 + 数组元素 + 属性槽 value/getter/setter）。
    //    与 zgc 构图一致：`HeapAccessV2::live_handles` + `object_references`（旧 obj_table
    //    遍历读不到 memory64 V2 堆，闭包 env 等对象会漏 → 捕获字符串被误回收）。
    for handle in access.live_handles(obj_table_count as u32) {
        if let Ok(references) = access.object_references(handle) {
            for raw in references {
                collect_string_handle(raw, &mut live);
            }
        }
    }

    // 3. host 侧表 raw 值（is_marked 恒 true → 保守覆盖全部表项）。
    let host_vals = collect_host_table_values(&mut gc_ctx, &mut |_| true);
    for v in host_vals {
        collect_string_handle(v, &mut live);
    }
    let mut side_vals = Vec::new();
    gc_ctx.with_state(|st| collect_side_table_backed_host_values(st, &mut side_vals));
    for v in side_vals {
        collect_string_handle(v, &mut live);
    }

    // 4. 清扫：未命中 → 空串（释放 UTF-16 数据）+ 空槽回收；命中 → 统计存活字节。
    let mut strings = caller
        .data()
        .runtime_strings
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut free = caller
        .data()
        .string_free_slots
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut live_bytes = 0usize;
    for (idx, s) in strings.iter_mut().enumerate() {
        if live.contains(&(idx as u32)) {
            live_bytes += s.utf16_len() * 2 + PER_ENTRY_OVERHEAD;
        } else if !s.is_empty() {
            *s = RuntimeString::empty();
            free.push(idx as u32);
        }
    }
    drop(free);

    caller
        .data()
        .runtime_string_approx_bytes
        .store(live_bytes, Ordering::Relaxed);
    // 下次阈值 = max(16MB, 存活量×2)：合法大活集按翻倍频率清扫，避免反复空扫。
    let next_sweep = live_bytes.saturating_mul(2).max(SWEEP_THRESHOLD_BYTES);
    caller
        .data()
        .runtime_string_next_sweep
        .store(next_sweep, Ordering::Relaxed);
}
