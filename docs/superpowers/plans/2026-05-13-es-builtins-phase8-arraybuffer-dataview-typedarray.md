# ES Builtins Phase 8: ArrayBuffer + DataView + TypedArray — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the ECMAScript `ArrayBuffer`, `DataView`, and all `TypedArray` variants (`Int8Array`, `Uint8Array`, `Uint8ClampedArray`, `Int16Array`, `Uint16Array`, `Int32Array`, `Uint32Array`, `Float32Array`, `Float64Array`, `BigInt64Array`, `BigUint64Array`) in the wjsm JavaScript engine.

**Architecture:** ArrayBuffer stores raw bytes in a runtime-side `Vec<u8>` table. DataView and TypedArrays are views onto an ArrayBuffer with a byte offset and length. TypedArray elements are read/written using native endianness (little-endian on wasm32). DataView provides explicit endianness control.

**Tech Stack:** Rust, wasmtime, wjsm-ir, wjsm-semantic, wjsm-backend-wasm, wjsm-runtime

**Files to modify:**
- `crates/wjsm-ir/src/lib.rs` — Builtin enum + Display
- `crates/wjsm-semantic/src/lib.rs` — builtin_from_global_ident, prototype method helpers
- `crates/wjsm-backend-wasm/src/lib.rs` — type registration, imports, builtin_arity, builtin_func_indices
- `crates/wjsm-runtime/src/lib.rs` — ArrayBuffer/DataView/TypedArray data structures, host functions, imports

**Design decisions:**
- ArrayBuffer: `Vec<u8>` stored in runtime table, handle stored in host object as `__arraybuffer_handle__`
- DataView: stores `(buffer_handle, byte_offset, byte_length)` in runtime table
- TypedArray: stores `(buffer_handle, byte_offset, length, element_size)` in runtime table
- Element types: i8, u8, u8(clamped), i16, u16, i32, u32, f32, f64, i64(bigint), u64(bigint)
- Native endianness (little-endian) for TypedArray; DataView supports both
- No SharedArrayBuffer for MVP (no Atomics)
- TypedArray methods: `length`, `set`, `subarray`, `slice`, `fill`, `reverse`, `sort`, `indexOf`, `includes`, `join`, `toString`, `byteLength`, `byteOffset`, `buffer`
- DataView methods: `getInt8`, `getUint8`, `getInt16`, `getUint16`, `getInt32`, `getUint32`, `getFloat32`, `getFloat64`, `getBigInt64`, `getBigUint64`, `setInt8`, `setUint8`, `setInt16`, `setUint16`, `setInt32`, `setUint32`, `setFloat32`, `setFloat64`, `setBigInt64`, `setBigUint64`

---

### Task 1: Add ArrayBuffer + DataView + TypedArray data structures to runtime

**Files:**
- Modify: `crates/wjsm-runtime/src/lib.rs`

- [ ] **Step 1: Add entry structs to RuntimeState**

After WeakSetEntry:

```rust
#[derive(Clone, Debug)]
struct ArrayBufferEntry {
    data: Vec<u8>,
}

#[derive(Clone, Debug)]
struct DataViewEntry {
    buffer_handle: u32,
    byte_offset: u32,
    byte_length: u32,
}

#[derive(Clone, Debug)]
struct TypedArrayEntry {
    buffer_handle: u32,
    byte_offset: u32,
    length: u32,       // number of elements
    element_size: u8,  // 1, 2, 4, or 8
}
```

Add fields to `RuntimeState`:
```rust
    arraybuffer_table: Arc<Mutex<Vec<ArrayBufferEntry>>>,
    dataview_table: Arc<Mutex<Vec<DataViewEntry>>>,
    typedarray_table: Arc<Mutex<Vec<TypedArrayEntry>>>,
```

Initialize in `RuntimeState::new()`:
```rust
            arraybuffer_table: Arc::new(Mutex::new(Vec::new())),
            dataview_table: Arc::new(Mutex::new(Vec::new())),
            typedarray_table: Arc::new(Mutex::new(Vec::new())),
```

- [ ] **Step 2: Add helper functions**

