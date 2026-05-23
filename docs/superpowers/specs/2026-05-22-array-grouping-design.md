# Array Grouping 设计文档

日期: 2026-05-22
状态: 待实现

## 1. 概述

实现 ECMAScript `Object.groupBy` 和 `Map.groupBy`（ES2024, Stage 4, test262 feature: "array-grouping"）。

两个静态方法共享同一个 `GroupBy` 抽象操作，但返回不同类型：

| API | 第一参数 | 第二参数 | 回调签名 | 返回值 |
|---|---|---|---|---|
| `Object.groupBy(items, callbackfn)` | 可迭代对象 | 回调 | `(element, index) → key` | null-prototype object `{ key: [elem,…] }` |
| `Map.groupBy(items, callbackfn)` | 可迭代对象 | 回调 | `(element, index) → key` | Map `key → [elem,…]` |

## 2. 架构

### 2.1 整体数据流

```
User code: Object.groupBy(arr, x => x.kind)
  → parser: swc_ast::CallExpr(Object.groupBy, [items, callbackfn])
  → semantic: builtin_from_static_member("Object", "groupBy") → Builtin::ObjectGroupBy
  → IR: instr %r = call builtin.object.groupBy(%items, %callbackfn)
  → backend: LocalGet %items, LocalGet %callbackfn, Call(host_import_319)
  → runtime host function: ObjectGroupBy(items, callbackfn)
       ↓
    1. GetIterator(items) → iteratorRecord
    2. Loop: IteratorStep → next, IteratorValue → element
    3. call_wasm_callback(callbackfn, undefined, [elem, index]) → key
    4. AddValueToKeyedGroup(groups, key, element, keyCoercion)
    5. index++
    6. Return groups → null-prototype object / Map
```

### 2.2 关键决策

- **内联迭代**：不在 WASM backend 生成循环代码，而是在 Rust host function 中直接迭代。匹配 Array.forEach / Array.map 的现有模式。
- **两个独立 Builtin**：Object.groupBy 和 Map.groupBy 的返回值构造逻辑差异大（Object 用字符串 key + 属性操作，Map 用 SameValueZero + 内部数据结构），拆分为两个 Builtin 变体比共享一个泛化函数更清晰。
- **Host function 签名**：`fn(i64, i64) -> i64`，两个 JS 值直接传 i64（同 Object.is / Object.setPrototypeOf 模式），不走 shadow stack。
- **不实现 thisArg**：Spec 中 Object.groupBy / Map.groupBy 的 callback 不接受 thisArg。
- **迭代方式**：严格按 spec 使用 `GetIterator` 协议。可为数组提供快速路径跳过 `Symbol.iterator` 查找（等价于 `[...items]`），但语义上不改变行为。

### 2.3 GroupBy 抽象操作映射

ECMAScript spec 定义 GroupBy(items, callbackfn, keyCoercion) 包含以下步骤：

1. RequireInternalSlot(callbackfn, [[Call]]) → 运行时检查 value::is_callable
2. GetIterator(items, sync) → inline iterator_from 逻辑
3. groups ← empty List → runtime 中对应 HashMap / object
4. index ← 0
5. Loop:
   a. IteratorStep(iteratorRecord) → iterator_next host function
   b. If false, break
   c. IteratorValue(iteratorRecord) → iterator_value host function
   d. Call(callbackfn, undefined, « value, F(index) ») → call_wasm_callback
   e. 根据 keyCoercion 将 key 加入到 groups：
      - Object.groupBy → ToPropertyKey(key) 后做 defineProperty
      - Map.groupBy → SameValueZero(key) 后做 Map.set

## 3. 实现步骤

### 3.1 wjsm-ir: Builtin 枚举

在 `wjsm-ir/src/builtin.rs` 新增变体（在 Object 静态方法区附近）：

```rust
// ── Array grouping ──
ObjectGroupBy,
MapGroupBy,
```

Display 映射（在 `crates/wjsm-ir/src/builtin.rs` Display impl）：
```rust
Self::ObjectGroupBy => "object.group_by",
Self::MapGroupBy => "map.group_by",
```

### 3.2 wjsm-semantic: 静态成员映射

在 `wjsm-semantic/src/builtins.rs` 的 `builtin_from_static_member` 中：

```rust
"Object" => match property {
    // ... 现有 ObjectKeys, ObjectValues 等
    "groupBy" => Some(Builtin::ObjectGroupBy),
    _ => None,
},
// 新增 Map 静态成员分支（当前 Map 不走 builtin_from_static_member，
// 因为现有 Map 用法都是 Map() / map.set / map.get 等）：
"Map" => match property {
    "groupBy" => Some(Builtin::MapGroupBy),
    _ => None,
},
```

