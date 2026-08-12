# Import、Export 与 ABI

这一章说明 `wjsm-native-abi` 定义的 vmctx、host symbol 与 call/root/source frame 契约。

## vmctx

vmctx（virtual machine context）是 generated code 与 runtime 之间的桥梁。它是一块固定宽度的内存，由 `wjsm-native-abi` 定义布局。generated code 通过 vmctx 访问：

- host operation 函数表；
- 当前 realm 的 intrinsics（原型句柄、全局构造器）；
- GC 状态（堆指针、分配指针、触发阈值）；
- shadow stack 指针。

`NATIVE_ABI_HASH` 覆盖 vmctx 布局、CallArgs、root/source frame 与 host symbol 签名。任何布局或协议变化都必须改变 hash，使旧 native cache miss。

## CallArgs

`CallArgs` 是函数调用的参数布局，定义在 `wjsm-native-abi`：

```text
(receiver: i64, arg: i64, arg_count: i32, flags: i32) -> result: i64
```

`JS_FUNC_TYPE_INDEX = 12` 是函数调用的标准类型索引。所有 JS 函数调用走同一签名——receiver 是 this 值，arg 是参数区域的起始指针，arg_count 是参数个数，flags 携带 `new` / `super()` 等标记。

## Root frame 与 source frame

| Frame | 用途 |
| --- | --- |
| `NativeRootFrame` | may-GC 点发布，列出活跃 boxed roots |
| `NativeSourceFrame` | 运行时错误堆栈映射，携带 `SourceSpan` |

root frame 在 safepoint 时由 collector 读取。source frame 在运行时错误发生时读取，把 native code 位置映射回源码行列。

## Host symbol allowlist

generated code 只能调用 `wjsm-native-abi` 注册的 host symbol。strict relocation 阶段校验所有外部调用指向 allowlist 内的 symbol，拒绝未注册的符号引用。这是受信 TCB 的一部分。

## 深入了解

- [WASM 与 Host ABI 索引](../reference/abi-index.md)
- [编译器内部结构](compiler-architecture.md)
- [活跃性、槽位与 GC Spill](liveness-slots-and-spills.md)
- [Direct Cranelift 后端概览](README.md)
