# Bug 修复：嵌套三元 Phi + 原型链查找 + 导入常量清理

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 修复 3 组 bug：嵌套三元 ?: 始终返回 0（WASM codegen Phi 缺失）、read_object_property_by_name 不遍历原型链、过期导入计数量。

**Architecture:** 3 个独立修复。(1) compiler_control.rs 的 compile_branch_body_with_context 在嵌套 Branch 后补充 merge block Phi 重发射。(2) runtime_values.rs 和 runtime_heap.rs 的属性查找函数在自身属性查找失败后沿 obj_ptr+0 的 proto_handle 递归遍历原型链，使用 HashSet 防环路。(3) 删除 compiler_core.rs 死代码 num_imports，修正 lib.rs Vec::with_capacity。

**Tech Stack:** Rust + wasm-encoder + wasmtime

---

### Task 1: 修复嵌套三元 Phi — 添加 merge block Phi 重发射

**Files:**
- Modify: `crates/wjsm-backend-wasm/src/compiler_control.rs:1107-1111`

- [ ] **Step 1: 在 compile_branch_body_with_context 的 Branch 处理器添加 merge Phi 重发射**

在 `compiler_control.rs` 的 `compile_branch_body_with_context` 函数中，找到 Branch 终结器处理（约 line 1107，`if true_terminates && false_terminates { ... }` 之后，`Ok(true_terminates && false_terminates)` 之前），插入 merge block Phi 重发射逻辑。

当前代码（lines 1104-1111）：
```rust
                self.emit(WasmInstruction::End);
                self.if_depth -= 1;

                if true_terminates && false_terminates {
                    self.emit(WasmInstruction::Unreachable);
                }

                Ok(true_terminates && false_terminates)
```

替换为：
```rust
                self.emit(WasmInstruction::End);
                self.if_depth -= 1;

                if true_terminates && false_terminates {
                    self.emit(WasmInstruction::Unreachable);
                }

                // 处理内层 merge block（如嵌套三元的中间 Phi）
                // compile_structured 的 Branch 处理器（lines 417-434）有此逻辑，
                // 但 compile_branch_body 缺少，导致内层 Phi 的 phi_local 从未被赋值。
                let merge = if false_is_merge {
                    false_idx
                } else if true_is_merge {
                    true_idx
                } else {
                    self.find_merge(blocks, true_idx, false_idx)
                };

                if self.compiled_blocks.contains(&merge)
                    && !(true_terminates && false_terminates)
                {
                    if let Some(merge_block) = blocks.get(merge) {
                        for instruction in merge_block.instructions() {
                            if let Instruction::Phi { dest, .. } = instruction {
                                if let Some(&phi_local) = self.phi_locals.get(&dest.0) {
                                    self.emit(WasmInstruction::LocalGet(phi_local));
                                    self.emit(WasmInstruction::LocalSet(
                                        self.local_idx(dest.0),
                                    ));
                                }
                            }
                        }
                    }
                }

                Ok(true_terminates && false_terminates)
```

- [ ] **Step 2: 编译验证**

```bash
cargo build 2>&1
```
Expected: 编译成功，无新增错误。

- [ ] **Step 3: 手动验证嵌套三元**

```bash
cargo run --quiet -- run fixtures/happy/ternary_nested.js 2>&1
```
Expected 输出: `2` 和 `4`（而非 `0` 和 `0`）。

- [ ] **Step 4: 更新 ternary_nested.expected 快照**

```bash
WJSM_UPDATE_FIXTURES=1 cargo test --package wjsm --test integration -- ternary_nested 2>&1
```
Expected: 快照更新为正确值 `2\n4`。

- [ ] **Step 5: Commit**

```bash
git add crates/wjsm-backend-wasm/src/compiler_control.rs fixtures/happy/ternary_nested.expected
git commit -m "fix: 嵌套三元 ?: 内层 Phi 未重发射导致始终返回 0"
```

