# Startup Snapshot 实施计划

**Goal**: 为 wjsm 实现 Deno/V8 风格的启动快照：把运行时 primordial/builtin 初始化状态序列化为可重定位快照，执行用户 JS 前直接恢复，减少短命 CLI / 小 fixture / future realm 创建的 cold-start 开销，同时保持 ECMAScript 语义、async scheduler 边界、non-moving GC 不变量不变。

**Architecture**: 不快照 Wasmtime `Instance` / `Store`。新增 `wjsm-runtime` 自有的 relocatable primordial heap snapshot：保存 wasm 线性内存中启动对象堆片段、obj_table 相对偏移、启动期 runtime strings、启动期无状态 `NativeCallable` 表项和原型句柄；恢复时按当前模块的 `__object_heap_start` 重定位，随后执行当前模块专属函数属性初始化，再进入用户 `main`。

**Tech Stack**: Rust 2024, wasmtime async (`instantiate_async`, `call_async`), wasm-encoder, 手写 little-endian 二进制格式（不走 JSON/serde 热路径），`OnceLock` 进程内缓存 + 可选磁盘 bytes cache，`cargo nextest` 验证。

**Baseline/Authority Refs**:
- V8 custom startup snapshots: `https://v8.dev/blog/custom-startup-snapshots`。核心事实：预初始化 JS heap，启动时反序列化；外部世界交互不能捕获。
- Deno `deno_core::create_snapshot`: `https://github.com/denoland/deno_core/blob/main/core/runtime/snapshot.rs`。核心事实：构建期/启动前准备 `JsRuntimeForSnapshot`，把 extension JS/ESM、module map、function template sidecar 等静态 runtime 状态放入 snapshot。
- Deno `InitMode::FromSnapshot`: `https://github.com/denoland/deno_core/blob/main/core/runtime/jsruntime.rs`。核心事实：从 snapshot 启动时跳过部分初始化工作。
- `docs/async-scheduler.md`：Store 只能由 scheduler owner 访问；worker 不碰 Store / RuntimeState / wasm memory / JS heap。
- `docs/adr/0002-runtimestate-stays-flat.md`：`RuntimeState` 保持扁平侧表集合；新增 snapshot 不能借机按领域重组状态。
- 当前源码证据：
  - `crates/wjsm-runtime/src/lib.rs:857-1006` 是执行启动路径。
  - `crates/wjsm-backend-wasm/src/compiler_module.rs:658-773` 当前把函数属性、`Array.prototype`、`Object.prototype` 初始化塞进 `main` 前缀。
  - `crates/wjsm-backend-wasm/src/compiler_module.rs:381-523` 导出 `__heap_ptr`、`__obj_table_ptr`、`__obj_table_count`、`__object_heap_start`、`__array_proto_handle`、`__object_proto_handle`。
  - `crates/wjsm-runtime/src/runtime_gc/roots.rs:115-129` 当前把 `0..__num_ir_functions` 视为函数属性 root，并额外标记 Array/Object prototype。

**Compatibility Boundary**:
- 现有 fixture `.expected` 输出不变。
- `wjsm-runtime` public API 仍是 async-only：`execute` / `execute_with_writer` 签名不变。
- `RuntimeState` 字段仍保持扁平，不拆成 snapshot/runtime 子状态结构。
- Snapshot 不包含用户代码执行后的对象、promise/timer/microtask/fetch/stream 活动状态、`SharedRuntimeState`、`eval_cache`、scheduler channel/counter。
- Cache miss / ABI mismatch 走 canonical cold bootstrap builder 重新生成；restore 阶段发现内部不变量破坏必须报错，不静默执行错误堆。
- Wasm helper/global 命名新增可接受；旧隐含函数属性布局 `0..num_ir_functions` 必须退役。

**Verification**:
- 每阶段运行对应 crate 定向测试；关键阶段运行 happy/modules/semantic fixture。
- 最终运行：
  ```bash
  cargo nextest run -p wjsm-runtime -E 'test(startup_snapshot) or test(async_scheduler) or test(async_reentry)'
  cargo nextest run -p wjsm-backend-wasm
  cargo nextest run -E 'test(happy__) or test(modules__) or test(semantic__)'
  cargo nextest run --workspace
  cargo test -p wjsm-runtime bench_execute_phases -- --ignored --nocapture
  ```
- 性能验收不只看 full workspace wall time；必须输出 cold bootstrap、snapshot restore、full execute cold、full execute snapshot 的阶段耗时。

**ADR Signal**: 新增持久 runtime 启动 ABI、snapshot 文件格式、primordial/function handle 边界。完成后新增 `docs/adr/0003-startup-snapshot-boundary.md`，并更新 `AGENTS.md` runtime load-bearing conventions。

