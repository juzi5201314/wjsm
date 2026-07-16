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
