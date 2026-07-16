# wjsm Generational ZGC 性能重构实施计划

> **状态**：Round 3 共识通过；主机资源/OOM安全修订已纳入  
> **父规格**：`docs/aegis/specs/2026-07-16-zgc-performance-design.md`  
> **执行约束**：项目不使用worktree；按任务顺序在主工作树执行。每个任务先建立RED证据，再实现，再运行GREEN命令并形成独立提交。Task 0失败时整个计划进入`blocked`。

## Goal

把全部collector迁移到统一shared memory64 `ManagedHeap`，实现真正的Generational ZGC concurrent young/old mark、concurrent relocation、精确remembered set、page/NLAB allocator、预测式pacing与平台内存优化；按固定采集方法证明归一化算法成本不弱于JDK 25 Generational ZGC 10%，并至少两项领先15%。

## Architecture

- 32位JS handle identity保持；动态对象地址使用u64/i64。
- imported shared memory64对象堆；Wasm共享heap word只使用Wasm `SeqCst`原子。
- u32 handle table在memory64中保留连续32 GiB虚拟区，8-byte `AtomicU64` entry按需提交。
- `ManagedHeap<M: HeapMemory>`静态单态化；生产使用`SharedHeapMemory`，协议测试使用native/loom backend。
- `Arc<GcRuntime>`统一heap/controller/workers/director/telemetry；mutator与collector context分离。
- NaN-box bit 38–43只给handle-backed heap reference着色；非引用值保持这些位为零。
- mark-sweep、G1、ZGC共享底座，旧memory32动态heap在协调切换任务中删除。

## Tech Stack

Rust 2024、Wasmtime 43.0.2、wasm-encoder、crossbeam-deque、loom、hdrhistogram、nextest、Miri/TSan、OpenJDK 25 GA diagnostic probe、JFR、perf。

## Compatibility Boundary

- 保持ECMAScript语义、32位boxed handle payload、`--gc mark-sweep|g1|zgc`、`gc()`、realm/async-hooks/side-table语义与snapshot开关。
- 内部memory index、object address、entry、support imports与snapshot format一次性升级。
- 私有`managed-heap-v2`编译门只允许存在于准备任务，默认关闭、不进入发布产物，并在协调切换任务中与旧heap一起删除。
- 不保留runtime fallback、公开feature、compat adapter或deprecated re-export。

## Verification

- 协议：loom、Miri pure model、TSan std model、SharedMemory nextest分离。
- 行为：三collector fixture matrix、WeakRef/finalization、realm、async-hooks、snapshot cold/hot。
- 性能：固定stock/instrumented JDK 25、scenario hash、physical/logical denominators、30样本、99% CI、perf/JFR/telemetry原始数据。
- 平台：本地只关闭实际capability；AVX-512、AArch64、Windows、macOS、多NUMA和4/16 GiB必须由具名runner关闭。

---

## 1. Plan Basis

### Facts

- 当前ZGC只在mutator safepoint内推进；分配仍是连续bump。
- 当前`GcAlgorithm`位于`Arc<Mutex<Box<dyn ...>>>`，`GcContext`持`StoreContextMut`。
- 当前动态对象在普通memory32，active handle entry为4 bytes。
- `Object.setPrototypeOf`会修改publish后的prototype header；header不能整体视为immutable。
- Wasm threads原子没有Acquire/Release选择，只有`SeqCst`语义。
- 当前benchmark没有提供可比较的GC CPU、mark/relocate bytes、barrier events或metadata分项。

### Architecture Integrity Lens

- Canonical owner：`GcRuntime` + `ManagedHeap`。
- 禁止owner：collector私有memory grow、host imports直接`Memory::data_mut`、worker持Store/Caller/WasmEnv。
- 切换策略：先在私有编译门下完成所有caller，随后单任务切换active ABI并删除旧path。
- Retirement：旧memory32 dynamic heap、4-byte entry、collector全局mutex、ZGC bump、无效benchmark入口。

### Complexity Budget

- `host_imports/core.rs`、`runtime_gc/api.rs`、`object_walker.rs`与backend巨型helper已超出继续add-in-place的安全边界。
- 新owner文件目标≤500行、函数目标≤30行；GC imports移出core；benchmark独立crate。
- 28个任务按严格依赖排序；准备任务不得修改active runtime ABI。

### TDD Route

- strict：entry/page/bitmap/epoch/barrier/young/old/relocation/pacer。
- light：engine config、Wasmtime integration、snapshot/realm/support。
- performance：固定失败gate后profile，不修改阈值或场景关闭失败。

---

# Phase A — 先证伪平台与测量合同

## Task 0：建立唯一Wasmtime engine config并通过shared memory64可行性门

**Files**

- Create：`crates/wjsm-engine-config/{Cargo.toml,src/lib.rs}`
- Create：`crates/wjsm-runtime/tests/shared_memory64_feasibility.rs`
- Modify：workspace `Cargo.toml`、`Cargo.lock`
- Modify：`runtime_engine_pool.rs`、`wjsm-runtime-support/build.rs`、snapshot build调用链、snapshot-format ABI外部输入

**Why**：在任何value/layout/control-plane投资前证明threads + multi-memory + main memory32 + imported shared memory64 + support cwasm precompile/deserialize真实可行，并消除build/runtime config漂移。

**GREEN**

```bash
cargo nextest run -p wjsm-engine-config
cargo nextest run -p wjsm-runtime --test shared_memory64_feasibility
cargo nextest run -p wjsm-runtime-support
```

