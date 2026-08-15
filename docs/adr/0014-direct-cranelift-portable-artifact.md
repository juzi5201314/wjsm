# ADR 0014: Direct Cranelift 与 portable `.wjsm` 终态

## Status

Accepted（2026-08-12）

Supersedes ADR 0011、ADR 0012 与 ADR 0013 中关于 Wasmtime runtime、Wasm backend、JIT stub、CLI backend selector 和多生产后端的决策。其后端无关语义、host 契约与 GC 分层继续有效。

## Context

ADR 0011–0013 建立了后端无关的 host、builtins、GC 与 `JsBackend` 边界，但生产执行仍依赖 Wasmtime/Wasm，JIT 只是未实现扩展点。该结构存在两套制品语义、重复 runtime owner 和从 semantic IR 经 Wasm 再进入 Cranelift 的间接路径。

项目已经完成一次性切换：SWC AST 降级为 verified semantic IR，构建生成 portable `.wjsm`，当前宿主再将其直接编译为 CLIF/native image，并由 `NativeRuntime` 执行。旧 Wasm backend、host、runtime bridge、JIT selector 与 cwasm 路径已经删除。

## Decision

### 1. 唯一生产编译链

生产编译链固定为：

```text
JS/TS source
  -> SWC parse
  -> semantic lowering + IR verification
  -> portable .wjsm
  -> direct IR -> CLIF
  -> relocatable native object
  -> verified executable image
  -> NativeRuntime
```

`wjsm-backend-native` 是唯一 native compiler/image/cache owner。运行时特化也只从同一份 verified semantic IR 调用该 compiler，生成进程内派生 overlay；它不是解释器、`cranelift-jit` 或第二执行后端。项目不保留 Wasm、解释器或 JIT fallback，也不暴露 backend selector。

### 2. `.wjsm` 是唯一跨平台用户制品

`PortableArtifact` 包含 semantic IR、module manifest、required builtins 与可选 source metadata。编码必须确定、带 semantic ABI/hash，并在 decode 后执行 limits、section、cross-reference 与 IR verification。

`.wjsm` 不包含机器码、Cranelift object、relocation、宿主指针或 native cache key。它可以在支持平台间携带；机器码只能由当前宿主派生。

### 3. Native image 与 cache 是 runtime 私有派生数据

`NativeImageRepository` 以 artifact digest、native ABI hash、native codegen source hash、target、Cranelift 版本和 codegen settings 组成 key。repository 只持有 `Weak<CompiledImage>`，由 runtime 的 `Arc` 决定 base image 生命周期；磁盘 cache 只保存当前宿主派生对象。校验失败的 cache 被 invalidated 后重编译，不能执行损坏字节。当合并 Program 含 `$builtin_main` 时，runtime 派生两份 image（builtin 段按 frontier IR digest，用户段按用户函数子 Program digest）。

热调用点的反馈达到稳定阈值后，后台 worker 仍通过 `NativeCompiler` 从同一 verified `Program` 编译 typed wrapper/body；owner thread 只在 dispatcher 边界用 `CompiledImage::load_single_entry` 完成 relocation、RW→RX 与 unwind 注册后发布。overlay 不进入 artifact digest、`.wjsm`、repository 或磁盘 cache；每调用点最多两个版本，全 agent 同时受 64 个 overlay 与 16 MiB code+rodata 上限约束。LRU 淘汰只移除选择表中的 `Arc`，正在执行的 activation 继续 pin mapping；`CompiledImage` drop 必须先注销 unwind 再释放 mapping。

### 4. `NativeRuntime` 是唯一运行时 owner

每个 agent 拥有独立的 pinned `NativeVmContext`、ManagedHeap、handle table、collector、scheduler、module/Promise/object side tables、反馈槽和 `SpecializationCoordinator`。后台 worker 不接触 runtime/GC/raw pointer；编译结果、失效与 RX overlay 发布只由 owner thread 处理。跨 agent 只允许 structured clone、SAB/Atomics 和显式 IPC 协议，不共享 GC handle 或 mutable runtime owner。

GC 可在 Mark-Sweep、G1、ZGC 中启动时选择；root frame、host roots、weak/ephemeron closure、allocation-pressure safepoint 与 telemetry 由同一 native owner 接合。Shape/IC epoch 或 prototype generation 变化会使对应 overlay 退出选择表，当前调用继续 generic；`WJSM_DISABLE_SPECIALIZATION=1` 只关闭反馈与 overlay，不改变 generic AOT、IC 或语义路径。

### 5. 平台能力 fail-closed

当前 production capability 只承诺 64-bit x86_64 Linux 与 x86_64 Windows。`NativeCompiler::new` 使用当前宿主 ISA，并在不支持的 target、缺 W^X/unwind/虚拟内存等必要能力时返回结构化 capability error。

交叉编译或 object emission 不等于真实平台执行证据。缺少实际 runner、AVX-512、大内存或多 NUMA 时，相关验证报告必须标记 `needs-capability-runner`，不能 skip-as-pass。

### 6. Platform native executable AOT 不在当前范围

`wjsm build --format native-executable` 返回稳定的 `native executable output is not implemented` 错误、退出码 1，并且不创建或覆盖输出文件。runtime 私有 relocatable object/native image 不构成用户可分发 executable。

### 7. 安全边界

Direct native code 不具备 Wasm memory/control-flow sandbox。artifact verifier、checked lowering、empty-trap gate、symbol allowlist、strict relocation 与 W^X 属于受信编译/加载 TCB，但不提供同进程不受信任代码隔离。

需要运行不受信任代码时，调用方必须使用独立 OS process、权限隔离与资源限制；不得把宿主秘密与不受信任程序放在同一 runtime process 后宣称安全隔离。

## Consequences

- 编译路径和 runtime ownership 唯一，删除了 Wasm/JIT 双轨、兼容桥和 backend selector。
- portable artifact 与 native image 生命周期分离：前者可分发，后者可丢弃重建。
- `wjsm-builtins`、`wjsm-host`、`wjsm-gc`、`wjsm-module` 继续保持后端无关；Cranelift/object/platform 依赖只进入 native backend/host。
- 新执行后端不再是当前 public extension contract。若未来重新引入，必须通过新的 ADR 定义 artifact、runtime owner、CLI 与完整语义证据，不能复活旧 fallback。
- 当前唯一明确未实现的用户能力是 platform native executable AOT。

## Verification

- portable artifact canonical encode/decode、limits、hash 与 semantic ABI tests。
- CLIF lowering、trap-free gate、relocation、W^X、unwind、cache corruption/lifecycle tests。
- native runtime 的同步/异步/module/snapshot/inspector/worker/GC fixture 与 workspace full suite。
- production source/manifests/dependency bounded scan 不含旧 Wasm/Wasmtime/JIT owner；唯一允许的 `wasmtime-internal-*` 是 Cranelift 依赖图强制携带的 `wasmtime-internal-core` 通用 math leaf。
- x86_64 Linux 真实 build/run；x86_64 Windows 零警告编译。缺少真实平台执行能力的组合保持 `needs-capability-runner`。

## References

- ADR 0010 — Generational/ZGC ManagedHeap
- ADR 0011 — Runtime 按后端无关性拆分（历史）
- ADR 0012 — Host builtins 后端解耦（历史）
- ADR 0013 — 多后端完全支撑契约（历史）
- `docs/backend-implementation-guide.md`
