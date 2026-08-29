# Direct Cranelift 后端实现指南

本文档描述 wjsm 当前唯一生产执行后端的边界、数据流与维护规则。它不是多后端接入教程：Wasm/Wasmtime backend、JIT stub、CLI backend selector 与兼容 fallback 已删除。若未来重新引入其他执行后端，必须先以新的 ADR 定义完整 artifact、runtime ownership 与语义验证契约。

## 架构概览

```text
JS / TS source
  -> wjsm-parser                 SWC AST
  -> wjsm-semantic               verified semantic IR
  -> wjsm-module                 module graph + portable manifest
  -> wjsm-artifact-format        portable .wjsm
  -> wjsm-backend-native         IR -> CLIF -> relocatable object -> native image
  -> wjsm-host-native            NativeRuntime + ManagedHeap + host APIs
```

Portable `.wjsm` 是 IR。generic native 在第一次执行静态模块前由当前宿主编译；语言仍是动态 JS。热路径 overlay 与 `eval` 在运行时再次调用同一 `NativeCompiler`，不是第二套后端，也不是「运行时不再编译」。

| Crate | 唯一职责 |
| --- | --- |
| `wjsm-ir` | 后端无关 semantic IR、builtin/runtime operation IDs 与 value ABI 常量 |
| `wjsm-semantic` | SWC AST 到 verified IR 的 lowering |
| `wjsm-module` | ESM/CJS graph、resolution 与 portable module manifest |
| `wjsm-artifact-format` | `.wjsm` canonical encode/decode、limits、hash、semantic ABI 与 verification |
| `wjsm-native-abi` | compiler/runtime 共用的 fixed-width vmctx、host symbol、call/root/source frame contract |
| `wjsm-backend-native` | 当前宿主 ISA、CLIF lowering、object emission、strict relocation、W^X/unwind、native image/cache |
| `wjsm-host-native` | pinned vmctx、agent/runtime、ManagedHeap/GC、scheduler、modules、snapshot、inspector 与 host APIs |
| `wjsm-builtins` | 后端无关 ECMAScript/Web/Node 语义算法 |
| `wjsm-host` | 后端无关 host/heap/exec 契约；不得拥有 native runtime state |
| `wjsm-gc` | ManagedHeap、HandleTableV2、object access、collector 与平台虚拟内存抽象 |

## 1. Portable artifact 边界

`PortableArtifact::from_input` 是构建边界。输入由 `ArtifactBuildInput` 组成：

- `Arc<Program>`：已 lowering、可再次 verification 的 semantic IR；
- `Arc<ModuleManifest>`：canonical logical URL 与 module graph；
- `BuildOptions`：是否携带 source map/source text；
- 可选 source text。

编码前必须验证 program/manifest。容器包含 manifest、program、required builtins 与可选 source metadata，并记录 section hash、semantic ABI hash 和整件 digest。

不可信字节只通过：

```rust
PortableArtifact::decode(bytes, &ArtifactLimits::default())
```

进入 runtime。decoder 必须在分配前应用总大小、section、module、function、block、instruction、string 与 cross-reference limits；decode 成功后再次验证 IR、manifest 和 required builtin set。

`.wjsm` 中禁止出现：

- Cranelift object 或 executable bytes；
- native relocation、宿主 pointer、runtime/image ID；
- native cache key、cache path 或 startup snapshot 私有地址；
- target/CPU 专属机器码。

## 2. Direct IR → CLIF compiler

`NativeCompiler::new()` 只为当前宿主构造 ISA。当前 production capability 是 64-bit x86_64 Linux/Windows；不支持的 target 立即返回 `UnsupportedTargetCapability`，不回退到其他 backend、解释器或 test heap。

`NativeCompiler::compile(&PortableArtifact)` 直接消费 artifact 中的 `Program`：

1. verifier 保证 CFG、Phi、ValueId、builtin 与 module cross-reference 有效；
2. 每个 IR function 降为 CLIF；Phi 变为 block arguments；
3. ECMAScript 动态语义调用具名 host/runtime operation；
4. may-GC 点按 liveness spill boxed roots 并发布 `NativeRootFrame`；
5. exception、stack budget、termination 走显式 return/status 协议；
6. production code 不允许 Cranelift trap、tail call 或 unchecked trapping conversion；
7. 每个函数必须产生 target-matching unwind metadata；
8. object emission 后由 strict loader 校验 section、symbol、relocation、range 与 alignment，再发布 RX mapping。

