# EvidenceBundleDraft

## 当前实现证据

- `ZgcCollector` 的 mark/relocate 由 `safepoint_step` 在 mutator 线程推进。
- `alloc_from_bump`/`sync_alloc_window` 证明 ZGC fast path仍是连续 bump。
- zPage记录 live/fragmentation/relocation 状态，但不承担可复用 page allocation。
- `object_walker` 按 handle table 枚举并构造任务/引用集合；host 写路径存在 `handle_for_ptr` 反查。
- `WasmEnv::from_caller` 在 host imports 反复解析 exports。
- NaN-box layout确认 tag为bit 32–36、runtime-string flag为bit 37、bit 38–50可承载GC color。

## 当前 benchmark 证据

### Criterion

命令：

```bash
cargo bench -p wjsm-runtime --bench gc_stress -- --noplot
```

观测：mark-sweep约4.20 ms，G1约10.78 ms，ZGC约4.13 ms；benchmark函数每个样本仍包含WAT解析/运行时执行边界，不能归因GC。

### `zgc_autoresearch`

命令：

```bash
cargo run --release -p wjsm-runtime --example zgc_autoresearch
```

观测：5个样本、每样本仅1个显式pause；ZGC约26–28k iter/s、max pause约0.16–0.19 ms；`barrier_events=0`、`load_barrier_hits=0`，不能覆盖活动并发期barrier成本。

### `zgc_barrier_pressure`

命令：

```bash
cargo run --release -p wjsm-runtime --example zgc_barrier_pressure
```

观测：ZGC wall约5.13 ms/cycle，mark-sweep约12.17 ms，G1约13.40 ms；ZGC barrier events仅3、load barrier hits仅6，且程序把整轮wall除以cycle标为barrier overhead，指标定义无效。

### profiler

命令：

```bash
perf stat -e cycles,instructions,branches,branch-misses,cache-references,cache-misses,page-faults target/release/deps/gc_stress-c917522b690a98c0 --bench gc/mixed_churn/zgc --noplot
perf record -g --call-graph dwarf -o /tmp/wjsm-zgc.data target/release/examples/zgc_autoresearch
perf report --stdio --no-children --percent-limit 0.5 -i /tmp/wjsm-zgc.data
```

观测：`WasmEnv::from_caller`、`object_walker`、allocator/`Vec`扩容出现在热点中；说明host export解析、扫描结构和临时分配必须进入计划。

## 外部证据

- 本机JDK：OpenJDK 25.0.3 Zulu；`java -XX:+UseZGC -Xlog:gc=info -version`确认Generational ZGC可用。
- JEP 439描述young/old双代、colored pointers、load/store barriers和多代并发周期。
- JEP 474使Generational ZGC成为默认模式。
- JEP 490在JDK 24移除非分代模式，因此JDK 25对标必须是Generational ZGC。
- JDK 25 GA源码确认pause mark-start/end、concurrent mark、relocation-set selection、relocate-start与concurrent relocate阶段，以及per-CPU small/medium page allocation。
- Wasmtime 43.0.2源码确认SharedMemory可clone并跨线程访问，其data由`UnsafeCell<u8>`表达；shared memory必须import，memory64由MemoryType支持。

## Design decision evidence

用户已确认：

- 算法/数据结构/设计不差于JVM，端到端wall不作为绝对硬门。
- 真并发shared memory重构。
- 全collector统一ManagedHeap。
- shared memory64真实大堆。
- portable scalar + SIMD平台特化。
- 32/256/1024 MiB PR矩阵和4/16 GiB nightly。
- 全面不劣10%且至少两项领先15%。
- 四节完整设计均批准。

## Planning evidence

- 批准规格：`docs/aegis/specs/2026-07-16-zgc-performance-design.md`。
- 可执行计划：`docs/aegis/plans/2026-07-16-zgc-performance.md`。
- 计划包含Task 0–27；每项具备files、why/boundary、RED、implementation、GREEN与commit边界。
- 覆盖telemetry/benchmark、value ABI、control plane、shared memory64、handle/page/worker、三collector迁移、barrier、young/old/relocation、weak/side tables、pacing、VM/NUMA/SIMD、JDK归一化对标、大堆nightly、旧path退役和最终全量验证。

## Plan consensus review evidence

- Reviewer：`plan-consensus-reviewer`，共3轮。
- Round 1：发现4 Blocking、8 Important、3 Minor；关键项为active entry切换过早、SharedMemory可行性门过晚、迁移中间态不可编译、JDK归一化不可采集。
- Round 2：确认首轮15项全部closed；新增审查聚焦active切换风险、32 GiB handle区、remset loom、profile收敛、mutable header和ADR/feature审计。
- 主代理裁决：Task 15保持全collector单点active ABI切换，避免公开runtime双heap；不设任意profile轮数上限，采用显著改善/结构瓶颈证据收敛；HeaderLayout覆盖prototype、length、property count、flags、backing reference等全部mutable-in-place字段。
- Round 3：reviewer验证上述裁决成立，`Consensus status: agree`，`open_issues: []`。
- 最终计划状态：`主代理与 plan-consensus-reviewer 已达成共识（Round 3）`。

## Host memory safety amendment

