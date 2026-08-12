# ADR 0013: 多后端完全支撑契约

## Status

Superseded by ADR 0014（2026-08-12）；后端无关语义、host 契约与 GC 分层继续有效

## Context

ADR 0011 完成运行时拆分后，wasm 工具链依赖收缩到 `wjsm-backend-wasm` + `wjsm-host-wasm`
两个 crate，但距离「完全支撑多编译器后端/多 runtime 后端开发」仍有三个缺口：

1. **~12.5K 行 ECMAScript/WHATWG 语义**以 `Caller<RuntimeState>` 形式耦合 wasmtime
   （另加 runtime_json 1077 行 + runtime_render 1261 行），无法被非 wasm 后端复用。
2. **无后端接入契约**：CLI `Target::Jit` 一律 bail，`wjsm-backend-jit` 是 6 行 stub，
   `HostRuntime` trait 是无实现的 marker。
3. **对象模型 `HeapAccessV2`**（1092 行，property slots/proto chain 语义）被锁在 host-wasm，
   绑定具体类型 `SharedHeapMemory`。

## Decision

### 1. JS 语义算法全量迁入 `wjsm-builtins`

所有 ECMAScript/WHATWG 语义算法（promise combinator、render、json、core typeof/abstract_eq/
strict_eq/abstract_compare、Atomics、Streams 全家、Fetch、Modules/CJS、collections_buffers、
gc property 路径、array_object、proxy_reflect_async）迁入 `wjsm-builtins`，签名改为
`<E: ExecContext>` 泛型单态化。host_imports 缩为薄注册层（照 `host_imports/promise.rs` 模式：
`Func::wrap` 闭包 → `WasmExecContext::new` → `wjsm_builtins::…` 一行调用）。

**不迁的四类豁免**（后端职责，非 JS 语义）：

- **分配/GC glue**：`gc_alloc_slow`、`gc_arr_new`、`allocate_v2_array_handle`、
  `take_next_handle`、`gc_safepoint_poll`、`gc_barrier_flush`、`gc_load_barrier_slow`
- **I/O 桥**：`fetch_http.rs`（reqwest 客户端）、`streams_fetch_body.rs`（tokio spawn body 桥）
- **再入基础设施**：`reentrant_async/mod.rs` 是 `call_js_async` 的实现基底
- **Bootstrap**：`create_global_object`、全局对象 wiring、Node web globals 安装（一次性冷路径）

### 2. `HeapAccessV2` 泛型化迁入 `wjsm-gc`

`HeapAccessV2` 从 `crates/wjsm-host-wasm/src/runtime_gc/heap_access_v2.rs` 迁至
`crates/wjsm-gc/src/heap_access.rs`，结构改 `pub struct HeapAccessV2<M: GrowableHeapMemory>`。
host-wasm 侧用类型别名 `pub type HeapAccessV2 = wjsm_gc::heap_access::HeapAccessV2<SharedHeapMemory>;`
保持全部 callsite 零改动。`HeapMemory` trait 增加 `read_c_string` 默认实现（按 `copy_to`
分块扫描），`SharedHeapMemory` 覆写为 memchr 快路径。

### 3. `JsBackend` trait + CLI 静态分发

新增 `crates/wjsm-host/src/backend.rs`：

```rust
pub trait JsBackend {
    type Artifact: Send;
    type ExecOptions;
    fn name(&self) -> &'static str;
    fn compile(&self, program: &wjsm_ir::Program, debug: bool) -> anyhow::Result<Self::Artifact>;
    fn artifact_bytes(artifact: &Self::Artifact) -> Option<&[u8]>;
    fn execute<'a, W: std::io::Write + 'a>(
        &'a self, artifact: &'a Self::Artifact, options: Self::ExecOptions, writer: W,
    ) -> impl Future<Output = anyhow::Result<(W, Vec<u8>)>> + 'a;
}
```

CLI `Target::Jit` 的 3 处 bail 改为 `match target { ... <Backend>::compile(...) }` 静态分发。
`wjsm-backend-jit` 实现 `JsBackend`（compile/execute 均 `bail!("JIT backend is not implemented yet")`，
保持用户可见错误不变）。`wjsm-host-wasm` 实现 `WasmBackend`。

**`execute` 的 Future 不要求 `Send`**：CLI 用单线程 `block_on` 驱动，wasmtime 执行链含
`MutexGuard` 非 `Send`，trait 不加 `Send` 约束。未来多线程后端可自行加约束。

### 4. `ExecContext` 是真实接缝，非 `HostRuntime`

`HostRuntime`（marker + blanket impl）是 ADR 0011 设计遗留，保留为 facade 公共 API。
新后端实现 `ExecContext` + `JsBackend` 即可接入，无需实现 `HostRuntime`。
`docs/backend-implementation-guide.md` 已重写为完整六步接入手册。

### 5. `exec_context_impl.rs` 按域拆分

wasmtime 后端的 `WasmExecContext` 实现（5491 行，394 方法）从单文件拆为
`exec_context_impl/` 目录（17 子模块），用 `macro_rules!` + `include!` 组织为单个
`impl ExecContext` 块。纯代码移动，公开面不变。

## Consequences

- **零 `dyn ExecContext`**：全仓不得出现 `dyn ExecContext`（当前 0 处，必须保持）。
  builtins 以 `<E: ExecContext>` 泛型实例化，编译期单态化，零 vtable 开销。
- **`wjsm-builtins` 零 wasmtime/tokio/reqwest 耦合**：所有 I/O 桥、分配 glue、再入基础设施
  留在 host-wasm，builtins 经 `ExecContext` trait 方法访问后端能力。
- **`wjsm-gc` 零 wasmtime 耦合**：`HeapAccessV2<M>` 泛型化，对象模型可移植。
- **新后端接入路径清晰**：实现 `HeapMemory` + `ExecContext` + `JsBackend` 三层即可。
- **CLI `Target` 扩展点明确**：新后端在 `compile_program_to_wasm` 的 match 接入。
- **性能零回退**：泛型单态化 + `#[inline]` 保证热点路径无间接层；gc-bench ±5% 内。

## Verification

- `rg -n 'wasmtime|Caller|WasmEnv' crates/wjsm-builtins/src/` → 0
- `rg -n 'tokio|reqwest|block_in_place' crates/wjsm-builtins/src/` → 0
- `rg -n 'dyn ExecContext' crates/` → 0
- `rg -n 'wasmtime' crates/wjsm-gc/src/ | rg -v '^\S+:\s*\d+:\s*//'` → 0
- `cargo nextest run --workspace` → 1805 tests pass
- `cargo run -- run -e 'console.log(1+2)'` → `3`
- `cargo run -- build -e 'console.log(1)' --target jit` → `Error: JIT backend is not implemented yet`

## References

- ADR 0011 — 运行时按后端无关性拆分（本 ADR 的前驱）
- ADR 0012 — host builtins 解耦（本 ADR 的细化）
- `docs/backend-implementation-guide.md` — 新后端实现六步手册
- `docs/plan.md` — 多后端完全支撑执行方案
