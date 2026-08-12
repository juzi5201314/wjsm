# 修改 Native ABI

这一章说明修改 `wjsm-native-abi` 定义的 vmctx、CallArgs、root/source frame 或 host symbol 时需要改动哪些地方。

## 影响范围

ABI 变更影响面广——它跨越编译器和运行时：

| 改动 | 影响范围 |
| --- | --- |
| vmctx 布局 | 所有 generated code 的 vmctx 访问 |
| CallArgs 签名 | 所有函数调用点 |
| Root frame 布局 | GC root scanning |
| Source frame 布局 | 运行时错误堆栈映射 |
| Host symbol 签名 | host dispatcher 路由 |
| Host symbol ID | artifact required builtin + cache key |

## 必须同步更新的位置

1. **`wjsm-native-abi`**：定义新布局/签名。
2. **`NATIVE_ABI_HASH`**：递增 hash，使旧 native cache 全部 miss。这是硬性要求——不递增 hash 会导致旧 cache 中的 image 与新 ABI 不兼容，运行时行为不可预测。
3. **`wjsm-backend-native`**：codegen 发射新布局的代码。
4. **`wjsm-host-native`**：host dispatcher 按新签名路由。
5. **`wjsm-gc`**：如果 root frame 布局变化，collector 的 root scanning 更新。
6. **artifact verifier**：如果 required builtin set 变化，更新 verifier。
7. **fixture**：验证端到端行为。

## 不能做的事

- 不能复用已分配的 host symbol ID 给不同的 operation。只能追加新的 ID。
- 不能在不递增 `NATIVE_ABI_HASH` 的情况下修改布局——旧 cache 不会自动失效。
- 不能在 portable `.wjsm` 中嵌入 native ABI 专有信息——artifact 必须保持 target-independent。

## 深入了解

- [Import、Export 与 ABI](../backend/imports-exports-and-abi.md)
- [WASM 与 Host ABI 索引](../reference/abi-index.md)
- [核心不变量](../reference/invariants.md)
- [新增 Host Import](adding-host-imports.md)