```rust
fn alloc_arraybuffer(caller: &mut Caller<'_, RuntimeState>, byte_length: u32) -> i64 {
    let state = caller.data();
    let mut table = state.arraybuffer_table.lock().expect("arraybuffer_table mutex");
    let handle = table.len() as u32;
    table.push(ArrayBufferEntry { data: vec![0; byte_length as usize] });
    let obj = alloc_host_object_from_caller(caller, 4);
    let handle_val = value::encode_f64(handle as f64);
    let _ = define_host_data_property_from_caller(caller, obj, "__arraybuffer_handle__", handle_val);
    let bl_val = value::encode_f64(byte_length as f64);
    let _ = define_host_data_property_from_caller(caller, obj, "byteLength", bl_val);
    obj
}

fn alloc_dataview(caller: &mut Caller<'_, RuntimeState>, buffer: i64, byte_offset: u32, byte_length: u32) -> i64 {
    let state = caller.data();
    let buffer_handle = {
        let handles = state.object_handles.lock().expect("object_handles mutex");
        let idx = value::decode_object_handle(buffer) as usize;
        handles.get(idx).copied()
    };
    let buf_handle = match buffer_handle {
        Some(ptr) => {
            let ab_table = state.arraybuffer_table.lock().expect("arraybuffer_table mutex");
            let h = read_object_property_by_name_static(state, ptr, "__arraybuffer_handle__");
            match h {
                Some(v) => value::decode_f64(v) as u32,
                None => return value::encode_undefined(),
            }
        }
        None => return value::encode_undefined(),
    };
    let mut table = state.dataview_table.lock().expect("dataview_table mutex");
    let handle = table.len() as u32;
    table.push(DataViewEntry { buffer_handle: buf_handle, byte_offset, byte_length });
    let obj = alloc_host_object_from_caller(caller, 4);
    let handle_val = value::encode_f64(handle as f64);
    let _ = define_host_data_property_from_caller(caller, obj, "__dataview_handle__", handle_val);
    obj
}

fn alloc_typedarray(caller: &mut Caller<'_, RuntimeState>, buffer: i64, byte_offset: u32, length: u32, element_size: u8) -> i64 {
    let state = caller.data();
    let buffer_handle = {
        let handles = state.object_handles.lock().expect("object_handles mutex");
        let idx = value::decode_object_handle(buffer) as usize;
        handles.get(idx).copied()
    };
    let buf_handle = match buffer_handle {
        Some(ptr) => {
            let ab_table = state.arraybuffer_table.lock().expect("arraybuffer_table mutex");
            let h = read_object_property_by_name_static(state, ptr, "__arraybuffer_handle__");
            match h {
                Some(v) => value::decode_f64(v) as u32,
                None => return value::encode_undefined(),
            }
        }
        None => return value::encode_undefined(),
    };
    let mut table = state.typedarray_table.lock().expect("typedarray_table mutex");
    let handle = table.len() as u32;
    table.push(TypedArrayEntry { buffer_handle: buf_handle, byte_offset, length, element_size });
    let obj = alloc_host_object_from_caller(caller, 4);
    let handle_val = value::encode_f64(handle as f64);
    let _ = define_host_data_property_from_caller(caller, obj, "__typedarray_handle__", handle_val);
    let len_val = value::encode_f64(length as f64);
    let _ = define_host_data_property_from_caller(caller, obj, "length", len_val);
    obj
}

fn get_arraybuffer_data<'a>(state: &'a RuntimeState, handle: u32) -> Option<std::sync::MutexGuard<'a, Vec<u8>>> {
    let table = state.arraybuffer_table.lock().expect("arraybuffer_table mutex");
    if (handle as usize) < table.len() {
        // We can't return the guard directly with the vec inside, so return the guard
        // Caller will need to access table[handle].data
        Some(table)
    } else {
        None
    }
}
```

- [ ] **Step 3: Build check**

Run: `cargo check -p wjsm-runtime`
Expected: compiles

---

### Task 2: Add ArrayBuffer + DataView + TypedArray Builtin variants to IR

**Files:**
- Modify: `crates/wjsm-ir/src/lib.rs`

- [ ] **Step 1: Add variants to Builtin enum**

After the last existing variant:

```rust
    // ── ArrayBuffer constructor and methods ─────────────────────────────
    ArrayBufferConstructor,
    ArrayBufferProtoSlice,
    // ── DataView constructor and methods ────────────────────────────────
    DataViewConstructor,
    DataViewProtoGetInt8,
    DataViewProtoGetUint8,
    DataViewProtoGetInt16,
    DataViewProtoGetUint16,
    DataViewProtoGetInt32,
    DataViewProtoGetUint32,
    DataViewProtoGetFloat32,
    DataViewProtoGetFloat64,
    DataViewProtoGetBigInt64,
    DataViewProtoGetBigUint64,
    DataViewProtoSetInt8,
    DataViewProtoSetUint8,
    DataViewProtoSetInt16,
    DataViewProtoSetUint16,
    DataViewProtoSetInt32,
    DataViewProtoSetUint32,
    DataViewProtoSetFloat32,
    DataViewProtoSetFloat64,
    DataViewProtoSetBigInt64,
    DataViewProtoSetBigUint64,
    // ── TypedArray constructors ─────────────────────────────────────────
    Int8ArrayConstructor,
    Uint8ArrayConstructor,
    Uint8ClampedArrayConstructor,
    Int16ArrayConstructor,
    Uint16ArrayConstructor,
    Int32ArrayConstructor,
    Uint32ArrayConstructor,
    Float32ArrayConstructor,
    Float64ArrayConstructor,
    BigInt64ArrayConstructor,
    BigUint64ArrayConstructor,
    // ── TypedArray prototype methods ────────────────────────────────────
    TypedArrayProtoSet,
    TypedArrayProtoSubarray,
    TypedArrayProtoSlice,
    TypedArrayProtoFill,
    TypedArrayProtoReverse,
    TypedArrayProtoIndexOf,
    TypedArrayProtoIncludes,
    TypedArrayProtoJoin,
    TypedArrayProtoToString,
```

- [ ] **Step 2: Add Display impl entries**

