# wjsm Generational ZGC 性能架构设计规格

**状态**：已批准；Round 3 共识通过；主机资源/OOM安全修订已纳入  
**日期**：2026-07-16  
**范围**：`wjsm-ir`、`wjsm-backend-wasm`、`wjsm-runtime`、`wjsm-runtime-support`、`wjsm-runtime-snapshot`、`wjsm-snapshot-format`、`wjsm-cli`、GC benchmark 与平台内存后端  
**架构审查要求**：是  
**主要目标**：使 wjsm ZGC 在算法、数据结构、并发模型和归一化单位成本上不弱于 JDK 25 Generational ZGC，并利用 Rust、Wasm handle 间接层和 NaN-box 空闲位形成可量化优势  
**兼容边界**：ECMAScript 语义、32 位 JS handle payload、`--gc mark-sweep|g1|zgc`、`gc()`、启动快照开关和现有运行时模块语义保持；内部对象堆、地址、barrier 与 snapshot ABI clean cutover

---

## 1. 问题陈述

当前 `ZgcCollector` 已具备 colored handle、增量 mark/relocate 状态机和 pause 统计，但与 JDK 25 Generational ZGC 仍存在结构性差距：

1. GC 工作由 mutator 在 `gc_safepoint_poll` 中单线程推进，不是真并发标记或并发重定位。
2. ZGC 分配 fast path 仍然是全堆连续 bump；zPage 主要承载统计和 relocation-set 元数据，不是能够回收、复用、commit/decommit 的 page allocator。
3. ZGC 非分代，没有 young/old 双周期、精确 old→young remembered set、原地晋升或跨 young cycle 的 old mark。
4. 普通 Wasmtime linear memory 需要 `StoreContextMut` 才能安全访问，后台 worker 无法在 mutator 执行时遍历对象堆。
5. `obj_table` 是 4-byte entry，只能容纳 32 位地址和极少颜色状态；真实 WJSM 堆受 memory32 的约 4 GiB 上限约束。
6. `object_walker` 会按 range 线性寻找对象并构造临时任务/引用数组；host 写路径存在 `handle_for_ptr` 线性反查。
7. host 热路反复执行 `WasmEnv::from_caller` export 查找。采样中该路径、对象扫描和临时分配已经成为可见成本。
8. 现有性能程序没有建立可信对标：Criterion 样本混入运行时生命周期；`zgc_autoresearch` 每轮只有末尾显式 full GC；`zgc_barrier_pressure` 把整轮 wall time误记为 barrier overhead。

因此，继续调整 `StepBudget`、阈值或现有 page 统计无法达到目标。必须重建对象堆边界、引用元数据、分配器、barrier、并发周期和性能证据体系。

---

## 2. 目标与验收语义

### 2.1 用户目标

- 不要求 WJSM 端到端 wall time 在所有负载上硬压过 JVM；JVM JIT 与 Wasmtime codegen 差异不得被误判为 GC 算法失败。
- 要求算法能力、数据结构、并发性、渐近复杂度和单位工作成本不弱于 JDK 25 Generational ZGC。
- 尾延迟优先；允许主动使用 Rust、Wasm、NaN-box 和 handle table 带来的结构优势。

### 2.2 硬验收

在预注册的 32/256/1024 MiB heap、10/50/80% live-set 矩阵中：

1. 按 §18.4 固定采集与归一化方法后，单位 allocated byte 的 GC CPU、单位 live byte 的 mark CPU、单位 relocated byte 的 relocation CPU、barrier fast-path retired instructions/event、metadata bytes/object 均不得比 JDK 25 ZGC 差超过 10%。
2. 上述核心指标中至少两项比 JDK 25 ZGC 优 15% 或以上。
3. ZGC p99.9 pause 不高于同条件 JDK 25 ZGC；PR 矩阵最大暂停小于 1 ms。
4. 所有暂停阶段的工作复杂度为 `O(mutators + root buffers)`，不得包含 heap/page/object 全扫描、对象复制或物理页释放。
5. allocation stall 只能在 relocation reserve 耗尽后发生，并具备独立计数与时间直方图。
6. 全部 ECMAScript fixture、GC matrix、WeakRef/FinalizationRegistry、realm、async-hooks、snapshot 与 side-table 生命周期测试通过。
7. 4/16 GiB、NUMA、长时间碎片与 uncommit 进入具备相应 RAM、ISA、NUMA 与操作系统能力的 nightly runner 验收；真实 WJSM 堆使用 shared memory64，不以 native-only 模型替代产品能力，本地缺少硬件能力不得被记为通过。

### 2.3 端到端数据

端到端 JS/Java 对等负载继续记录 throughput、latency、RSS、CPU 和 startup/steady-state，但只用于发现产品瓶颈和 Wasmtime 上限，不作为 GC 架构的单一否决条件。

### 2.4 Stop condition

- `done`：设计中的能力矩阵、正确性门、归一化性能门、retirement 和文档/ADR 同步全部完成。
- `blocked`：Wasmtime 43 无法提供满足内存模型的 shared memory64 原语，且没有经过证明的等价 owner-level 方案。
- `needs-verification`：实现存在但缺少预注册矩阵、并发模型或端到端证据。
- `scope-exceeded`：继续工作需要改变 ECMAScript 语义、公开 JS value 表示或引入第二套长期对象堆 owner。

---

## 3. 非目标

