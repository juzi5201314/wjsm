# 动态代码、Eval 与解释器

这一章说明 `eval`、`Function` 构造器和 `node:vm` 的动态编译路径。

## Eval 模式编译

后端有两种编译模式：normal 和 eval（见[Normal 与 Eval 编译模式](../backend/normal-and-eval-modes.md)）。eval 模式用于运行时生成的代码：

- `eval(source)` 走 `dispatch/eval.rs`。
- `new Function(...)` 通过类似路径编译函数体。
- `node:vm.Script` 和 `vm.SourceTextModule` 编译动态源码。

eval 代码与主模块共用同一个 `NativeRuntime` 和 `ManagedHeap`。它能看见当前 realm 的全局对象，但不能看见编译期主模块的局部变量——那些绑定不在 eval 的作用域记录里。没有单独的 WASM module，也没有 Store instantiate。

## 宿主路径

`execute_eval_script`（`dispatch/modules.rs`）接收源码，经 `lower_eval_module_with_scope_and_strict` 得到 IR，打成 `PortableArtifact`，再由 `NativeImageRepository::prepare` 交给 `NativeCompiler` 生成 native image 并执行。结果是 NaN-box `i64`。

磁盘缓存可用时，相同 IR digest 的重复 eval 可以命中 native image 磁盘缓存。

## 动态 import

`import(specifier)` 是动态 ESM 加载。`modules.rs` 解析 specifier，加载源码，同样编成 native image。返回 Promise，resolve 时给出模块的 namespace 对象。

动态 import 的模块如果已经在当前 realm 加载过，直接返回缓存的 namespace 对象。模块缓存按 realm 隔离，仍在同一堆上。

## 限制

wjsm 的 eval 是 AOT 编译，不是解释执行。每次 eval 都走 `NativeCompiler`，没有 AST 解释器 fallback。这与 V8 的解释器 + JIT 不同，但对 AOT 运行时是唯一路径。

## 深入了解

- [Normal 与 Eval 编译模式](../backend/normal-and-eval-modes.md)
- [模块加载与执行上下文](module-loading.md)
- [编译缓存](../startup/compilation-cache.md)