---

## Decision Hygiene Review

```text
First-principles invariants:
- Non-negotiable goal: 每个新执行实例从同一份 pristine runtime startup heap 开始，跳过重复 bootstrap，但不能保存用户运行态。
- Non-negotiable constraints: Store/wasm memory 只在 scheduler owner 上访问；RuntimeState 保持扁平；GC handle/root 解释必须精确；snapshot bytes 必须 ABI 绑定。
- Historical assumptions to delete: obj_table[0..num_ir_functions) 永远是函数属性对象；bootstrap 必须内联在 main 前缀；host post-bootstrap 可以散落在 execute 启动路径。

Owner / retirement matrix:
- New canonical owner: crates/wjsm-runtime/src/startup_snapshot*.rs 持有 snapshot 格式、capture、restore、cache；backend 只负责导出可调用 bootstrap/function-props 阶段。
- Old owner: main 前缀隐式 bootstrap 与 lib.rs 中散落的 post 原型初始化。
- Compat-only carrier: main 里的 bootstrap guard 保留为 cold builder/安全幂等入口，不作为第二套语义 owner。
- Delete-first / retirement trigger: 所有函数属性 `0..num_ir_functions` 假设改为 `function_props_base..function_props_base+num_ir_functions` 后，旧 GC root 规则退役。

Falsification matrix:
- Dependency-removal test: 删除 snapshot cache 后，cold builder 仍能生成等价 runtime startup heap；删除 cold bootstrap 后，cache miss 无法恢复，故 cold bootstrap 是 builder owner 不是 fallback。
- Counterexample scenario: 用户改写 `Array.prototype` 后下一次独立 run 仍污染，说明 snapshot bytes 被运行态反写或 RuntimeState 共享错误。
- Must fail / degrade / remain correct cases: ABI hash mismatch 必须重建；损坏 snapshot 必须拒绝并重建一次；active promise/timer/stream state 不得被捕获；GC 后原型链仍可达。

Verdict:
- Adopt: relocatable primordial heap snapshot + sidecar whitelist。
- Blocking gaps: 当前 bootstrap 未拆分；函数属性 handle 布局依赖 num_ir_functions；部分 primordial 字符串仍运行时分配。
- Next evidence: P0 阶段计时决定 snapshot 热路径是否进入默认开启。
```

---

## Plan Pressure Test

```text
- Owner / contract / retirement:
  - Owner: startup_snapshot.rs/startup_snapshot_cache.rs 是 runtime snapshot 唯一 owner；compiler_module.rs 只生成 bootstrap/function-props wasm 阶段；runtime_gc/roots.rs 只消费新的 function_props_base contract。
  - Contract: snapshot = pristine runtime startup heap；不包含 active async/host state；ABI hash 绑定对象布局、NaN-box tags、NativeCallable snapshot enum、primordial string table、bootstrap version。
  - Retirement: 退役 `0..num_ir_functions` 函数属性隐含布局；退役 execute 路径中散落 post 原型初始化。
- Verification scope:
  - Unit: binary format, whitelist, relative relocation, corrupted/ABI mismatch。
  - Integration: snapshot on/off 输出一致、prototype pollution 不跨 run、GC 后 primordial 可达、async scheduler/reentry 不回退。
  - Performance: cold bootstrap vs restore 阶段计时。
- Task executability:
  - 先拆 bootstrap，再改 handle contract，再固化字符串，再实现 format/capture/restore/cache，最后默认接入。
- Pressure result: proceed；该顺序先退休错误 owner，再写优化代码，避免 snapshot 复制现有隐式布局。
```

## Plan-Time Complexity Check