- 不把 WASM GC proposal、`externref` 或宿主 tracing GC 引入本次架构。
- 不实现运行时热切换 collector；collector 仍在 Runtime 创建时选定。
- 不改变 JS 可观察对象身份、相等性、属性顺序、WeakRef、FinalizationRegistry 或异常语义。
- 不用牺牲 mark-sweep/G1 的独立策略来伪装 ZGC 指标；三者共享底座但各自保留 collector policy。
- 不保留可发布、可运行时选择或长期存在的旧 memory32 动态对象堆 fallback；实施分支允许使用默认关闭且不进入发布产物的私有 `managed-heap-v2` 编译门来保持逐任务可编译，该门必须在统一切换任务中与旧路径一并删除。
- 不以手工 benchmark 数字、单个样本或只测显式 full GC 的结果宣称赶超。

---

## 4. 基线与权威来源

### 4.1 本地基线

- `docs/aegis/specs/2026-07-03-pluggable-gc-v2-design.md`
- `docs/aegis/plans/2026-07-03-pluggable-gc-v2.md`
- `docs/adr/0003-startup-snapshot-boundary.md`
- `docs/adr/0004-build-time-embedded-runtime.md`
- `docs/adr/0005-pluggable-gc-v2.md`
- `docs/adr/0008-node-vm-multi-realm.md`
- `docs/adr/0009-async-hooks-host-core.md`
- 当前 `runtime_gc`、backend support emitter、snapshot ABI 和 benchmark 源码

`docs/aegis/baseline/` 当前为空；本规格不虚构额外 baseline snapshot。

### 4.2 外部基线

- JDK 25 GA Generational ZGC 源码：`src/hotspot/share/gc/z/`
- JEP 439：Generational ZGC
- JEP 474：Generational ZGC by Default
- JEP 490：Remove the Non-Generational ZGC Mode
- Wasmtime 43.0.2 `SharedMemory`、memory64、threads 和自定义内存相关契约

### 4.3 Baseline Role Alignment

- Product / Requirement Baseline：本规格 §2 的算法同级、尾延迟优先和归一化验收。
- Architecture / Runtime Boundary Baseline：统一 `ManagedHeap`、shared memory64、32 位 handle identity、store-wide roots 和 build-time snapshot ABI。
- 结果：旧 GC v2 对“真线程并发、分代 ZGC、共享 heap substrate”的排除已被新需求显式取代；属于批准后的设计变更，不是实现漂移。
- scope：requirements 与 architecture。

---

## 5. 决策矩阵

| ID | 决策 | 结论 |
|---|---|---|
| D1 | 并发模型 | imported shared memory64 + 后台 GC worker；young/old mark 与 relocation 真并发 |
| D2 | heap owner | 单一 `ManagedHeap`；mark-sweep/G1/ZGC 全部迁移，不保留算法私有底座 |
| D3 | JS identity | boxed handle payload 保持 32 位；handle 从分配到死亡恒定 |
| D4 | object address | 内部对象地址升级为 u64/i64；memory64 支持真实 16 GiB+ 堆 |
| D5 | handle entry | `AtomicU64`，保存地址与 publish/relocation/generation 状态 |
| D6 | reference color | 使用 NaN-box bit 38–43 的双 epoch young/old mark 与 remembered color |
| D7 | allocation | per-mutator NLAB；host slow path 领取 page，不进行每对象 host call |
| D8 | page | 64 KiB commit granule；heap-relative small/medium page；large/humongous 独占范围 |
| D9 | generational | young/old 双周期；old mark 跨 young cycle；精确 old→young remembered slots |
| D10 | relocation | page relocation state + mutator handshake + assist + epoch retirement |
| D11 | worker | Runtime 固定 worker pool、可复用 packet、work stealing；不按 cycle 创建线程 |
| D12 | pacing | 以 allocation/survival/live growth/mark/relocate throughput 预测 runway |
| D13 | 平台 | portable scalar 基线 + x86_64 BMI2/AVX2/AVX-512 与 AArch64 NEON 运行时特化 |
| D14 | VM | Linux/Windows/macOS 独立 commit/decommit、NUMA 与 large-page hint 后端 |
| D15 | benchmark | collector kernel + end-to-end 双层；结构化原始数据与归一化门 |
| D16 | retirement | 旧动态 object memory、4-byte handle entry、连续 bump ZGC 和旧 benchmark 删除 |

---

## 6. 架构总览

```text
Wasm mutator
  ├─ NLAB top/end globals ────────────────┐
  ├─ inline load/store barriers           │
  ├─ per-mutator SATB/remset ring         │
  └─ safepoint/handshake poll             │
                                           ▼
ManagedHeap
  ├─ SharedHeapMemory (imported shared memory64)
  ├─ HandleTable<AtomicU64>
  ├─ VirtualSpace + PageAllocator
  ├─ ObjectStartMap + mark/remset bitmaps
  ├─ YoungGeneration / OldGeneration
  ├─ RelocationDescriptors + EpochDomain
  ├─ GcWorkerPool + WorkPackets
  ├─ GcDirector + Pacer
  └─ GcTelemetry
       ▲             ▲              ▲
       │             │              │
   MarkSweepPolicy  G1Policy  GenerationalZgcPolicy
```

### 6.1 canonical owner

`runtime_gc::heap` 唯一拥有动态对象内存、page、handle、对象边界、mark/remset/relocation 元数据和物理内存状态。collector 不得直接 grow Wasmtime memory、改 heap globals 或维护第二份地址真相。

### 6.2 backend owner

`wjsm-backend-wasm` 只拥有：

- NLAB allocation fast path；
- handle resolve fast path；
- heap reference load/store barrier 代码生成；
- phase masks、buffer top/end 和 rare slow-path imports；
- memory index 与 memory64 address 类型的一致性。