---

### Task 2: 修复原型链查找 — read_object_property_by_name（caller 版本）

**Files:**
- Modify: `crates/wjsm-runtime/src/runtime_values.rs:296-360`

- [ ] **Step 1: 在 read_object_property_by_name 末尾添加原型链遍历**

当前 `read_object_property_by_name` 在自身属性未找到时返回 `None`（line 359）。在此处添加原型链回退查找。

阅读 obj_ptr+0 处的 proto_handle：
```rust
use std::collections::HashSet;
```

修改函数签名处的 imports（文件顶部已经有 `use super::*`，需要确认 `HashSet` 可用）。

在 line 358 `}` 之后、line 359 `None` 之前，替换 `None` 为原型链遍历：

```rust
    // 自身属性未找到 → 沿 [[Prototype]] 链查找
    let proto_handle = {
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
            return None;
        };
        let data = memory.data(&*caller);
        if obj_ptr + 4 > data.len() {
            return None;
        }
        u32::from_le_bytes([
            data[obj_ptr],
            data[obj_ptr + 1],
            data[obj_ptr + 2],
            data[obj_ptr + 3],
        ])
    };
    // 0xFFFF_FFFF 是 null prototype 的哨兵值
    if proto_handle == 0xFFFF_FFFF || proto_handle == 0 {
        return None;
    }
    let proto_ptr = resolve_handle_idx(caller, proto_handle as usize)?;
    // 防止原型链环路（如 proto 指向自身或被篡改）
    let mut visited: HashSet<usize> = HashSet::new();
    visited.insert(obj_ptr);
    read_object_property_by_name_proto_walk(caller, proto_ptr, prop_name, &mut visited)
```

然后在同一文件中添加辅助函数：

```rust
/// 沿原型链递归查找属性（带 visited set 防环路）
fn read_object_property_by_name_proto_walk(
    caller: &mut Caller<'_, RuntimeState>,
    obj_ptr: usize,
    prop_name: &str,
    visited: &mut HashSet<usize>,
) -> Option<i64> {
    if !visited.insert(obj_ptr) {
        return None; // 环路检测
    }

    // 先查自身属性
    let num_props = {
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
            return None;
        };
        let data = memory.data(&*caller);
        if obj_ptr + 16 > data.len() {
            return None;
        }
        u32::from_le_bytes([
            data[obj_ptr + 12],
            data[obj_ptr + 13],
            data[obj_ptr + 14],
            data[obj_ptr + 15],
        ]) as usize
    };
    let mut name_ids = Vec::with_capacity(num_props);
    {
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
            return None;
        };
        let data = memory.data(&*caller);
        for i in 0..num_props {
            let slot_offset = obj_ptr + 16 + i * 32;
            if slot_offset + 4 > data.len() {
                break;
            }
            name_ids.push(u32::from_le_bytes([
                data[slot_offset],
                data[slot_offset + 1],
                data[slot_offset + 2],
                data[slot_offset + 3],
            ]));
        }
    }
    for (i, name_id) in name_ids.iter().enumerate() {
        let name_bytes = read_string_bytes(caller, *name_id);
        if name_bytes == prop_name.as_bytes() {
            let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                return None;
            };
            let data = memory.data(&*caller);
            let slot_offset = obj_ptr + 16 + i * 32;
            if slot_offset + 32 > data.len() {
                return None;
            }
            return Some(i64::from_le_bytes([
                data[slot_offset + 8],
                data[slot_offset + 9],
                data[slot_offset + 10],
                data[slot_offset + 11],
                data[slot_offset + 12],
                data[slot_offset + 13],
                data[slot_offset + 14],
                data[slot_offset + 15],
            ]));
        }
    }

    // 自身未找到 → 继续沿原型链
    let proto_handle = {
        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
            return None;
        };
        let data = memory.data(&*caller);
        if obj_ptr + 4 > data.len() {
            return None;
        }
        u32::from_le_bytes([
            data[obj_ptr],
            data[obj_ptr + 1],
            data[obj_ptr + 2],
            data[obj_ptr + 3],
        ])
    };
    if proto_handle == 0xFFFF_FFFF || proto_handle == 0 {
        return None;
    }
    let proto_ptr = resolve_handle_idx(caller, proto_handle as usize)?;
    read_object_property_by_name_proto_walk(caller, proto_ptr, prop_name, visited)
}
```