```text
- Target files:
  - 修改: crates/wjsm-backend-wasm/src/compiler_module.rs, compiler_core.rs, compiler_helpers.rs, compiler_array_helpers.rs, lib.rs。
  - 修改: crates/wjsm-runtime/src/lib.rs, runtime_gc/roots.rs, runtime_gc/api.rs, runtime_host_helpers.rs。
  - 新增: crates/wjsm-runtime/src/startup_snapshot.rs, startup_snapshot_format.rs, startup_snapshot_cache.rs。
  - 新增/修改测试: crates/wjsm-runtime/tests/startup_snapshot.rs 或 lib.rs cfg(test) 模块；fixtures/happy/startup_snapshot_*.js。
  - 文档: docs/adr/0003-startup-snapshot-boundary.md, docs/async-scheduler.md, AGENTS.md。
- Existing size / shape signals:
  - lib.rs 1500+ 行且启动路径集中在 857-1006；不能继续加长 execute 主体。
  - compiler_module.rs 已承载大量 codegen；bootstrap 生成应抽 helper method，不在 compile_function 内继续堆代码。
  - RuntimeState 已有约 50 个扁平侧表；ADR 0002 禁止领域分组。
- Owner fit:
  - snapshot format/cache 是 runtime owner；backend 只提供导出的 wasm 阶段函数和 globals。
  - GC roots 只更新 root contract，不拥有 snapshot。
- Add-in-place risk:
  - 在 lib.rs 直接写格式解析/磁盘 cache 会污染执行入口；必须新文件。
  - 在 NativeCallable enum 上直接 derive serde 会把运行态变体误放进格式；必须专用 snapshot enum。
- Better file boundary:
  - `startup_snapshot_format.rs`: 纯 bytes encode/decode + ABI hash。
  - `startup_snapshot.rs`: Store/WasmEnv capture/restore。
  - `startup_snapshot_cache.rs`: OnceLock + 磁盘 cache + cold builder。
- Recommendation: add owner files + extract backend bootstrap helpers + split task。
```

---

## Snapshot ABI 与热路径设计

### Snapshot 内容

```rust
pub(crate) struct StartupSnapshot<'a> {
    pub header: StartupSnapshotHeader,
    pub object_bytes: &'a [u8],
    pub handle_offsets: &'a [u32],
    pub runtime_strings: Vec<&'a str>,
    pub native_callables: Vec<SnapshotNativeCallable>,
    pub async_iterator_prototype: i64,
    pub async_gen_prototype: i64,
    pub array_proto_values: i64,
}

pub(crate) struct StartupSnapshotHeader {
    pub magic: [u8; 8],
    pub format_version: u32,
    pub abi_hash: u64,
    pub heap_used: u32,
    pub obj_table_count: u32,
    pub function_props_base: u32,
    pub object_proto_handle: u32,
    pub array_proto_handle: u32,
}
```

热路径要求：

- `object_bytes` 直接 `copy_from_slice` 到 `memory[object_heap_start..]`。
- `handle_offsets` 恢复时只做 `object_heap_start + rel_offset`，写入 handle table。
- decode 时校验 bounds，一次性生成 immutable decoded snapshot；execute hot path 不做 JSON parse，不做 HashMap 构建。
- `runtime_strings` 恢复为 `Vec<String>` 是当前 `RuntimeState` 结构的必要分配；后续如果要继续优化，单独计划把 runtime string side table 改为 Cow-like intern table。

### Snapshot 排除项

这些字段必须在 capture 时断言为空或初始化态：

```text
timers, cancelled_timers, microtask_queue, promise_table, continuation_table,
async_generator_table, async_from_sync_iterators, combinator_contexts,
module_namespace_cache, error_table, map_table, set_table, weakmap_table,
weakset_table, weakref_table, finalization_registry_table,
pending_cleanup_callbacks, proxy_table, arraybuffer_table, dataview_table,
typedarray_table, headers_table, fetch_response_table, fetch_request_table,
abort_signal_table, http_response_table, readable_stream_table, reader_table,
stream_controller_table, byob_request_table, writable_stream_table,
writer_table, transform_stream_table, eval_cache, SharedRuntimeState,
host_completion_tx, async_op_counter
```

### NativeCallable whitelist

```rust
pub(crate) enum SnapshotNativeCallable {
    EvalIndirect,
    AsyncIteratorProtoSymbolAsyncIterator,
    ArrayProtoValues,
    ArrayConstructor,
    ObjectConstructor,
    ObjectProtoToString,
    ObjectProtoValueOf,
    FunctionConstructor,
    StringConstructor,
    BooleanConstructor,
    NumberConstructor,
    SymbolConstructor,
    BigIntConstructor,
    RegExpConstructor,
    ErrorConstructor,
    TypeErrorConstructor,
    RangeErrorConstructor,
    SyntaxErrorConstructor,
    ReferenceErrorConstructor,
    URIErrorConstructor,
    EvalErrorConstructor,
    AggregateErrorConstructor,
    MapConstructor,
    SetConstructor,
    WeakMapConstructor,
    WeakSetConstructor,
    WeakRefConstructor,
    FinalizationRegistryConstructor,
    DateConstructorGlobal,
    PromiseConstructor,
    ArrayBufferConstructorGlobal,
    DataViewConstructorGlobal,
    BigInt64ArrayConstructor,
    BigUint64ArrayConstructor,
    ProxyConstructor,
    GcCollect,
    SharedArrayBufferConstructor,
    AtomicsGlobal,
    AgentStart,
    AgentBroadcast,
    AgentReceiveBroadcast,
    AgentGetReport,
    AgentReport,
    AgentSleep,
    AgentMonotonicNow,
    HeadersConstructor,
    ResponseConstructor,
    RequestConstructor,
    AbortControllerConstructor,
    ReadableStreamConstructor,
    WritableStreamConstructor,
    TransformStreamConstructor,
    CountQueuingStrategyConstructor,
    ByteLengthQueuingStrategyConstructor,
}
```