### 6.3 host owner

host imports 只承担 page refill、buffer full、relocation assist、handshake poll、显式 collection 和 telemetry snapshot。普通属性/元素 fast path 不得因 GC 进入 host。

### 6.4 GC control plane

`RuntimeState` 持有单一 `Arc<GcRuntime>`，其中包含 `Arc<ManagedHeap>`、collector controller、worker pool、director 和 telemetry。现有 `Arc<Mutex<Box<dyn GcAlgorithm>>>` 与 `GcContext<StoreContextMut>` 不得延续到并发架构：

- `MutatorGcContext` 只在 handshake、root snapshot、显式 GC 与 rare slow path 中借用 Store；
- `CollectorContext` 只包含 `Send + Sync` 的 heap、root snapshot、work queues、side-table snapshot 与 telemetry；
- `GcController` 方法使用 `&self` 和显式原子/窄锁状态，不以包围整个 collector 的全局 mutex 串行化 worker；
- worker 永远不持有 `StoreContextMut`、`Caller`、`WasmEnv` 或普通 `Memory` borrow；
- root snapshot 在暂停边界完成后发布为 immutable work packet，后台扫描不回借 RuntimeState。

host 侧动态对象访问通过 `ManagedHeap`/`HeapAccess`，不得继续从 `WasmEnv::from_caller` 逐次查询动态对象 memory 与 globals。

---

## 7. 内存与地址 ABI

### 7.1 多 memory 边界

- 现有 `env.memory` 继续承载静态数据、primordial string constants 和非 GC 动态区。
- 新增 `env.__heap_memory`：imported shared memory64，承载全部动态 GC 对象、handle table 与 barrier buffers。
- shadow stack 与调试 barrier memory 保持独立 owner；其是否共享由用途决定，不与对象 heap 混合。
- user module 与 support module import/re-export 同一 `__heap_memory`。

### 7.2 SharedHeapMemory

Rust 生产后端以 `SharedHeapMemory` 包装 `wasmtime::SharedMemory`；协议测试通过 sealed `HeapMemory` trait 使用 `NativeHeapMemory` / loom backend，`ManagedHeap<M>` 在生产中静态单态化为 `ManagedHeap<SharedHeapMemory>`，不得在热路引入 `dyn HeapMemory`：

- 只暴露对齐的 `HeapWord`、分级 header 字段、byte-copy 和 page lifecycle API；
- 与 worker 竞争的 value slot 与 mutable header word 使用原子访问；raw bytes 只有在被 pin 或进入 relocation handshake 后才能批量复制；
- immutable header（type、layout class、allocation size、owning handle）在 publish 前写完；mutable-in-place header（prototype、length、property count、flags、backing引用）通过原子/barrier API 更新；
- 每个 unsafe block 记录对齐、范围、别名、原子顺序与生命周期不变量；
- 平台 VM 操作只接受 page-aligned、已保留范围，并通过 RAII 管理状态。

### 7.3 memory64

所有动态对象地址、page range、heap globals 和相关 host imports 使用 u64/i64。JS value 仍保存 u32 handle，因此公开 value width 和 identity 不变。

### 7.4 handle entry

`ColoredHandleEntry(AtomicU64)` 的高 48 位保存 byte address，低 16 位保存状态。首版状态集合固定为：

- `Free`
- `StableYoung`
- `StableOld`
- `RelocatingYoung`
- `RelocatingOld`
- `PinnedOld`
- `Retired`

Wasm与Rust共同访问的entry使用`SeqCst` load/store/CAS；非法状态转换只通过类型化CAS helper暴露。handle按块分配，回收后进入epoch quarantine；在所有旧epoch worker/mutator退出前不得复用。


shared memory64布局固定为：`[0, 32 GiB)`连续handle table虚拟区；随后是按`max_mutators * per_mutator_buffer_budget`计算并按64 KiB对齐的control/barrier-buffer区；`object_heap_base = align_up(32 GiB + control_reserved, 64 KiB)`；对象堆独立grow于`[object_heap_base, object_heap_base + max_heap_size)`。因此4/16 GiB表示对象堆cap，不是整个memory地址上限；handle resolve仍为`handle_base + handle * 8`，对象地址永不落入handle/control区。reserved与committed metadata分别报告。

### 7.5 Engine config 与可行性门

新增独立 `wjsm-engine-config` crate作为runtime、support cwasm build与snapshot build的唯一Wasmtime feature/tunable owner。它固定threads、multi-memory、memory64、Cranelift与precompile compatibility fingerprint；所有artifact将fingerprint纳入ABI hash。

在任何value/layout/control-plane投资前，必须通过永久feasibility test：同一engine config编译main memory32 + imported shared memory64 user/support modules，执行Wasm原子访问与host线程并发访问，并完成support cwasm precompile/deserialize。失败即进入`blocked`，不得继续编码或引入第二heap方案。
---

## 8. NaN-box reference color

当前 value layout 已确认：tag 占 bit 32–36，runtime-string flag 占 bit 37，bit 38–50 可用于 GC metadata。

首版使用六个位：

- `YOUNG_MARK_0` / `YOUNG_MARK_1`
- `OLD_MARK_0` / `OLD_MARK_1`
- `REMEMBERED_0` / `REMEMBERED_1`

bit 44–50 保留并要求为零。颜色只允许出现在 heap reference slot；root/local/host-visible value 必须规范化。所有类型判断与 handle decode 忽略 GC color；语义比较前必须 strip color。