注意：Map.groupBy 还可能触发 `builtin_from_global_ident` 路径（当 Map 作为整体被引用时返回 MapConstructor），但 `map.groupBy` 的静态成员访问走的是 `builtin_from_static_member`。需确保 `Map` bare ident 仍映射到 `MapConstructor`。

### 3.3 wjsm-backend-wasm: Func indices + 编译

**compiler_core.rs** — 新增 func indices（319-320 可用，上一个连续值是 318 PrivateHas）：
```rust
builtin_func_indices.insert(Builtin::ObjectGroupBy, 319);
builtin_func_indices.insert(Builtin::MapGroupBy, 320);
```

**compiler_builtins.rs** — 添加到 2-arg 直接传值模式（与 ObjectSetPrototypeOf / ObjectIs 同分支）：
```rust
Builtin::ObjectGroupBy | Builtin::MapGroupBy => {
    let a = args.first().context("groupBy expects 2 args")?;
    let b = args.get(1).context("groupBy expects 2 args")?;
    self.emit(WasmInstruction::LocalGet(self.local_idx(a.0)));
    self.emit(WasmInstruction::LocalGet(self.local_idx(b.0)));
    let func_idx = self.builtin_func_indices
        .get(builtin)
        .copied()
        .with_context(|| format!("no WASM func index for builtin {builtin}"))?;
    self.emit(WasmInstruction::Call(func_idx));
    if let Some(d) = dest {
        self.emit(WasmInstruction::LocalSet(self.local_idx(d.0)));
    }
    Ok(())
}
```

### 3.4 wjsm-runtime: Host function 实现

#### ObjectGroupBy(items, callbackfn) → null-prototype object

Signature: `fn(&mut Caller<RuntimeState>, items: i64, callbackfn: i64) -> i64`

逻辑：
1. 检查 `value::is_callable(callbackfn)`，否 → 设置 TypeError，返回 undefined
2. 创建 null-prototype object：
   ```
   let obj = object_create_null(&mut caller);
   ```
   使用 `alloc_object` + 设置 `__proto__` 为 null（参考 Object.create(null) 的实现）
3. 获取迭代器：
   - 如果是 Array（检查 `value::is_array(items)`），直接读取 `length` 和索引访问——这是 spec 行为等价的快速路径
   - 否则使用 `GetIterator` 协议：调用 iterator_from 内联实现的完整逻辑（见 `core.rs` 的 iterator_from）
4. index = 0
5. Loop:
   - 获取 next element（数组：`read_array_elem`；迭代器：advance + `iterator_value`）
   - 如 done → break
   - `call_wasm_callback(caller, callbackfn, undefined, &[element, index_val])` → key
   - 将 key 转为字符串（ToPropertyKey）：通过 `toString` 或 `String(key)` 语义
   - 在 result object 上查找 key 属性：
     - 存在且是数组 → push element（扩展数组 length + write_array_elem）
     - 不存在 → 创建新数组 `[element]`，`define_host_data_property(result, key_str, arr)`
   - index++
6. 返回 result

关键 helpers（全部已存在于 runtime）：
- `resolve_handle` / `resolve_object_ptr` — 解析 NaN-boxed 值到内存指针
- `read_array_elem` / `write_array_elem` / `write_array_length` — 数组操作
- `alloc_array` / `alloc_object` — 分配对象
- `define_host_data_property` — 在 object 上设置属性
- `call_wasm_callback` — 调用 JS 回调
- `find_property_slot_by_name_id` / `read_object_property_by_name` — 属性查找
- `store_runtime_string` — 字符串存储
- `read_value_string_bytes` — 值转字符串

#### MapGroupBy(items, callbackfn) → Map

Signature: `fn(&mut Caller<RuntimeState>, items: i64, callbackfn: i64) -> i64`

逻辑：
1. 同上检查 callbackfn 可调用
2. 创建 Map：`alloc_map_entry` + 设置 `__map_handle__` 属性（参考 `map_constructor_fn` 的实现）
3. 迭代逻辑同 ObjectGroupBy（共享内联迭代代码）
4. 对每个 (element, index)：
   - `call_wasm_callback(callbackfn, undefined, &[element, index_val])` → key
   - 在 Map 内部查找 key（SameValueZero 比较，参考 `map_proto_get_fn` / `map_proto_set_fn` 中的 `same_value_zero` 逻辑）
   - 找到 → 值是一个数组 → push element
   - 没找到 → 创建 `[element]`，`map_set_internal(map_handle, key, array)`
5. 返回 Map 值

