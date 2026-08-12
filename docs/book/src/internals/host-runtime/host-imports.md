# Host Import 注册与包装层

这一章说明 host operation 的注册机制和包装层设计。

## 包装层职责

generated code 通过 vmctx 调用 host operation。包装层（host call dispatch）的职责是：

1. 从 vmctx 取出调用参数；
2. 做 NaN-box 值与 Rust 类型的类型转换；
3. 调用 `wjsm-builtins` 的泛型算法；
4. 把返回值编码回 NaN-box 值。

包装层**不做语义决策**。所有逻辑在 `wjsm-builtins` 的算法里，包装层只负责类型转换和异常传播。

## 注册表

`wjsm-native-abi` 定义 host symbol 注册表。每个 host operation 有一个稳定的 wire ID，编译器和 runtime 共用。`NATIVE_ABI_HASH` 覆盖这些 ID 和签名。

新增 builtin 或 runtime operation 时：

1. 在 `wjsm-ir` 添加 `Builtin` 或 `NativeCallable` variant。
2. 在 `wjsm-native-abi` 注册 host symbol 和签名。
3. 在 `wjsm-host-native` 的 host dispatcher 实现完整异常与 reentry 语义。
4. 更新 required builtin / ABI hash 覆盖。

不能用 `fail_dispatch`、no-op、ignored fixture 或 fallback 代替缺失实现。

## 异常与再入

host operation 可能在执行过程中触发 GC 或 reentry。包装层负责：

- 在 may-GC 点发布 `NativeRootFrame`；
- 在 reentry 时恢复正确的 realm 和 scope；
- 把算法抛出的异常编码为 `TAG_EXCEPTION` 值返回。

## 深入了解

- [ExecContext 与 Builtins 的解耦设计](exec-context-and-builtins.md)
- [Import、Export 与 ABI](../backend/imports-exports-and-abi.md)
- [新增 Builtin](../development/adding-builtins.md)
