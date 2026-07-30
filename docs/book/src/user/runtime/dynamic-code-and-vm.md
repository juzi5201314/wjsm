# 动态代码与隔离上下文

wjsm 是 AOT 编译器，但运行时保留了一条编译通道，`eval`、`new Function` 和 `node:vm` 都走这条路径。

## eval 与 new Function

两者都在运行时把源码编译成 WASM 再执行：

```bash
wjsm run -e 'console.log(eval("1 + 2"))'
wjsm run -e 'const double = new Function("a", "return a * 2"); console.log(double(21))'
```

代价与普通启动不同：每次动态编译都要走完解析、lowering、codegen，属于运行时开销，不适合放在热路径里。

> <details><summary>动态 `eval` 编译都走什么流程？</summary>
>
> wjsm 的 eval **不是** AST 解释器——它真的把字符串编译成 WASM。流程：
>
> 1. 解析（`swc_core`）
> 2. 语义 lowering（生成 IR）
> 3. WASM 编译
> 4. 实例化到当前 Store
> 5. 调用入口函数
>
> 这意味着每次 `eval` 都比 Node 慢——Node 是「解释执行」或「JIT 编译」，而 wjsm 是「全流程 AOT 编译」。但 wjsm 的优势是「eval 出来的代码和 AOT 编译的代码用同一套执行路径」，没有解释器和 JIT 的兼容性问题。
>
> 经验值：100 行的 eval 代码在 wjsm 下大概 50-200ms 编译耗时。Node 的等量代码解释执行是微秒级。如果代码 eval 一次会运行很久，wjsm 的开销可以忽略；如果 eval 频繁调用，就该重新设计——把动态部分数据化（参数传入），而不是代码化（eval 字符串）。
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

活跃 Realm 数量上限默认 1024，可用 `WJSM_VM_MAX_REALMS` 调整。

## 注意 inline 模式的限制

`-e` 传入的代码按单文件编译，不解析 `import` 声明。要用 `node:vm` 这类模块，请写成文件再运行，参见[文件、内联源码与标准输入](../getting-started/input-modes.md)。

## 深入了解

- [运行时 Eval 通道与解释执行路径](../../internals/runtime-features/dynamic-code.md)
- [`node:vm` 单堆多 Realm 的实现与隔离边界](../../internals/runtime-features/node-vm.md)
- [Eval 编译模式与 Normal 模式的代码生成差异](../../internals/backend/normal-and-eval-modes.md)
