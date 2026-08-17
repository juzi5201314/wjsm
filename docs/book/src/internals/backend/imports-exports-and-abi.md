# Import、Export 与 ABI

这一章说明 `wjsm-native-abi` 定义的 vmctx、host symbol 与 call/root/source frame 契约。

## vmctx

vmctx（virtual machine context）是 generated code 与 runtime 之间的桥梁。它是一块固定宽度的内存，由 `wjsm-native-abi` 定义布局。generated code 通过 `*mut NativeVmContext` 访问：

- host symbol 与 `HostOperationDispatcher`；
- call arena、function table、当前 image；
- GC / 分配 / 屏障状态指针；
- handle table、IC 与反馈区基址。

`native_abi_hash()` 覆盖 vmctx 布局、root/source/call frame 与 host symbol 签名。任何布局或协议变化都会换 hash，使旧 native cache miss。

## 函数入口与 CallArgs

`NativeSlowEntry` 定义在 `wjsm-native-abi`：

```text
(ctx: *mut NativeVmContext, env: i64, this_value: i64, args_base: u32, args_count: u32) -> i64
```

`CallArgs`（`wjsm-host`）是 call arena 上的一段连续槽：`base: u32` + `len: u32`。IR 形参 `$env` / `$this` 对应这里的 `env` / `this_value`。返回值是 NaN-box `i64`；异常带 `TAG_EXCEPTION`。

## Root frame 与 source frame

| Frame | 用途 |
| --- | --- |
| `NativeRootFrame` | may-GC 点发布，列出活跃 boxed roots |
| `NativeSourceFrame` | 运行时错误堆栈映射，携带 `SourceSpan` |

root frame 在 safepoint 时由 collector 读取。source frame 在运行时错误发生时读取，把 native code 位置映射回源码行列。

## Host symbol allowlist

generated code 只能调用 `wjsm-native-abi` 注册的 host symbol。strict relocation 阶段校验所有外部调用指向 allowlist 内的 symbol，拒绝未注册的符号引用。这是受信 TCB 的一部分。

## 深入了解

- [Native ABI 索引](../reference/abi-index.md)
- [编译器内部结构](compiler-architecture.md)
- [活跃性、槽位与 GC Spill](liveness-slots-and-spills.md)
- [Direct Cranelift 后端概览](README.md)
