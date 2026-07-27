# 新后端实现指南

本文档说明如何为 wjsm 实现一个新的执行后端（如 native / C / cranelift-direct / llvm），
使 JS 代码能在非 wasmtime 环境中编译并执行。wjsm 已完成多后端完全支撑：

- **所有 JS 语义算法**位于 `wjsm-builtins`，以 `<E: ExecContext>` 泛型单态化，零 `dyn`
- **对象模型 `HeapAccessV2<M>`** 位于 `wjsm-gc`，泛型 `M: GrowableHeapMemory`
- **`JsBackend` trait** 位于 `wjsm-host`，定义 IR → 制品 → 执行的编译后端契约
- **CLI `Target` enum** 静态分发到具体后端，零 vtable 开销

## 架构概览

```
wjsm-host          后端无关宿主能力 trait
                   ├ HostRuntime        ← 历史 marker（blanket impl），真实接缝是 ExecContext
                   ├ HeapContext        ← 堆/侧表最小操作集（18 方法）
                   ├ ExecContext        ← builtins 完整能力（~330 方法，按域分组）
                   └ JsBackend          ← 编译后端契约（compile/execute，静态分发）

wjsm-gc            后端无关 GC 算法 + 对象模型
                   ├ MarkSweepV2/G1V2/ZgcV2  （泛型 <M: GrowableHeapMemory>）
                   ├ HandleTableV2            （8-byte atomic handle 表）
                   ├ HeapMemory/GrowableHeapMemory trait
                   └ HeapAccessV2<M>          ← 对象模型（property slots/proto chain，1093 行）

wjsm-builtins      后端无关 JS 语义算法（<E: ExecContext> 泛型单态化）
                   ├ promise/promise_combinators/proxy_reflect_async
                   ├ render/json/core/core_async
                   ├ atomics/streams/fetch/modules
                   └ collections_buffers/gc/array_object/...

wjsm-host-wasm     wasmtime 后端（当前唯一生产后端）
                   ├ WasmBackend               （JsBackend 实现）
                   ├ WasmExecContext            （ExecContext 实现，~5491 行按域拆分为 17 子模块）
                   ├ SharedHeapMemory           （GrowableHeapMemory 实现，wasmtime shared memory64）
                   ├ compile_source             （编译编排：parse → lower → compile）
                   └ host I/O 桥               （fetch_http / streams_fetch_body / reentrant_async）

wjsm-backend-jit   JIT 后端 stub（JsBackend 实现，compile/execute 均 bail!）
wjsm-backend-wasm  IR → WASM 字节 codegen（被 host-wasm 依赖）

你的新后端          实现 HeapMemory + ExecContext + JsBackend 即可接入
```

## 六步接入法

### 步骤 1：实现 `HeapMemory` + `GrowableHeapMemory` trait

`wjsm-gc` 的 GC 算法通过这两个 trait 访问堆内存。你的后端提供一个 `M: GrowableHeapMemory` 实现：

```rust
use wjsm_gc::heap::{HeapMemory, HeapAddress, HeapMemoryError, GrowableHeapMemory};

pub struct NativeHeap {
    // 你的堆内存存储（如 Box<[AtomicU64]> 或 mmap 区域）
}

impl HeapMemory for NativeHeap {
    fn byte_len(&self) -> u64 { /* 当前已提交字节数 */ }
    fn load_word(&self, addr: HeapAddress) -> Result<u64, HeapMemoryError> { /* 原子读 8 字节 */ }
    fn store_word(&self, addr: HeapAddress, val: u64) -> Result<(), HeapMemoryError> { /* 原子写 8 字节 */ }
    fn copy_from(&self, addr: HeapAddress, bytes: &[u8]) -> Result<(), HeapMemoryError> { /* 写字节 */ }
    fn copy_to(&self, addr: HeapAddress, len: u64) -> Result<Vec<u8>, HeapMemoryError> { /* 读字节 */ }
    fn read_c_string(&self, addr: HeapAddress) -> Result<Vec<u8>, HeapMemoryError> {
        // 默认实现按 copy_to 分块扫描；可用 memchr 快路径覆写
    }
}

impl GrowableHeapMemory for NativeHeap {
    fn maximum_byte_len(&self) -> u64 { /* 最大可增长字节数 */ }
    fn grow_to(&self, byte_len: u64) -> Result<(), String> { /* 增长堆 */ }
}
```

`NativeHeapMemory`（`wjsm-gc` 内置）是参考实现，用 `Box<[AtomicU64]>` 模拟内存。
`SharedHeapMemory`（`wjsm-host-wasm`）是 wasmtime shared memory64 的实现。

