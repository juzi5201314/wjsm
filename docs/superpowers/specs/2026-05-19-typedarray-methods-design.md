# TypedArray 方法全量补全 — 设计文档

## 概述

补全 11 种 TypedArray 变体的全部原型方法（23 个）、新增 BigInt64Array/BigUint64Array 构造器，
并将现有的 `set`/`slice`/`subarray` 三个桩方法实现完整逻辑。

### 现状

| 组件 | 已实现 | 缺失 |
|------|--------|------|
| 构造器 | Int8Array ~ Float64Array (9 个) | BigInt64Array, BigUint64Array |
| 原型 getter | length, byteLength, byteOffset | buffer getter |
| 原型方法 | set/slice/subarray（空桩） | 其余 20 个方法 |
| 语义路由 | 无 `builtin_from_typedarray_proto_method()` | `typedArr.method()` 走通用路径失败 |

## 新增 Builtin 变体列表

### 构造器（2 个）
- `BigInt64ArrayConstructor`, `BigUint64ArrayConstructor`

### 简单方法 — Type 3/16 固定参数（5 个）
- `TypedArrayProtoFill` — `(this, value, start, end)` → Type 12 shadow stack
- `TypedArrayProtoReverse` — `(this)` → Type 3
- `TypedArrayProtoIndexOf` — `(this, search, fromIndex)` → Type 16
- `TypedArrayProtoLastIndexOf` — `(this, search, fromIndex)` → Type 16 (新增)
- `TypedArrayProtoIncludes` — `(this, search, fromIndex)` → Type 16

### 复杂方法 — Type 12 shadow stack 回调（12 个）
- `TypedArrayProtoForEach` — `(this, callback, thisArg)`
- `TypedArrayProtoMap` — `(this, callback, thisArg)`
- `TypedArrayProtoFilter` — `(this, callback, thisArg)`
- `TypedArrayProtoReduce` — `(this, callback, initialValue)`
- `TypedArrayProtoReduceRight` — `(this, callback, initialValue)`
- `TypedArrayProtoFind` — `(this, callback, thisArg)`
- `TypedArrayProtoFindIndex` — `(this, callback, thisArg)`
- `TypedArrayProtoSome` — `(this, callback, thisArg)`
- `TypedArrayProtoEvery` — `(this, callback, thisArg)`
- `TypedArrayProtoSort` — `(this, compareFn)`
- `TypedArrayProtoCopyWithin` — `(this, target, start, end)` → 不需要回调，但需要 Type 16 三参
- `TypedArrayProtoAt` — `(this, index)` → Type 2 双参

### 字符串方法 — Type 3/16（2 个）
- `TypedArrayProtoJoin` — `(this, separator)` → Type 16
- `TypedArrayProtoToString` — `(this)` → Type 3

### 迭代器方法 — Type 3（3 个，复用 ArrayIterator）
- `TypedArrayProtoEntries` — `(this)` → Type 3
- `TypedArrayProtoKeys` — `(this)` → Type 3
- `TypedArrayProtoValues` — `(this)` → Type 3

### 已存在需实现的桩方法（3 个）
- `TypedArrayProtoSet` — 实现完整逻辑
- `TypedArrayProtoSlice` — 实现完整逻辑
- `TypedArrayProtoSubarray` — 实现完整逻辑

**共计：25 个新 Builtin 变体 + 3 个修复**

## 调用模式分类

| 函数签名 (WASM) | WASM Type | 适用方法 |
|---|---|---|
| `(i64) -> i64` | Type 3 | reverse, toString, entries, keys, values, TypedArrayProtoLength, ByteLength, ByteOffset |
| `(i64, i64) -> i64` | Type 2 (现有) | at |
| `(i64, i64, i64) -> i64` | Type 16 (现有) | indexOf, lastIndexOf, includes, join, set, slice, subarray, copyWithin |
| `(i64, i64, i32, i32) -> i64` | Type 12 (shadow stack) | fill, forEach, map, filter, reduce, reduceRight, find, findIndex, some, every, sort |