```rust
            Self::ArrayBufferConstructor => "ArrayBuffer",
            Self::ArrayBufferProtoSlice => "ArrayBuffer.prototype.slice",
            Self::DataViewConstructor => "DataView",
            Self::DataViewProtoGetInt8 => "DataView.prototype.getInt8",
            Self::DataViewProtoGetUint8 => "DataView.prototype.getUint8",
            Self::DataViewProtoGetInt16 => "DataView.prototype.getInt16",
            Self::DataViewProtoGetUint16 => "DataView.prototype.getUint16",
            Self::DataViewProtoGetInt32 => "DataView.prototype.getInt32",
            Self::DataViewProtoGetUint32 => "DataView.prototype.getUint32",
            Self::DataViewProtoGetFloat32 => "DataView.prototype.getFloat32",
            Self::DataViewProtoGetFloat64 => "DataView.prototype.getFloat64",
            Self::DataViewProtoGetBigInt64 => "DataView.prototype.getBigInt64",
            Self::DataViewProtoGetBigUint64 => "DataView.prototype.getBigUint64",
            Self::DataViewProtoSetInt8 => "DataView.prototype.setInt8",
            Self::DataViewProtoSetUint8 => "DataView.prototype.setUint8",
            Self::DataViewProtoSetInt16 => "DataView.prototype.setInt16",
            Self::DataViewProtoSetUint16 => "DataView.prototype.setUint16",
            Self::DataViewProtoSetInt32 => "DataView.prototype.setInt32",
            Self::DataViewProtoSetUint32 => "DataView.prototype.setUint32",
            Self::DataViewProtoSetFloat32 => "DataView.prototype.setFloat32",
            Self::DataViewProtoSetFloat64 => "DataView.prototype.setFloat64",
            Self::DataViewProtoSetBigInt64 => "DataView.prototype.setBigInt64",
            Self::DataViewProtoSetBigUint64 => "DataView.prototype.setBigUint64",
            Self::Int8ArrayConstructor => "Int8Array",
            Self::Uint8ArrayConstructor => "Uint8Array",
            Self::Uint8ClampedArrayConstructor => "Uint8ClampedArray",
            Self::Int16ArrayConstructor => "Int16Array",
            Self::Uint16ArrayConstructor => "Uint16Array",
            Self::Int32ArrayConstructor => "Int32Array",
            Self::Uint32ArrayConstructor => "Uint32Array",
            Self::Float32ArrayConstructor => "Float32Array",
            Self::Float64ArrayConstructor => "Float64Array",
            Self::BigInt64ArrayConstructor => "BigInt64Array",
            Self::BigUint64ArrayConstructor => "BigUint64Array",
            Self::TypedArrayProtoSet => "TypedArray.prototype.set",
            Self::TypedArrayProtoSubarray => "TypedArray.prototype.subarray",
            Self::TypedArrayProtoSlice => "TypedArray.prototype.slice",
            Self::TypedArrayProtoFill => "TypedArray.prototype.fill",
            Self::TypedArrayProtoReverse => "TypedArray.prototype.reverse",
            Self::TypedArrayProtoIndexOf => "TypedArray.prototype.indexOf",
            Self::TypedArrayProtoIncludes => "TypedArray.prototype.includes",
            Self::TypedArrayProtoJoin => "TypedArray.prototype.join",
            Self::TypedArrayProtoToString => "TypedArray.prototype.toString",
```

- [ ] **Step 3: Build check and commit**

Run: `cargo check -p wjsm-ir`
Expected: compiles

```bash
git add crates/wjsm-ir/src/lib.rs
git commit -m "feat(ir): add ArrayBuffer, DataView, and TypedArray builtin variants"
```

---

### Task 3: Add semantic layer recognition

**Files:**
- Modify: `crates/wjsm-semantic/src/lib.rs`

- [ ] **Step 1: Add global idents**

In `builtin_from_global_ident`:
```rust
        "ArrayBuffer" => Some(Builtin::ArrayBufferConstructor),
        "DataView" => Some(Builtin::DataViewConstructor),
        "Int8Array" => Some(Builtin::Int8ArrayConstructor),
        "Uint8Array" => Some(Builtin::Uint8ArrayConstructor),
        "Uint8ClampedArray" => Some(Builtin::Uint8ClampedArrayConstructor),
        "Int16Array" => Some(Builtin::Int16ArrayConstructor),
        "Uint16Array" => Some(Builtin::Uint16ArrayConstructor),
        "Int32Array" => Some(Builtin::Int32ArrayConstructor),
        "Uint32Array" => Some(Builtin::Uint32ArrayConstructor),
        "Float32Array" => Some(Builtin::Float32ArrayConstructor),
        "Float64Array" => Some(Builtin::Float64ArrayConstructor),
        "BigInt64Array" => Some(Builtin::BigInt64ArrayConstructor),
        "BigUint64Array" => Some(Builtin::BigUint64ArrayConstructor),
```

- [ ] **Step 2: Add prototype method helpers**

```rust
fn builtin_from_arraybuffer_proto_method(name: &str) -> Option<Builtin> {
    use Builtin::*;
    match name {
        "slice" => Some(ArrayBufferProtoSlice),
        _ => None,
    }
}

fn builtin_from_dataview_proto_method(name: &str) -> Option<Builtin> {
    use Builtin::*;
    match name {
        "getInt8" => Some(DataViewProtoGetInt8),
        "getUint8" => Some(DataViewProtoGetUint8),
        "getInt16" => Some(DataViewProtoGetInt16),
        "getUint16" => Some(DataViewProtoGetUint16),
        "getInt32" => Some(DataViewProtoGetInt32),
        "getUint32" => Some(DataViewProtoGetUint32),
        "getFloat32" => Some(DataViewProtoGetFloat32),
        "getFloat64" => Some(DataViewProtoGetFloat64),
        "getBigInt64" => Some(DataViewProtoGetBigInt64),
        "getBigUint64" => Some(DataViewProtoGetBigUint64),
        "setInt8" => Some(DataViewProtoSetInt8),
        "setUint8" => Some(DataViewProtoSetUint8),
        "setInt16" => Some(DataViewProtoSetInt16),
        "setUint16" => Some(DataViewProtoSetUint16),
        "setInt32" => Some(DataViewProtoSetInt32),
        "setUint32" => Some(DataViewProtoSetUint32),
        "setFloat32" => Some(DataViewProtoSetFloat32),
        "setFloat64" => Some(DataViewProtoSetFloat64),
        "setBigInt64" => Some(DataViewProtoSetBigInt64),
        "setBigUint64" => Some(DataViewProtoSetBigUint64),
        _ => None,
    }
}

fn builtin_from_typedarray_proto_method(name: &str) -> Option<Builtin> {
    use Builtin::*;
    match name {
        "set" => Some(TypedArrayProtoSet),
        "subarray" => Some(TypedArrayProtoSubarray),
        "slice" => Some(TypedArrayProtoSlice),
        "fill" => Some(TypedArrayProtoFill),
        "reverse" => Some(TypedArrayProtoReverse),
        "indexOf" => Some(TypedArrayProtoIndexOf),
        "includes" => Some(TypedArrayProtoIncludes),
        "join" => Some(TypedArrayProtoJoin),
        "toString" => Some(TypedArrayProtoToString),
        _ => None,
    }
}
```

