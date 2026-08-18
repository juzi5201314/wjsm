# NativeHeapMemory 与逻辑堆地址

对象堆需要远超 4 GiB 的地址空间。生产 owner 是 `wjsm-gc::NativeHeapMemory`：OS 虚拟内存上的 mmap reservation，外加从 `object_heap_base()` 起算的 64 位逻辑字节偏移。没有 Wasmtime shared memory、没有 wasm page 计数，也没有独立导入的 `memory` / `shadow_memory` / `heap_memory`。

历史上 ADR 0010 用过「shared memory64」这个 Wasm 时代名字。那只是旧文档用语，不是当前实现。

## NativeHeapMemory

`NativeHeapMemory`（`crates/wjsm-gc/src/heap/native_memory.rs`）按 `ManagedHeapLayout` 工作：

1. `reserve` 完整逻辑容量，初始不提交物理页。
2. JS / GC 只使用逻辑地址（`HeapAddress`）。句柄、side table、snapshot 也只存逻辑地址。
3. 宿主指针只在本实现内部把逻辑偏移换成虚拟地址，不流出堆模块。
4. 提交窗口按 64 KiB granule 单调增长；已发布地址稳定。
5. 字访问走 `AtomicU64`。mutator 与 GC worker 共享同一 reservation。

`NativeVmContext::heap_object_delta` 是「逻辑地址 → 进程虚拟地址」的差值。属性快链要先加这个 delta，才能对真实映射做 load。

测试用 `TestHeapMemory`（进程内缓冲），同样走 `GrowableHeapMemory`，不进入生产路径。

## 并发

ZGC / 并发标记的工作线程是宿主 OS 线程。它们通过 `NativeHeapMemory` 的原子字访问同步，不经过 WASM `atomics` 指令。JS `Atomics` 只作用于 `SharedArrayBuffer` 的独立 backing，与对象堆后备存储无关。

## 深入了解

- [ManagedHeap 架构](managed-heap.md)
- [Native ABI 索引](../reference/abi-index.md)
- [用户侧的内存配置](../../user/configuration/memory.md)