- 当前主机：`MemTotal=16,376,952 KiB`（约15.6 GiB），`MemAvailable=11,224,916 KiB`（约10.7 GiB）。
- 当前cgroup：`/init.scope`，`memory.max=max`，`memory.current=4,190,076,928` bytes（约3.9 GiB），`memory.peak=14,314,049,536` bytes（约13.3 GiB），历史`oom/oom_kill=0`。
- Swap为4 GiB且几乎未使用，但性能准入明确不把swap计入available；内核`vm.overcommit_memory=1`只允许虚拟地址保留，不代表可安全提交对象堆。
- 原计划的32/64 GiB runner标签能排除16 GiB本机运行，但缺少程序级preflight；用户指出在仅4 GiB available主机存在宿主OOM风险，此判断成立。
- 规格§18.5与计划Task 1/23/24/25已增加fail-closed资源准入：`required_total=4*heap`、`required_available=3*heap+headroom`、RLIMIT/Job虚拟地址检查、exit 78、无自动缩heap、顺序/独占执行、large profile硬cgroup/Job隔离、90% watchdog、swap/PSI/OOM事件熔断。
- fake `HostResourceProvider`合同固定：4 GiB available时1/4/16 GiB均在spawn前拒绝且child计数为零；256 MiB可运行。
- 按新公式，当前主机允许1 GiB PR上限，但4 GiB与16 GiB大堆均会在spawn前拒绝。

## Task 0 implementation evidence

- 新增 `crates/wjsm-engine-config`：`EngineConfig::artifact()` / `EngineConfig::runtime(RuntimeEngineOptions)` 为唯一 `Config` 构造 owner；固定 threads、shared_memory、memory64、multi-memory、bulk-memory、backtrace(50)、address-map。
- Canonical artifact 固定 Cranelift + epoch interruption；runtime 保留 compiler / opt / epoch / memory reservation / guest-debug 语义（guest_debug 强制 Cranelift）。
- `compatibility_fingerprint` 基于 `Engine::precompile_compatibility_hash()` + 固定种子 FNV-1a；snapshot-format 以纯函数显式接收每个 engine 的 external input，不使用 first-writer-wins 全局状态；默认路径复用 active Engine，显式关闭 snapshot 时不计算 fingerprint。
- AArch64 明确拒绝无法提供 threads capability 的 Winch profile，不静默关闭 threads 或切换 compiler；具名 AArch64 runner 由 Task 23 关闭条件测试证据。
- 迁移：`runtime_engine_pool`、`runtime_startup`、snapshot/realm/bench 冷路径、`wjsm-runtime-support/build.rs` 与 support deserialize 测试全部复用 owner；除 engine-config 外无 `wasmtime::Config::new()`。
- workspace `wasmtime = "=43.0.2"`，`Cargo.lock` 记录 `wasmtime 43.0.2`。
- feasibility：user main memory32 + imported shared memory64；user/support 双 module 共享；object address `> 32 GiB`；Wasm SeqCst atomic 与 host `AtomicU64` 各 +10000，最终值 20000；support cwasm 由 canonical build engine precompile、由相同 fingerprint 的默认 runtime engine deserialize/instantiate。
- feasibility 的两个 unsafe 边界分别记录可信 cwasm 来源/兼容 fingerprint，以及 shared mapping 的范围、对齐、生命周期、UnsafeCell 和全原子访问不变量。
- 动态 runtime profile 的既有 support cwasm 编译 fallback 本任务不扩张也不提前删除；最终退役门由计划 Task 15/26 承担。

### GREEN commands

```text
cargo nextest run -p wjsm-engine-config
# Summary: 2 tests run: 2 passed, 0 skipped

cargo nextest run -p wjsm-runtime --test shared_memory64_feasibility
# Summary: 1 test run: 1 passed, 0 skipped

cargo nextest run -p wjsm-runtime -E 'test(startup_snapshot)'
# Summary: 9 tests run: 9 passed, 296 skipped

cargo nextest run -p wjsm-runtime-support
# Summary: 9 tests run: 9 passed, 0 skipped

cargo nextest run -p wjsm-snapshot-format
# Summary: 6 tests run: 6 passed, 0 skipped

cargo nextest run -p wjsm-runtime -E 'test(engine_pool)'
# Summary: 6 tests run: 6 passed, 299 skipped

cargo check -p wjsm-runtime-snapshot
# Finished dev profile; 0 warnings
```

状态：Task 0 GREEN 完成；规格 review 已通过，质量 findings 已修复并由新增命令验证；按用户要求在 Phase A 结束统一 review。

## Task 1 implementation evidence

### RED

```text
cargo nextest run -p wjsm-gc-bench
# 缺少 crate / schema / resource owner，失败。

cargo nextest run -p wjsm-runtime -E 'test(gc_telemetry)'
# 缺少 GcTelemetry API，失败。
```

### 实现事实

- 新增 `wjsm-gc-bench` 专用 CLI：`capabilities`、`preflight`、`prepare-jdk`、`baseline`、`run`、`micro`、`compare`、`replay`、`gate`。
- `HostResourceProvider` 以 `min(physical, finite cgroup/job limit)` 和 `min(MemAvailable, finite remaining)` 计算有效资源；准备/compare 在 `javac`/`java` spawn 前 fail-closed，资源不足返回 exit 78。
- report 包含版本、硬件、counter source、resource snapshot、全部预算公式、admission 与 99% deterministic bootstrap CI；compile/Wasmtime compile/instantiate/startup 不计入 `steady_state_ns`。
- `GcTelemetry` 使用 HDR histogram 保存 pause 分位数和精确 min/max；当前无法从 memory32 collector 获得的 physical allocation、GC CPU、barrier split、JDK numerator 明确编码为 null。
- JDK 25 probe/patch 与 Java driver 已加入；错误/非 JDK 25 环境输出 `needs-verification` JSON，不作为普通测试失败。
- `wjsm-gc-bench` 的 11 个 Rust tests 均标记 ignore，避免 workspace 全量 nextest 运行 benchmark contract；实际 benchmark 只经专用 CLI 入口执行。