- [ ] **Write test**：固定config fingerprint；编译user/support双module，共享同一memory64；保留32 GiB handle虚拟区后在`object_heap_base > 32 GiB`执行Wasm atomic与host线程并发访问；用同fingerprint precompile support cwasm并在runtime engine deserialize/instantiate。
- [ ] **Verify RED**：运行三条命令；预期缺少config owner与feasibility target。
- [ ] **Implement**：新增唯一config crate，固定threads/memory64/multi-memory/Cranelift和fingerprint；runtime、support、snapshot build全部复用。
- [ ] **Verify GREEN**：三条命令全部通过；若任一失败，状态=`blocked`，不继续Task 1。
- [ ] **Commit**：`feat: centralize Wasmtime engine configuration`。

## Task 1：固定telemetry、benchmark CLI与JDK 25 diagnostic probe

**Files**

- Create：`crates/wjsm-gc-bench/Cargo.toml`
- Create：`src/{main,schema,scenario,resource,wjsm_driver,jvm_driver,jdk_probe,stats,report,gate}.rs`
- Create：`jdk-probe/0001-zgc-benchmark-counters.patch`
- Create：`java/src/WjsmGcBench.java`
- Create：runtime `runtime_gc/telemetry/{mod,histogram,json}.rs`
- Modify：workspace/runtime/CLI manifests与args

**CLI contract**

- 子命令：`capabilities`、`preflight`、`prepare-jdk`、`baseline`、`run`、`micro`、`compare`、`replay`、`gate`。
- 参数：`--engine --gc --heap --live-set --scenario --samples --duration --workers --seed --output --manifest --profile --jdk-home --jdk-probe-home`。
- Adversarial：`--relocate-every-page --barrier-buffer-capacity --safepoint-every-allocation`。

**Metric contract**

- GC CPU/allocated byte：GC worker + pause + mutator assist thread CPU / physical allocated bytes。
- mark CPU/live byte、relocation CPU/relocated byte：instrumented JDK与WJSM同名phase CPU / physical bytes。
- barrier retired instructions/event：reference scenario perf instructions减非引用control，再分别除以`barrier_load_fast_events`与`barrier_store_fast_events`；load/store拥有独立JSON schema字段和gate。
- metadata bytes/object：committed handle/page/bitmap/remset/forwarding/worker metadata / live objects。
- stock JDK仅用于pause/RSS/end-to-end；内部指标只用固定`jdk-25-ga` patch build。

**Resource contract**

- `effective_total=min(physical_total, finite cgroup/job limit)`；`effective_available=min(MemAvailable, finite cgroup/job remaining)`；swap不计预算。
- `required_total=4*max_heap_cap`；`required_available=3*max_heap_cap+max(2 GiB, 10%*effective_total)`；虚拟地址上限还必须容纳32 GiB handle区、control、object heap和Wasmtime guards。
- 失败在spawn前返回`needs-resource-runner`/exit 78，不自动缩heap；大堆profile还必须具备delegated cgroup v2或Windows Job Object硬隔离。

**GREEN**

```bash
cargo nextest run -p wjsm-gc-bench
cargo nextest run -p wjsm-runtime -E 'test(gc_telemetry)'
cargo run --release -p wjsm-gc-bench -- preflight --heap 1g --profile pr --output /tmp/wjsm-gc-preflight.json
cargo run --release -p wjsm-gc-bench -- baseline --engine wjsm --gc zgc --heap 32m --scenario churn --samples 30 --output /tmp/wjsm-zgc-baseline.json
```

- [ ] **Write test**：CLI/JSON/manifest/logical graph/denominators；用fake `HostResourceProvider`模拟4 GiB available，断言1/4/16 GiB均exit 78且child spawn计数为零，256 MiB通过；缺JDK numerator时gate=`needs-verification`。
- [ ] **Verify RED**：运行前两条命令；预期crate/schema/resource owner不存在。
- [ ] **Implement**：实现全部CLI、telemetry、JDK probe、JFR/perf、统计；实现Linux cgroup/meminfo/PSI/rlimit、Windows Job Object、macOS host resource provider与fail-closed admission。
- [ ] **Verify GREEN**：运行四条命令；raw JSON含资源snapshot、预算公式、admission decision、版本/硬件/counter来源，且steady-state不含compile/instantiate。
- [ ] **Commit**：`feat: establish reproducible GC benchmark contract`。

## Task 2：添加非激活的NaN-box GC color helpers

**Files**

- Modify：`wjsm-ir/src/value.rs`
- Test：value unit/property tests

**Boundary**：本任务只添加bit 38–43常量、`strip_gc_color`和`is_handle_backed_reference`；不改active entry size、backend store、snapshot ABI或runtime heap。

**GREEN**

```bash
cargo nextest run -p wjsm-ir
cargo run -- run -e 'const x={}; const y=x; console.log(x===y)'
```

- [ ] **Write test**：全部handle-backed tags与runtime string可着色；number、static/inline string、bool、null、undefined保持bit 38–43为零；strip后语义不变。
- [ ] **Verify RED**：缺少helpers而失败。
- [ ] **Implement**：添加纯函数和常量，不接入active runtime。
- [ ] **Verify GREEN**：测试通过，smoke输出`true`，active snapshot hash不变。
- [ ] **Commit**：`feat: define inactive GC reference colors`。