`NATIVE_ABI_HASH` 覆盖 generated code 可见的 vmctx/CallArgs/frame layout、host/runtime operation signatures 与 value constants。任何布局或协议变化都必须改变 hash，并使旧 native cache miss。

### 2.1 值的机器表示（boxed i64 / typed f64）

函数内部的每个 SSA 值与帧局部各自绑定一个 Cranelift `Variable`，其机器类型由 `value_repr::ValueRepr` 规划：

- **boxed**（`types::I64`）是默认表示，持有 NaN-Box 编码值。所有 ABI 边界（函数参数与返回值、host dispatcher 参数、GC root frame 槽、resume live 槽）一律是 boxed i64，`NATIVE_ABI_HASH` 因此不受本节影响。
- **typed**（`types::F64`）持有**原始机器浮点位**，供 f64 分析可靠证明为 number 的值使用。逐条指令不再 `box_f64_result` / `bitcast`，循环携带的归纳变量整轮迭代常驻浮点寄存器。

提升资格：

- SSA 值必须属于**可靠**证明集合（`FunctionCompileInput::typed_f64_values`）。base 编译即静态分析结果；特化 overlay 只取入口 tag 守卫背书的种子分析结果，运行时反馈推测出的 number 留在 boxed 表示，由循环头守卫兜底。
- 在此之上，定义点还必须真的产出机器浮点数：`f64const`、`fadd`/`fsub`/`fmul`/`fdiv`、`fneg`、内联 Math builtin 与 typed math thunk 是 producer；φ 与一元 `+` 只是搬运，资格随源值；`LoadVar` 的资格随帧局部。**`Call` 的返回值一律不合格**——即便分析证明被调函数每条 `Return` 都返回 number，抛出路径仍按 ABI 返回 exception 编码（一个 NaN-Box 值），而 `IsException` 是调用点之后的独立指令，逃逸时的 NaN 规范化会把异常静默改写成 `NaN`。落到 host dispatcher 的算子（`%`、`**`、位运算）同理只有 boxed 结果。
- 帧局部还要求该名字被读过、被写过、**全部 `LoadVar` 的产出都已证明 f64**，且**全部 `StoreVar` 的源值本身 typed**。「全部 load 已证明」保护入口初值：入口把局部定义成 `undefined`（一个 NaN-Box 值），这条要求等价于「`variable_ssa` 解出的入口定义到不了任何 load」。「被写过」保护 GC：`boxed_frame_local_names` 只把「全部 store 都写入已证明 f64」的局部排除出 root 槽，零 store 的局部仍会被钉住，而 typed 局部写回的是原始浮点位。
- 含 `Suspend` / `GeneratorSuspend` 的函数整体退回 boxed：活跃值经宿主 continuation 以 boxed 形态往返。

转换点由 `value_repr` 的 `use_value_boxed` / `use_value_f64` / `define_value_boxed` / `define_value_f64` 统一收口，φ 边与 `StoreVar` 走 `use_value_as` + `define_value_as` 逐对选择表示，两端一致时不产出指令。**typed → boxed 必须规范化 NaN**：硬件默认 QNaN 的位模式与 `value::BOX_BASE` 相同，不规范化会被运行时误判成句柄。反方向只需 `bitcast`。

overlay 的循环头类型守卫对 typed 活跃值恒真，直接省略；它们进入 `store_resume_lives` 时按 boxed 规范化，deopt / OSR 对端读到的仍是合法 NaN-Box 值。

`NativeCompiler::compile_specialized_function` 只接受已验证 `Program`、目标函数、变量槽快照和实际参数 tag profile。wrapper 保持 `NativeSlowEntry` ABI，入口 tag 不符时读取 base function table 的 slow entry 回落；编译前克隆 `Program`，用与 AOT 相同的值类 / `typed_cfg` 按种子重建 CFG。命中时 number 参数从 call arena 进入重建后的 typed body。循环头类型 miss 调用 `DeoptToGeneric` 恢复 generic 循环头 live；generic 循环头在 `osr_entry` 非零时 OSR 进入 overlay body。typed body 继续复用 generic dispatcher、Shape IC、`RootPlan`、W^X 与 unwind 路径。

## 3. Native image 与 cache

`NativeImageRepository` 是 base image 内存去重与磁盘 cache 的唯一协调者，不是强生命周期 owner。磁盘路径来自调用方传入的 `cache_dir`；CLI / in-process 入口用 `wjsm-module` 的 `resolve_cache_dir()` 解析（`WJSM_CACHE_DIR` > `${XDG_CACHE_HOME}/wjsm` > `${HOME}/.cache/wjsm`，空串显式禁用）。`NativeCacheKey` 绑定：