### GREEN

```text
cargo nextest run -p wjsm-runtime -E 'test(gc_telemetry)'
# Summary: 1 test run: 1 passed, 303 skipped

cargo check -p wjsm-gc-bench
# Finished dev profile; 0 warnings

cargo fmt --all -- --check
# 通过

cargo run --release -p wjsm-gc-bench -- preflight --heap 1g --profile pr --output /tmp/wjsm-gc-preflight.json
# exit 0；JSON admission=admitted，含 required_total/available/virtual 公式。

cargo run --release -p wjsm-gc-bench -- baseline --engine wjsm --gc zgc --heap 32m --scenario churn --samples 30 --output /tmp/wjsm-zgc-baseline.json
# exit 0；30 个样本，steady-state mean=3_557_174_843 ns，99% CI=[3_477_254_643.7333336, 3_649_131_578.5333333]。

cargo run --release -p wjsm-gc-bench -- prepare-jdk --jdk-home /definitely-not-jdk25 --jdk-probe-home /tmp/wjsm-jdk-probe --output /tmp/wjsm-jdk-probe-wrong.json
# exit 0；JSON status=needs-verification。

cargo nextest run -p wjsm-gc-bench
# 0 run / 11 skipped；nextest 对无可运行选择返回 code 4，符合 benchmark tests 全部 ignore 的隔离设计。

git apply --check --ignore-space-change crates/wjsm-gc-bench/jdk-probe/0001-zgc-benchmark-counters.patch
# 以 OpenJDK master 相邻 ZGC source 验证补丁上下文；[INFERENCE] 仍需具名 JDK 25 GA runner 在 Task 24/25 提供最终 apply/build 证据。
```

状态：Task 1 GREEN 完成；JDK 25 instrumentation 与 physical/cpu/barrier raw numerator 的当前状态为 `needs-verification`，这是 gate 合同而非通过声明。

## Task 2 implementation evidence

### RED

```text
cargo nextest run -p wjsm-ir
# `GC_COLOR_MASK`、`strip_gc_color`、`is_handle_backed_reference` unresolved imports，失败。
```

### 实现事实

- `GC_COLOR_SHIFT=38`、`GC_COLOR_BITS=6`、`GC_COLOR_MASK=0x0000_0FC0_0000_0000` 只定义在 `wjsm-ir::value`。
- `is_handle_backed_reference` 复用既有 `tag_needs_root` 语义，因此 object/array/function/closure/bound/native/bigint/symbol/regexp/proxy/scope-record/exception/iterator/enumerator/runtime string 可着色；number/static string/bool/null/undefined/array hole 不可着色。
- `strip_gc_color` 只清除 bit 38–43，不改变 tag 或低 32 位 handle identity；没有 backend store、entry size、snapshot format 或 runtime heap 调用方改动。

### GREEN

```text
cargo nextest run -p wjsm-ir
# Summary: 22 tests run: 22 passed, 0 skipped

cargo nextest run -p wjsm-snapshot-format
# Summary: 6 tests run: 6 passed, 0 skipped

cargo run -- run -e 'const x={}; const y=x; console.log(x===y)'
# stdout: true（176.46 秒；后续 runtime 命令遵循 180 秒绝对上限）

cargo fmt --all -- --check
# 通过
```

状态：Task 2 GREEN 完成；snapshot-format 的静态 ABI hash test 通过且该 crate 不依赖 `wjsm-ir`，因此 inactive IR-only color constants 不进入 active snapshot ABI。

## Phase A review repair evidence

- 第一轮规格审查发现 11 项 Important：cgroup path/delegation、host probe 错误语义、profile replay、stock JDK/JDK collector、跨 engine scenario、未接入 controls、telemetry 只读 last、共同 denominator、缺失样本/pause hard gate、raw f64 color stripping。
- `resource_platform.rs` 现在从 `/proc/self/cgroup` 与 `/proc/self/mountinfo` 推导实际 cgroup v2 mount；只有 finite `memory.max`、`memory.swap.max=0`、parent memory subtree delegation、可写 limit 和 `memory.events` 同时存在时声明 hard isolation。探测错误写入 `probe_errors`，admission 转为 `needs-resource-runner`。
- `RunConfiguration` 保存 profile；replay 使用该 profile。非 JDK 25/JDK probe/classes 缺失、stock JDK、没有 JDK mark-sweep counterpart 均输出 `needs-verification`；不再把请求的 JDK collector 静默替换为 ZGC。
- WJSM 与 Java driver 使用同一 v1 workload contract/hash；JDK 输出必须回传该 hash 才接受样本。非默认的 workers/relocation/barrier/safepoint controls 在其 owner 到位前明确返回 `needs-verification`，不假装生效。
- `GcExecutionStats.cumulative` 在 `RuntimeState::store_last_gc_stats` 汇总所有 cycle；telemetry 读取 cumulative freed/relocated bytes。gate 以各 engine 独立 numerator/denominator 求值，任何 sample/counter/pause distribution 缺失均为 `needs-verification`，同时检查 p99.9 与 WJSM max <1ms。
- `strip_gc_color` 只作用于 `tag_needs_root` heap-backed reference；raw f64 payload 即使碰巧占用 bit 38–43 也原样保留。