---

# Phase B — 私有编译门下构建新底座

## Task 3：建立私有`managed-heap-v2`门与并发control plane

**Files**

- Create：`runtime_gc/{control,mutator,collector_context}.rs`
- Modify：runtime/backend/support manifests，添加默认关闭且不发布的内部feature
- Test：`gc_control_plane.rs`

**Boundary**：旧RuntimeState与active collector继续运行；新control plane只在feature test构造。feature在Task 15删除。

**GREEN**

```bash
cargo nextest run -p wjsm-runtime --features managed-heap-v2 --test gc_control_plane
cargo nextest run -p wjsm-runtime --no-default-features -E 'test(runtime_options_default)'
```

- [ ] **Write test**：`CollectorContext: Send + Sync`；不捕获Store/Caller/WasmEnv；controller无包围整个算法的mutex。
- [ ] **Verify RED**：缺少types/feature而失败。
- [ ] **Implement**：`GcRuntimeV2`、Mutator/Collector contexts、RootSnapshot契约；不切换active字段。
- [ ] **Verify GREEN**：feature与默认路径分别GREEN。
- [ ] **Commit**：`refactor: stage concurrent GC control plane`。

## Task 4：实现泛型HeapMemory、SharedHeapMemory与测试后端

**Files**

- Create：`heap/{mod,memory,word,native_memory}.rs`
- Create：tests `managed_heap_memory.rs`、`gc_protocol_miri.rs`

**Interface**

```rust
pub(crate) trait HeapMemory: sealed::Sealed + Send + Sync { /* checked word/range API */ }
pub(crate) type RuntimeManagedHeap = ManagedHeap<SharedHeapMemory>;
```

生产热路不得出现`dyn HeapMemory`。共享value/mutable-header word使用SeqCst；Rust私有metadata才允许更弱ordering。

**GREEN**

```bash
cargo nextest run -p wjsm-runtime --features managed-heap-v2 --test managed_heap_memory
cargo +nightly miri test -p wjsm-runtime --features managed-heap-v2 --test gc_protocol_miri
```

- [ ] **Write test**：对齐/越界、u64地址不截断、SeqCst跨线程、publish/resolve、mutable header、raw byte pin/copy边界；Miri只使用NativeHeapMemory。
- [ ] **Verify RED**：missing heap backend而失败。
- [ ] **Implement**：sealed generic API、Shared/Native backend、中文SAFETY不变量；不修改Linker active memory。
- [ ] **Verify GREEN**：nextest与Miri通过。
- [ ] **Commit**：`feat: add generic managed heap memory backends`。

## Task 5：实现8-byte handle table、连续虚拟区与epoch quarantine

**Files**：`heap/{handle,epoch}.rs`；tests `gc_loom_model.rs`、`gc_concurrency_model.rs`。

**Boundary**：实现V2 entry但不修改active `HANDLE_TABLE_ENTRY_SIZE=4`。memory64区间固定为`[0,32 GiB)`handle table、其后的对齐control buffer区和`[object_heap_base, object_heap_base+max_heap_size)`对象堆；4/16 GiB仅表示对象堆cap。resolve为`handle_base+handle*8`。

**GREEN**

```bash
cargo nextest run -p wjsm-runtime --features managed-heap-v2 -E 'test(handle_table)'
cargo nextest run -p wjsm-runtime --features managed-heap-v2 --test gc_loom_model -E 'test(handle_)'
```

- [ ] **Write test**：高位u32 handle寻址、reserved/committed分离、object heap base不与handle/control区重叠、4/16 GiB cap地址计算、publish/resolve、young→old、relocating、retire/quarantine/reuse、loom ABA竞争。
- [ ] **Verify RED**：missing V2 table而失败。
- [ ] **Implement**：AtomicU64 states、SeqCst shared entry、block commit、epoch participants/quarantine。
- [ ] **Verify GREEN**：测试通过；stable resolve反汇编保持一次entry load与直接寻址。
- [ ] **Commit**：`feat: add sparse-commit atomic handle table`。

## Task 6：实现page/NLAB/object map/bitmap allocator

**Files**：`heap/{page,allocator,bitmap,object_map}.rs`；test `managed_heap_allocator.rs`。

**GREEN**

```bash
cargo nextest run -p wjsm-runtime --features managed-heap-v2 --test managed_heap_allocator
cargo run --release -p wjsm-gc-bench -- micro --component allocator --heap 256m --samples 30 --output /tmp/gc-allocator.json
```

- [ ] **Write test**：split/coalesce、small/medium/large/humongous、NLAB refill、object-start/size、双bitmap、reserve隔离、reserved/committed metadata。
- [ ] **Verify RED**：allocator types与`micro allocator`component缺失。
- [ ] **Implement**：64KiB granule、heap-relative pages、streaming bitmap iterator、无分配fast path；在Task 1既有CLI中注册allocator component。
- [ ] **Verify GREEN**：nextest与micro command通过，无全局mutex/heap allocation于NLAB fast path。
- [ ] **Commit**：`feat: add page and NLAB managed allocator`。

## Task 7：实现固定worker pool并分离loom/Miri/TSan模型

**Files**：`worker/{mod,packet,queue}.rs`；tests `gc_loom_model.rs`、`gc_protocol_miri.rs`、`gc_concurrency_model.rs`。

**GREEN**