相比 JVM colored oop，本设计不需要 remap color：引用保存 handle，不保存物理地址。relocation 只更新 handle entry，全部引用下一次 resolve 自动观察新地址。

---

## 9. 分配器与 page layout

### 9.1 NLAB

每个 mutator 持有 `top/end/page_id`：

```text
new_top = top + aligned_size
if new_top <= end:
    commit local top
    initialize object
else:
    gc_alloc_slow(size, layout)
```

fast path 无锁、无 host call、无统计哈希；allocated bytes 使用线程局部计数，在 refill/handshake 时批量汇总。

### 9.2 page class

- commit granule：64 KiB。
- small page：按 heap max 选择 64 KiB–2 MiB 的 2 次幂。
- medium page：small page 的固定倍数，上限 32 MiB。
- large/humongous：按对象大小分配连续 granule。

page class 在 heap 创建时确定，运行中不改变对象边界解释。

### 9.3 metadata

每个 page 拥有：

- generation、state、NUMA node、age、live bytes；
- object-start/size map；
- 双 mark bitmap；
- 精确 reference-slot remembered bitmap；
- relocation descriptor range；
- committed/resident/last-used 状态。

对象枚举从 bitmap 和 object-start map 流式产生，不扫描全 handle table，不创建每对象 `Vec`。

### 9.4 promotion 与 reserve

- 稠密 survivor/large/humongous page 优先原地晋升。
- 稀疏 page 进入 relocation set。
- relocation reserve 与 mutator free reserve 分离，防止 collector 在复制途中耗尽空间。
- 完全死亡 page 先回收，用作 relocation target。

---

## 10. Barrier 设计

### 10.1 load barrier

fast path：

1. 检查 value 是否为 handle-backed reference；
2. 提取 u32 handle；
3. `i64.atomic.load` handle entry；
4. `StableYoung/StableOld/PinnedOld` 直接解码 address；
5. `Relocating*` 进入 assist path；
6. `Free/Retired` 触发 verifier/trap，不返回伪地址。

### 10.2 store barrier

store barrier 读取旧 heap word，并依据当前 young/old mark mask 与 remembered mask决定：

- SATB：旧引用未具当前 mark color时，将旧 handle 写入 per-mutator ring；
- old→young：目标对象为 old 且新引用指向 young 时，将 slot address 写入 remembered ring；
- 仅当新值是handle-backed reference（runtime string必须带bit 37 runtime-handle flag）时才设置当前epoch color；number、静态/inline string、bool、null、undefined及其他非引用值必须保持bit 38–43为零后原子写回。

barrier ring 预分配、按 cache line 分离 producer/consumer index。buffer 有空间时不得进入 host。buffer 满时优先 mutator assist/drain；只有需要扩大全局 packet pool 时进入 rare host slow path。

### 10.3 scan self-heal

marker 扫描 slot 后可用 CAS 写入当前 mark/remembered color。颜色竞争只允许产生重复 work，不允许遗漏引用。

### 10.4 barrier verifier

debug/verification build 记录对象堆 memory index 的所有动态引用 load/store 位置，并验证：

- 发布后引用 load 必经 resolve；
- 发布后引用 store 必经 barrier；
- 初始化期写只发生在对象 release-publish 前；
- bulk copy 按 source/destination generation 选择逐槽 barrier 或已证明安全的 publish copy。
- mutable-in-place header（尤其`Object.setPrototypeOf`写入的prototype）使用与普通slot相同的原子/relocation协调；verifier拒绝把它归类为publish后immutable字段。

---

## 11. Generational ZGC 周期

### 11.1 young cycle

1. `PauseYoungMarkStart`：翻转 young/remembered epoch，封存 NLAB，snapshot roots 与 active remembered buffer。
2. `ConcurrentYoungMark`：worker drain roots、SATB、remembered slots 与 young object graph。
3. `PauseYoungMarkEnd`：flush mutator-local buffers，完成 termination handshake 和弱引用边界确认。
4. `ConcurrentYoungSelectRelocationSet`：计算 live bytes、年龄、碎片与复制收益。
5. `PauseYoungRelocateStart`：发布 relocation epoch 与 page states，等待 mutator 越过 handshake。
6. `ConcurrentYoungRelocate`：复制稀疏 page、原地晋升稠密 page、更新 handle entries。
7. `YoungEpochReclaim`：旧访问 epoch 清空后回收 source pages 与 quarantined handles。

### 11.2 old cycle

1. old mark 由 young mark-start 协调启动。
2. old mark 可跨多个 young cycle 并发推进。
3. young cycle 向 old cycle提供新晋升对象和 young→old roots。
4. `PauseOldMarkEnd` 只完成 buffer/termination handshake。
5. old relocation-set selection、relocate-start 和 relocate 与 mutator 并发。
6. old relocation 与 young relocation 的 page/epoch 状态机禁止冲突；同一 page 只属于一个 generation owner。

### 11.3 weak/finalization/side tables

WeakRef、FinalizationRegistry、Promise/continuation、stream、proxy、native callable、async-hooks 与 realm side tables 使用统一 store-wide root/weak processing 接口。清理必须在 handle 进入 quarantine 前完成；callback 调度发生在 GC 周期发布完成之后。

---

## 12. 并发 relocation 与内存模型

### 12.1 relocation protocol