- portable artifact digest；
- native ABI hash；
- native codegen source hash；
- 当前 target；
- Cranelift 版本；
- codegen/ISA settings。

同 key 的并发 prepare 由 in-flight gate 合并；repository 只保存 `Weak<CompiledImage>`，调用方持有的 `Arc` 决定 image 生命周期。未打开磁盘缓存时 miss 只编译不落盘。打开后 header/object/hash 或权限校验失败计为 invalidated 并重新编译，绝不能执行损坏 bytes。

磁盘缓存有自动 LRU 上限：每次 store 后节流检查目录总字节（顶层 `*.wnat` 与 `builtin_ir/*.bin` 一并统计），超过上限按 mtime 删除最旧条目直到低于上限。上限默认 256 MiB，`WJSM_CACHE_MAX_BYTES` 可覆盖；`0` 禁用自动淘汰。手动管理走 `wjsm cache stats / clear / prune --max-bytes N`。淘汰只删条目文件，不触碰同目录下的其它文件；删除与 `create_new + rename` 原子写入无竞态。

`CompiledImage` 拥有 executable mappings、entry table、反馈/IC buffer 与 unwind registration。base image 由 runtime `Arc` 持有；repository 只保存 `Weak`。特化 overlay 通过 `load_single_entry` 复用 strict relocation、RW→RX 与 unwind 注册，但不分配自己的反馈/IC buffer、不进入 repository 或磁盘 cache。drop 顺序必须先注销 unwind，再释放 mapping；function table 与选择表不得永久缓存裸 code pointer。

## 4. NativeRuntime ownership

`NativeRuntime` 包含：

- owner-thread 约束与 pinned `NativeVmContext`；
- 一个 `NativeAgentState`；
- 一个只持 `Weak<CompiledImage>` 的 `NativeImageRepository`；
- 一个按需启动 bounded worker 的 `SpecializationCoordinator`；
 - 一个 production `ManagedHeap`/`HandleTableV2`/`HeapAccessV2` owner；
 - 一个 production `ManagedHeap`/`HandleTableV2`/`HeapAccessV2` owner 与 `GenerationalZgc` collector；
 - module、Promise、continuation、worker、scheduler、snapshot、inspector 与 host side tables。

`NativeRuntime::execute` 的顺序是：

1. 校验 owner thread，重置输出并恢复 startup snapshot；
2. 由 repository 对 artifact 执行进程内命中、可选磁盘 load 或 direct compile；
3. 配置 module manifest、program/image registry、反馈槽与 source metadata；
4. 发布当前 base image 与 call/root/source frame；
5. 在 dispatcher owner 边界 drain 后台编译结果，经 strict loader 发布或丢弃 overlay；
6. `PrepareCall` 按 site/target/tag signature、IC epoch 与 prototype generation 选择 overlay 或 generic entry；
7. 调用 native entry，并由 activation `Arc` pin 住正在执行的 overlay；
8. drain Promise/microtask/external event loop；
9. materialize/传播 JS exception，关闭 child resources；
10. 返回 stdout/stderr/exit code/cache stats。

每个 worker/test262 agent 创建独立 runtime/heap/scheduler。跨 agent 仅通过 structured clone、SAB/Atomics 与 IPC 传递；不得读取另一 agent 的 handle 或共享 mutable runtime table。

## 5. ManagedHeap 与 GC 接合

Production heap 只有一个 owner：同一 layout、`HandleTableV2`、`HeapAccessV2<NativeHeapMemory>` 与 `GenerationalZgc` collector。ZGC 使用共享页、固定 worker pool、分代 mark/relocate 与 epoch reclaim。host side table 只保存 stable handle/generation，不保存可跨 safepoint 的 raw address。

Generated code 在 may-GC edge 发布 live boxed roots。runtime collector 的 strong closure 合并：

- native root frames；
- active call arena/activation/continuation；
- host side roots 与 weak/ephemeron/finalizer closure；
- ZGC barrier ring 中的 SATB/remembered references。

ZGC 的 mutator allocation 走 `HeapAccessV2` 的 NLAB/page allocator；分配压力请求 safepoint，worker 在 mutator 继续运行时执行 mark、稀疏页 relocation 与 handle epoch reclaim。显式 `gc()` 等待完整 `GenerationalZgc` cycle，不能回退到旧的全量搬迁 collector。

