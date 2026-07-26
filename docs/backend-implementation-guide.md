# 新后端实现指南

本文档说明如何为 wjsm 实现一个新的执行后端（如 native / C / cranelift-direct），
使 JS 代码能在非 wasmtime 环境中执行。

## 架构概览

```
wjsm-host          后端无关能力 trait（HostRuntime / HeapContext / ConsoleHost / GcHost / ObjectHost / AsyncHost）
     ↑ 实现
wjsm-host-wasm     wasmtime 后端（当前唯一生产后端）
wjsm-host-native   ← 你要实现的新后端

wjsm-gc            后端无关 GC 算法（MarkSweepV2 / G1V2 / ZgcV2 / HandleTableV2）
     ↑ 依赖
wjsm-host-wasm     使用 wjsm-gc 算法 + SharedHeapMemory（wasmtime shared memory64）
wjsm-host-native   使用 wjsm-gc 算法 + 你的 HeapMemory 实现

wjsm-host-wasm     编译编排 owner（compile_source / compile_source_with_debug，
                   parse → lower → compile 内联在 host-wasm；经 wjsm-runtime facade re-export）
wjsm-host-native   调用 wjsm_runtime::compile_source 编译 JS → WASM，再用自己的方式执行
```

## 步骤 1：实现 `HeapMemory` trait

`wjsm-gc` 的 GC 算法通过 `HeapMemory` trait 访问堆内存。你需要提供一个实现：

```rust
use wjsm_gc::heap::{HeapMemory, HeapAddress, HeapMemoryError, GrowableHeapMemory};

pub struct NativeHeap {
    // 你的堆内存存储
}

impl HeapMemory for NativeHeap {
    fn byte_len(&self) -> u64 { /* 当前已提交字节数 */ }
    fn load_word(&self, addr: HeapAddress) -> Result<u64, HeapMemoryError> { /* 原子读 8 字节 */ }
    fn store_word(&self, addr: HeapAddress, val: u64) -> Result<(), HeapMemoryError> { /* 原子写 8 字节 */ }
    fn copy_from(&self, addr: HeapAddress, bytes: &[u8]) -> Result<(), HeapMemoryError> { /* 写字节 */ }
    fn copy_to(&self, addr: HeapAddress, len: u64) -> Result<Vec<u8>, HeapMemoryError> { /* 读字节 */ }
}

impl GrowableHeapMemory for NativeHeap {
    fn maximum_byte_len(&self) -> u64 { /* 最大可增长字节数 */ }
    fn grow_to(&self, byte_len: u64) -> Result<(), String> { /* 增长堆 */ }
}
```

`NativeHeapMemory`（`wjsm-gc` 内置）是一个参考实现，用 `Box<[AtomicU64]>` 模拟内存，
可用于测试。

## 步骤 2：实现 `HandleRegionBackend` trait

`HandleTableV2` 需要一个 handle region 后端来存储 8-byte atomic handle entries：

```rust
use wjsm_gc::heap::{HandleRegionBackend, HANDLE_REGION_BYTES};

pub struct NativeHandleRegion {
    base: *mut u8,  // 你的 handle region 基址
}

impl HandleRegionBackend for NativeHandleRegion {
    fn base_ptr(&self) -> *mut u8 { self.base }
}
```

`PlatformHandleRegion`（`wjsm-gc` 内置）是默认实现，用平台虚拟内存（`mmap`/`VirtualAlloc`）。
如果你的后端不需要 wasmtime shared memory，直接用 `HandleTableV2::new(layout)` 即可
（内部使用 `PlatformHandleRegion`）。

## 步骤 3：使用 GC 算法

```rust
use wjsm_gc::{MarkSweepV2, G1V2, ZgcV2, ManagedHeapLayout, RootSnapshot};

let layout = ManagedHeapLayout::new(max_heap_size, control_reserved)?;
let heap = NativeHeap::new(...);

// MarkSweep
let mut gc = MarkSweepV2::new(heap, layout)?;
let roots = RootSnapshot::new(epoch, live_handles);
let report = gc.collect(&roots, |handle| { /* 清理侧表 */ })?;

// G1
let mut g1 = G1V2::new(heap, layout, worker_count)?;
let report = g1.collect_full(&roots, |handle| { /* 清理侧表 */ })?;

// ZGC
let mut zgc = ZgcV2::new(heap, layout)?;
let outcome = zgc.safepoint_step(&roots, budget, |handle| { /* 清理侧表 */ })?;
```

