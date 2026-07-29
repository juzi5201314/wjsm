# 项目定位与适用场景

wjsm 是一个 AOT 编译的 JavaScript/TypeScript 运行时。它先把源码编译成 WebAssembly，再执行编译产物，而不是边解析边解释执行。

## 与 V8 系运行时的差别

Node.js、Deno、Bun 都基于 V8：源码进引擎，由解释器起跑，热代码再由 JIT 逐层优化。wjsm 走另一条路：

| 环节 | V8 系运行时 | wjsm |
| --- | --- | --- |
| 语法解析 | V8 内置解析器 | `swc_core`，解析时就接受 TypeScript 语法 |
| 中间表示 | V8 字节码，引擎内部 | 自有语义 IR，可以用 `wjsm dump-ir` 打印 |
| 机器码 | 运行期 JIT，按热度分层 | 运行前一次性编译为 WebAssembly |
| 执行 | V8 | Wasmtime（Cranelift 或 Winch 编译 Wasm） |
| 对象堆 | V8 GC | 自有 ManagedHeap，GC 算法可选 |

带来的实际差别是：编译结果是一份可以单独保存的 `.wasm`，程序行为在执行前就定型；代价是没有运行期反优化和再优化，动态特性重的代码拿不到 JIT 的自适应收益。

## 适合用 wjsm 的场景

- 参与运行时开发。整条流水线都可以逐阶段 dump（`dump-ast`、`dump-ir`、`dump-wat`、`disasm`），阶段边界清晰。
- 验证「JS 编译到 Wasm」这条技术路线：想看某段语义降级成什么 IR、生成什么 Wasm 指令。
- 运行已被测试覆盖的程序。`fixtures/happy` 下 1300 多个用例是持续验证过的行为基线。
- 把 JS 逻辑编译成 `.wasm` 交给 Rust 宿主嵌入，见[作为 Rust 库嵌入](../workflows/embedding.md)。

## 不适合的场景

- 直接跑现成的 Node.js 应用。Node 内置模块只实现了一部分，`node_modules` 里任意包能不能跑取决于它用到的语义和 API 是否已覆盖。
- 需要完整 ECMAScript 一致性的场合。Test262 通过率就是当前真实水位，不要按「应该支持」来推断。
- 想把 `.wasm` 交给任意 WebAssembly 运行时独立执行。产物依赖 wjsm 的宿主 ABI 和 support module，细节见 [WASM 产物与宿主要求](../output/wasm-artifacts.md)。
- 依赖 `--target jit`。这个后端只有静态接入契约，没有实现。

## 版本状态

`wjsm version --extended` 会打印当前二进制的版本、Rust edition、Git 提交和默认后端：

```text
wjsm 0.1.0
  Edition: 2024
  Git: 694e72d6
  Target: wasm
```

`0.1.0` 意味着 CLI 参数、配置键、`.wasm` 产物 ABI 都可能在小版本间变化。产物不要跨 wjsm 版本复用，重新编译即可。

## 深入了解

- [项目目标与非目标](../../internals/foundations/goals-and-non-goals.md)：哪些能力被明确排除在设计之外，以及原因。
- [多后端完全支撑契约](../../internals/backend/multi-backend-boundary.md)：JIT 后端留下的接入点长什么样。