```bash
cargo nextest run -p wjsm-runtime --features managed-heap-v2 -E 'test(gc_worker)'
cargo nextest run -p wjsm-runtime --features managed-heap-v2 --test gc_loom_model -E 'test(worker_)'
RUSTFLAGS="-Zsanitizer=thread" cargo +nightly test -p wjsm-runtime --features managed-heap-v2 --test gc_concurrency_model
```

- [ ] **Write test**：packet reuse/work-steal/park/wake/termination；loom模型不链接crossbeam/Wasmtime；Miri不执行worker deque；TSan使用std原子+生产queue。
- [ ] **Verify RED**：缺少worker与分层targets而失败。
- [ ] **Implement**：crossbeam deque、packet slab、inflight termination、ordered shutdown；协议模型使用独立backend。
- [ ] **Verify GREEN**：三类命令各自在支持范围内通过，warmup后packet allocation不增长。
- [ ] **Commit**：`feat: add reusable GC worker pool`。

---

# Phase C — 私有门下迁移所有caller

## Task 8：生成feature-gated shared memory64 backend/support ABI

**Files**

- Modify：backend `lib.rs`、`support_module.rs`、`support_object_helpers.rs`、`compiler_module/*`
- Split：`compiler_helpers/helpers_object.rs`为`helpers_object/{mod,alloc,resolve,property,array}.rs`
- Test：backend heap-memory64 WAT/import tests

**GREEN**

```bash
cargo nextest run -p wjsm-backend-wasm --features managed-heap-v2 -E 'test(heap_memory64)'
cargo run --features managed-heap-v2 -- dump-wat -e 'const x={a:[1,2,3]}; console.log(x.a[1])' --skeleton
```

- [ ] **Write test**：user/support共享`env.__heap_memory` shared memory64；dynamic object address i64；static memory保持memory32；V2 entry为8 bytes。
- [ ] **Verify RED**：feature WAT/import缺失。
- [ ] **Implement**：新memory index、i64 globals、NLAB/resolve helpers和V2 support artifact；默认path不变。
- [ ] **Verify GREEN**：feature WAT正确，默认backend tests仍GREEN。
- [ ] **Commit**：`feat: stage shared memory64 backend ABI`。

## Task 9：迁移feature-gated host动态对象访问到HeapAccessV2

**Files**

- Rewrite：`runtime_gc/heap_access.rs`
- Modify：`runtime_heap.rs`、`wasm_env.rs`、`host_imports/{gc,object,array_object,string,collections,iterator,promise,proxy,typed_array}.rs`及结构搜索命中模块
- Move：GC实现从`host_imports/core.rs`到`gc.rs`

**GREEN**

```bash
cargo nextest run -p wjsm-runtime --features managed-heap-v2 -E 'test(host_heap_access_v2)'
```

- [ ] **Write test**：对象/数组/property/runtime-string/proxy/collection在V2 heap resolve/write；target API携带handle，不依赖raw ptr反查。
- [ ] **Verify RED**：旧main memory/ptr API不满足。
- [ ] **Implement**：feature-gated concrete HeapAccessV2；缓存静态main-memory handles；消除V2 `WasmEnv::from_caller`热路与`handle_for_ptr`。
- [ ] **Verify GREEN**：测试通过；结构搜索确认V2 host动态对象不调用`env.memory.data_mut`。
- [ ] **Commit**：`refactor: stage centralized host heap access`。

## Task 10：迁移mark-sweep policy到V2底座

**Files**：`runtime_gc/mark_sweep/*`、roots/control；feature integration tests。

**GREEN**

```bash
cargo nextest run -p wjsm-runtime --features managed-heap-v2 -E 'test(mark_sweep_v2)'
```

- [ ] **Write test**：V2 root snapshot/object map mark、page sweep、handle quarantine、OOM/full collection与side-table cleanup。
- [ ] **Verify RED**：旧GcContext/memory32依赖失败。
- [ ] **Implement**：feature-gatedMarkSweepV2使用ManagedHeap；active默认collector不切换。
- [ ] **Verify GREEN**：V2与默认mark-sweep tests同时通过。
- [ ] **Commit**：`refactor: stage mark-sweep managed heap policy`。

## Task 11：迁移G1 policy到V2底座

**Files**：`runtime_gc/g1/*`；feature tests。

**GREEN**

```bash
cargo nextest run -p wjsm-runtime --features managed-heap-v2 -E 'test(g1_v2)'
```

- [ ] **Write test**：eden/survivor/old、precise remset、young/mixed evacuation、humongous、promotion failure。
- [ ] **Verify RED**：私有RegionSpace与V2不兼容。
- [ ] **Implement**：G1V2复用pages/bitmaps/workers/telemetry；active默认G1不切换。
- [ ] **Verify GREEN**：V2/default G1 tests通过，无V2第二handle table。
- [ ] **Commit**：`refactor: stage G1 managed heap policy`。

## Task 12：迁移现有增量ZGC policy到V2底座

**Files**：`runtime_gc/zgc/*`、scheduler/roots；feature tests。

**GREEN**

```bash
cargo nextest run -p wjsm-runtime --features managed-heap-v2 -E 'test(zgc_v2_incremental)'
```

- [ ] **Write test**：现有mark/relocate语义在V2 page/handle/object map上保持，尚不启用concurrent worker或colored store。
- [ ] **Verify RED**：旧bump/4-byte entry依赖失败。
- [ ] **Implement**：ZgcV2 policy复用ManagedHeap但保持增量safepoint行为，作为切换前功能等价基线。
- [ ] **Verify GREEN**：V2与默认ZGC tests通过。
- [ ] **Commit**：`refactor: stage ZGC managed heap policy`。