```text
cargo check -p wjsm-gc-bench
cargo check -p wjsm-runtime
cargo check -p wjsm-ir
cargo fmt --all -- --check
# 均通过，零 warning。

cargo run --release -p wjsm-gc-bench -- preflight --heap 1g --profile pr --output /tmp/wjsm-gc-preflight-review.json
# exit 0；实际 cgroup_path=/sys/fs/cgroup/init.scope，memory_controller=true，delegated=false，probe_errors=[]。

cargo run --release -p wjsm-gc-bench -- run --engine jdk --gc zgc --heap 32m --scenario churn --samples 1 --jdk-home /definitely-not-jdk25 --jdk-probe-home /tmp/wjsm-jdk-probe --output /tmp/wjsm-jdk-run-review.json
# exit 0；JSON status=needs-verification，未 spawn benchmark child。

cargo run --release -p wjsm-gc-bench -- baseline --engine wjsm --gc zgc --heap 32m --scenario churn --samples 1 --output /tmp/wjsm-zgc-review-smoke.json
# exit 0；canonical workload v1/hash 生效，steady_state_ns=4_045_969_265，cumulative reclaimed_bytes=18_288。

cargo nextest run -p wjsm-runtime -E 'test(gc_telemetry) | test(gc_execution_stats_accumulate_all_cycles)'
# Summary: 2 tests run: 2 passed, 306 skipped

cargo nextest run -p wjsm-ir
# Summary: 22 tests run: 22 passed, 0 skipped
```

状态：Phase A review fixes 已实现；用户已取消 Phase A 复审并要求整个 28 项计划完成后才统一 reviewer。最终 canonical workload 30-sample distribution 因用户禁止耗时运行仍为 `needs-verification`。

## Task 3 implementation evidence

### RED

```text
cargo nextest run -p wjsm-runtime --features managed-heap-v2 --test gc_control_plane
# package 不包含 managed-heap-v2 feature，失败。
```

### 实现事实

- runtime/backend/support 都声明默认关闭的私有 `managed-heap-v2` feature；runtime feature 只传播给 backend/support，未改变默认 ABI。
- `GcRuntimeV2` 只持有 epoch、participant id 与 active count atomics；`MutatorContext` 仅发布 `Arc<[u32]>` handle roots；`CollectorContext` 只观察不可变 `RootSnapshot`。
- 新 control-plane 文件不导入 Store/Caller/WasmEnv，也没有包围 collector algorithm 的 mutex。
- `gc_control_plane.rs` 自身 cfg-gate，避免 `--no-default-features` 仍编译 feature-only imports。

### GREEN

```text
cargo nextest run -p wjsm-runtime --features managed-heap-v2 --test gc_control_plane
# Summary: 3 tests run: 3 passed, 0 skipped

cargo nextest run -p wjsm-runtime --no-default-features -E 'test(runtime_options_default)'
# Summary: 1 test run: 1 passed, 308 skipped

cargo fmt --all -- --check
# 通过
```

状态：Task 3 GREEN 完成；feature 默认关闭，不切换 active runtime。

## Task 4 implementation evidence

### RED

```text
cargo nextest run -p wjsm-runtime --features managed-heap-v2 --test managed_heap_memory
# HeapAddress / HeapMemoryError / ManagedHeap / NativeHeapMemory / SharedHeapMemory unresolved imports，失败。
```

### 实现事实

- `HeapMemory` 为 sealed crate-private trait，生产 owner `ManagedHeap<M>` 静态单态化，不使用 `dyn HeapMemory`。
- `SharedHeapMemory` 通过 Wasmtime `SharedMemory::data()` 的稳定 base pointer 执行 checked `AtomicU64` SeqCst word load/store；raw byte copy 使用 AtomicU8，文档限制为未发布对象区。
- `NativeHeapMemory` 使用 `Arc<[AtomicU64]>` 和 CAS byte update，可模拟高于 u32 的 base address，覆盖 alignment/bounds/SeqCst/copy 语义。
- runtime feature 不再把 Task 4 memory-only test 无谓传播到 backend/support；backend/support 保留各自默认关闭的 private feature。

### GREEN / needs-verification

```text
cargo nextest run -p wjsm-runtime --features managed-heap-v2 --test managed_heap_memory
# Summary: 4 tests run: 4 passed, 0 skipped

cargo nextest run -p wjsm-runtime --no-default-features -E 'test(runtime_options_default)'
# Summary: 1 test run: 1 passed, 308 skipped

cargo check -p wjsm-runtime --features managed-heap-v2
# Finished dev profile; 0 warnings

cargo +nightly miri test -p wjsm-runtime --features managed-heap-v2 --test gc_protocol_miri
# 180 秒硬超时；Miri 仍在编译 Wasmtime/SWC dependency graph，测试体未执行。
```

状态：Task 4 implementation 完成；Miri protocol evidence 为 `needs-verification`，遵循用户 180 秒运行上限，未重试长跑。

## Task 5 implementation evidence

### RED

```text
cargo nextest run -p wjsm-runtime --features managed-heap-v2 --test handle_table
# unresolved imports: HandleGeneration / HandleId / HandleState / HandleTableV2 / ManagedHeapLayout；失败。
```

### 实现事实

- `HandleTableV2` 以 Wasmtime shared memory64 min=max 32 GiB 取得完整连续虚拟 mapping；engine 通过唯一 `wjsm-engine-config` owner 配置，reserve 失败明确返回 `HandleTableError::VirtualReservation`，没有 native 或 map fallback。
- entry 是 `AtomicU64`：high 48 bit byte address、low 16 bit state（Free、StableYoung、StableOld、RelocatingYoung、RelocatingOld、PinnedOld、Retired）。active 4-byte `obj_table` 未被读取或修改。
- 64 KiB commit bitset 仅记录首次发布 block；内存由 OS demand paging 支持，`resolve` 不查询 block 或锁，直接计算 `region_base + handle * 8` 并执行一个 SeqCst entry load。
- epoch participant 在读地址前进入、reclaim 只在全部旧 epoch participant 退出后将 Retired slot 置 Free 并放入 reusable stack；旧 sparse `BTreeMap` staging owner 已删除。