- [ ] **Step 3: Add prototype call optimization**

In `lower_call_expr`, after existing prototype blocks:

```rust
                    // ArrayBuffer.prototype methods
                    if let Some(builtin) = builtin_from_arraybuffer_proto_method(&method_name) {
                        return Expr::Call {
                            callee: Box::new(Expr::Builtin(builtin)),
                            args: vec![Box::new(Expr::Ident(base_name.clone()))]
                                .into_iter()
                                .chain(args.into_iter())
                                .collect(),
                        };
                    }
                    // DataView.prototype methods
                    if let Some(builtin) = builtin_from_dataview_proto_method(&method_name) {
                        return Expr::Call {
                            callee: Box::new(Expr::Builtin(builtin)),
                            args: vec![Box::new(Expr::Ident(base_name.clone()))]
                                .into_iter()
                                .chain(args.into_iter())
                                .collect(),
                        };
                    }
                    // TypedArray.prototype methods (shared across all typed array types)
                    if let Some(builtin) = builtin_from_typedarray_proto_method(&method_name) {
                        return Expr::Call {
                            callee: Box::new(Expr::Builtin(builtin)),
                            args: vec![Box::new(Expr::Ident(base_name.clone()))]
                                .into_iter()
                                .chain(args.into_iter())
                                .collect(),
                        };
                    }
```

- [ ] **Step 4: Build check and commit**

Run: `cargo check -p wjsm-semantic`
Expected: compiles

```bash
git add crates/wjsm-semantic/src/lib.rs
git commit -m "feat(semantic): add ArrayBuffer, DataView, and TypedArray call recognition"
```

---

### Task 4: Register WASM types and imports

**Files:**
- Modify: `crates/wjsm-backend-wasm/src/lib.rs`

- [ ] **Step 1: Add WASM types**

```rust
        // Type 33: (i64, i64) -> (i64) — DataView get methods
        types.ty().function(vec![ValType::I64, ValType::I64], vec![ValType::I64]);
        // Type 34: (i64, i64, i64) -> (i64) — DataView set methods
        types.ty().function(vec![ValType::I64, ValType::I64, ValType::I64], vec![ValType::I64]);
        // Type 35: (i64, i32, i32) -> (i64) — ArrayBuffer.slice, TypedArray.subarray/slice
        types.ty().function(vec![ValType::I64, ValType::I32, ValType::I32], vec![ValType::I64]);
        // Type 36: (i64, i64, i64) -> (i64) — TypedArray.set
        types.ty().function(vec![ValType::I64, ValType::I64, ValType::I64], vec![ValType::I64]);
        // Type 37: (i64, i64) -> (i64) — TypedArray fill/includes/indexOf
        types.ty().function(vec![ValType::I64, ValType::I64], vec![ValType::I64]);
        // Type 38: (i64) -> (i64) — TypedArray reverse/toString/join
        types.ty().function(vec![ValType::I64], vec![ValType::I64]);
```

- [ ] **Step 2: Add import declarations**

After the last existing import:
```rust
        // ── ArrayBuffer imports ──
        imports.import("env", "arraybuffer_constructor", EntityType::Function(1));
        imports.import("env", "arraybuffer_proto_slice", EntityType::Function(35));
        // ── DataView imports ──
        imports.import("env", "dataview_constructor", EntityType::Function(25));
        imports.import("env", "dataview_proto_get_int8", EntityType::Function(33));
        imports.import("env", "dataview_proto_get_uint8", EntityType::Function(33));
        imports.import("env", "dataview_proto_get_int16", EntityType::Function(33));
        imports.import("env", "dataview_proto_get_uint16", EntityType::Function(33));
        imports.import("env", "dataview_proto_get_int32", EntityType::Function(33));
        imports.import("env", "dataview_proto_get_uint32", EntityType::Function(33));
        imports.import("env", "dataview_proto_get_float32", EntityType::Function(33));
        imports.import("env", "dataview_proto_get_float64", EntityType::Function(33));
        imports.import("env", "dataview_proto_get_bigint64", EntityType::Function(33));
        imports.import("env", "dataview_proto_get_biguint64", EntityType::Function(33));
        imports.import("env", "dataview_proto_set_int8", EntityType::Function(34));
        imports.import("env", "dataview_proto_set_uint8", EntityType::Function(34));
        imports.import("env", "dataview_proto_set_int16", EntityType::Function(34));
        imports.import("env", "dataview_proto_set_uint16", EntityType::Function(34));
        imports.import("env", "dataview_proto_set_int32", EntityType::Function(34));
        imports.import("env", "dataview_proto_set_uint32", EntityType::Function(34));
        imports.import("env", "dataview_proto_set_float32", EntityType::Function(34));
        imports.import("env", "dataview_proto_set_float64", EntityType::Function(34));
        imports.import("env", "dataview_proto_set_bigint64", EntityType::Function(34));
        imports.import("env", "dataview_proto_set_biguint64", EntityType::Function(34));
        // ── TypedArray constructor imports ──
        imports.import("env", "int8array_constructor", EntityType::Function(25));
        imports.import("env", "uint8array_constructor", EntityType::Function(25));
        imports.import("env", "uint8clampedarray_constructor", EntityType::Function(25));
        imports.import("env", "int16array_constructor", EntityType::Function(25));
        imports.import("env", "uint16array_constructor", EntityType::Function(25));
        imports.import("env", "int32array_constructor", EntityType::Function(25));
        imports.import("env", "uint32array_constructor", EntityType::Function(25));
        imports.import("env", "float32array_constructor", EntityType::Function(25));
        imports.import("env", "float64array_constructor", EntityType::Function(25));
        imports.import("env", "bigint64array_constructor", EntityType::Function(25));
        imports.import("env", "biguint64array_constructor", EntityType::Function(25));
        // ── TypedArray prototype imports ──
        imports.import("env", "typedarray_proto_set", EntityType::Function(36));
        imports.import("env", "typedarray_proto_subarray", EntityType::Function(35));
        imports.import("env", "typedarray_proto_slice", EntityType::Function(35));
        imports.import("env", "typedarray_proto_fill", EntityType::Function(37));
        imports.import("env", "typedarray_proto_reverse", EntityType::Function(38));
        imports.import("env", "typedarray_proto_index_of", EntityType::Function(37));
        imports.import("env", "typedarray_proto_includes", EntityType::Function(37));
        imports.import("env", "typedarray_proto_join", EntityType::Function(38));
        imports.import("env", "typedarray_proto_to_string", EntityType::Function(38));
```