collection 后按 retired handle 清理 weak/side-table state。cooperative poll 与 `NewObject`、`NewArray`、`new Array(length)` 的 native object allocation 都只在 root frame 已发布且 raw access region 为空时触发 allocation-pressure collection；后三条路径在 `HeapExhausted` 时完整收集并重试一次。runtime 在执行开始前预建、冻结并显式作为 host root 的 OOM `RangeError` 及专用 exception side-table entry；仍不可恢复时直接返回该 entry，稳定抛出可捕获的 `RangeError("JavaScript heap out of memory")`，不从已满 managed heap 分配 error/prototype/message，也不随重复 OOM 增长 exception 表。这是 #337 受控堆上限契约在上述 object allocation 路径上的 native 终态；array growth、`allocate_array_values`、rest 参数与字符串 intern 不在本保证内。collector 选择在 runtime 初始化后不可切换。

## 6. 添加或修改运行时语义

按 owning layer 修改，禁止并行实现第二套路径：

1. 语言 lowering/early error：`wjsm-semantic`，同步 IR snapshot；
2. backend-independent 算法：`wjsm-builtins`/`wjsm-host`；
3. native object/Promise/module/I/O 状态：`wjsm-host-native` 对应 domain dispatcher；
4. generated-code ABI/lowering：`wjsm-native-abi` + `wjsm-backend-native`；
5. heap/handle/barrier/collector：`wjsm-gc`；
6. observable CLI contract：`wjsm-cli` + fixture。

新增 builtin/runtime operation 时，保持 wire ID 稳定、更新 required builtin/ABI hash 覆盖，并在 host dispatcher 实现完整异常与 reentry 语义。不能用 `fail_dispatch`、no-op、ignored fixture 或 fallback 代替缺失实现。

## 7. 平台实现规则

当前公开 production support：

- x86_64 Linux；
- x86_64 Windows。

平台模块负责 virtual memory、W^X、instruction-cache coherence 与 unwind register/unregister。交叉编译只证明 cfg/source 能编译，不等于真实执行通过。缺少实际 runner、AVX-512、大内存或多 NUMA 时，能力报告使用 `needs-capability-runner`。

本项目当前不配置 native GitHub Actions CI；平台证据来自真实宿主命令和 capability JSON。既有 mdBook 发布 workflow 与 native runtime 验证无关。

## 8. Native executable 与安全边界

`wjsm build --format native-executable` 把预链 `wjsm-exec` stub、canonical `.wjsm`、预编译 `NativeObject` 与制品内源码快照打成真实 ELF/PE。overlay 正文整层 zstd（ADR 0018）。打包失败不创建或覆盖目标文件。runtime 私有 relocatable object/image 不得单独包装成伪 executable。合同见 ADR 0016、ADR 0017、ADR 0018 与 ADR 0019。

Direct native code 不提供 Wasm sandbox。artifact verifier、checked lowering、strict relocation、symbol allowlist 与 W^X 是编译/加载 TCB，不是同进程不受信任代码隔离。运行不受信任程序必须使用独立 OS process、权限与资源限制。

## 9. 诊断与验证

先确定失败阶段：parse → lower → artifact → CLIF/object → image load/cache → host/runtime。

```bash
cargo run -- dump-ast -e 'const x = 1'
cargo run -- dump-ir -e 'const x = 1'
cargo run -- dump-clif -e 'const x = 1'
cargo run -- build -e 'console.log(1)' -o /tmp/hello.wjsm
cargo run -- validate /tmp/hello.wjsm
cargo run -- run /tmp/hello.wjsm
```

`WJSM_DISABLE_SPECIALIZATION=1` 用同一 binary 关闭反馈与 overlay，供 generic AOT 行为/性能对照。typed overlay 可通过 `NativeCompiler::specialized_diagnostics` 检查 wrapper tag guard、base `call_indirect` fallback 与 body 的 f64 指令；该诊断不加载或发布 image。

提交前至少执行与改动 owner 对应的窄测试。终态门包括：

```bash
cargo fmt --check
cargo check --workspace
cargo nextest run --workspace
cargo tree --workspace --edges normal,build
```

Lowering 变化需要 semantic IR snapshots；observable 行为需要 happy/errors/modules fixture。生成内容必须审阅，不得通过修改 expected 隐藏错误。

## 参考

- [ADR 0010](adr/0010-generational-zgc-managed-heap.md)
- [ADR 0014](adr/0014-direct-cranelift-portable-artifact.md)
- `crates/wjsm-artifact-format/src/lib.rs`
- `crates/wjsm-backend-native/src/lib.rs`
- `crates/wjsm-backend-native/src/cache.rs`
- `crates/wjsm-host-native/src/lib.rs`
- `crates/wjsm-native-abi/src/lib.rs`
