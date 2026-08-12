# Normal 与 Eval 编译模式

Direct native 后端有两种编译模式：normal 和 eval。两者产出的 native image 都由 `NativeRuntime` 执行，但入口约定和上下文不同。

## 何时用哪种

| 模式 | 触发 | 上下文 |
| --- | --- | --- |
| normal | `wjsm run`、`wjsm build`、`wjsm test` | 独立 vmctx，完整 bootstrap |
| eval | `eval()`、`new Function()`、`node:vm.Script` | 共享当前 realm 的 vmctx |

eval 模式的代码在运行时编译，不是编译期预知的。它走完整的解析→lowering→CLIF→native 路径，但 image 挂载到当前 realm 而非新建独立 runtime。

## 差异

| 维度 | Normal | Eval |
| --- | --- | --- |
| vmctx | 独立 | 共享当前 realm |
| 入口签名 | 标准 `NativeCallable` | eval entry，接受 caller scope |
| 局部变量 | 独立 | 不能访问编译期主模块的局部变量 |
| 全局对象 | 当前 realm 的全局 | 当前 realm 的全局 |
| 编译缓存 | 按 artifact digest 缓存 | 按源码 hash 缓存 |

eval 编译的 native image 会进入编译缓存。相同源码的重复 `eval` 不需要重新编译，但首次 `eval` 比普通启动慢——它要走完整编译流程。

## 性能特征

`eval` 的编译是运行时开销。100 行的 eval 代码编译耗时约 50–200ms（release 构建）。如果 `eval` 在热路径里频繁调用，应把动态部分数据化（参数传入），而不是代码化（eval 字符串）。

> <details><summary>为什么 eval 不用解释器？</summary>
>
> wjsm 是 AOT 运行时，没有 AST 解释器或字节码 VM。eval 走的编译路径和主模块完全一致——解析、lowering、CLIF、native image——只是触发时机在运行时。
>
> 这意味着 eval 出来的代码和 AOT 编译的代码用同一套执行路径，没有解释器和 JIT 的兼容性问题。代价是每次 eval 都有编译延迟。
>
> 实际影响：如果 eval 一次会运行很久（比如编译一个函数然后反复调用），wjsm 的开销可以忽略。如果 eval 频繁调用短代码，编译延迟会主导。
>
> </details>

## 深入了解

- [编译器内部结构](compiler-architecture.md)
- [动态代码、Eval 与解释器](../runtime-features/dynamic-code.md)
- [Portable artifact 边界](../../../../backend-implementation-guide.md)