## Task 13：准备feature-gated snapshot/support artifact ABI

**Files**

- Modify：snapshot-format、startup_snapshot、runtime-snapshot build、runtime-support build/src/abi
- Test：V2 snapshot/support compatibility tests

**GREEN**

```bash
cargo nextest run -p wjsm-snapshot-format
cargo nextest run -p wjsm-runtime-support --features managed-heap-v2
cargo nextest run -p wjsm-runtime --features managed-heap-v2 -E 'test(startup_snapshot_v2)'
```

- [ ] **Write test**：V2 page metadata/8-byte entries/generation、engine fingerprint、old format拒绝、support cwasm V2 deserialize。
- [ ] **Verify RED**：V2格式与artifact缺失。
- [ ] **Implement**：新增未激活V2 format与artifact，不提升active FORMAT_VERSION。
- [ ] **Verify GREEN**：V2和active V1 tests均通过。
- [ ] **Commit**：`feat: stage managed heap snapshot ABI`。

## Task 14：准备feature-gated realm、handle-remap与side-table迁移

**Files**：`handle_remap.rs`、realm/realm_clone/runtime_node_vm、roots、Promise/stream/proxy/async side-table owners。

**GREEN**

```bash
cargo nextest run -p wjsm-runtime --features managed-heap-v2 -E 'test(vm_gc_realm_roots_v2) | test(realm_clone_v2) | test(side_table_gc_v2)'
```

- [ ] **Write test**：realm共享单ManagedHeap；clone/remap只操作handles；conditional roots/side tables在V2 GC后无悬垂handle。
- [ ] **Verify RED**：旧WasmEnv/main-memory假设失败。
- [ ] **Implement**：feature-gated V2 realm/root/side-table adapters，不复制collector/worker/epoch。
- [ ] **Verify GREEN**：feature tests与默认realm tests同时通过。
- [ ] **Commit**：`refactor: stage realm and side-table heap migration`。

## Task 15：协调切换active ABI并删除私有门和旧dynamic heap

**Files**：IR constants、backend/runtime/support/snapshot/realm/三collector全部active caller；删除feature cfg与旧heap owner。

**Why**：B1/B3的原子切换点。只有前置Tasks 0–14全部GREEN后执行。

Task 15保持单一active切换任务是架构约束：按collector拆分会让公开`--gc`模式同时依赖两套dynamic heap，或让尚未切换的collector不可用。风险通过切换前release-candidate gate降低，而不是引入runtime双轨。

**Pre-cutover GREEN（仍在私有门下）**

```bash
cargo nextest run --workspace --all-features
WJSM_TEST_GC=mark-sweep cargo nextest run --all-features -E 'test(happy__)'
WJSM_TEST_GC=g1 cargo nextest run --all-features -E 'test(happy__)'
WJSM_TEST_GC=zgc cargo nextest run --all-features -E 'test(happy__)'
```

**GREEN**

```bash
cargo nextest run --workspace
WJSM_TEST_GC=mark-sweep cargo nextest run -E 'test(happy__)'
WJSM_TEST_GC=g1 cargo nextest run -E 'test(happy__)'
WJSM_TEST_GC=zgc cargo nextest run -E 'test(happy__)'
cargo run -- run --gc mark-sweep -e 'const x={a:[1,2,3]}; gc(); console.log(x.a[1])'
cargo run -- run --gc g1 -e 'const x={a:[1,2,3]}; gc(); console.log(x.a[1])'
cargo run -- run --gc zgc -e 'const x={a:[1,2,3]}; gc(); console.log(x.a[1])'
```

- [ ] **Write cutover audit**：先确认Tasks 0–14均已形成提交且当前HEAD通过全部Pre-cutover GREEN，再生成逐caller cutover manifest，枚举active 4-byte entry、memory32 dynamic heap、old GcContext、private feature与V1 snapshot/support references；切换前audit必须RED。
- [ ] **Verify RED**：audit报告全部旧active owners；任一Pre-cutover GREEN失败则不得开始active切换。
- [ ] **Implement**：以单一原子提交切换8-byte entry/shared memory64/GcRuntimeV2/三policy/V2 snapshot/support/realm，并删除private feature定义、cfg和旧dynamic heap主路径；不按collector拆分active ABI。
- [ ] **Verify GREEN**：全部命令通过；active tree不含private gate、runtime fallback或两套dynamic heap owner。
- [ ] **Commit**：`refactor: cut over all collectors to managed heap`。

---

# Phase D — Colored barriers与Generational ZGC

## Task 16：实现colored load/store barrier与verifier

**Files**：backend `compiler_helpers/barrier.rs`、object helpers；runtime `zgc/barrier.rs`、heap_access；tests `gc_barrier_protocol.rs`、loom model。

**GREEN**

```bash
cargo nextest run -p wjsm-backend-wasm -E 'test(gc_barrier)'
cargo nextest run -p wjsm-runtime --test gc_barrier_protocol
cargo nextest run -p wjsm-runtime --test gc_loom_model -E 'test(satb_) | test(remembered_)'
```