### 步骤 2：装配 `HeapAccessV2<M>` 对象模型

`HeapAccessV2<M: GrowableHeapMemory>`（`wjsm-gc/src/heap_access.rs`）是 wjsm 的对象模型 owner：
property slots、proto chain、element 存储、handle 解析。它泛型化于堆内存类型，
不在 `wjsm-host-wasm` 中了。

```rust
use wjsm_gc::heap_access::HeapAccessV2;

let heap_access = HeapAccessV2::<NativeHeap>::with_heap_limit(
    NativeHeap::new(max_bytes),
    handle_table_base,
    object_heap_base,
);
// 用 heap_access.alloc_object / obj_get / obj_set / read_property_slot 等
```

wasmtime 后端的类型别名在 `crates/wjsm-host-wasm/src/runtime_gc/mod.rs`:
```rust
pub type HeapAccessV2 = wjsm_gc::heap_access::HeapAccessV2<crate::heap::SharedHeapMemory>;
```

### 步骤 3：装配 GC 算法

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

### 步骤 4：实现 `ExecContext` trait（真实接缝）

**这是最重要的步骤**。`ExecContext`（`crates/wjsm-host/src/exec_context.rs`）定义了
builtins 调用的全部宿主能力，约 330 个方法，按域分组。用以下命令生成分组清单：

```bash
rg -n 'fn ' crates/wjsm-host/src/exec_context.rs | sort -t: -k3
```

你的后端实现 `ExecContext` + `HeapContext`（后者是前者的 super-trait，18 个堆/侧表原语）：

```rust
use wjsm_host::{ExecContext, HeapContext, Value, Handle, /* ... */};

pub struct NativeExecContext<'a> {
    // 你的运行时上下文（heap access、side tables、caller state 等）
}

impl HeapContext for NativeExecContext<'_> {
    fn read_shadow_arg(&mut self, args_base: i32, index: u32) -> Value { /* ... */ }
    fn write_output(&mut self, bytes: &[u8]) { /* ... */ }
    fn resolve_handle(&mut self, handle: Handle) -> bool { /* ... */ }
    fn alloc_object(&mut self, capacity: u32) -> Value { /* ... */ }
    fn alloc_array(&mut self, capacity: u32) -> Value { /* ... */ }
    fn gc_collect(&mut self) -> GcOutcome { /* ... */ }
    // ... 共 18 个
}

impl ExecContext for NativeExecContext<'_> {
    fn store_string(&mut self, s: &str) -> Value { /* ... */ }
    fn call_js(&mut self, func: Value, this: Value, args: &[Value]) -> anyhow::Result<Value> { /* ... */ }
    fn alloc_promise(&mut self) -> Value { /* ... */ }
    // ... 约 330 个方法
}
```

**参考实现**：`crates/wjsm-host-wasm/src/exec_context_impl/`（按域拆分为 17 子模块，
用 `macro_rules!` + `include!` 组织为单个 `impl ExecContext` 块）。

**关键约束**：
- **零 `dyn ExecContext`**：全仓不得出现 `dyn ExecContext`。builtins 以 `<E: ExecContext>`
  泛型实例化，编译期单态化。你的后端直接 `impl ExecContext for NativeExecContext` 即可。
- **`call_js` 是同步再入桥**：builtins 内同步再入一律走 `ctx.call_js(func, this, args)`，
  由后端实现桥接（wasm 后端用 `Func::wrap` 闭包 + wasmtime call；native 后端可用函数指针表）。
- **async 方法返回 `ExecFuture<'c, T>`**：后端用 `Box::pin(async move { ... })` 实现；
  CLI 侧 `block_on` 单线程驱动。

### 步骤 5：实现 `JsBackend` trait

`JsBackend`（`crates/wjsm-host/src/backend.rs`）定义编译后端契约：

```rust
use wjsm_host::JsBackend;
use wjsm_ir::Program;

pub struct NativeBackend;

impl JsBackend for NativeBackend {
    type Artifact = Vec<u8>;        // 或 native 镜像 / C 源码包
    type ExecOptions = NativeExecOptions;

    fn name(&self) -> &'static str { "native" }

    fn compile(&self, program: &Program, debug: bool) -> anyhow::Result<Self::Artifact> {
        // IR → 你的制品（native 镜像 / C 源码 / 其他）
    }

    fn artifact_bytes(artifact: &Self::Artifact) -> Option<&[u8]> {
        // 制品的持久化字节（build -o 写盘）；不可序列化返回 None
    }

    fn execute<'a, W: std::io::Write + 'a>(
        &'a self, artifact: &'a Self::Artifact, options: Self::ExecOptions, writer: W,
    ) -> impl Future<Output = anyhow::Result<(W, Vec<u8>)>> + 'a {
        // 执行制品，输出到 writer，返回 (writer, diagnostics)
    }
}
```