### GREEN

```text
cargo nextest run -p wjsm-runtime --features managed-heap-v2 --test handle_table
# Summary: 3 tests run: 3 passed, 0 skipped

cargo nextest run -p wjsm-runtime --features managed-heap-v2 --test gc_concurrency_model
# Summary: 1 test run: 1 passed, 0 skipped

cargo nextest run -p wjsm-runtime --features managed-heap-v2 --test gc_loom_model
# Summary: 1 test run: 1 passed, 0 skipped

cargo check -p wjsm-runtime --features managed-heap-v2
# Finished dev profile; 0 warnings

cargo nextest run -p wjsm-runtime --no-default-features -E 'test(runtime_options_default)'
# Summary: 1 test run: 1 passed, 308 skipped

cargo rustc -p wjsm-runtime --test handle_table --release --features managed-heap-v2 -- --emit=asm
# 通过；high-u32 test assembly 中 rcx = 34359738360 (`u32::MAX * 8`)，随后 `movq (%rax,%rcx), %r14`。
# 该指令是 base + handle*8 的单个 8-byte direct load；没有 map lookup、lock 或额外 entry load。
```

状态：Task 5 GREEN 完成；Task 4 Miri protocol 仍为 `needs-verification`，因为此前 180 秒内只完成 Wasmtime/SWC 依赖编译，未执行测试体。

## Task 6 implementation evidence

### RED

```text
cargo nextest run -p wjsm-runtime --features managed-heap-v2 --test page_allocator
# unresolved imports: AllocationClass / ManagedAllocator / Nlab / ObjectRef / PageId；失败。
```

### 实现事实

- `ManagedHeap::allocate` 委派给 `ManagedAllocator`；`ObjectRef` 仅保存 heap-relative offset，分配路径不产生 host pointer。
- `PageConfig` 在 heap 创建时选择 64 KiB–2 MiB page；NLAB 命中仅更新 mutator-local top、预分配 object map/bitmap 原子 metadata 和 byte counter，不获取 allocator mutex 或进行 heap allocation。
- medium、large、humongous 从 coalescing free range 获取连续 page；relocation reserve 从同一 free range 隔离出去，不能被 mutator 分配。
- page metadata 使用 object-start bitmap + `AtomicU64` size table 和 current/previous 双 mark bitmap；`PageObjectIter` 按 bitmap streaming，不构造对象列表。
- `wjsm-gc-bench micro --component allocator` 已移除“counters unavailable”占位报告，受既有 admission gate 后执行真实 allocator loop 并输出每样本 elapsed/object/allocated/committed counters。

### GREEN

```text
cargo nextest run -p wjsm-runtime --features managed-heap-v2 --test page_allocator
# Summary: 4 tests run: 4 passed, 0 skipped

cargo nextest run -p wjsm-runtime --features managed-heap-v2 --lib -E 'test(managed_heap_delegates_nlab_allocation)'
# Summary: 1 test run: 1 passed, 209 skipped

cargo check -p wjsm-runtime --features managed-heap-v2
# Finished dev profile; 0 warnings

cargo check -p wjsm-gc-bench
# Finished dev profile; 0 warnings

cargo nextest run -p wjsm-runtime --features managed-heap-v2 --test managed_heap_memory --test handle_table --test gc_concurrency_model --test gc_loom_model --test page_allocator
# Summary: 13 tests run: 13 passed, 0 skipped

cargo nextest run -p wjsm-runtime --no-default-features -E 'test(runtime_options_default)'
# Summary: 1 test run: 1 passed, 308 skipped

cargo run --release -p wjsm-gc-bench -- micro --component allocator --heap 256m --samples 30 --output /tmp/wjsm-allocator-micro-final.json
# exit 0; report status=passed, admission=admitted, 30 samples。
# 每样本真实测量：4096 allocated objects；首样本 allocated_bytes=523856、committed_bytes=524288、elapsed_ns=388045。
```

状态：Task 6 GREEN 完成；无 active allocator、collector 或 Linker ABI 切换。

## Task 7 implementation evidence

### RED

```text
cargo nextest run -p wjsm-runtime --features managed-heap-v2 --test gc_worker
# unresolved imports: GcPacketKind / GcWorkPacket / GcWorkerPool / WorkerPoolError；失败。
```

### 实现事实

- `runtime_gc/worker` 是 feature-private 的唯一 V2 worker owner；`GcWorkPacket` 为 Copy 的 page/bitmap/root/relocation range value，不含 Store、Caller、WasmEnv 或用户 callback context。
- `PacketSlab` 在创建时预分配固定 slots；submit 从 free list 取得 `PacketId`，worker 完成后归还，容量耗尽显式报错而不增长或分配 fallback。
- 每 worker 持有 crossbeam FIFO deque，提交进 injector，消费顺序为 local → injector batch → peer steal；deterministic unit test 直接将 packet 放入 owner local deque 并由 peer 成功 steal。
- `inflight` 是 drain/termination 真相：shutdown 先停止 admission、worker 在 `inflight == 0` 前继续消费，最后 join。Condvar 仅协调 park/wake/idle，不承担工作正确性。
- 新 V2 mutex 全部使用 `parking_lot`；active scheduler 未更改。

