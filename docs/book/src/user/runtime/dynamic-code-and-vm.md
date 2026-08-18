# 动态代码与隔离上下文

wjsm 是 AOT 编译器，但运行时保留了一条编译通道，`eval`、`new Function` 和 `node:vm` 都走这条路径。

## eval 与 new Function

两者都在运行时把源码编译为当前宿主的 native image 再执行：

```bash
wjsm run -e 'console.log(eval("1 + 2"))'
wjsm run -e 'const double = new Function("a", "return a * 2"); console.log(double(21))'
```

代价与普通启动不同：每次动态编译都要走完解析、lowering、codegen，属于运行时开销，不适合放在热路径里。

> <details><summary>动态 `eval` 编译都走什么流程？</summary>
>
> wjsm 的 eval **不是** AST 解释器——它把字符串编译成与普通入口相同的 native 执行路径。流程：
>
> 1. 解析（`swc_core`）
> 2. 语义 lowering（生成 IR）
> 3. Cranelift 编译为 native image
> 4. 在当前 `NativeRuntime` / Realm 中调用入口
>
> 这意味着每次 `eval` 都比 Node 的解释执行更重。如果代码 eval 一次会运行很久，编译开销可以忽略；如果 eval 频繁调用，就该把动态部分数据化（参数传入），而不是代码化（eval 字符串）。
>
> </details>

## node:vm 多上下文

`node:vm` 提供独立的全局环境，用于执行不该看到宿主变量的代码：

```js
// vm-demo.mjs
import vm from "node:vm";

console.log(vm.runInNewContext("1 + 41"));

const context = { x: 1 };
vm.createContext(context);
console.log(vm.runInContext("x + 1", context));
```

```bash
wjsm run vm-demo.mjs
```

输出 `42` 和 `2`。每个上下文是一个独立 Realm，拥有自己的全局对象和内置构造器，但和主程序共用同一个托管堆，因此对象可以跨上下文传递。

## 注意 inline 模式的限制

`-e` 传入的代码按单文件编译，不解析 `import` 声明。要用 `node:vm` 这类模块，请写成文件再运行，参见[文件、内联源码与标准输入](../getting-started/input-modes.md)。

## 深入了解

- [运行时 Eval 通道](../../internals/runtime-features/dynamic-code.md)
- [`node:vm` 单堆多 Realm 的实现与隔离边界](../../internals/runtime-features/node-vm.md)
- [Eval 编译模式与 Normal 模式的代码生成差异](../../internals/backend/normal-and-eval-modes.md)