禁止捕获含运行态 handle/Arc/Mutex 状态的变体：`PromiseResolvingFunction`、`PromiseCombinatorReaction`、`AsyncGeneratorMethod`、`ArrayLikeIteratorNext`、`AsyncFromSync*`、`MapSetMethod`、`DateMethod`、`WeakMapMethod`、`WeakSetMethod`、`ProxyRevoker`、所有 `*Method { handle, .. }`、`EvalFunction`。

---

## Tasks 总览

| 阶段 | 任务 | 验证 |
|---|---|---|
| P0 | 阶段计时与开关基线 | ignored bench 输出阶段耗时 |
| P1 | 拆分 wasm bootstrap/function-props | backend tests + happy fixtures |
| P2 | 退休函数属性隐含 handle 布局 | runtime GC roots tests + fixtures |
| P3 | 固定 primordial 字符串与 ABI hash 输入 | backend data tests |
| P4 | 实现 snapshot format + whitelist | runtime unit tests |
| P5 | 实现 capture/restore | restore 等价 + GC 原型 tests |
| P6 | 实现 cache builder + execute 接入 | snapshot on/off 一致性 |
| P7 | 性能验收与默认策略 | bench + workspace |
| P8 | 文档与 ADR | docs read-through + final workspace |

---

# P0：阶段计时与 feature flag 基线

**Why**: snapshot 只优化 bootstrap/restore。先量化 `Engine::new`、`Module::new`、linker、instantiate、bootstrap、full execute，避免把 snapshot 当成测试总耗时万能优化。

**Files**:
- modify: `crates/wjsm-runtime/src/lib.rs`

**Steps**:

- [ ] 扩展 `bench_execute_phases`，新增输出项：
  ```text
  BENCH engine only
  BENCH module only
  BENCH store only
  BENCH linker register
  BENCH instantiate_async
  BENCH bootstrap cold
  BENCH host post-bootstrap
  BENCH full execute cold
  ```
- [ ] 抽出内部 helper `instantiate_for_startup_bench(wasm: &[u8])`，只在 `#[cfg(test)]` 可见，避免 benchmark 复制 execute 启动代码。
- [ ] 新增环境开关解析 helper：
  ```rust
  fn startup_snapshot_enabled() -> bool {
      !matches!(std::env::var("WJSM_STARTUP_SNAPSHOT").as_deref(), Ok("0") | Ok("false") | Ok("off"))
  }
  ```
  该开关只用于 benchmark/debug；最终默认开启。
- [ ] 运行：
  ```bash
  cargo test -p wjsm-runtime bench_execute_phases -- --ignored --nocapture
  ```
- [ ] 记录输出到计划执行记录；如果 bootstrap cold 小于 restore 预估上界，则 P7 默认策略改为仅显式开启。

---

# P1：拆分 wasm bootstrap 与函数属性初始化

**Why**: 当前 `compiler_module.rs:658-773` 把函数属性、`Array.prototype`、`Object.prototype` 都塞进 `main`。restore 后再次进入 `main` 会重复分配。必须拆成幂等导出阶段。

**Files**:
- modify: `crates/wjsm-backend-wasm/src/compiler_module.rs`
- modify: `crates/wjsm-backend-wasm/src/lib.rs`
- add tests: `crates/wjsm-backend-wasm/tests/startup_bootstrap_exports.rs`

**Implementation contract**:

新增导出函数：

```text
__wjsm_bootstrap_once: () -> i64
__wjsm_init_function_props: () -> i64
```

新增 globals：

```text
__bootstrap_done: mutable i32
__function_props_done: mutable i32
__function_props_base: mutable i32
```

`main` prologue 改成：

```text
call __wjsm_bootstrap_once
if result is exception: return result
call __wjsm_init_function_props
if result is exception: return result
user body
```

**Steps**:

- [ ] 在 compiler struct 中增加 global/function index 字段：`bootstrap_done_global_idx`、`function_props_done_global_idx`、`function_props_base_global_idx`、`bootstrap_func_idx`、`init_function_props_func_idx`。
- [ ] 在 `compile_module` 中先注册两个 helper function index，再注册 exports。
- [ ] 把 `compiler_module.rs:691-773` 的 Array/Object prototype 初始化移动到 `compile_bootstrap_once_function()`。
- [ ] 把 `compiler_module.rs:658-690` 的函数属性初始化移动到 `compile_init_function_props_function()`。
- [ ] `compile_bootstrap_once_function()` 开头检查 `__bootstrap_done != 0` 直接返回 `undefined`；成功后设置 `__function_props_base = __obj_table_count`，再设置 `__bootstrap_done = 1`。
- [ ] `compile_init_function_props_function()` 开头检查 `__function_props_done != 0` 直接返回；从 `__function_props_base` 开始分配/写入函数属性对象，完成后设置 `__function_props_done = 1`。
- [ ] `compile_function()` 不再直接发射 bootstrap 代码，只发射 main 的两段 call。
- [ ] 新增 wasmparser test 断言 exports 包含 `__wjsm_bootstrap_once`、`__wjsm_init_function_props`、`__function_props_base`、`__bootstrap_done`、`__function_props_done`。
- [ ] 运行：
  ```bash
  cargo nextest run -p wjsm-backend-wasm
  cargo nextest run -E 'test(happy__)'
  ```

---

# P2：退休函数属性隐含 handle 布局

**Why**: 当前 GC roots 把 `0..num_ir_functions` 当成函数属性对象。snapshot 后 primordial handles 位于最前，函数属性起点必须由 `__function_props_base` 决定。

**Files**:
- modify: `crates/wjsm-backend-wasm/src/compiler_module.rs`
- modify: `crates/wjsm-runtime/src/wasm_env.rs`
- modify: `crates/wjsm-runtime/src/runtime_gc/api.rs`
- modify: `crates/wjsm-runtime/src/runtime_gc/roots.rs`
- modify as needed: `crates/wjsm-runtime/src/runtime_host_helpers.rs`

**Implementation contract**:

`WasmEnv` 增加：

```rust
pub(crate) function_props_base: Global,
pub(crate) bootstrap_done: Global,
pub(crate) function_props_done: Global,
```

`GcContext` 增加：

```rust
pub fn function_props_base(&self) -> usize;
```

GC root 规则改为：

```rust
let base = ctx.function_props_base();
let n = ctx.num_ir_functions();
for h in base..base + n {
    if h < ctx.obj_table_count() { visit(h as Handle); }
}
```

**Steps**:

- [ ] 在 runtime 启动路径提取三个新 global，写入 `WasmEnv`。
- [ ] 更新 `GcContext` 从 env 读取 `function_props_base`。
- [ ] 修改 `runtime_gc/roots.rs:115-129`，删除 `0..num_ir_functions` 假设。
- [ ] 搜索 `num_ir_functions()` 的调用点，确认没有其他函数属性起点假设；如有，同步改为 base/count 区间。
- [ ] 新增 runtime GC 单测：构造 `function_props_base = 3, num_ir_functions = 2`，断言 roots 访问 3、4，不访问 0、1。
- [ ] 运行：
  ```bash
  cargo nextest run -p wjsm-runtime -E 'test(runtime_gc)'
  cargo nextest run -E 'test(happy__) or test(semantic__)'
  ```

---

# P3：固定 primordial 字符串表与 ABI hash 输入

**Why**: snapshot 对象 slot 里的属性名是 wasm memory 中的 name id。若属性名地址依赖用户模块字符串，snapshot restore 会读错 key。

**Files**:
- modify: `crates/wjsm-backend-wasm/src/constants.rs`
- modify: `crates/wjsm-backend-wasm/src/compiler_module.rs`
- add tests: `crates/wjsm-backend-wasm/tests/primordial_strings.rs`

**Primordial strings**:

```text
length, name,
push, pop, includes, indexOf, join, concat, slice, fill, reverse, flat,
shift, unshift, sort, at, copyWithin, forEach, map, filter, reduce,
reduceRight, find, findIndex, some, every, flatMap, splice, isArray,
toString, valueOf, Symbol.toStringTag, AsyncIterator, AsyncGenerator
```

**Steps**:

- [ ] 在 constants 中为 primordial strings 分配固定 offset 区间，位于 `USER_STRING_START` 之前。
- [ ] 把 `compile_module` 中硬编码预写 typeof/promise/property descriptor 字符串的逻辑抽成 `write_reserved_string(offset, text)`。
- [ ] 在编译用户函数前预写全部 primordial strings，并填充 `string_ptr_cache`。
- [ ] 修改 `__wjsm_bootstrap_once` 使用固定 offset，不在 bootstrap hot path 为这些名字调用 `intern_data_string` 增长用户字符串区。
- [ ] 修改 host post-bootstrap 中 `define_host_data_property_with_env(..., "Symbol.toStringTag", ...)` 的路径，优先使用固定 primordial name id，避免恢复后再分配相同属性名。
- [ ] 新增 wasm data section 测试：两个不同用户源码编译出的 primordial string offsets 完全一致。
- [ ] 把 primordial strings 列表、format version、NativeCallable snapshot enum discriminants 纳入 `startup_snapshot_format::abi_hash()` 输入。
- [ ] 运行：
  ```bash
  cargo nextest run -p wjsm-backend-wasm
  ```