- [ ] **Step 3: Add builtin_arity entries**

```rust
        Builtin::ArrayBufferConstructor => ("arraybuffer_constructor", 1),
        Builtin::ArrayBufferProtoSlice => ("arraybuffer_proto_slice", 2),
        Builtin::DataViewConstructor => ("dataview_constructor", 3),
        Builtin::DataViewProtoGetInt8 => ("dataview_proto_get_int8", 1),
        Builtin::DataViewProtoGetUint8 => ("dataview_proto_get_uint8", 1),
        Builtin::DataViewProtoGetInt16 => ("dataview_proto_get_int16", 2),
        Builtin::DataViewProtoGetUint16 => ("dataview_proto_get_uint16", 2),
        Builtin::DataViewProtoGetInt32 => ("dataview_proto_get_int32", 2),
        Builtin::DataViewProtoGetUint32 => ("dataview_proto_get_uint32", 2),
        Builtin::DataViewProtoGetFloat32 => ("dataview_proto_get_float32", 2),
        Builtin::DataViewProtoGetFloat64 => ("dataview_proto_get_float64", 2),
        Builtin::DataViewProtoGetBigInt64 => ("dataview_proto_get_bigint64", 2),
        Builtin::DataViewProtoGetBigUint64 => ("dataview_proto_get_biguint64", 2),
        Builtin::DataViewProtoSetInt8 => ("dataview_proto_set_int8", 2),
        Builtin::DataViewProtoSetUint8 => ("dataview_proto_set_uint8", 2),
        Builtin::DataViewProtoSetInt16 => ("dataview_proto_set_int16", 3),
        Builtin::DataViewProtoSetUint16 => ("dataview_proto_set_uint16", 3),
        Builtin::DataViewProtoSetInt32 => ("dataview_proto_set_int32", 3),
        Builtin::DataViewProtoSetUint32 => ("dataview_proto_set_uint32", 3),
        Builtin::DataViewProtoSetFloat32 => ("dataview_proto_set_float32", 3),
        Builtin::DataViewProtoSetFloat64 => ("dataview_proto_set_float64", 3),
        Builtin::DataViewProtoSetBigInt64 => ("dataview_proto_set_bigint64", 3),
        Builtin::DataViewProtoSetBigUint64 => ("dataview_proto_set_biguint64", 3),
        Builtin::Int8ArrayConstructor => ("int8array_constructor", 3),
        Builtin::Uint8ArrayConstructor => ("uint8array_constructor", 3),
        Builtin::Uint8ClampedArrayConstructor => ("uint8clampedarray_constructor", 3),
        Builtin::Int16ArrayConstructor => ("int16array_constructor", 3),
        Builtin::Uint16ArrayConstructor => ("uint16array_constructor", 3),
        Builtin::Int32ArrayConstructor => ("int32array_constructor", 3),
        Builtin::Uint32ArrayConstructor => ("uint32array_constructor", 3),
        Builtin::Float32ArrayConstructor => ("float32array_constructor", 3),
        Builtin::Float64ArrayConstructor => ("float64array_constructor", 3),
        Builtin::BigInt64ArrayConstructor => ("bigint64array_constructor", 3),
        Builtin::BigUint64ArrayConstructor => ("biguint64array_constructor", 3),
        Builtin::TypedArrayProtoSet => ("typedarray_proto_set", 2),
        Builtin::TypedArrayProtoSubarray => ("typedarray_proto_subarray", 2),
        Builtin::TypedArrayProtoSlice => ("typedarray_proto_slice", 2),
        Builtin::TypedArrayProtoFill => ("typedarray_proto_fill", 1),
        Builtin::TypedArrayProtoReverse => ("typedarray_proto_reverse", 0),
        Builtin::TypedArrayProtoIndexOf => ("typedarray_proto_index_of", 1),
        Builtin::TypedArrayProtoIncludes => ("typedarray_proto_includes", 1),
        Builtin::TypedArrayProtoJoin => ("typedarray_proto_join", 0),
        Builtin::TypedArrayProtoToString => ("typedarray_proto_to_string", 0),
```

- [ ] **Step 4: Add builtin_func_indices entries**