- [ ] **Step 2: 编译验证**

```bash
cargo build 2>&1
```
Expected: 编译成功。

---

### Task 3: 修复原型链查找 — read_object_property_by_name_from_store（store 版本）

**Files:**
- Modify: `crates/wjsm-runtime/src/runtime_heap.rs:378-432`
- Modify: `crates/wjsm-runtime/src/runtime_promises.rs:746`

- [ ] **Step 1: 修改 read_object_property_by_name_from_store 签名，添加 obj_table_ptr_global 参数**

```rust
pub(crate) fn read_object_property_by_name_from_store(
    store: &mut Store<RuntimeState>,
    memory: &Memory,
    obj_table_ptr_global: &Global,
    obj_ptr: usize,
    prop_name: &str,
) -> Option<i64> {
```

- [ ] **Step 2: 在函数末尾（line 431 `None` 处）添加原型链回退**

自身属性查找失败后，读 obj_ptr+0 获取 proto_handle，遍历原型链：

```rust
    // 自身属性未找到 → 沿 [[Prototype]] 链查找
    let proto_handle = {
        let data = memory.data(&mut *store);
        if obj_ptr + 4 > data.len() {
            return None;
        }
        u32::from_le_bytes([
            data[obj_ptr],
            data[obj_ptr + 1],
            data[obj_ptr + 2],
            data[obj_ptr + 3],
        ])
    };
    if proto_handle == 0xFFFF_FFFF || proto_handle == 0 {
        return None;
    }
    let proto_ptr =
        resolve_handle_idx_from_store(store, memory, obj_table_ptr_global, proto_handle as usize)?;
    let mut visited: HashSet<usize> = HashSet::new();
    visited.insert(obj_ptr);
    read_object_property_by_name_from_store_proto_walk(
        store, memory, obj_table_ptr_global, proto_ptr, prop_name, &mut visited,
    )
}
```

并在同文件添加辅助函数：

```rust
fn read_object_property_by_name_from_store_proto_walk(
    store: &mut Store<RuntimeState>,
    memory: &Memory,
    obj_table_ptr_global: &Global,
    obj_ptr: usize,
    prop_name: &str,
    visited: &mut HashSet<usize>,
) -> Option<i64> {
    if !visited.insert(obj_ptr) {
        return None;
    }
    // 查自身属性
    let num_props = {
        let data = memory.data(&mut *store);
        if obj_ptr + 16 > data.len() {
            return None;
        }
        u32::from_le_bytes([
            data[obj_ptr + 12],
            data[obj_ptr + 13],
            data[obj_ptr + 14],
            data[obj_ptr + 15],
        ]) as usize
    };
    let mut name_ids = Vec::with_capacity(num_props);
    {
        let data = memory.data(&mut *store);
        for i in 0..num_props {
            let slot_offset = obj_ptr + 16 + i * 32;
            if slot_offset + 4 > data.len() {
                break;
            }
            name_ids.push(u32::from_le_bytes([
                data[slot_offset],
                data[slot_offset + 1],
                data[slot_offset + 2],
                data[slot_offset + 3],
            ]));
        }
    }
    for (index, name_id) in name_ids.iter().enumerate() {
        if read_string_bytes_from_store(store, memory, *name_id) == prop_name.as_bytes() {
            let data = memory.data(&mut *store);
            let slot_offset = obj_ptr + 16 + index * 32;
            if slot_offset + 16 > data.len() {
                return None;
            }
            return Some(i64::from_le_bytes([
                data[slot_offset + 8],
                data[slot_offset + 9],
                data[slot_offset + 10],
                data[slot_offset + 11],
                data[slot_offset + 12],
                data[slot_offset + 13],
                data[slot_offset + 14],
                data[slot_offset + 15],
            ]));
        }
    }
    // 继续沿原型链
    let proto_handle = {
        let data = memory.data(&mut *store);
        if obj_ptr + 4 > data.len() {
            return None;
        }
        u32::from_le_bytes([
            data[obj_ptr],
            data[obj_ptr + 1],
            data[obj_ptr + 2],
            data[obj_ptr + 3],
        ])
    };
    if proto_handle == 0xFFFF_FFFF || proto_handle == 0 {
        return None;
    }
    let proto_ptr = resolve_handle_idx_from_store(
        store, memory, obj_table_ptr_global, proto_handle as usize,
    )?;
    read_object_property_by_name_from_store_proto_walk(
        store, memory, obj_table_ptr_global, proto_ptr, prop_name, visited,
    )
}
```