---

# P4：实现 snapshot 二进制格式与 NativeCallable whitelist

**Why**: 高性能 restore 要求格式可直接 bounds-check + slice copy；不能用 JSON，也不能把完整 `NativeCallable` enum 自动序列化。

**Files**:
- create: `crates/wjsm-runtime/src/startup_snapshot_format.rs`
- modify: `crates/wjsm-runtime/src/lib.rs`（`mod startup_snapshot_format;`）
- add tests: `crates/wjsm-runtime/tests/startup_snapshot_format.rs`

**Binary format**:

```text
magic[8] = "WJSMSNP\0"
format_version: u32 le
abi_hash: u64 le
heap_used: u32 le
obj_table_count: u32 le
function_props_base: u32 le
object_proto_handle: u32 le
array_proto_handle: u32 le
async_iterator_prototype: i64 le
async_gen_prototype: i64 le
array_proto_values: i64 le
section_count: u32 le
sections: repeated { kind: u32, offset: u32, len: u32 }
payload sections: object_bytes, handle_offsets, runtime_strings, native_callables
```

**Steps**:

- [ ] 定义 `SnapshotNativeCallable`，只包含启动期无状态变体；实现 `try_from_native_callable` 与 `into_native_callable`。
- [ ] 对所有含运行态 handle/Arc/Mutex 的 `NativeCallable` 变体返回 `Err(SnapshotError::UnsupportedNativeCallable)`。
- [ ] 实现 `encode_snapshot(snapshot: &StartupSnapshotOwned) -> Vec<u8>`，所有数值 little-endian，section offsets 4 字节对齐。
- [ ] 实现 `decode_snapshot(bytes: &[u8]) -> Result<StartupSnapshotView<'_>>`，只借用原始 bytes，不在 hot path 复制 `object_bytes` / `handle_offsets`。
- [ ] 实现 `abi_hash() -> u64`，输入包含：format version、NaN-box tag 常量、heap object layout version、primordial string table、`SnapshotNativeCallable` discriminant 表、bootstrap ABI version。
- [ ] 单测覆盖：roundtrip、bad magic、bad version、bad section bounds、bad UTF-8 runtime string、unsupported callable。
- [ ] 运行：
  ```bash
  cargo nextest run -p wjsm-runtime -E 'test(startup_snapshot_format)'
  ```

---

# P5：实现 capture / restore

**Why**: capture/restore 是 snapshot 语义核心。capture 只能在用户 JS 前运行；restore 必须按当前模块内存布局重定位，不能写入绝对地址。

**Files**:
- create: `crates/wjsm-runtime/src/startup_snapshot.rs`
- modify: `crates/wjsm-runtime/src/lib.rs`
- modify: `crates/wjsm-runtime/src/wasm_env.rs`
- add tests: `crates/wjsm-runtime/tests/startup_snapshot.rs`

**API**:

```rust
pub(crate) fn capture_startup_snapshot(
    store: &mut Store<RuntimeState>,
    env: &WasmEnv,
) -> Result<StartupSnapshotOwned>;

pub(crate) fn restore_startup_snapshot(
    store: &mut Store<RuntimeState>,
    env: &WasmEnv,
    snapshot: StartupSnapshotView<'_>,
) -> Result<()>;
```

**Capture contract**:

- [ ] 读取 `__object_heap_start`、`__heap_ptr`、`__obj_table_ptr`、`__obj_table_count`、`__function_props_base`、`__array_proto_handle`、`__object_proto_handle`。
- [ ] 保存 `memory[object_heap_start..heap_ptr]` 为 `object_bytes`。
- [ ] 保存 `obj_table[0..obj_table_count]`，每个非零 entry 转换为 `entry - object_heap_start`；若 entry 不在 object heap 范围内，返回错误。
- [ ] 保存 `runtime_strings`、`native_callables`、`async_iterator_prototype`、`async_gen_prototype`、`array_proto_values`。
- [ ] 断言排除项为空或初始化态：microtask/timer/promise/fetch/stream/eval/shared/scheduler 状态不得进入 snapshot。

**Restore contract**:

- [ ] 校验 `snapshot.abi_hash == abi_hash()`。
- [ ] 校验当前 wasm memory 容量足够容纳 `object_bytes` 与 handle table entries；不足时 `memory.grow` 到所需页数。
- [ ] `copy_from_slice` 恢复 object heap。
- [ ] 重写 handle table：`abs = object_heap_start + rel_offset`；rel_offset 为 0 的空槽写 0。
- [ ] 写回 globals：`__heap_ptr`、`__obj_table_count`、`__function_props_base`、`__array_proto_handle`、`__object_proto_handle`、`__bootstrap_done = 1`、`__function_props_done = 0`。
- [ ] 重建 `RuntimeState` 的 runtime_strings/native_callables/async prototypes/array_proto_values；其他侧表保持新实例初始化态。

**Tests**:

- [ ] `capture_restore_capture_is_identical`：seed bootstrap 后 capture，restore 到新 instance，再 capture，bytes 等价。
- [ ] `restore_relocates_object_heap`：用不同用户字符串长度的 wasm 触发不同 `object_heap_start`，restore 后属性查找仍正确。
- [ ] `capture_rejects_runtime_native_callable`：手动塞入 `PromiseResolvingFunction` 后 capture 报错。
- [ ] `restore_marks_bootstrap_done_but_not_function_props_done`：restore 后 main 会跳过 bootstrap，但仍初始化当前模块函数属性。
- [ ] 运行：
  ```bash
  cargo nextest run -p wjsm-runtime -E 'test(startup_snapshot)'
  ```

---

# P6：实现 cache builder 并接入 execute 启动路径

**Why**: Deno 的收益来自启动时直接使用已有 snapshot。wjsm 先实现 runtime-owned persistent cache：首个 cache miss 运行 cold builder，后续进程读取 bytes；进程内用 `OnceLock` 避免重复 decode。

**Files**:
- create: `crates/wjsm-runtime/src/startup_snapshot_cache.rs`
- modify: `crates/wjsm-runtime/src/lib.rs`
- modify: `crates/wjsm-runtime/src/runtime_host_helpers.rs`（如需固定 name-id helper）

**Cache key**:

```text
wjsm-startup-snapshot-v{format_version}-{abi_hash}-{target_arch}-{debug_or_release}.bin
```

**Cache path**:

```text
WJSM_STARTUP_SNAPSHOT_CACHE=<dir>  // 显式目录
默认: std::env::temp_dir()/wjsm/startup-snapshot/  // 测试/开发安全默认；发布嵌入路径另开计划
```

**Internal flow**:

```text
execute_with_writer_shared_inner
  -> instantiate_async user module
  -> extract WasmEnv
  -> startup_snapshot_cache::get_or_build(&engine).await
       cache hit: decode bytes once
       cache miss: build_seed_snapshot().await, write temp file, atomic rename
  -> restore_startup_snapshot(user store, user env, snapshot)
  -> main.call_async
       main skips __wjsm_bootstrap_once
       main runs __wjsm_init_function_props
       main executes user body
```

**Seed builder**:

- [ ] 使用 `compile_source("")` 生成 seed wasm。
- [ ] 使用同一套 `register_linker/register_common_bridges/register_complex_bridges` instantiate seed。
- [ ] 调 seed instance 的 `__wjsm_bootstrap_once.call_async()`。
- [ ] 执行从 `execute_with_writer_shared_inner` 抽出的 `init_host_primordials(&mut store, &wasm_env)`，创建 `%AsyncIteratorPrototype%` / `AsyncGenerator.prototype`。
- [ ] 调 `capture_startup_snapshot`。
- [ ] 禁止 seed builder 递归调用 `get_or_build`；用 `StartupMode::BuildSnapshot` 参数显式绕过 restore。

**Hot path constraints**:

- [ ] `OnceLock<Result<Arc<[u8]>>>` 保存原始 bytes；decode view 在 restore 前轻量创建。
- [ ] 磁盘写入走临时文件 + atomic rename；并发进程竞争时，rename 失败者重新读已存在文件。
- [ ] ABI mismatch 删除旧 cache 并重建一次；重建后仍 mismatch 直接错误。
- [ ] `WJSM_STARTUP_SNAPSHOT=0` 时跳过 cache/restore，执行 cold bootstrap，用于 A/B benchmark 和紧急诊断。

**Tests**:

- [ ] `snapshot_cache_hit_skips_builder`：第一次 build，第二次命中同一 bytes。
- [ ] `snapshot_cache_abi_mismatch_rebuilds_once`：写入旧 hash 文件，运行后替换为新 hash。
- [ ] `execute_snapshot_on_off_same_output`：同一 fixture 在开/关 snapshot 下 stdout/stderr/exit 一致。
- [ ] 运行：
  ```bash
  cargo nextest run -p wjsm-runtime -E 'test(startup_snapshot_cache) or test(execute_with_writer)'
  cargo nextest run -E 'test(happy__) or test(modules__)'
  ```

