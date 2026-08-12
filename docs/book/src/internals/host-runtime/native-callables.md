# NativeCallable 注册表

`Builtin` enum 在语义层定义，`NativeCallable` 在后端把它映射到具体的运行时函数。这一章说明注册表的工作方式。

## 从 Builtin 到 NativeCallable

```
语义层                    后端                      运行时
Builtin::ArrayMap  →  NativeCallable::ArrayMap  →  builtins::array_map::<E>
```

`wjsm-semantic/src/builtins.rs` 识别已知调用形态并发射 `Instruction::CallBuiltin(Builtin::Xxx)`。`wjsm-backend-native` 在 codegen 时查 `NativeCallable` 注册表得到 host operation 的 wire ID。运行时 host dispatcher 把 wire ID 路由到 `wjsm-builtins` 的泛型算法。

三层各司其职：语义层决定「这是什么操作」，后端决定「调哪个函数」，builtins 决定「怎么执行」。

## 新增 NativeCallable

1. 在 `wjsm-ir` 或 `wjsm-semantic` 的 `Builtin` enum 添加新 variant。
2. 在 `wjsm-native-abi` 注册对应的 host symbol 和签名。
3. 在 `wjsm-host-native` 的 host dispatcher 实现完整异常与 reentry 语义。
4. 在 `wjsm-builtins` 的对应域文件添加算法函数。
5. 更新 required builtin / ABI hash 覆盖。

## wire ID 稳定性

`NativeCallable` 的 wire ID 在 artifact 和 native image 之间共享。已有的 wire ID 不能重新分配给其他 operation——只能追加新的。修改 wire ID 必须递增 `NATIVE_ABI_HASH`，使旧 native cache 失效。

## 深入了解

- [核心 JavaScript Builtins](javascript-builtins.md)
- [Host Import 注册与包装层](host-imports.md)
- [Import、Export 与 ABI](../backend/imports-exports-and-abi.md)
- [新增 Builtin](../development/adding-builtins.md)