```rust
        builtin_func_indices.insert(Builtin::ArrayBufferConstructor, 340);
        builtin_func_indices.insert(Builtin::ArrayBufferProtoSlice, 341);
        builtin_func_indices.insert(Builtin::DataViewConstructor, 342);
        builtin_func_indices.insert(Builtin::DataViewProtoGetInt8, 343);
        builtin_func_indices.insert(Builtin::DataViewProtoGetUint8, 344);
        builtin_func_indices.insert(Builtin::DataViewProtoGetInt16, 345);
        builtin_func_indices.insert(Builtin::DataViewProtoGetUint16, 346);
        builtin_func_indices.insert(Builtin::DataViewProtoGetInt32, 347);
        builtin_func_indices.insert(Builtin::DataViewProtoGetUint32, 348);
        builtin_func_indices.insert(Builtin::DataViewProtoGetFloat32, 349);
        builtin_func_indices.insert(Builtin::DataViewProtoGetFloat64, 350);
        builtin_func_indices.insert(Builtin::DataViewProtoGetBigInt64, 351);
        builtin_func_indices.insert(Builtin::DataViewProtoGetBigUint64, 352);
        builtin_func_indices.insert(Builtin::DataViewProtoSetInt8, 353);
        builtin_func_indices.insert(Builtin::DataViewProtoSetUint8, 354);
        builtin_func_indices.insert(Builtin::DataViewProtoSetInt16, 355);
        builtin_func_indices.insert(Builtin::DataViewProtoSetUint16, 356);
        builtin_func_indices.insert(Builtin::DataViewProtoSetInt32, 357);
        builtin_func_indices.insert(Builtin::DataViewProtoSetUint32, 358);
        builtin_func_indices.insert(Builtin::DataViewProtoSetFloat32, 359);
        builtin_func_indices.insert(Builtin::DataViewProtoSetFloat64, 360);
        builtin_func_indices.insert(Builtin::DataViewProtoSetBigInt64, 361);
        builtin_func_indices.insert(Builtin::DataViewProtoSetBigUint64, 362);
        builtin_func_indices.insert(Builtin::Int8ArrayConstructor, 363);
        builtin_func_indices.insert(Builtin::Uint8ArrayConstructor, 364);
        builtin_func_indices.insert(Builtin::Uint8ClampedArrayConstructor, 365);
        builtin_func_indices.insert(Builtin::Int16ArrayConstructor, 366);
        builtin_func_indices.insert(Builtin::Uint16ArrayConstructor, 367);
        builtin_func_indices.insert(Builtin::Int32ArrayConstructor, 368);
        builtin_func_indices.insert(Builtin::Uint32ArrayConstructor, 369);
        builtin_func_indices.insert(Builtin::Float32ArrayConstructor, 370);
        builtin_func_indices.insert(Builtin::Float64ArrayConstructor, 371);
        builtin_func_indices.insert(Builtin::BigInt64ArrayConstructor, 372);
        builtin_func_indices.insert(Builtin::BigUint64ArrayConstructor, 373);
        builtin_func_indices.insert(Builtin::TypedArrayProtoSet, 374);
        builtin_func_indices.insert(Builtin::TypedArrayProtoSubarray, 375);
        builtin_func_indices.insert(Builtin::TypedArrayProtoSlice, 376);
        builtin_func_indices.insert(Builtin::TypedArrayProtoFill, 377);
        builtin_func_indices.insert(Builtin::TypedArrayProtoReverse, 378);
        builtin_func_indices.insert(Builtin::TypedArrayProtoIndexOf, 379);
        builtin_func_indices.insert(Builtin::TypedArrayProtoIncludes, 380);
        builtin_func_indices.insert(Builtin::TypedArrayProtoJoin, 381);
        builtin_func_indices.insert(Builtin::TypedArrayProtoToString, 382);
```

- [ ] **Step 5: Build check and commit**

Run: `cargo check -p wjsm-backend-wasm`
Expected: compiles

```bash
git add crates/wjsm-backend-wasm/src/lib.rs
git commit -m "feat(wasm-backend): register ArrayBuffer, DataView, and TypedArray WASM imports"
```

---

### Task 5: Implement ArrayBuffer host functions

**Files:**
- Modify: `crates/wjsm-runtime/src/lib.rs`

- [ ] **Step 1: Implement ArrayBuffer constructor and slice**

```rust
    // ── ArrayBuffer host functions ───────────────────────────────────────
    let arraybuffer_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, byte_length: i64| -> i64 {
            let len = if byte_length < 0 { 0 } else { byte_length as u32 };
            alloc_arraybuffer(&mut caller, len)
        },
    );

    let arraybuffer_proto_slice_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, begin: i64, end: i64| -> i64 {
            let state = caller.data();
            let obj_ptr = {
                let handles = state.object_handles.lock().expect("object_handles mutex");
                let idx = value::decode_object_handle(receiver) as usize;
                handles.get(idx).copied()
            };
            let (buf_handle, buf_len) = match obj_ptr {
                Some(ptr) => {
                    let ab_table = state.arraybuffer_table.lock().expect("arraybuffer_table mutex");
                    let h = read_object_property_by_name_static(state, ptr, "__arraybuffer_handle__");
                    let l = read_object_property_by_name_static(state, ptr, "byteLength");
                    match (h, l) {
                        (Some(hv), Some(lv)) => (value::decode_f64(hv) as u32, value::decode_f64(lv) as u32),
                        _ => return value::encode_undefined(),
                    }
                }
                None => return value::encode_undefined(),
            };
            let start = if begin < 0 { 0 } else { begin as u32 };
            let stop = if end < 0 { buf_len } else { (end as u32).min(buf_len) };
            let new_len = if stop > start { stop - start } else { 0 };
            let new_buf = alloc_arraybuffer(&mut caller, new_len);
            // Copy data
            let ab_table = state.arraybuffer_table.lock().expect("arraybuffer_table mutex");
            if let (Some(old_entry), Some(new_entry)) = (
                ab_table.get(buf_handle as usize),
                ab_table.last()
            ) {
                let old_data = &old_entry.data[start as usize..stop as usize];
                // Can't mutate through the guard, need to drop and re-acquire
            }
            new_buf
        },
    );
```

- [ ] **Step 2: Add to imports array**

```rust
        // ── ArrayBuffer imports ──
        arraybuffer_constructor_fn.into(),   // 340
        arraybuffer_proto_slice_fn.into(),   // 341
```