### GREEN

```text
cargo nextest run -p wjsm-runtime --features managed-heap-v2 --lib -E 'test(worker_steals_packet_from_peer_local_deque)|test(managed_heap_delegates_nlab_allocation)'
# Summary: 2 tests run: 2 passed, 209 skipped

cargo nextest run -p wjsm-runtime --features managed-heap-v2 --test page_allocator --test gc_worker --test gc_concurrency_model --test gc_loom_model --test gc_protocol_miri
# Summary: 12 tests run: 12 passed, 0 skipped

cargo check -p wjsm-runtime --features managed-heap-v2
# Finished dev profile; 0 warnings

cargo nextest run -p wjsm-runtime --features managed-heap-v2 --lib --test managed_heap_memory --test handle_table --test page_allocator --test gc_worker --test gc_concurrency_model --test gc_loom_model --test gc_protocol_miri
# Summary: 228 tests run: 228 passed, 2 skipped

cargo nextest run -p wjsm-runtime --no-default-features -E 'test(runtime_options_default)'
# Summary: 1 test run: 1 passed, 308 skipped

cargo run --release -p wjsm-gc-bench -- micro --component allocator --heap 256m --samples 30 --output /tmp/wjsm-allocator-micro-task7.json
# exit 0; report status=passed, admission=admitted, 30 samples。
```

### needs-verification：TSan / Miri

```text
# 初始全局 RUSTFLAGS TSan：rustc 拒绝未插桩 std 的 sanitizer ABI。
# host/target 隔离配置：
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/wjsm-tsan-host-isolated \
CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS='-Z sanitizer=thread -C unsafe-allow-abi-mismatch=sanitizer' \
cargo +nightly -Z target-applies-to-host -Z host-config \
  --config 'target-applies-to-host = false' test --target x86_64-unknown-linux-gnu \
  -p wjsm-runtime --features managed-heap-v2 --test gc_concurrency_model
# host/proc-macro ABI 错误已消失；两次均在 180 秒内编译 Wasmtime/SWC/runtime graph，未到测试体。

cargo +nightly miri test -p wjsm-runtime --features managed-heap-v2 --test gc_protocol_miri
# 未重跑：Task 4 已在同一 180 秒上限内仅完成依赖编译、未执行测试体。新增 test 只构造 Copy packet，绝不执行 worker deque。
```

状态：Task 7 core GREEN；TSan 与 Miri 不得标记为通过，保留 `needs-verification`，等待允许超过 180 秒或预热的专用 sanitizer/Miri runner。

## Task 8 implementation evidence

### RED

```text
cargo nextest run -p wjsm-backend-wasm --features managed-heap-v2 -E 'test(heap_memory64)'
# user module missing env.__heap_memory import；失败。
```

### 实现事实

- `wjsm-ir` 是 V2 memory ABI 的唯一常量 owner：minimum 32 GiB / 524288 wasm pages 仅保留 handle table；maximum 为 high48 address ABI 的 256 TiB / 2^32 pages，确保 control/object heap 可位于 32 GiB 之后独立 grow。`__heap_memory` index 2，i64 cursor globals 为 `__heap_alloc_ptr`/`__heap_alloc_end`/`__heap_object_start`/`__heap_limit_v2`。
- user 与 support module 共同维护 legacy/V2 两套 ABI artifact：runtime-support build-time 始终生成两组 cwasm，V2 runtime 只选择 V2 artifact；runtime source/eval/agent compile 通过 V2 backend feature bridge 实际编译 memory64 user module。
- V2 `obj_new` 使用 i64 NLAB cursor，直接初始化 memory64 object header，并以 `handle * 8` 对 shared heap 写 `I64AtomicStore`（high48 address + low16 state）。V2 get/set/delete/array/element 先以 `I64AtomicLoad` resolve handle，再调用明确的 Task 9 dynamic host ABI。
- compiler 的 V2 support helper binding 已拆为 `helpers_object/{alloc,resolve,property,array}.rs`；Eval V2 和 Normal V2 均走同一 `wjsm_support` ABI，不会 inline static memory32 helper。
- default feature 保持静态 helper；legacy WAT tests 只约束 default ABI，V2 有 `heap_memory64` 独立 ABI test，未以 skip 掩盖 V2 合同。

### GREEN

```text
cargo nextest run -p wjsm-backend-wasm
# Summary: 64 tests run: 64 passed, 0 skipped

cargo nextest run -p wjsm-backend-wasm --features managed-heap-v2
# Summary: 59 tests run: 59 passed, 0 skipped

cargo check -p wjsm-backend-wasm
# Finished dev profile; 0 warnings

cargo check -p wjsm-backend-wasm --features managed-heap-v2
# Finished dev profile; 0 warnings

cargo check -p wjsm-runtime --features managed-heap-v2
# Finished dev profile; 0 warnings; V2 runtime-support artifact precompile chain passed.

cargo nextest run -p wjsm-runtime --no-default-features -E 'test(runtime_options_default)'
# Summary: 1 test run: 1 passed, 308 skipped
```

状态：Task 8 GREEN 完成；V2 dynamic host import 的 concrete owner 由下一任务 `HeapAccessV2` 实现。

## Task 9 implementation evidence

### RED

```text
cargo nextest run -p wjsm-runtime --features managed-heap-v2 --test heap_access_v2 \
  -E 'test(v2_runtime_executes_collection_values_without_memory32_reverse_lookup)'
# Runtime error: unresolved V2 handle 46。

cargo nextest run -p wjsm-runtime --features managed-heap-v2 --test heap_access_v2 \
  -E 'test(v2_runtime_executes_proxy_property_access_without_memory32_reverse_lookup)'
# Runtime error: unresolved V2 handle 1。
```