## 流水线四层变更

### Layer 1 — IR (`wjsm-ir/src/builtin.rs`)
- 新增 25 个 `Builtin` 枚举变体
- 新增对应 `Display` 实现

### Layer 2 — Semantic (`wjsm-semantic/src/builtins.rs` + `lowerer_calls_eval.rs`)
- 新增 `builtin_from_typedarray_proto_method()` 函数
- 在 `builtin_from_global_ident()` 中新增 `BigInt64Array`/`BigUint64Array`
- 在 `lower_call_expr` 中新增 TypedArray 方法分发块
- 新增 `builtin_call_signature` 条目

### Layer 3 — Backend WASM (`wjsm-backend-wasm/src/compiler_core.rs` + `compiler_builtins.rs`)
- 注册新 WASM import + func_indices
- 在 `compile_builtin_call` 中新增 TypedArray 方法分支
- 简单方法 → 直接 `Call(func_idx)`；回调方法 → `compile_proto_method_call()`

### Layer 4 — Runtime (`wjsm-runtime/src/host_imports/collections_buffers.rs` + `runtime_render.rs`)
- 实现 28 个 host 函数
- BigInt64Array/BigUint64Array 构造器（element_size=8）
- 回调方法：从 shadow stack 读 callback/thisArg，遍历 TypedArray 元素调 callback
- TypedArray 渲染：`render_value` 中新增 `__typedarray_handle__` 分支

## TypedArray 元素读写模式

所有 TypedArray 方法通过以下路径操作底层数据：
1. 从 receiver 对象读取 `__typedarray_handle__` 获取 handle
2. 从 `typedarray_table[handle]` 获取 `TypedArrayEntry { buffer_handle, byte_offset, length, element_size }`
3. 从 `arraybuffer_table[buffer_handle].data` 获取底层 `Vec<u8>`
4. 元素 at index i 的字节偏移 = `byte_offset + i * element_size`
5. 使用 `from_le_bytes`/`to_le_bytes` 进行类型转换

**特殊处理：**
- Uint8ClampedArray：写入时 clamp 到 [0, 255]，round half-to-even
- BigInt64Array/BigUint64Array：元素以 bigint i64 值读写
- Float32Array/Float64Array：使用 `f32::from_le_bytes`/`f64::from_le_bytes` 读写

## 回调调用模式

回调方法（forEach/map/filter/reduce/find 等）复用 Array.prototype 相同的 Type 12 shadow stack 模式：
1. WASM 层面通过 `compile_proto_method_call()` 把 callback 和 thisArg 写入 shadow stack
2. Runtime 层面 `Func::wrap` 签名为 `(caller, env_obj: i64, this_val: i64, args_base: i32, args_count: i32) -> i64`
3. 从 shadow stack 读取 callback 和 thisArg
4. 遍历 TypedArray 元素，对每个元素调用 `call_function` 从 runtime 回调到 WASM

## 渲染（render_value）

新增 `__typedarray_handle__` 检测分支，渲染为 `Int8Array(3) [1, 2, 3]` 格式。

## 测试策略

新增 fixture 文件覆盖：
- `fixtures/happy/typedarray_basics.js` — 构造、get/set 元素
- `fixtures/happy/typedarray_methods.js` — set, subarray, slice, fill
- `fixtures/happy/typedarray_iteration.js` — forEach, map, filter, reduce, find, some, every
- `fixtures/happy/typedarray_sort_reverse.js` — sort, reverse
- `fixtures/happy/typedarray_search.js` — indexOf, lastIndexOf, includes
- `fixtures/happy/typedarray_bigint.js` — BigInt64Array, BigUint64Array
- `fixtures/happy/typedarray_iterators.js` — entries, keys, values
- `fixtures/happy/typedarray_copy_util.js` — copyWithin, at, join, toString