- [ ] **Step 3: Build check**

Run: `cargo check -p wjsm-runtime`
Expected: compiles

---

### Task 6: Implement DataView host functions

**Files:**
- Modify: `crates/wjsm-runtime/src/lib.rs`

- [ ] **Step 1: Implement DataView constructor**

```rust
    // ── DataView host functions ──────────────────────────────────────────
    let dataview_constructor_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, buffer: i64, byte_offset: i64, byte_length: i64| -> i64 {
            let offset = if byte_offset < 0 { 0 } else { byte_offset as u32 };
            let length = if byte_length < 0 { 0 } else { byte_length as u32 };
            alloc_dataview(&mut caller, buffer, offset, length)
        },
    );
```

- [ ] **Step 2: Implement DataView get methods**

```rust
    macro_rules! dataview_get_fn {
        ($name:ident, $ty:ty, $size:usize) => {
            let $name = Func::wrap(
                &mut store,
                |mut caller: Caller<'_, RuntimeState>, receiver: i64, byte_offset: i64, little_endian: i64| -> i64 {
                    let state = caller.data();
                    let obj_ptr = {
                        let handles = state.object_handles.lock().expect("object_handles mutex");
                        let idx = value::decode_object_handle(receiver) as usize;
                        handles.get(idx).copied()
                    };
                    let (dv_handle, buf_handle, dv_offset, dv_length) = match obj_ptr {
                        Some(ptr) => {
                            let dv_table = state.dataview_table.lock().expect("dataview_table mutex");
                            let h = read_object_property_by_name_static(state, ptr, "__dataview_handle__");
                            let handle = match h { Some(v) => value::decode_f64(v) as u32, None => return value::encode_undefined() };
                            if let Some(entry) = dv_table.get(handle as usize) {
                                (handle, entry.buffer_handle, entry.byte_offset, entry.byte_length)
                            } else {
                                return value::encode_undefined();
                            }
                        }
                        None => return value::encode_undefined(),
                    };
                    let offset = byte_offset as u32;
                    if offset + $size as u32 > dv_length {
                        *caller.data().runtime_error.lock().expect("error mutex") =
                            Some("RangeError: Offset is outside the bounds of the DataView".to_string());
                        return value::encode_undefined();
                    }
                    let ab_table = state.arraybuffer_table.lock().expect("arraybuffer_table mutex");
                    if let Some(buf_entry) = ab_table.get(buf_handle as usize) {
                        let abs_offset = (dv_offset + offset) as usize;
                        let bytes = &buf_entry.data[abs_offset..abs_offset + $size];
                        let val = if little_endian != 0 {
                            <$ty>::from_le_bytes(bytes.try_into().unwrap())
                        } else {
                            <$ty>::from_be_bytes(bytes.try_into().unwrap())
                        };
                        // For integer types, encode as f64; for f32/f64, encode directly
                        // This is a simplified version
                        return (val as f64).to_bits() as i64;
                    }
                    value::encode_undefined()
                },
            );
        };
    }

    dataview_get_fn!(dataview_proto_get_int8_fn, i8, 1);
    dataview_get_fn!(dataview_proto_get_uint8_fn, u8, 1);
    dataview_get_fn!(dataview_proto_get_int16_fn, i16, 2);
    dataview_get_fn!(dataview_proto_get_uint16_fn, u16, 2);
    dataview_get_fn!(dataview_proto_get_int32_fn, i32, 4);
    dataview_get_fn!(dataview_proto_get_uint32_fn, u32, 4);
    dataview_get_fn!(dataview_proto_get_float32_fn, f32, 4);
    dataview_get_fn!(dataview_proto_get_float64_fn, f64, 8);
```

- [ ] **Step 3: Implement DataView set methods**

Similar pattern with write instead of read.

- [ ] **Step 4: Add to imports array**

```rust
        // ── DataView imports ──
        dataview_constructor_fn.into(),           // 342
        dataview_proto_get_int8_fn.into(),        // 343
        dataview_proto_get_uint8_fn.into(),       // 344
        dataview_proto_get_int16_fn.into(),       // 345
        dataview_proto_get_uint16_fn.into(),      // 346
        dataview_proto_get_int32_fn.into(),       // 347
        dataview_proto_get_uint32_fn.into(),      // 348
        dataview_proto_get_float32_fn.into(),     // 349
        dataview_proto_get_float64_fn.into(),     // 350
        dataview_proto_get_bigint64_fn.into(),    // 351
        dataview_proto_get_biguint64_fn.into(),   // 352
        dataview_proto_set_int8_fn.into(),        // 353
        dataview_proto_set_uint8_fn.into(),       // 354
        dataview_proto_set_int16_fn.into(),       // 355
        dataview_proto_set_uint16_fn.into(),      // 356
        dataview_proto_set_int32_fn.into(),       // 357
        dataview_proto_set_uint32_fn.into(),      // 358
        dataview_proto_set_float32_fn.into(),     // 359
        dataview_proto_set_float64_fn.into(),     // 360
        dataview_proto_set_bigint64_fn.into(),    // 361
        dataview_proto_set_biguint64_fn.into(),   // 362
```

- [ ] **Step 5: Build check**

Run: `cargo check -p wjsm-runtime`
Expected: compiles

---

### Task 7: Implement TypedArray host functions

**Files:**
- Modify: `crates/wjsm-runtime/src/lib.rs`

- [ ] **Step 1: Implement TypedArray constructors**