### 实现事实

- `runtime_gc/heap_access_v2.rs` 是 V2 host memory64 地址、8-byte atomic handle entry、object/array/property slot 和 prototype-chain lookup 的唯一 owner；legacy `heap_access.rs` 不再混入 V2 owner。
- V2 property key 将 static main-memory string、runtime string 和 symbol canonicalize 为统一 name ID；host data/accessor property、Map/Set side-table handle、Proxy handler/trap 与 iterator result 均使用该 ID，不反查 memory32 raw pointer。
- V2 Array.prototype 懒初始化为当前 module 的完整 table method set，加上 values/keys/entries 与 `Symbol.iterator`；动态 string、iterator 和 collection constructor 都保留 ECMAScript property semantics。
- backend 默认构建可显式 emit V2 support module；runtime-support build-time 验证并预编译 legacy/V2 cwasm。V2 feature bridge 绑定 runtime source、debug、eval 与 agent compiler 到实际启用 V2 的 backend crate。
- callback invocation 仍通过 `WasmEnv` 维护独立 shadow-stack frame；该 ABI 不读取或写入 V2 dynamic object heap。对象/数组/property 的 V2 owner 不再进行 raw pointer reverse lookup。

### GREEN

```text
cargo nextest run -p wjsm-runtime --features managed-heap-v2 --test heap_access_v2
# Summary: 20 tests run: 20 passed, 0 skipped

cargo check -p wjsm-runtime --no-default-features --features managed-heap-v2
# Finished dev profile; 0 warnings

cargo nextest run -p wjsm-runtime --no-default-features --features managed-heap-v2 --test heap_access_v2
# Summary: 20 tests run: 20 passed, 0 skipped

cargo check -p wjsm-runtime
# Finished dev profile; 0 warnings
```

状态：Task 9 GREEN；已独立提交；进入 Task 10。

## Task 10 implementation evidence

### RED

```text
cargo nextest run -p wjsm-runtime --features managed-heap-v2 --test mark_sweep_v2
# unresolved import `wjsm_runtime::MarkSweepV2`

cargo nextest run -p wjsm-runtime --features managed-heap-v2 --lib
# max_heap_size_* 与 process_env_proxy_reads_keys_and_rejects_writes 暴露
# V2 NLAB、hybrid primordial 与 host array/proxy descriptor owner 缺口。
```

### 实现事实

- `MarkSweepV2` 以 `ManagedHeap<SharedHeapMemory>` 分配 object pages、以 `HandleTableV2` publish/retire/reclaim handles，并只消费 immutable handle-only `RootSnapshot`。
- mark 周期先 clear current bitmap，再遍历 handle graph 写入 allocator object map；sweep 顺序固定为 retire handle → side-table cleanup → whole dedicated-page release → epoch advance/quarantine reclaim。
- `allocate_or_collect` 仅捕获 allocator `OutOfPages`，运行 full collection 后重试；其他 allocation/handle/memory 错误保持原始失败。
- allocator object address 统一为 `object_heap_base + page * page_bytes`，避免 control/handle region 与 object page offset 混淆。
- 扩展 V2 gate 还修复了统一 NLAB globals、dynamic Array growth、Array length/push/join、host result arrays、Proxy ownKeys/descriptor、`process.env` 与 legacy startup primordial hybrid owner；Task 15 前 owner 由 atomic handle publication 判定。

### GREEN

```text
cargo nextest run -p wjsm-runtime --features managed-heap-v2 --test mark_sweep_v2
# Summary: 2 tests run: 2 passed, 0 skipped

cargo nextest run -p wjsm-runtime --features managed-heap-v2 --test heap_access_v2
# Summary: 22 tests run: 22 passed, 0 skipped

cargo nextest run -p wjsm-runtime --features managed-heap-v2 --lib
# Summary: 209 tests run: 209 passed, 2 skipped

cargo nextest run -p wjsm-runtime --lib
# Summary: 207 tests run: 207 passed, 2 skipped

cargo check -p wjsm-runtime --features managed-heap-v2
cargo check -p wjsm-runtime
# Both finished with 0 warnings
```

状态：Task 10 GREEN；已独立提交；进入 Task 11。

## Task 11 implementation evidence

### RED

```text
cargo nextest run -p wjsm-runtime --features managed-heap-v2 --test g1_v2
# g1_v2_retains_old_to_survivor_edges_across_young_collections: FAILED
# assertion `left == right` failed: left: 1, right: 0
# g1_v2_redirties_promoted_objects_with_surviving_young_children: FAILED
# assertion failed: next.remembered_cards_scanned > 0

cargo nextest run -p wjsm-runtime --features managed-heap-v2 --test g1_v2 -E 'test(g1_v2_records_collection_telemetry)'
# error[E0599]: no method named `telemetry_snapshot` found for struct `G1V2`
```

两条 remembered-set 回归证明旧实现会在 young collection 后丢失仍指向 survivor 的 old 卡，且 promotion 后的新 old destination 未重新入卡；telemetry 合同在实现前也不存在。

### GREEN

```text
cargo fmt --all && cargo nextest run -p wjsm-runtime --features managed-heap-v2 --test g1_v2
# Summary: 7 tests run: 7 passed, 0 skipped

cargo nextest run -p wjsm-runtime --features managed-heap-v2 -E 'test(g1_v2)'
# Summary: 7 tests run: 7 passed, 357 skipped

cargo nextest run -p wjsm-runtime -E 'test(g1_)'
# Summary: 18 tests run: 18 passed, 291 skipped

cargo check -p wjsm-runtime --features managed-heap-v2
# Finished `dev` profile [unoptimized + debuginfo] target(s) in 6.87s
```