1. selector 将 page 从 `Marked` CAS 为 `RelocationSelected`。
2. relocate-start handshake 发布新访问 epoch，确保没有 mutator 保留跨 epoch 未受保护的 raw address。
3. worker 将 handle entry 从 `Stable*` CAS 为 `Relocating*`。
4. relocation descriptor 保存 source、destination、size 与 copy state。
5. worker 或 mutator assist 取得 copy ownership；每个heap type由静态`HeaderLayout`列出字段类别：type、layout class、allocation size、capacity与owning handle在publish后immutable并按byte复制；prototype、logical length、property count、flags与可变backing reference属于mutable-in-place words，逐字段`SeqCst` snapshot复制；value slots同样逐word复制。entry进入`Relocating*`后mutator不得再写source，verifier禁止对整个header做未分类bulk copy。
6. 完成后以`SeqCst` CAS发布destination `Stable*` entry。
7. source page 在 epoch grace period 后回收。

### 12.2 写竞争

mutator 观察到 `Relocating*` 时不得继续写 source。它协助或等待 descriptor 完成，再重新 resolve destination。relocate-start handshake 消除“已解析 source、尚未进入写 barrier”的跨 epoch窗口。

### 12.3 原子顺序

- Wasm threads原子指令按规范只有`SeqCst`语义；所有mutator与worker共享的heap value/header word在Wasm侧必须使用atomic load/store/RMW，禁止用普通load/store模拟acquire/release。
- Rust侧与Wasm共享的heap word使用`SeqCst`参与同一顺序；Rust私有metadata（work queue、phase epoch、termination counters）才允许依据happens-before使用Acquire/Release/Relaxed。
- `SeqCst`成本计入barrier硬门，不能通过弱化正确性访问规避；任何Rust私有metadata的memory-order弱化都必须有loom模型和目标平台反汇编证据。

---

## 13. Worker、调度与 pacing

### 13.1 GcWorkerPool

- Runtime 创建固定 worker pool；不按 cycle 创建线程。
- worker 使用可复用 work packet 与 work-stealing deque。
- packet payload 为 page range、bitmap word range、root range 或 relocation range，不保存临时对象集合。
- termination 使用全局 inflight work + epoch，空闲 worker 通过 condvar/parking 休眠。
- worker 数根据 CPU、mutator 利用率、剩余 runway 与历史 bytes/ns 调整，至少保留一个 mutator 核心。

### 13.2 GcDirector

分别维护 young/old：

- allocation bytes/ns；
- survival/live-growth rate；
- mark bytes/ns；
- relocate bytes/ns；
- free/relocation reserve；
- previous cycle prediction error。

当预计 cycle 完成时间接近 free-space runway 时启动。固定 4 MiB debt 不再是 ZGC 主触发条件。

### 13.3 mutator assist 与 stall

- assist work 与 allocation debt 成比例。
- assist packet 有硬上限，避免单次 allocation 形成长暂停。
- allocation stall 只在 relocation reserve 耗尽时允许，必须记录原因、持续时间、heap state 与 director prediction error。

---

## 14. 物理内存、NUMA 与 SIMD

### 14.1 commit/decommit

VirtualSpace 保留最大地址范围；PageAllocator 按需 grow/commit。空闲 page 达到 delay 或 soft-max 压力时：

- Linux：page-aligned `madvise`/等价机制；
- Windows：VirtualAlloc/VirtualFree 对应 decommit/recommit；
- macOS：对应 madvise；
- 不支持平台保留正确但无 decommit 的 portable 实现，并在能力报告中显式标记。

### 14.2 NUMA

当检测到多个 NUMA node：

- page free list 按 node 分片；
- NLAB 优先领取 mutator-local node page；
- worker 优先处理本地 page；
- relocation 优先 node-local destination；
- fallback 到跨 node 必须计数。

### 14.3 SIMD/位运算

portable scalar 实现始终存在；运行时选择：

- x86_64 BMI2/AVX2/AVX-512：mark/remset bitmap、object copy、card/slot scan；
- AArch64 NEON：对应路径；
- 每个 ISA path 与 scalar path 共享同一 property-based correctness suite。

特化只接受经 microbenchmark 证明的实现；不在对象 allocation fast path 做重复 feature detection。

---

## 15. Snapshot、realm 与 support ABI

### 15.1 startup snapshot

- snapshot 格式升级，记录 shared heap pages、page metadata、handle entries 和 generation。
- restored primordial pages进入 old generation；不可移动对象显式 `PinnedOld`，不以地址区间隐式推断。
- ABI hash 加入 memory64、heap entry layout、color bits、page layout、support imports 与 builtin bundle。
- 旧 snapshot 按现有 ABI mismatch 机制冷启动，不增加第二套 restore path。

### 15.2 realm

node:vm realms 继续共享单 Store/ManagedHeap。realm clone/remap只操作 handle identity 和 side tables；不拥有独立 collector、worker、epoch 或 object heap。

### 15.3 support module

三 GC flavor 的 import/export surface保持一致。差异仅在 compiler-emitted barrier/policy fast path；memory index、entry layout 和 shared heap ABI 单一。

---

## 16. 统一 collector substrate

### 16.1 mark-sweep

迁移到 `ManagedHeap` 后继续使用 STW mark/sweep policy：

- NLAB/page allocator共享；
- atomic heap words和handle entries共享；
- 不执行 relocation；
- 作为最简单正确性基线。

### 16.2 G1

G1迁移到同一 page/object-start/remset/worker substrate：

- 保留 region evacuation、young/mixed policy；
- 删除私有 `RegionSpace` 与重复分配 owner；
- 使用共享 remembered bitmap 和 telemetry。

### 16.3 ZGC