## 步骤 4：实现 `HeapContext` trait（可选，用于 HostRuntime 委托）

如果你要实现 `wjsm-host` 的 `HostRuntime` trait（使 builtins 可复用），需要实现
`HeapContext`：

```rust
use wjsm_host::{HeapContext, Value, Handle, GcOutcome, AsyncHookEvent};

pub struct NativeHeapContext<'a> {
    // 你的运行时上下文
}

impl HeapContext for NativeHeapContext<'_> {
    fn read_shadow_arg(&mut self, args_base: i32, index: u32) -> Value { /* ... */ }
    fn read_string_utf8(&mut self, val: Value) -> String { /* ... */ }
    fn write_output(&mut self, bytes: &[u8]) { /* ... */ }
    fn resolve_handle(&mut self, handle: Handle) -> bool { /* ... */ }
    fn array_length(&mut self, handle: Handle) -> Option<u32> { /* ... */ }
    fn array_elem(&mut self, handle: Handle, index: u32) -> Option<Value> { /* ... */ }
    fn get_property(&mut self, handle: Handle, key: &str) -> Option<Value> { /* ... */ }
    fn alloc_object(&mut self, capacity: u32) -> Value { /* ... */ }
    fn alloc_array(&mut self, capacity: u32) -> Value { /* ... */ }
    fn set_property(&mut self, handle: Handle, key: &str, value: Value) { /* ... */ }
    fn delete_property(&mut self, handle: Handle, key: &str) -> bool { /* ... */ }
    fn gc_collect(&mut self) -> GcOutcome { /* ... */ }
    fn heap_used_bytes(&mut self) -> usize { /* ... */ }
    fn async_emit_begin(&mut self) { /* ... */ }
    fn async_hook_callbacks(&mut self, event: AsyncHookEvent, promise: bool) -> Vec<Value> { /* ... */ }
    fn async_emit_end(&mut self) { /* ... */ }
    fn push_temp_roots(&mut self, roots: &[Value]) -> usize { /* ... */ }
    fn truncate_temp_roots(&mut self, len: usize) { /* ... */ }
}
```

`WasmHeapContext`（`wjsm-host-wasm`）是参考实现，用 `Caller<RuntimeState>` 实现。

## 步骤 5：执行编译后的代码

`wjsm_runtime::compile_source`（实现位于 `wjsm-host-wasm`）把 JS 编译为 WASM 字节。你的后端需要：
1. 调用 `compile_source` 获得 WASM 字节
2. 用自己的方式执行（如：把 WASM 翻译为 native 代码，或用另一个 WASM 运行时）
3. 在执行过程中通过 `HeapContext` 提供 host 能力

## 参考实现

- `crates/wjsm-host-wasm/src/heap_context_impl.rs`：`WasmHeapContext`（`HeapContext` 的 wasmtime 实现）
- `crates/wjsm-gc/src/heap/native_memory.rs`：`NativeHeapMemory`（`HeapMemory` 的纯 Rust 实现）
- `crates/wjsm-gc/src/heap/handle.rs`：`PlatformHandleRegion`（`HandleRegionBackend` 的平台虚拟内存实现）

## 注意事项

- `wjsm-gc` 的 GC 算法是**线程安全**的（`Send + Sync`），可在多线程环境中使用
- `HeapMemory` 的 `load_word`/`store_word` 必须是**原子操作**（`AtomicU64`），因为 GC worker 线程会并发访问
- `HandleTableV2` 的 handle entry 是8-byte atomic（高 48 bit 地址 + 低 16 bit 状态）
- `RootSnapshot` 是 GC 的根集合接口：你的 mutator 需要在 safepoint 时收集所有活跃 handle，构造 `RootSnapshot` 传给 GC 算法
- `GrowableHeapMemory::grow_to` 在分配路径被调用，需要能动态扩展堆
