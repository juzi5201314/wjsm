# Bug 修复：嵌套三元 Phi + 原型链查找 + 导入常量清理

日期: 2026-05-23

## 背景

发现 5 个 bug，经分析分为 3 个独立修复：

## Bug A: 嵌套三元表达式 ?: 始终返回 0

### 根因

`compile_branch_body` (compiler_control.rs:1057) 处理嵌套 Branch 终结器时，创建 if/else
后直接返回，不处理内层 merge block 的 Phi 指令。外层 `compile_structured` 的 Branch
处理器 (lines 417-434) 有 merge 后的 Phi 重发射逻辑，但 `compile_branch_body` 缺少这段。

内层 Phi 的 `phi_local` 从未被赋值，WASM local 默认为 0。

### 修复

在 `compile_branch_body` 的 Branch match arm 末尾（`self.emit(WasmInstruction::End)` 之后、
`Ok(...)` 之前），添加：

```rust
// 处理内层 merge block（如嵌套三元的中间 Phi）
let merge = if false_is_merge {
    false_idx
} else if true_is_merge {
    true_idx
} else {
    self.find_merge(blocks, true_idx, false_idx)
};

// 当 merge 已被编译，重新发射 Phi 指令
if self.compiled_blocks.contains(&merge)
    && !(true_terminates && false_terminates)
    && let Some(merge_block) = blocks.get(merge)
{
    for instruction in merge_block.instructions() {
        if let Instruction::Phi { dest, .. } = instruction
            && let Some(&phi_local) = self.phi_locals.get(&dest.0)
        {
            self.emit(WasmInstruction::LocalGet(phi_local));
            self.emit(WasmInstruction::LocalSet(self.local_idx(dest.0)));
        }
    }
}
```

### 影响

- 修复嵌套三元表达式返回值
- 解决 modules fixture 超时（三元返回 0 导致模块链接中死循环）
- 可能同时修复 WASM 类型不匹配 function[375-376]（三元导致的栈损坏连锁反应）

## Bug B: read_object_property_by_name 不遍历原型链

### 根因

`read_object_property_by_name` (runtime_values.rs:297) 和
`read_object_property_by_name_from_store` (runtime_heap.rs:378) 仅遍历自身属性槽
(obj_ptr+16)，不读取 obj_ptr+0 处的 proto_handle 来追踪 [[Prototype]] 链。

对象内存布局：
```
offset 0:  proto_handle (u32) — 0xFFFFFFFF = null proto
offset 4:  type (u8)
offset 5-7: padding
offset 8-11: capacity (u32)
offset 12-15: num_props (u32)
offset 16+: property slots (32 bytes each: name_id:u32 + flags:i32 + value:i64 + reserved:16)
```

### 修复

在自身属性查找失败后，通过 obj_ptr+0 读取 proto_handle，若不为 0xFFFFFFFF，
通过 handle table 解析原型对象指针，递归查找。使用 visited set 防止原型链环路。

```rust
// 自身属性未找到 → 沿原型链查找
let proto_handle = u32::from_le_bytes([data[obj_ptr], ...]);
if proto_handle != 0xFFFF_FFFF {
    if let Some(proto_ptr) = resolve_handle_idx(caller, proto_handle as usize) {
        // visited set 防止环路
        if visited.insert(proto_ptr) {
            return read_object_property_by_name_inner(caller, proto_ptr, prop_name, visited);
        }
    }
}
```

### 影响

- `async_iterator_from` 中 `Symbol.asyncIterator` 查找现在能遍历原型链
- 所有通过 `read_object_property_by_name` 的继承属性查找受益
- 大部分现有调用者传入的是自身属性名（`__map_handle__`、`"value"` 等），不受影响

## Bug C: 导入计数过期常量

### 根因

- 编译器 `num_imports = 375/389` (compiler_core.rs:15) — 死代码，从未使用
- 运行时 `Vec::with_capacity(378)` (lib.rs:195) — 比实际 381 少 3

实际导入数（经精确计数）：编译器和运行时都是 381，顺序一致。

### 修复

1. 删除 `compiler_core.rs:15` 的 `let num_imports = ...` 行
2. `lib.rs:195` 改为 `Vec::with_capacity(381)`

## 验证计划

1. 运行 `ternary_nested.js` → 应输出 `2, 4`（而非 `0, 0`）
2. 更新 `ternary_nested.expected` 快照
3. 运行全部 happy 路径集成测试
4. 运行 modules 集成测试（确认不再超时）
5. 运行 semantic snapshot 测试