Map 操作复用已有的基础设施：
- `caller.data().map_table` — Map 数据存储
- `same_value_zero` 函数
- Map 的 `keys` / `values` 向量

### 3.5 wjsm-test262: Feature 注册

在 `crates/wjsm-test262/src/config.rs` 的 `SUPPORTED_FEATURES` 中追加：
```rust
"array-grouping",
```

### 3.6 测试

| 类型 | 文件 | 预期 |
|---|---|---|
| Happy fixture | `fixtures/happy/object_group_by_basic.js` | `Object.groupBy([1,2,3], x => x%2)` → `{1:[1,3], 0:[2]}` |
| Happy fixture | `fixtures/happy/object_group_by_string.js` | `Object.groupBy(["a","b","c"], x => x)` |
| Happy fixture | `fixtures/happy/map_group_by_basic.js` | `Map.groupBy([1,2,3], x => x%2)` |
| Happy fixture | `fixtures/happy/group_by_iterable.js` | 自定义可迭代对象作为 items |
| Happy fixture | `fixtures/happy/group_by_map_as_items.js` | 用 Map 作为 items 输入 |
| Error fixture | `fixtures/errors/group_by_non_callable.js` | callbackfn 非 callable → TypeError |
| Error fixture | `fixtures/errors/group_by_non_iterable.js` | items 非 iterable → TypeError |
| IR snapshot | `fixtures/semantic/group_by.ir` | 验证 lowering 后 IR shape |

## 4. 错误处理

| 条件 | 错误类型 | 行为 |
|---|---|---|
| callbackfn 不是可调用对象 | TypeError | 设置 runtime_error，返回 undefined |
| items 不是可迭代对象（既不是数组，也没有 Symbol.iterator） | TypeError | 设置 runtime_error，返回 undefined |
| callbackfn 执行过程中抛出异常 | 传播 | `call_wasm_callback` 返回 Err → 中断迭代，返回 undefined |
| 迭代过程中抛出异常（next() 抛出等） | 传播 | 异常传播，迭代终止 |
| items 为 null/undefined | TypeError | 在 GetIterator 步骤失败 |

## 5. 边界情况

- **空数组/可迭代对象返回空**：返回空 object / 空 Map
- **所有元素分组到同一个 key**：结果为单个 key 的数组
- **Symbol 作为 key（Object.groupBy）**：Symbol 通过 ToPropertyKey 转为字符串（如 `"Symbol(desc)"`），作为属性名
- **Symbol 作为 key（Map.groupBy）**：Map 使用 SameValueZero，Symbol 保持唯一性
- **undefined 作为 key（Object.groupBy）**：ToPropertyKey(undefined) → `"undefined"`
- **null / undefined 作为 items**：TypeError（GetIterator 失败）
- **字符串作为 items**：字符串实现了迭代器协议，按字符分组
- **稀疏数组的 holes**：数组的 `[Symbol.iterator]()` 对 holes 返回 `undefined`。`Object.groupBy([1,,3], x => x)` → hole 位置 callback 收到 `undefined`

## 6. 文件变更清单

| # | 文件 | 变更 |
|---|---|---|
| 1 | `crates/wjsm-ir/src/builtin.rs` | +ObjectGroupBy, +MapGroupBy 枚举变体 + Display |
| 2 | `crates/wjsm-semantic/src/builtins.rs` | +"groupBy" → ObjectGroupBy/MapGroupBy 映射 |
| 3 | `crates/wjsm-backend-wasm/src/compiler_core.rs` | +func indices 319, 320 |
| 4 | `crates/wjsm-backend-wasm/src/compiler_builtins.rs` | +2-arg 编译分支 |
| 5 | `crates/wjsm-runtime/src/host_imports/array_object.rs` | +object_group_by_fn 定义 + import 注册 |
| 6 | `crates/wjsm-runtime/src/host_imports/collections_buffers.rs` | +map_group_by_fn 定义 + import 注册 |
| 7 | `crates/wjsm-test262/src/config.rs` | +"array-grouping" |
| 8 | `fixtures/happy/object_group_by_basic.js` | Happy-path fixture |
| 9 | `fixtures/happy/object_group_by_string.js` | Happy-path fixture |
| 10 | `fixtures/happy/map_group_by_basic.js` | Happy-path fixture |
| 11 | `fixtures/happy/group_by_map_as_items.js` | Happy-path fixture（Map 作为 items） |
| 12 | `fixtures/errors/group_by_non_callable.js` | Error-path fixture |
| 13 | `fixtures/errors/group_by_non_iterable.js` | Error-path fixture |
| 14 | `tests/integration/fixtures.rs` | 新增 fixture 自动被发现，无需修改 |