ZGC拥有 young/old generation、concurrent mark/relocate、color/barrier、director policy。它不得绕过共享 heap owner直接操作 Wasmtime memory。

---

## 17. Telemetry 与 profiler

`GcTelemetry` 输出版本化结构数据：

- cycle/generation/phase times；
- mutator 与 GC thread CPU；
- allocated/marked/relocated/reclaimed bytes；
- barrier fast/slow、buffer flush、assist；
- NLAB refill、page alloc、commit/decommit、NUMA fallback；
- pause/stall HDR histogram；
- metadata、committed、resident、fragmentation；
- director prediction 与实际误差。

CLI 保留人类可读摘要，并支持稳定 JSON 输出。benchmark、测试和 profiler 读取同一 telemetry owner，不维护平行计数器。

Profiler 证据至少包含：

- `perf stat` cycles/instructions/branches/cache/TLB/page-faults；
- `perf record`/等价工具的 mutator 与 worker flame graph；
- Wasmtime/WAT barrier 反汇编与指令计数；
- RSS/commit/decommit time series；
- JDK JFR 与 `-Xlog:gc*` 对照。

---

## 18. Benchmark 设计

### 18.1 `wjsm-gc-bench`

新增独立 benchmark crate，Task 1必须一次性固定完整CLI surface：

- 子命令：`capabilities`、`preflight`、`prepare-jdk`、`baseline`、`run`、`micro`、`compare`、`replay`、`gate`；
- 公共参数：`--engine`、`--gc`、`--heap`、`--live-set`、`--scenario`、`--samples`、`--duration`、`--workers`、`--seed`、`--output`、`--manifest`、`--profile`、`--jdk-home`、`--jdk-probe-home`；
- adversarial参数：`--relocate-every-page`、`--barrier-buffer-capacity`、`--safepoint-every-allocation`；
- 统一deterministic scenario、WJSM collector-kernel/JS driver、stock/instrumented JDK driver、CPU/heap/worker pinning、host/cgroup资源准入、telemetry/JFR/perf采集、JSON schema与统计汇总。

现有 `gc_stress.rs`、`zgc_autoresearch.rs`、`zgc_barrier_pressure.rs` 的有效 workload迁入新 harness；旧测量入口删除。JDK内部归一化指标来自固定`jdk-25-ga`源码上的版本化diagnostic patch；stock JDK仅用于pause、RSS与端到端对照，不能混用两类数据。

### 18.2 场景矩阵

- churn：高短命率；
- request：短命 request graph + 长命 cache；
- chain/cycle/wide graph；
- property mutation与array slot mutation；
- string、array、typed backing、humongous；
- weak/finalization；
- burst、steady saturation、idle/uncommit；
- 10/50/80% live-set；
- 32/256/1024 MiB PR matrix，4/16 GiB nightly。

### 18.3 测量纪律

- compile、module instantiate、warmup、steady state分开。
- 每个配置至少30个独立样本。
- 固定JDK 25 GA、heap cap、worker数、CPU affinity与场景seed。
- 报告bootstrap 99% CI、effect size和原始样本。
- 不以单次最大/最小值替代分布。
- benchmark结果包含git revision、JDK/Wasmtime版本、CPU、内核、NUMA、页配置和完整参数。


### 18.4 归一化方法

每个scenario输出相同logical graph hash、对象数、引用边数、size/reference-density分布，并同时记录logical与physical denominators。五项硬指标固定如下：

| 指标 | WJSM numerator | JDK 25 numerator | denominator |
|---|---|---|---|
| GC CPU / allocated byte | named GC worker thread CPU + pause CPU + mutator-assist CPU | instrumented JDK中ZGC worker/VM phase thread CPU；barrier留在mutator指标 | 两端collector实际分配physical bytes |
| mark CPU / live byte | young/old mark worker与mark pause CPU | instrumented JDK young/old mark worker与mark pause CPU | mark-end physical live bytes |
| relocation CPU / relocated byte | relocate worker + assist CPU | instrumented JDK relocate worker CPU | 成功复制并发布的physical bytes |
| barrier retired instructions / event | dedicated reference-load/store scenario的`perf instructions`减去同binary非引用slot control，分别除以`barrier_load_fast_events`与`barrier_store_fast_events` | 相同方法；JDK diagnostic counter以同名独立JSON字段提供load/store fast-path events | load/store分开报告并分别应用gate |
| metadata bytes / object | handle/page/bitmap/remset/forwarding/worker metadata committed bytes | diagnostic patch汇总ZGC page table、mark bitmap、remset、forwarding与worker metadata committed bytes | mark-end live object count |

instrumented JDK patch只增加单调counter与退出时JSON，不改变collector决策；其源码tag、patch hash、build flags和counter开销校准必须写入manifest。CPU统一使用thread CPU time；wall duration不得替代CPU。reserved与committed metadata分开，硬门使用committed。若JDK probe缺任一numerator，相关gate状态为`needs-verification`而非估算通过。

物理对象布局不同可能影响per-byte结果，因此报告同时给出per-object/per-reference-edge视图与两端size/reference-density分布；不能通过扩大WJSM对象来人为降低per-byte成本。

### 18.5 主机资源准入与OOM隔离

任何benchmark在spawn WJSM/JDK前必须执行`preflight`。`HostResourceSnapshot`读取Linux `/proc/meminfo`、当前cgroup v2 `memory.max/current/high/events`、swap、PSI与`RLIMIT_AS`；Windows读取Job Object/commit信息，macOS读取host statistics与rlimit。资源未知、读取失败或硬隔离能力不足时，大堆profile fail-closed。

准入计算固定为：