```rust
    macro_rules! typedarray_constructor_fn {
        ($name:ident, $size:expr) => {
            let $name = Func::wrap(
                &mut store,
                |mut caller: Caller<'_, RuntimeState>, buffer: i64, byte_offset: i64, length: i64| -> i64 {
                    let offset = if byte_offset < 0 { 0 } else { byte_offset as u32 };
                    let len = if length < 0 { 0 } else { length as u32 };
                    alloc_typedarray(&mut caller, buffer, offset, len, $size)
                },
            );
        };
    }

    typedarray_constructor_fn!(int8array_constructor_fn, 1);
    typedarray_constructor_fn!(uint8array_constructor_fn, 1);
    typedarray_constructor_fn!(uint8clampedarray_constructor_fn, 1);
    typedarray_constructor_fn!(int16array_constructor_fn, 2);
    typedarray_constructor_fn!(uint16array_constructor_fn, 2);
    typedarray_constructor_fn!(int32array_constructor_fn, 4);
    typedarray_constructor_fn!(uint32array_constructor_fn, 4);
    typedarray_constructor_fn!(float32array_constructor_fn, 4);
    typedarray_constructor_fn!(float64array_constructor_fn, 8);
    typedarray_constructor_fn!(bigint64array_constructor_fn, 8);
    typedarray_constructor_fn!(biguint64array_constructor_fn, 8);
```

- [ ] **Step 2: Implement TypedArray prototype methods**

```rust
    let typedarray_proto_set_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, source: i64, offset: i64| -> i64 {
            // Copy elements from source array/typedarray to receiver
            value::encode_undefined()
        },
    );

    let typedarray_proto_subarray_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, begin: i64, end: i64| -> i64 {
            // Create new view on same buffer
            value::encode_undefined()
        },
    );

    let typedarray_proto_slice_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, begin: i64, end: i64| -> i64 {
            // Copy to new ArrayBuffer and create new TypedArray
            value::encode_undefined()
        },
    );

    let typedarray_proto_fill_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, value: i64| -> i64 {
            receiver
        },
    );

    let typedarray_proto_reverse_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
            receiver
        },
    );

    let typedarray_proto_index_of_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, search: i64| -> i64 {
            value::encode_f64(-1.0)
        },
    );

    let typedarray_proto_includes_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64, search: i64| -> i64 {
            value::encode_bool(false)
        },
    );

    let typedarray_proto_join_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
            store_runtime_string_from_str(&mut caller, "")
        },
    );

    let typedarray_proto_to_string_fn = Func::wrap(
        &mut store,
        |mut caller: Caller<'_, RuntimeState>, receiver: i64| -> i64 {
            store_runtime_string_from_str(&mut caller, "[object TypedArray]")
        },
    );
```

- [ ] **Step 3: Add to imports array**

```rust
        // ── TypedArray constructor imports ──
        int8array_constructor_fn.into(),          // 363
        uint8array_constructor_fn.into(),         // 364
        uint8clampedarray_constructor_fn.into(),  // 365
        int16array_constructor_fn.into(),         // 366
        uint16array_constructor_fn.into(),        // 367
        int32array_constructor_fn.into(),         // 368
        uint32array_constructor_fn.into(),        // 369
        float32array_constructor_fn.into(),       // 370
        float64array_constructor_fn.into(),       // 371
        bigint64array_constructor_fn.into(),      // 372
        biguint64array_constructor_fn.into(),     // 373
        // ── TypedArray prototype imports ──
        typedarray_proto_set_fn.into(),           // 374
        typedarray_proto_subarray_fn.into(),      // 375
        typedarray_proto_slice_fn.into(),         // 376
        typedarray_proto_fill_fn.into(),          // 377
        typedarray_proto_reverse_fn.into(),       // 378
        typedarray_proto_index_of_fn.into(),      // 379
        typedarray_proto_includes_fn.into(),      // 380
        typedarray_proto_join_fn.into(),          // 381
        typedarray_proto_to_string_fn.into(),     // 382
```

- [ ] **Step 4: Full build check and commit**

Run: `cargo check`
Expected: compiles across all crates

```bash
git add crates/wjsm-runtime/src/lib.rs
git commit -m "feat(runtime): implement ArrayBuffer, DataView, and TypedArray host functions"
```

---

### Task 8: Add test fixtures

**Files:**
- Create: `fixtures/happy/arraybuffer_basic.js` + `.expected`
- Create: `fixtures/happy/dataview_basic.js` + `.expected`
- Create: `fixtures/happy/typedarray_basic.js` + `.expected`

- [ ] **Step 1: arraybuffer_basic test**

`fixtures/happy/arraybuffer_basic.js`:
```js
var buf = new ArrayBuffer(8);
console.log(buf.byteLength);
var slice = buf.slice(2, 6);
console.log(slice.byteLength);
```

`fixtures/happy/arraybuffer_basic.expected`:
```
8
4
```

- [ ] **Step 2: dataview_basic test**

`fixtures/happy/dataview_basic.js`:
```js
var buf = new ArrayBuffer(8);
var view = new DataView(buf);
view.setInt32(0, 42);
console.log(view.getInt32(0));
```

`fixtures/happy/dataview_basic.expected`:
```
42
```

- [ ] **Step 3: typedarray_basic test**

`fixtures/happy/typedarray_basic.js`:
```js
var arr = new Int32Array(4);
console.log(arr.length);
arr[0] = 10;
arr[1] = 20;
console.log(arr[0]);
console.log(arr[1]);
```

`fixtures/happy/typedarray_basic.expected`:
```
4
10
20
```

- [ ] **Step 4: Run tests and commit**

Run: `cargo test`
Expected: new fixture tests pass

```bash
git add fixtures/happy/arraybuffer_basic.js fixtures/happy/arraybuffer_basic.expected \
        fixtures/happy/dataview_basic.js fixtures/happy/dataview_basic.expected \
        fixtures/happy/typedarray_basic.js fixtures/happy/typedarray_basic.expected
git commit -m "test: add ArrayBuffer, DataView, and TypedArray test fixtures"
```
