# 新增 Host Import

这一章说明如何向 `wjsm-native-abi` 添加新的 host operation，并让 generated code 能调用它。

## 什么时候需要

host import 是 generated code 调用 runtime 的通道。新增以下内容时通常需要新的 host operation：

- 新的 `Builtin` variant 需要运行时支持；
- 新的 runtime operation（如新的 GC 接口）；
- 新的 source/debug frame 操作。

## 步骤

1. **ABI 注册**：在 `wjsm-native-abi` 添加 host symbol 定义。记录签名（参数类型、返回类型、是否 may-GC）。
2. **ABI hash 递增**：如果新增或修改了 host symbol 签名，`NATIVE_ABI_HASH` 必须递增，使旧 native cache miss。
3. **dispatcher 实现**：在 `wjsm-host-native` 的 host dispatcher 添加新 operation 的实现。实现必须包含完整的异常与 reentry 语义。
4. **语义层连接**：如果新 host operation 对应一个 `Builtin` variant，在 `wjsm-semantic/src/builtins.rs` 添加语义拦截，发射 `CallBuiltin(NewVariant)`。
5. **codegen 连接**：在 `wjsm-backend-native` 的 codegen 添加新 `Builtin` variant 的处理。
6. **required builtin 更新**：如果新 operation 是 artifact required builtin set 的一部分，更新 artifact verifier。
7. **测试**：添加 fixture 验证行为。

## 包装层规则

host dispatcher 函数是薄包装，职责是：

- 从 vmctx 取参数；
- 类型转换（NaN-box ↔ Rust 类型）；
- 调用 `wjsm-builtins` 的泛型算法；
- 把返回值编码回 NaN-box。

**不做语义决策**。所有逻辑在 `wjsm-builtins` 或 `wjsm-host-native` 的对应 domain dispatcher 里。不能用 `fail_dispatch`、no-op 或 fallback 代替缺失实现。

## 深入了解

- [Host Import 注册与包装层](../host-runtime/host-imports.md)
- [新增 Builtin](adding-builtins.md)
- [Import、Export 与 ABI](../backend/imports-exports-and-abi.md)