```text
effective_total     = min(physical_total, finite_cgroup_or_job_limit)
effective_available = min(MemAvailable, finite_cgroup_or_job_remaining)
safety_headroom     = max(2 GiB, 10% * effective_total)
required_total      = 4 * max_object_heap_cap
required_available  = 3 * max_object_heap_cap + safety_headroom
```

同时要求`RLIMIT_AS`/Job虚拟地址上限容纳handle虚拟区、control区、object heap与Wasmtime guard reservation。swap不计入available；发生swap-in/out、PSI full pressure或cgroup `oom/oom_kill`增量时当前样本无效并终止剩余矩阵。准入失败返回`needs-resource-runner`（退出码78），不得自动缩小heap或把skip记为通过。

4/16 GiB profile只在具备delegated cgroup v2或Windows Job Object硬限制的runner执行；WJSM与JDK顺序运行且持有全机large-benchmark独占锁。每个child使用`memory.max = 2 * heap_cap + 2 GiB`、`memory.swap.max = 0`或等价Job限制；supervisor在90%预算发送终止信号，硬限制负责在95–100%边界内隔离失败。构建与JDK probe编译在独立job完成，大堆runner只下载并执行预构建artifact，不同时运行rustc/javac。

资源准入必须有可注入`HostResourceProvider`测试：模拟仅4 GiB available时，1/4/16 GiB请求均在spawn前拒绝，256 MiB请求可通过；测试断言child process计数仍为零。
---

## 19. 正确性验证

### 19.1 deterministic tests

- NaN-box color encode/decode/semantic normalization；
- handle entry状态转换；
- page allocator split/coalesce/commit/decommit；
- object-start/mark/remset bitmap；
- NLAB refill和publish；
- young/old phase状态机；
- director prediction与stall边界。

### 19.2 并发模型

loom模型覆盖：

- SATB overwrite与marker竞争；
- remembered epoch flip；
- relocation copy ownership/CAS；
- mutator assist；
- epoch reclaim；
- handle quarantine与复用；
- mark termination。

并发协议测试分三条互不混用的后端：`gc_loom_model`使用loom atomics/heap，不链接Wasmtime或crossbeam；`gc_protocol_miri`只覆盖纯pack/state/epoch与`NativeHeapMemory`，不执行worker deque；`gc_concurrency_model`使用std原子与生产queue供TSan运行。SharedMemory integration另由普通nextest验证。

### 19.3 adversarial runtime tests

- barrier ring容量为1；
- 每次分配触发handshake；
- 每页都进入relocation set；
- worker数1与CPU-1；
- mutator在同一slot上高频竞争写；
- WeakRef/finalizer在young/old relocation交界；
- snapshot restore后首轮young/old cycle；
- realm创建/销毁与old mark交错；
- OOM发生在page refill、relocation reserve和memory64 grow。

### 19.4 全量行为

三collector运行GC密集fixture；默认算法全量workspace；ZGC运行完整happy/errors及适用test262分片。不能通过修改fixture期望来隐藏语义错误。

---

## 20. 实施切片

| Slice | 交付 | 独立验收 |
|---|---|---|
| S-1 | Wasmtime shared memory64/multi-memory/threads/support-cwasm可行性与统一engine config | 永久feasibility/precompile-deserialize test；失败即blocked |
| S0 | 可信benchmark、telemetry schema、instrumented JDK 25 probe | CLI/采集方法固定；当前三GC基线可复现 |
| S1 | additive NaN-box color、SharedHeapMemory/native protocol backend、page/handle/worker primitives | native/loom/Wasmtime memory model tests；旧runtime主路径不切换 |
| S2 | 私有`managed-heap-v2`编译门下的VirtualSpace、object-start/bitmap、NLAB与collector适配 | feature-gated integration tests；不进入发布产物 |
| S3 | backend/host/三collector/snapshot/realm/support协调切换 | 同一切换任务改8-byte entry与memory64 ABI，全部公开collector GREEN，删除私有门与旧heap owner |
| S4 | NaN-box color、backend load/store barrier、verifier | barrier WAT/反汇编、非引用值与mutable-header tests |
| S5 | concurrent young generation | young正确性、pause与单位mark成本门 |
| S6 | concurrent old generation与精确跨代协调 | old跨young cycle、weak/side-table与大live-set门 |
| S7 | concurrent relocation、assist、epoch reclaim | relocation/setPrototypeOf race、max pause、relocated-byte成本门 |
| S8 | director/pacer、allocation stall、NUMA/decommit/SIMD | 本地capability gate + 指定跨平台/ISA/NUMA CI矩阵 |
| S9 | profile-driven优化、全矩阵、旧入口清理、ADR/baseline同步 | 全部硬验收、lingering-reference负检查 |

每个切片都必须先通过语义/并发门，再接受性能结果；不得把正确性债务推迟到S9。

---

## 21. 复杂度预算

### 21.1 当前压力

- `host_imports/core.rs` 3438行：不得继续加入GC实现。
- `runtime_gc/api.rs` 761行：options/stats/telemetry/phase types需要拆 owner。
- `runtime_gc/object_walker.rs` 727行：由page/object map streaming scanner替代。
- `runtime_gc/zgc/mod.rs` 当前可控，但新分代/并发职责不得继续堆入单文件。

### 21.2 文件边界

计划中的主要新 owner：