**参考实现**：
- `crates/wjsm-host-wasm/src/backend_impl.rs`：`WasmBackend`（Artifact = Vec<u8>，ExecOptions = RuntimeOptions）
- `crates/wjsm-backend-jit/src/lib.rs`：`JitBackend`（stub，compile/execute 均 bail!）

### 步骤 6：CLI `Target` enum 接线

在 `crates/wjsm-cli/src/cli_args.rs` 的 `Target` enum 添加你的后端变体，
然后在 `crates/wjsm-cli/src/lib.rs::compile_program_to_wasm` 的 match 接入：

```rust
let bytes: Vec<u8> = match target {
    Target::Wasm => {
        let a = <runtime::WasmBackend as runtime::JsBackend>::compile(
            &runtime::WasmBackend, program, debug_codegen)?;
        <runtime::WasmBackend as runtime::JsBackend>::artifact_bytes(&a)
            .map(|b| b.to_vec()).unwrap_or_default()
    }
    Target::Jit => {
        let a = <wjsm_backend_jit::JitBackend as runtime::JsBackend>::compile(
            &wjsm_backend_jit::JitBackend, program, debug_codegen)?;
        // ...
    }
    Target::Native => {
        let a = <wjsm_backend_native::NativeBackend as runtime::JsBackend>::compile(
            &wjsm_backend_native::NativeBackend, program, debug_codegen)?;
        // ...
    }
};
```

执行路径同理：`block_on_wasm_execute` 或等效函数按 `Target` 分发到 `<Backend>::execute`。

## 后端职责清单（明确归属）

以下四类功能**不迁入 `wjsm-builtins`**，是各后端各自的实现职责：

1. **Bootstrap / 全局对象安装**：`create_global_object`、全局对象 wiring、NativeCallable 索引、
   Node web globals 安装。一次性冷路径，每个后端自行组织。
2. **再入基础设施**：`reentrant_async/mod.rs` 是 `call_js_async` 的实现基底（wasmtime 后端用
   `Func::wrap` 闭包 + wasmtime call；native 后端需自行实现 async 再入桥）。**不迁**。
3. **I/O 实现**：`fetch_http.rs`（reqwest 客户端）、`streams_fetch_body.rs`（tokio spawn body 桥）
   是 wasm 后端的 I/O 实现。native 后端需自行实现 `http_fetch_begin` / `http_body_pull` 等
   trait 方法的 I/O 桥。
4. **模块 instantiate 流程**：`RuntimeModuleInstantiationContext`、`Module::new`/`Linker`/
   `instantiate_async` 是 wasm 后端 loader 的私有实现。native 后端需自行实现
   `module_instantiate_sync` / `module_instantiate_async`。

## HostRuntime 是历史 marker

`HostRuntime`（`wjsm-host/src/runtime_trait.rs`）是 marker + blanket impl：
```rust
pub trait HostRuntime: ConsoleHost + ObjectHost + GcHost + AsyncHost {}
impl<T> HostRuntime for T where T: ConsoleHost + ObjectHost + GcHost + AsyncHost {}
```

它是 ADR 0011 时期的设计遗留，**真实接缝是 `ExecContext`**。新后端只需实现
`ExecContext` + `JsBackend`，无需实现 `HostRuntime`。`HostRuntime` 保留为 facade 公共 API
（`wjsm-runtime` re-export），不影响新后端接入。

## 不变量（必须保持）

- `rg -n 'wasmtime|Caller|WasmEnv' crates/wjsm-builtins/src/` → **0 匹配**
- `rg -n 'tokio|reqwest|block_in_place' crates/wjsm-builtins/src/` → **0 匹配**
- `rg -n 'dyn ExecContext' crates/` → **0 匹配**
- `rg -n 'wasmtime' crates/wjsm-gc/src/ | rg -v '^\S+:\s*\d+:\s*//'` → **0 匹配**（注释豁免）

## 参考

- `crates/wjsm-host/src/exec_context.rs` — ExecContext trait 全貌（~330 方法）
- `crates/wjsm-host-wasm/src/exec_context_impl/` — WasmExecContext 按域拆分实现
- `crates/wjsm-gc/src/heap_access.rs` — HeapAccessV2 对象模型
- `crates/wjsm-host/src/backend.rs` — JsBackend trait
- `crates/wjsm-host-wasm/src/backend_impl.rs` — WasmBackend 参考
- `docs/adr/0013-multi-backend-contract.md` — 多后端契约 ADR