- [ ] **Write test**：stable/relocating load、SATB、old→young、1-slot buffer、runtime-string reference、所有非引用值bits清零、mutable prototype header、bulk copy verifier。
- [ ] **Verify RED**：barrier contracts缺失。
- [ ] **Implement**：Wasm SeqCst atomic fast path、preallocated rings、reference-only coloring、mutable-header barrier/verifier。
- [ ] **Verify GREEN**：测试通过；stable WAT无host call，非引用值不带color。
- [ ] **Commit**：`feat: add colored GC barriers`。

## Task 17：实现concurrent young mark

**Files**：`zgc/young.rs`、rewrite `mark.rs`、control/roots/worker/telemetry；tests `gc_young_concurrent.rs`。

**GREEN**

```bash
cargo nextest run -p wjsm-runtime --test gc_young_concurrent
cargo nextest run -p wjsm-runtime --test gc_loom_model -E 'test(young_)'
cargo run --release -p wjsm-gc-bench -- run --engine wjsm --gc zgc --heap 32m --scenario churn --samples 30 --output /tmp/young.json
```

- [ ] **Write test**：mark-start root snapshot、SATB、new allocation black、termination、pause内无page scan/copy。
- [ ] **Verify RED**：young phase不存在。
- [ ] **Implement**：type-state phases、work packets、concurrent mark/termination/telemetry。
- [ ] **Verify GREEN**：测试和benchmark通过，max pause<1ms。
- [ ] **Commit**：`feat: implement concurrent young marking`。

## Task 18：实现精确跨代remset、age与原地晋升

**Files**：`zgc/{young,barrier}.rs`、page/bitmap；young tests。

**GREEN**

```bash
cargo nextest run -p wjsm-runtime --test gc_young_concurrent -E 'test(remset_) | test(promotion_)'
cargo nextest run -p wjsm-runtime --test gc_loom_model -E 'test(remset_) | test(promotion_)'
cargo run --release -p wjsm-gc-bench -- run --engine wjsm --gc zgc --heap 256m --scenario request --live-set 50 --samples 30 --output /tmp/remset.json
```

- [ ] **Write test**：old→young写/覆写/删除、double buffer、slot去重、dense/humongous原地晋升；loom穷举epoch flip与并发slot写、promotion publish与young mark竞争。
- [ ] **Verify RED**：remset/promotion缺失。
- [ ] **Implement**：精确slot bitmap、age/survival、in-place promotion。
- [ ] **Verify GREEN**：young work不随old heap总大小线性增长。
- [ ] **Commit**：`feat: add precise remembered sets and promotion`。

## Task 19：实现跨young cycle的concurrent old mark

**Files**：`zgc/old.rs`、young/director/roots；tests `gc_old_concurrent.rs`。

**GREEN**

```bash
cargo nextest run -p wjsm-runtime --test gc_old_concurrent
cargo nextest run -p wjsm-runtime --test gc_loom_model -E 'test(old_)'
cargo run --release -p wjsm-gc-bench -- run --engine wjsm --gc zgc --heap 1024m --scenario request --live-set 80 --samples 30 --output /tmp/old.json
```

- [ ] **Write test**：old由young mark-start协调、跨young cycles、promotion frontier、young→old roots、termination。
- [ ] **Verify RED**：old controller缺失。
- [ ] **Implement**：独立old epoch/bitmap/queues与young-old handshake。
- [ ] **Verify GREEN**：old mark按old live bytes归一化，young pause不执行old全量工作。
- [ ] **Commit**：`feat: implement concurrent old marking`。

## Task 20：实现concurrent relocation、assist与epoch reclaim

**Files**：rewrite `zgc/relocate.rs`；handle/page/epoch/barrier/worker；tests `gc_relocation_concurrent.rs`。

**GREEN**

```bash
cargo nextest run -p wjsm-runtime --test gc_relocation_concurrent
cargo nextest run -p wjsm-runtime --test gc_loom_model -E 'test(relocation_) | test(epoch_reclaim)'
cargo run --release -p wjsm-gc-bench -- run --engine wjsm --gc zgc --heap 256m --scenario mutation --relocate-every-page --samples 30 --output /tmp/relocate.json
```

- [ ] **Write test**：copy ownership、assist、same-slot竞争、prototype更新竞争、destination publish、grace period、young/old互斥。
- [ ] **Verify RED**：旧同步relocate失败。
- [ ] **Implement**：descriptor、relocate-start handshake、atomic mutable-header/value snapshot、assist与epoch reclaim。
- [ ] **Verify GREEN**：pause内无copy、max<1ms、无source lost write。
- [ ] **Commit**：`feat: implement concurrent ZGC relocation`。

## Task 21：完成WeakRef/finalization/realm/side-table并发周期语义

**Files**：weak_refs、roots、realm/async/promise/stream/proxy side tables；integration fixtures/tests。

**GREEN**

```bash
cargo nextest run -p wjsm-runtime --test integration -E 'test(vm_gc_realm_roots) | test(startup_snapshot_gc_fixes)'
WJSM_TEST_GC=zgc cargo nextest run -E 'test(happy__weakref_gc) | test(happy__finalization_registry_cleanup) | test(happy__gc_async_await) | test(happy__async_hooks_destroy_gc)'
```

- [ ] **Write test**：side table不反向保活、finalizer一次、realm destroy与old mark、snapshot restore后weak cleanup。
- [ ] **Verify RED**：旧sweep-only cleanup失败。
- [ ] **Implement**：root snapshot/conditional roots/weak queues；cleanup早于quarantine，callback晚于cycle publish。
- [ ] **Verify GREEN**：两条命令通过，三collector语义一致。
- [ ] **Commit**：`feat: integrate host roots with concurrent GC`。