---

# P7：性能验收与默认开启策略

**Why**: snapshot 是高性能功能，不是结构玩具。必须用阶段耗时证明 restore 比 cold bootstrap 更便宜，并确认 workspace 总耗时变化符合预期。

**Files**:
- modify: `crates/wjsm-runtime/src/lib.rs`（bench 输出最终化）

**Steps**:

- [ ] 扩展 `bench_execute_phases` 输出：
  ```text
  BENCH bootstrap cold
  BENCH snapshot build cold
  BENCH snapshot decode
  BENCH snapshot restore
  BENCH full execute snapshot off
  BENCH full execute snapshot on warm cache
  ```
- [ ] 运行：
  ```bash
  WJSM_STARTUP_SNAPSHOT=0 cargo test -p wjsm-runtime bench_execute_phases -- --ignored --nocapture
  WJSM_STARTUP_SNAPSHOT=1 cargo test -p wjsm-runtime bench_execute_phases -- --ignored --nocapture
  cargo nextest run --workspace
  ```
- [ ] 默认开启条件：`snapshot restore` 必须低于 `bootstrap cold + host post-bootstrap`；`full execute snapshot on warm cache` 不能比 off 慢。
- [ ] 若 workspace wall time 改善小于 5%，保留默认开启仍可接受，但文档必须说明瓶颈在 process/linker/compile，而 snapshot 主要服务 cold-start latency 与未来 realm/agent 创建。
- [ ] 若 restore 慢于 bootstrap，默认关闭，保留显式 `WJSM_STARTUP_SNAPSHOT=1`，并把失败原因写入 ADR 的 rejected alternative。

---

# P8：文档、ADR、维护规则

**Why**: snapshot 引入持久 ABI。未来新增 builtin/side table 时必须知道如何更新 whitelist 与 hash，否则会产生难查的启动堆错配。

**Files**:
- create: `docs/adr/0003-startup-snapshot-boundary.md`
- modify: `docs/async-scheduler.md`
- modify: `AGENTS.md`
- modify: `docs/aegis/INDEX.md`

**ADR content**:

- [ ] Status: Accepted after implementation verification。
- [ ] Decision: wjsm 使用 relocatable primordial heap snapshot，不使用 Wasmtime Instance/Store snapshot。
- [ ] Snapshot includes/excludes 清单。
- [ ] ABI hash 输入清单。
- [ ] 函数属性 handle layout 从 `0..num_ir_functions` 改为 `function_props_base..function_props_base+num_ir_functions`。
- [ ] RuntimeState 继续保持扁平，遵守 ADR 0002。
- [ ] Async scheduler 状态不进入 snapshot。
- [ ] 新增 builtin/NativeCallable/primordial string 时的维护步骤。

**Docs updates**:

- [ ] `docs/async-scheduler.md` 增加：startup snapshot 不保存 scheduler/worker/async op 状态，restore 只在 scheduler owner 上执行。
- [ ] `AGENTS.md` 增加 load-bearing convention：snapshot boundary、ABI hash、whitelist、GC root rule。
- [ ] `docs/aegis/INDEX.md` 增加本计划和 ADR/文档引用。

**Final verification**:

```bash
cargo nextest run -p wjsm-runtime -E 'test(startup_snapshot) or test(async_scheduler) or test(async_reentry)'
cargo nextest run -p wjsm-backend-wasm
cargo nextest run -E 'test(happy__) or test(modules__) or test(semantic__)'
cargo nextest run --workspace
cargo test -p wjsm-runtime bench_execute_phases -- --ignored --nocapture
```

---

## Self-review checklist

- Spec coverage: 覆盖 Deno/V8 类 startup snapshot 的核心作用：预初始化 runtime heap，启动时恢复，跳过重复 bootstrap。
- 占位扫描: 本计划不含空白占位；每阶段有明确文件、contract、测试命令。
- Type consistency: 新 API 名称在 backend/runtime/cache 三层一致：`__wjsm_bootstrap_once`、`__wjsm_init_function_props`、`__function_props_base`、`StartupSnapshotOwned/View`。
- Compatibility: public runtime API 不变；fixture 输出不变；snapshot 排除 active async/host state。
- Plan-time complexity: 新 owner files 承载格式/cache/capture；不继续膨胀 `lib.rs` 和 `compiler_module.rs` 热点块。
- Verification: 每阶段有定向命令，最终有 workspace + bench。
- Retirement: `0..num_ir_functions` 函数属性根规则明确退役；execute 中散落 host post-bootstrap 初始化抽为单一 helper。