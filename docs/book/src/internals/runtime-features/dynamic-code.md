# 动态代码、Eval 与解释器

这一章说明 `eval`、`Function` 构造器和 `node:vm` 的动态编译路径。

## Eval 模式编译

后端有两种编译模式：normal 和 eval（见[Normal 与 Eval 编译模式](../backend/normal-and-eval-modes.md)）。eval 模式用于运行时生成的代码：

- `eval(source)` 调用 `runtime_eval.rs` 的 host 函数。
- `new Function(...)` 通过类似路径编译函数体。
- `node:vm.Script` 和 `vm.SourceTextModule` 编译动态源码。

eval 模式编译产出的 WASM 可以访问当前 realm 的全局对象和 support module，但不能访问编译期主模块的局部变量——eval 代码与主模块在独立的 WASM module 里。

## `runtime_eval.rs`

这个文件实现 eval 的 host 侧：接收源码字符串，调用 `compile_source` 编译为 WASM，实例化到当前 realm，执行入口函数。结果通过 NaN-box 值返回。

eval 编译的 WASM 会进入编译缓存（见[编译缓存](../startup/compilation-cache.md)），相同源码的重复 eval 不需要重新编译。

## 动态 import

`import(specifier)` 是动态 ESM 加载。`modules.rs` 解析 specifier，加载源码，编译为 WASM，实例化。返回 Promise，resolve 时给出模块的 namespace 对象。

动态 import 的模块如果已经在当前 realm 加载过，直接返回缓存的 namespace 对象。模块缓存按 realm 隔离。

## 限制

wjsm 的 eval 是 AOT 编译，不是解释执行。每次 eval 都调用 `compile_source` 生成 WASM，没有 AST 解释器 fallback。这与 V8 的解释器 + JIT 不同，但对 AOT 运行时是合理的设计。

## 深入了解

- [Normal 与 Eval 编译模式](../backend/normal-and-eval-modes.md)
- [模块加载与执行上下文](module-loading.md)
- [编译缓存](../startup/compilation-cache.md)