---

# Phase E — Pacing与平台能力

## Task 22：实现young/old director、assist与stall

**Files**：`zgc/director.rs`、scheduler、allocator/control/telemetry/CLI。

**GREEN**

```bash
cargo nextest run -p wjsm-runtime -E 'test(gc_director)'
cargo run --release -p wjsm-gc-bench -- run --engine wjsm --gc zgc --heap 256m --scenario saturation --samples 30 --output /tmp/pacer.json
```

- [ ] **Write test**：EWMA、prediction error、young/old start、reserve、assist cap、stall only reserve exhaustion、OOM。
- [ ] **Verify RED**：固定scheduler不满足。
- [ ] **Implement**：runway model、比例assist、structured stall reason。
- [ ] **Verify GREEN**：无无因stall，稳定负载prediction收敛。
- [ ] **Commit**：`feat: add predictive ZGC pacing`。

## Task 23：实现平台VM、NUMA与SIMD并定义capability矩阵

**Files**

- Create：`heap/platform/{mod,linux,windows,macos,portable}.rs`
- Modify：heap/page/bitmap/worker affinity、benchmark capabilities/resource providers
- Create：`.github/workflows/zgc-capability-matrix.yml`

**Runner contract**

- Linux x86_64 AVX2；Linux x86_64 AVX-512；Linux AArch64 NEON；Windows x86_64；macOS arm64；Linux multi-NUMA。
- 每个runner报告RAM、MemAvailable、cgroup/Job limit与remaining、RLIMIT_AS、swap/PSI、ISA、NUMA、page/decommit和hard-isolation能力；skip只记录未覆盖，不能关闭gate。

**GREEN（本地）**

```bash
cargo nextest run -p wjsm-runtime -E 'test(heap_platform) | test(bitmap_simd)'
cargo run --release -p wjsm-gc-bench -- capabilities --output /tmp/gc-capabilities.json
```

- [ ] **Write test**：commit/decommit/recommit、capability JSON、NUMA local/fallback、scalar与当前ISA一致。
- [ ] **Verify RED**：platform backend/capability缺失。
- [ ] **Implement**：cfg VM backend、RAII ranges、node-local lists、一次ISA dispatch和CI workflow。
- [ ] **Verify GREEN**：本地只关闭portable/实际ISA；本地缺少的ISA/OS/NUMA能力必须保持`needs-capability-runner`，禁止auto-skip为通过，直到Task 25具名CI证据到达。
- [ ] **Commit**：`feat: add GC platform memory capabilities`。

---

# Phase F — 归一化对标、大堆与退役

## Task 24：运行JDK 25归一化矩阵并profile到硬门

**Files**：benchmark gate/report、profile证明的runtime/backend owners、evidence bundle。

**GREEN**

```bash
cargo build --release -p wjsm-gc-bench
target/release/wjsm-gc-bench preflight --heap 1g --profile pr --output /tmp/zgc-pr-resources.json
target/release/wjsm-gc-bench compare --jdk-home "$JDK25_HOME" --jdk-probe-home "$JDK25_PROBE_HOME" --heaps 32m,256m,1024m --live-sets 10,50,80 --scenarios churn,request,chain,cycle,wide,mutation,humongous,idle-uncommit --samples 30 --output /tmp/zgc-compare
target/release/wjsm-gc-bench gate --manifest /tmp/zgc-compare/manifest.json
```

- [ ] **Write failing gate**：preflight资源准入必须先通过；五项指标`WJSM<=JDK*1.10`，至少两项`<=JDK*0.85`，p99.9不高于JDK，max<1ms；缺resource snapshot/counter/patch hash/physical distribution即非零。
- [ ] **Verify RED**：首次完整矩阵记录每个失败，不改阈值、denominator或scenario。
- [ ] **Implement**：逐失败项perf/JFR/WAT/反汇编归因，只修改owner级热点；同时报告per-object/per-edge防止per-byte失真。每轮必须产生99% CI排除零的改善，或用新证据定位到不同结构瓶颈；连续两轮既无显著改善也无新瓶颈时状态转为`needs-architecture-revision`并回到父规格对应owner，不能放宽阈值或无限重复同一局部调整。
- [ ] **Verify GREEN**：全部硬门通过，保存raw JSON、JFR、perf摘要、patch hash与99% CI。
- [ ] **Commit**：按热点拆提交，最终`perf: meet JDK 25 ZGC normalized gates`。

## Task 25：关闭4/16 GiB、跨平台ISA与NUMA nightly矩阵

**Files**：`.github/workflows/zgc-nightly.yml`、benchmark manifests、resource-isolation supervisor与evidence。

**Runner requirements**

- 4 GiB：physical/cgroup total≥32 GiB且effective available满足公式；16 GiB：total≥64 GiB且effective available满足公式；WJSM/JDK严格顺序执行。
- runner必须提供delegated cgroup v2（`memory.max`、`memory.swap.max=0`、`memory.events`）或Windows Job Object；无硬隔离能力即`needs-resource-runner`。
- build/JDK-probe由独立job完成并上传artifact；large runner不运行rustc/javac，持有全机独占锁。
- AVX-512、AArch64、Windows、macOS、多NUMA使用Task 23具名capability runners。

**GREEN（每个heap在对应runner独立执行）**

