# Direct Cranelift 后端

wjsm 当前只有一个生产执行后端：`wjsm-backend-native` 负责把 verified semantic IR 降为 Cranelift IR、生成 relocatable native object，并发布经过验证的 native image；`wjsm-host-native` 负责在 owner thread 上执行它。

## 代码链路

```text
PortableArtifact
  -> NativeCompiler::compile
  -> CLIF functions / host operation calls
  -> object emission
  -> strict relocation + W^X + unwind registration
  -> CompiledImage
  -> NativeRuntime::execute
```

`NativeCompiler::new()` 用当前宿主 ISA 初始化。当前生产 capability 是 x86_64 Linux 与 x86_64 Windows；不支持 target 直接返回 capability error。

## ABI 与 cache

`wjsm-native-abi` 定义 vmctx、CallArgs、root/source frame、host symbol 与 value layout。`NATIVE_ABI_HASH`、artifact semantic ABI、codegen source hash、target、Cranelift 版本和 settings 共同决定 native cache key。

`.wjsm` 不包含 image 或 relocation。损坏/stale cache 只能 invalidated 后重编译，不能作为 fallback 执行。

## 运行时 owner

`NativeRuntime` 持有 pinned `NativeVmContext`、`NativeAgentState`、ManagedHeap、collector、scheduler、module/Promise/worker/inspector tables 与 `NativeImageRepository`。它带 owner-thread 检查且不可跨线程共享。每个 agent 独立拥有 mutable runtime state；跨 agent 使用 structured clone、SAB/Atomics 或 IPC。

## 维护规则

- 新语义算法放在 `wjsm-builtins`/`wjsm-host`，不把 native types 传播到 backend-independent crate。
- native side table 只保存 handle/generation，不保存跨 safepoint raw address。
- may-GC call 通过 root frame 和 ManagedHeap protocol；不能新增无 epoch 的地址缓存或第二 handle table。
- codegen changes 必须配 CLIF、image lifecycle、relocation/unwind/W^X 与 observable fixture evidence。
- `--format native-executable` 是明确的 NotImplemented contract；runtime-private image 不伪装成用户 executable。

完整 owner、artifact 与安全边界见 [后端实现指南](../../../../backend-implementation-guide.md) 与 [ADR 0014](../../../../adr/0014-direct-cranelift-portable-artifact.md)。