状态：Task 11 GREEN；`G1V2` 保持 feature-gated，默认 active G1 未切换；进入 Task 12。

## Task 12 implementation evidence

### RED

```text
cargo nextest run -p wjsm-runtime --features managed-heap-v2 --test zgc_v2
# error[E0432]: unresolved imports `wjsm_runtime::ZgcV2`,
# `wjsm_runtime::ZgcV2Phase`, `wjsm_runtime::ZgcV2StepOutcome`
```

V2 contract 在实现前不存在；legacy ZGC 的 bump/page-entry 路径没有可与 `ManagedHeap`、8-byte `HandleTableV2` 和 immutable `RootSnapshot` 组合的 policy owner。

### GREEN

```text
cargo fmt --all && cargo nextest run -p wjsm-runtime --features managed-heap-v2 --test zgc_v2
# Summary: 2 tests run: 2 passed, 0 skipped

cargo nextest run -p wjsm-runtime --features managed-heap-v2 -E 'test(zgc_v2_incremental)'
# Summary: 1 test run: 1 passed, 365 skipped

cargo nextest run -p wjsm-runtime -E 'test(zgc_)'
# Summary: 15 tests run: 15 passed, 294 skipped

cargo check -p wjsm-runtime --features managed-heap-v2
# Finished `dev` profile [unoptimized + debuginfo] target(s) in 13.83s
```

状态：Task 12 GREEN；`ZgcV2` 保持 feature-gated，默认 active ZGC 未切换；进入 Task 13。

## Task 13 implementation evidence

### RED

```text
cargo nextest run -p wjsm-snapshot-format --test managed_heap_v2
# error[E0432]: unresolved imports `wjsm_snapshot_format::ManagedHeapV2Generation`,
# `ManagedHeapV2Handle`, `ManagedHeapV2Layout`, `ManagedHeapV2Page`,
# `ManagedHeapV2Snapshot`, `decode_managed_heap_v2_snapshot`,
# `encode_managed_heap_v2_snapshot`
```

V2 snapshot/artifact ABI 在实现前不存在；active V1 format 不能携带 managed page metadata、8-byte atomic handle entry/generation 或 artifact engine/support fingerprints。

### GREEN

```text
cargo nextest run -p wjsm-snapshot-format
# Summary: 9 tests run: 9 passed, 0 skipped

cargo nextest run -p wjsm-runtime-support --features managed-heap-v2
# Summary: 12 tests run: 12 passed, 0 skipped

cargo nextest run -p wjsm-runtime --features managed-heap-v2 -E 'test(startup_snapshot_v2)'
# Summary: 1 test run: 1 passed, 366 skipped

cargo nextest run -p wjsm-runtime -E 'test(startup_snapshot)'
# Summary: 9 tests run: 9 passed, 300 skipped

cargo nextest run -p wjsm-runtime-snapshot --features managed-heap-v2 --test managed_heap_v2_artifact
# Summary: 1 test run: 1 passed, 0 skipped

cargo check -p wjsm-runtime --features managed-heap-v2
# Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.97s
```

状态：Task 13 GREEN；managed V2 snapshot/support ABI 保持 private feature-gated，active V1 snapshot format 与 restore 未切换；进入 Task 14。

## Task 14 implementation evidence

### RED

```text
cargo nextest run -p wjsm-runtime --features managed-heap-v2 --test realm_side_tables_v2
# error[E0432]: unresolved imports `wjsm_runtime::V2ConditionalRoots`,
# `wjsm_runtime::remap_realm_handles_v2`
```

旧 realm clone 只能通过 WasmEnv/main-memory clone/remap；没有能够验证 shared V2 handle table、条件 root 或 side-table dangling-handle 过滤的 owner。

### GREEN

```text
cargo nextest run -p wjsm-runtime --features managed-heap-v2 -E 'test(vm_gc_realm_roots_v2) | test(realm_clone_v2) | test(side_table_gc_v2)'
# Summary: 3 tests run: 3 passed, 367 skipped

cargo nextest run -p wjsm-runtime --test integration
# Summary: 87 tests run: 87 passed, 0 skipped

cargo check -p wjsm-runtime --features managed-heap-v2
# Finished `dev` profile [unoptimized + debuginfo] target(s) in 5.63s
```

状态：Task 14 GREEN；V2 realm/root/side-table adapter 保持 private feature-gated，default realm path 未切换；进入 Task 15 cutover audit。

## Task 15 activation audit and gate revision

```text
cargo nextest run --workspace --all-features
# Summary: 1766 tests run: 1274 passed, 491 failed, 1 timed out, 16 skipped
```

失败不是单一import问题：backend/host在`managed-heap-v2`下已消费8-byte atomic handles、shared memory64与V2 globals，但primordial snapshot、function table、realm clone、eval/runtime module及部分host dynamic object仍生产V1 addresses/handles。代表性症状为`unresolved V2 handle`、`indirect call type mismatch`、`invalid legacy source handle`和object property capacity不足。

原计划要求该命令在active cutover前GREEN，与feature的编译期activation语义矛盾。用户已批准修订为：先保持该命令RED以枚举完整迁移缺口，完成单一V2 runtime activation，再将同一命令作为cutover GREEN硬门。禁止通过重新隔离feature、V1 fallback或runtime双轨规避失败。