```bash
target/release/wjsm-gc-bench preflight --heap "$HEAP_CAP" --profile nightly --output /tmp/zgc-nightly-resources.json
target/release/wjsm-gc-bench compare --jdk-home "$JDK25_HOME" --jdk-probe-home "$JDK25_PROBE_HOME" --heap "$HEAP_CAP" --live-sets 10,50,80 --duration 3600s --scenarios saturation,request,humongous,idle-uncommit --output /tmp/zgc-nightly
target/release/wjsm-gc-bench gate --manifest /tmp/zgc-nightly/manifest.json --profile nightly
```

- [ ] **Write gate**：资源准入、独占锁、顺序执行、child hard ceiling=`2*heap+2 GiB`、90% supervisor终止、swap/PSI/oom_events零增量，以及1小时fragmentation/stall/RSS/NUMA/uncommit/ISA gates。
- [ ] **Verify RED**：fake 4 GiB available与无hard-isolation runner均在spawn前exit 78；首次具名runner性能失败保持open，本机不足不得缩小矩阵。
- [ ] **Implement**：workflow分离build/run；配置cgroup/Job硬限制、RSS watchdog和样本熔断；只修复大堆/平台owner问题，不加scenario特判。
- [ ] **Verify GREEN**：4/16 GiB具名runner通过；无host OOM、swap、PSI full、oom_event、持续RSS/handle/page泄漏。
- [ ] **Commit**：`perf: validate isolated large-heap and platform ZGC`。

## Task 26：删除旧benchmark、scheduler与残余owner并执行负检查

**Files**：删除`gc_stress.rs`、`zgc_autoresearch.rs`、`zgc_barrier_pressure.rs`旧入口及迁移后无owner代码；更新tests。

**GREEN**

```bash
cargo nextest run --workspace
```

- [ ] **Write negative audit**：逐manifest与源码路径禁止旧GcContext、collector全局mutex、4-byte entry、`alloc_from_bump`、dynamic heap `env.memory.data_mut`、旧benchmark入口、Cargo manifests中的`managed-heap-v2`定义、所有`cfg(feature = "managed-heap-v2")`和文档中的残留feature说明。
- [ ] **Verify RED**：删除前audit报告残余。
- [ ] **Implement**：迁移有效workload到bench crate并删除全部残余，不留alias/shim/re-export。
- [ ] **Verify GREEN**：workspace与audit通过。
- [ ] **Commit**：`refactor: retire legacy GC paths and benchmarks`。

## Task 27：最终全量验证与ADR/AGENTS闭环

**Files**：新增superseding ADR（统一ManagedHeap/shared memory64/Generational ZGC/旧ADR 0005退役）；修改ADR 0003的startup snapshot格式与restore边界、ADR 0004的support cwasm/engine fingerprint边界、AGENTS.md、Aegis evidence/checkpoint/reflection/index。

**GREEN**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo nextest run --workspace
WJSM_TEST_GC=mark-sweep cargo nextest run -E 'test(happy__)'
WJSM_TEST_GC=g1 cargo nextest run -E 'test(happy__)'
WJSM_TEST_GC=zgc cargo nextest run -E 'test(happy__)'
cargo +nightly miri test -p wjsm-runtime --test gc_protocol_miri
RUSTFLAGS="-Zsanitizer=thread" cargo +nightly test -p wjsm-runtime --test gc_concurrency_model
```

Smoke：

```bash
cargo run -- run --gc zgc -e 'let roots=[]; for(let i=0;i<1e6;i++){let o={i,next:roots[i&1023]}; if((i&255)===0)roots[i&1023]=o;} gc(); console.log(roots.length)'
```

- [ ] **Write closure checklist**：逐项映射父规格硬门、platform gates、retirement；缺证据只能`needs-verification`。
- [ ] **Verify RED**：文档更新前运行全命令；warning/race/fixture/perf/capability任一失败阻断完成。
- [ ] **Implement**：修复阻断；写superseding ADR并逐条标明ADR 0005被取代的所有权、并发、分代与entry决策；同步ADR 0003/0004的snapshot/support ABI与engine fingerprint；更新AGENTS的WASM contract、GC、perf/debug流程和evidence，只记录已验证事实。
- [ ] **Verify GREEN**：全命令、Task 24/25 gates与lingering audit通过。
- [ ] **Commit**：`docs: record generational ZGC architecture`。

---

## Plan Self-Review

- Task 0先证伪Wasmtime/engine parity；失败即停止。
- Task 2不改active entry size；8-byte/new memory只在私有门中准备。
- Tasks 8–14迁移全部caller；Task 15单点切换并删除私有门/旧dynamic heap，避免不可编译中间状态与长期双轨。
- Wasm共享heap原子语义明确为SeqCst；Rust私有metadata才允许弱序。
- reference-only color、runtime string、按heap type分类的全部mutable-in-place header与relocation写竞争均有测试。
- loom/Miri/TSan/SharedMemory测试后端分离。
- benchmark CLI、JDK probe、numerator/denominator、runner capability与缺证据语义已固定。
- snapshot/support和realm/side table拆成独立准备任务。
- 最终计划结束不保留fallback、private feature、旧benchmark或重复owner。

## Execution Handoff

计划只能从Task 0开始。推荐subagent-driven执行，每个任务后独立代码审查；主代理在Task 15协调切换、Task 20并发relocation、Task 24性能gate与Task 27完成候选处执行架构复审。