```text
crates/wjsm-runtime/src/runtime_gc/
  heap/
    mod.rs
    memory.rs
    platform/{linux,windows,macos,portable}.rs
    page.rs
    allocator.rs
    handle.rs
    bitmap.rs
    object_map.rs
    epoch.rs
  worker/{mod.rs,packet.rs,queue.rs}
  telemetry/{mod.rs,histogram.rs,json.rs}
  zgc/
    mod.rs
    barrier.rs
    young.rs
    old.rs
    mark.rs
    relocate.rs
    director.rs
```

具体文件名可在实施计划中按现有Rust模块布局校正，但owner边界不得退回通用巨型文件。

### 21.3 Budget result

- Source Complexity：高风险但可治理；通过新owner模块而不是add-in-place。
- Test Complexity：高风险；benchmark、loom model、runtime adversarial tests分目录。
- Decision / Plan Complexity：高风险；本规格固定不变量，实施计划按slice拆分。
- 结论：`at-risk`，已通过模块/切片边界治理。

---

## 22. Anti-Entropy 与 retirement

### 22.1 Anti-Entropy Declaration

- Deletion Class：contract-carrying internal code retirement。
- Old Path：memory32动态对象堆、4-byte obj_table entry、全堆连续bump ZGC、算法私有heap owner、非原子发布后引用访问、旧benchmark入口。
- New Canonical Owner：`ManagedHeap` + shared memory64 ABI。
- Preserved Behavior：ECMAScript语义、32位handle identity、CLI collector选择、snapshot开关、realm/async-hooks/side-table语义。
- Retired Behavior：假并发ZGC、非分代ZGC、旧heap fallback、无效benchmark指标。
- External Boundary Touched：内部Wasm/support/snapshot ABI；无公开持久数据。
- Source-of-Truth Data Risk：无。
- User Confirmation Required：否。

### 22.2 Retirement Decision

- Path：`delete-first`；实施分支中的私有`managed-heap-v2`编译门只用于保持准备任务可编译，不是runtime fallback、公开feature或发布兼容边界。
- 统一切换任务必须同时启用新ABI、迁移全部内部caller、删除私有门和旧动态heap主路径；计划结束不保留compat adapter、双对象堆或deprecated re-export。
- snapshot通过版本/hash失效机制处理，不反序列化旧内部布局。

### 22.3 Verification Plan

- Main-path：三collector都通过统一ManagedHeap运行。
- Lingering-reference：结构搜索确认旧heap globals、旧entry解码、旧allocator和旧benchmark无主路径引用。
- Negative：构建/测试明确拒绝旧snapshot ABI与未barrier的发布后heap access。
- Boundary：CLI、fixture、snapshot cold/hot、realm和support artifacts通过。

---

## 23. 风险与证伪条件

| 风险 | 证伪/控制 |
|---|---|
| shared atomics使mutator吞吐大幅下降 | barrier反汇编、单位指令/branch/cache指标；未达10%门则重做fast path，不关闭正确性barrier |
| memory64扩大地址计算成本 | 对object heap单独memory64，静态memory保持memory32；比较指令与cache影响 |
| relocation写竞争丢失更新 | loom + 1-slot/高频写adversarial；协议失败即阻断并发relocation |
| Wasmtime SharedMemory能力不足 | 以43.0.2真实API与运行实验验证；若无法满足原子/稳定地址，不写第二heap fallback，返回blocked重新决策 |
| metadata在小堆过重 | heap-relative page、按需bitmap与metadata bytes/object硬门 |
| worker抢占mutator |动态worker、保留mutator核心、mutator utilization与CPU分离统计 |
| snapshot/realm roots遗漏 | cold/hot双矩阵、realm clone/destroy与并发old cycle交错测试 |
| SIMD路径语义漂移 | scalar/property-based对照与跨ISA CI |
| 平台能力无法本机覆盖 | 本机只关闭portable/Linux/AVX2可验证项；AVX-512、AArch64、Windows、macOS、多NUMA与4/16 GiB只由具名capability runner关闭，不以skip计通过 |
| build/runtime engine config漂移 | 单一`wjsm-engine-config` owner、compatibility fingerprint与support cwasm precompile/deserialize test |
|benchmark被JIT/Wasmtime污染|collector-kernel归一化层与端到端层分离，原始数据可复现 |

架构证伪条件：如果中央handle间接层在完整barrier/relocation协议下无法将per-access成本控制在JDK 25的110%以内，必须回到entry/color/heap layout设计，不允许通过放宽指标或减少测试矩阵宣称完成。

---

## 24. ADR 与文档信号

本设计落地后需要修订/补充ADR：

1. supersede ADR 0005中“safepoint增量等价并发”“非分代ZGC”“算法独立heap layout”“obj_table低2位color”决策；
2. 记录shared memory64对象堆、AtomicU64 handle entry、NaN-box reference color与统一ManagedHeap owner；
3. 记录Generational ZGC young/old周期、worker/pacer与relocation epoch；
4. 同步ADR 0003/0004的snapshot/support ABI边界；
5. 同步AGENTS.md的WASM contract、GC算法与大堆限制。

ADR只在实现与证据稳定后接受；本设计不把未执行架构写成已落地事实。

---

## 25. 外部参考

- OpenJDK JEP 439：<https://openjdk.org/jeps/439>
- OpenJDK JEP 474：<https://openjdk.org/jeps/474>
- OpenJDK JEP 490：<https://openjdk.org/jeps/490>
- JDK 25 GA ZGC source：<https://github.com/openjdk/jdk/tree/jdk-25-ga/src/hotspot/share/gc/z>
- Wasmtime SharedMemory API：<https://docs.rs/wasmtime/43.0.2/wasmtime/struct.SharedMemory.html>