- [ ] **Step 3: 更新调用点 — runtime_promises.rs:746**

```rust
        && let Some(then) = read_object_property_by_name_from_store(store, memory, obj_table_ptr_global, ptr, "then")
```

- [ ] **Step 4: 编译验证**

```bash
cargo build 2>&1
```
Expected: 编译成功。

- [ ] **Step 5: Commit**

```bash
git add crates/wjsm-runtime/src/runtime_values.rs crates/wjsm-runtime/src/runtime_heap.rs crates/wjsm-runtime/src/runtime_promises.rs
git commit -m "fix: read_object_property_by_name 沿 [[Prototype]] 链查找属性"
```

---

### Task 4: 清理过期导入常量

**Files:**
- Modify: `crates/wjsm-backend-wasm/src/compiler_core.rs:15`
- Modify: `crates/wjsm-runtime/src/lib.rs:195`

- [ ] **Step 1: 删除 compiler_core.rs 死代码**

```rust
    pub(crate) fn new_with_data_base(mode: CompileMode, data_base: u32) -> Self {
        let mut types = TypeSection::new();
```

删除 `let num_imports = if mode == CompileMode::Eval { 389u32 } else { 375u32 };` 这一行。

- [ ] **Step 2: 修正 lib.rs Vec::with_capacity**

```rust
    let mut imports: Vec<Extern> = Vec::with_capacity(381);
```

- [ ] **Step 3: 编译验证**

```bash
cargo build 2>&1
```
Expected: `num_imports` 未使用警告消失。

- [ ] **Step 4: Commit**

```bash
git add crates/wjsm-backend-wasm/src/compiler_core.rs crates/wjsm-runtime/src/lib.rs
git commit -m "chore: 删除过期 num_imports 死代码，修正 imports Vec capacity"
```

---

### Task 5: 全量验证

- [ ] **Step 1: 运行 happy 路径集成测试**

```bash
cargo test --package wjsm --test integration -- happy 2>&1
```
Expected: 全部通过，特别是 ternary_nested 输出正确值。

- [ ] **Step 2: 运行 modules 集成测试（确认不再超时）**

```bash
timeout 120 cargo test --package wjsm --test integration -- modules 2>&1
```
Expected: 在 120 秒内完成，全部通过。

- [ ] **Step 3: 运行 errors 集成测试**

```bash
cargo test --package wjsm --test integration -- errors 2>&1
```
Expected: 全部通过。

- [ ] **Step 4: 运行 semantic snapshot 测试**

```bash
cargo test -p wjsm-semantic --test lowering_snapshots 2>&1
```
Expected: 全部通过。

- [ ] **Step 5: Commit（如有快照更新）**

```bash
git add -A && git commit -m "test: 更新测试快照以反映嵌套三元修复" || echo "no changes"
```
