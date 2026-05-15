use wasmtime::*;
use wjsm_ir::value;

use crate::types::*;
use crate::runtime::*;

pub(crate) fn create_host_functions(store: &mut Store<RuntimeState>) -> Vec<(usize, Func)> {
    let gc_collect = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, RuntimeState>, requested_size: i32| -> i32 {
            // 获取全局变量
            let heap_ptr = {
                let Some(Extern::Global(g)) = caller.get_export("__heap_ptr") else {
                    return 0;
                };
                g.get(&mut caller).i32().unwrap_or(0)
            };
            let obj_table_ptr = {
                let Some(Extern::Global(g)) = caller.get_export("__obj_table_ptr") else {
                    return 0;
                };
                g.get(&mut caller).i32().unwrap_or(0)
            };
            let obj_table_count = {
                let Some(Extern::Global(g)) = caller.get_export("__obj_table_count") else {
                    return 0;
                };
                g.get(&mut caller).i32().unwrap_or(0)
            };
            let object_heap_start = {
                let Some(Extern::Global(g)) = caller.get_export("__object_heap_start") else {
                    return 0;
                };
                g.get(&mut caller).i32().unwrap_or(0)
            };
            let num_ir_functions = {
                let Some(Extern::Global(g)) = caller.get_export("__num_ir_functions") else {
                    return 0;
                };
                g.get(&mut caller).i32().unwrap_or(0)
            };
            let shadow_sp = {
                let Some(Extern::Global(g)) = caller.get_export("__shadow_sp") else {
                    return 0;
                };
                g.get(&mut caller).i32().unwrap_or(0)
            };

            // 初始化/清除标记位图（在获取内存之前）
            {
                let mut mark_bits = caller
                    .data()
                    .gc_mark_bits
                    .lock()
                    .expect("gc_mark_bits mutex");
                let needed_words = ((obj_table_count as usize + 63) / 64).max(mark_bits.len());
                if mark_bits.len() < needed_words {
                    mark_bits.resize(needed_words, 0);
                } else {
                    mark_bits.fill(0);
                }
            }

            // ── 构建根集 ──
            // 从三个来源收集根对象：
            //   1. 影子栈帧（调用栈上的对象/函数引用）
            //   2. 函数属性对象（前 num_ir_functions 个句柄）
            //   3. 定时器回调
            let mut roots: Vec<(usize, usize)> = Vec::new();
            {
                let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                    return 0;
                };
                let data = memory.data(&caller);

                let add_root = |handle_idx: usize, data: &[u8], roots: &mut Vec<(usize, usize)>| {
                    let slot_addr = obj_table_ptr as usize + handle_idx * 4;
                    if slot_addr + 4 <= data.len() {
                        let obj_ptr = u32::from_le_bytes([
                            data[slot_addr],
                            data[slot_addr + 1],
                            data[slot_addr + 2],
                            data[slot_addr + 3],
                        ]) as usize;
                        if obj_ptr != 0 {
                            roots.push((handle_idx, obj_ptr));
                        }
                    }
                };

                // 3a. 影子栈：从 shadow_stack_base 扫描到 shadow_sp
                // shadow_sp 是栈指针，影子栈在 shadow_stack_base 处，每帧 8 字节
                let shadow_stack_base = object_heap_start as usize - SHADOW_STACK_SIZE as usize;
                let shadow_sp_usize = shadow_sp as usize;
                if shadow_sp_usize > shadow_stack_base {
                    let frame_count = (shadow_sp_usize - shadow_stack_base) / 8;
                    for frame in 0..frame_count {
                        let frame_addr = shadow_stack_base + frame * 8;
                        if frame_addr + 8 <= data.len() {
                            let val = i64::from_le_bytes([
                                data[frame_addr],
                                data[frame_addr + 1],
                                data[frame_addr + 2],
                                data[frame_addr + 3],
                                data[frame_addr + 4],
                                data[frame_addr + 5],
                                data[frame_addr + 6],
                                data[frame_addr + 7],
                            ]);
                            if value::is_object(val) {
                                let handle_idx = (val as u64 & 0xFFFF_FFFF) as usize;
                                add_root(handle_idx, data, &mut roots);
                            } else if value::is_function(val) {
                                // Functions are stored in handle table too
                                let func_idx = (val as u64 & 0xFFFF_FFFF) as usize;
                                if func_idx < num_ir_functions as usize {
                                    add_root(func_idx, data, &mut roots);
                                }
                            } else if value::is_closure(val) {
                                // 闭包值的 env_obj 可能包含对象引用
                                let closure_idx = value::decode_closure_idx(val) as usize;
                                let closures =
                                    caller.data().closures.lock().expect("closures mutex");
                                if let Some(entry) = closures.get(closure_idx) {
                                    if value::is_object(entry.env_obj) {
                                        let handle_idx =
                                            value::decode_object_handle(entry.env_obj) as usize;
                                        add_root(handle_idx, data, &mut roots);
                                    }
                                }
                            }
                        }
                    }
                }

                // 3b. 函数属性对象（前 num_ir_functions 个条目）始终标记
                for handle_idx in 0..num_ir_functions as usize {
                    add_root(handle_idx, data, &mut roots);
                }

                // 3c. 定时器回调
                {
                    let timers = caller.data().timers.lock().expect("timers mutex");
                    for timer in timers.iter() {
                            let val = timer.callback;
                          if value::is_function(val) {
                            let func_idx = (val as u64 & 0xFFFF_FFFF) as usize;
                            if func_idx < num_ir_functions as usize {
                                add_root(func_idx, data, &mut roots);
                            }
                        } else if value::is_closure(val) {
                            // 闭包回调：将 env_obj 中的对象标记为根
                            let closure_idx = value::decode_closure_idx(val) as usize;
                            let closures = caller.data().closures.lock().expect("closures mutex");
                            if let Some(entry) = closures.get(closure_idx) {
                                if value::is_object(entry.env_obj) {
                                    let handle_idx =
                                        value::decode_object_handle(entry.env_obj) as usize;
                                    add_root(handle_idx, data, &mut roots);
                                }
                            }
                        }
                    }
                }

                // 3d. 闭包表中的 env_obj
                {
                    let closures = caller.data().closures.lock().expect("closures mutex");
                    for entry in closures.iter() {
                        if value::is_object(entry.env_obj) {
                            let handle_idx = value::decode_object_handle(entry.env_obj) as usize;
                            add_root(handle_idx, data, &mut roots);
                        }
                    }
                }

                // 3e. 模块命名空间对象缓存（dynamic import 返回的命名空间对象必须保持可达）
                {
                    let cache = caller
                        .data()
                        .module_namespace_cache
                        .lock()
                        .expect("module namespace cache mutex");
                    for &val in cache.values() {
                        if value::is_object(val) {
                            let handle_idx = value::decode_object_handle(val) as usize;
                            add_root(handle_idx, data, &mut roots);
                        }
                    }
                }

                // 去重
                roots.sort();
                roots.dedup_by_key(|&mut (handle_idx, _)| handle_idx);
            } // data 借用结束

            // Phase 1: Mark - 递归标记所有可达对象
            for (handle_idx, obj_ptr) in roots {
                mark_object_recursive(
                    &mut caller,
                    handle_idx,
                    obj_ptr,
                    obj_table_ptr as usize,
                    obj_table_count as usize,
                );
            }

            // Phase 2: Sweep + Compact
            // 将存活对象移动到堆开头，更新 handle table

            // 首先获取标记位图的快照
            let mark_snapshot: Vec<u64> = {
                let mark_bits = caller
                    .data()
                    .gc_mark_bits
                    .lock()
                    .expect("gc_mark_bits mutex");
                mark_bits.clone()
            };

            // 获取内存数据的可变引用
            let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                return 0;
            };
            let data = memory.data_mut(&mut caller);

            let heap_base = object_heap_start as usize;

            // 收集存活对象信息
            let mut live_objects: Vec<(usize, usize, usize)> = Vec::new(); // (handle_idx, old_ptr, size)
            for handle_idx in 0..obj_table_count as usize {
                let word_idx = handle_idx / 64;
                let bit_idx = handle_idx % 64;
                if word_idx < mark_snapshot.len()
                    && (mark_snapshot[word_idx] & (1u64 << bit_idx)) != 0
                {
                    // 存活对象
                    let slot_addr = obj_table_ptr as usize + handle_idx * 4;
                    if slot_addr + 4 > data.len() {
                        continue;
                    }
                    let old_ptr = u32::from_le_bytes([
                        data[slot_addr],
                        data[slot_addr + 1],
                        data[slot_addr + 2],
                        data[slot_addr + 3],
                    ]) as usize;
                    if old_ptr == 0 {
                        continue;
                    }
                    // 计算对象大小
                    if old_ptr + 12 > data.len() {
                        continue;
                    }
                    let capacity = u32::from_le_bytes([
                        data[old_ptr + 4],
                        data[old_ptr + 5],
                        data[old_ptr + 6],
                        data[old_ptr + 7],
                    ]) as usize;
                    let size = 12 + capacity * 32;
                    live_objects.push((handle_idx, old_ptr, size));
                }
            }

            // 按旧指针排序，保持内存布局顺序
            live_objects.sort_by_key(|&(_, old_ptr, _)| old_ptr);

            // 计算新的位置
            let mut current_ptr = heap_base;
            for (_, _, size) in &live_objects {
                current_ptr += size;
            }
            let new_heap_end = current_ptr;
            let freed_space = heap_ptr as usize - new_heap_end;

            // 检查是否释放了足够空间
            if freed_space < requested_size as usize {
                // 空间不足，返回失败
                return 0;
            }

            // 实际移动对象
            let mut current_ptr = heap_base;
            for &(handle_idx, old_ptr, size) in &live_objects {
                if old_ptr != current_ptr {
                    // 移动对象（使用 ptr::copy 避免重叠问题）
                    unsafe {
                        std::ptr::copy(
                            data.as_ptr().add(old_ptr),
                            data.as_mut_ptr().add(current_ptr),
                            size,
                        );
                    }
                }
                // 更新 handle table
                let slot_addr = obj_table_ptr as usize + handle_idx * 4;
                data[slot_addr..slot_addr + 4].copy_from_slice(&(current_ptr as u32).to_le_bytes());
                current_ptr += size;
            }

            // 更新 heap_ptr 全局变量
            {
                let Some(Extern::Global(g)) = caller.get_export("__heap_ptr") else {
                    return 0;
                };
                g.set(&mut caller, Val::I32(new_heap_end as i32)).ok();
            }

            // 重置分配计数器
            {
                let mut counter = caller
                    .data()
                    .alloc_counter
                    .lock()
                    .expect("alloc_counter mutex");
                *counter = 0;
            }

            new_heap_end as i32
        },
    );

    // ── Import 22: console_error(i64) → () ────────────────────────────────
    // Already created above as `console_error`.

    // ── Import 27: set_timeout(i64, i64) → i64 ────────────────────────────

    vec![
        (22, gc_collect),
    ]
}
