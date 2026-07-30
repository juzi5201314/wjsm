# 执行模型

wjsm 的执行分两步：先把整个程序编译成一个 WebAssembly 模块，再由宿主实例化并调用它的入口函数。这一章说明这两步各自做了什么，以及它对程序行为的实际影响。

## 编译在前，执行在后

`wjsm run app.ts` 内部依次完成：

1. 解析源码（按扩展名选择 JS / JSX / TS / TSX 语法）。
2. 语义降级：作用域分析、提升、TDZ 标记，产出 wjsm 自己的 IR。
3. 代码生成：IR 编译为 WebAssembly 字节。
4. 执行：Wasmtime 实例化模块，链接宿主函数，调用入口。

前三步在程序开始运行前全部完成。任何解析错误、早期错误（early error）都在第 4 步之前就报出来，此时程序还没有产生任何副作用。这与「边解析边执行」的引擎不同：

```bash
wjsm run -e 'console.log("first"); const x = ;'
```

这段代码不会打印 `first`——编译在执行之前就失败了，副作用还没来得及发生。

## 编译产物的形态

编译结果是一个 WebAssembly 模块，它 import 一组 wjsm 宿主函数（`console.log`、属性读写、对象分配等），export 入口函数。`wjsm build` 就是把这份字节码写到磁盘：

```bash
wjsm build -e 'console.log(1)' -o /tmp/one.wasm
wjsm size /tmp/one.wasm
```

对一个只有 `console.log(1)` 的程序，Import 与 Export 段合计约占产物体积的 88%，Code 段只有 8% 左右。原因是模块声明了完整的宿主 ABI 面，而不是只声明这段代码用到的部分。这也解释了为什么产物不能交给任意 WebAssembly 运行时执行——它需要 wjsm 宿主提供那些 import。

> <details><summary>产物体积为什么和代码量不成正比？</summary>
>
> 上面那个 `console.log(1)` 编译出的 `.wasm` 有 25 KB 左右，但用户的代码只有一条指令。原因不在代码生成，而在 ABI：每个 wjsm 产物都必须 import 约 500+ 个宿主函数（属性读写、对象分配、字符串处理、Promise 各种方法……），这些 import 的声明占掉 Import 段的绝大部分体积。
>
> 这是一笔固定开销——你的程序用了多少 import 不影响总体积，但必须全部声明。这是「产物不独立」的成本之一。要让 wjsm 产物跑起来，必须由 wjsm 宿主提供这整套 import。
>
> 想看自己的代码实际有多大？看 `wjsm size` 的 `Code` 和 `Data` 段；`Import` 和 `Export` 是固定开销，统计它没意义。
>
> </details>

## 值与对象在哪里

JavaScript 值统一用 64 位整数编码（NaN-boxing），可以直接放在 WebAssembly 的局部变量和栈上。对象、数组、字符串等有身份的数据放在一块托管堆里，值中保存的是句柄索引而非裸指针，垃圾回收器可以在不破坏引用的前提下移动对象。

变长参数与 GC safepoint 的溢出数据走一块独立的影子栈线性内存，冷启动 64 KiB，按需增长，软上限默认 16 MiB。

## 启动路径

宿主在实例化用户模块之前需要准备好全局对象、原型链、内置函数表。这部分状态在构建 wjsm 时就已固化为嵌入工件（启动快照与预编译的 support 模块），随二进制分发，因此常规启动不需要在用户机器上重建，也不依赖首次运行时的磁盘缓存。

启动快照默认开启，可以用 `WJSM_STARTUP_SNAPSHOT=0` 关掉来对比行为差异；关掉只影响启动耗时，不改变程序语义。

## 异步执行

`Promise`、`await`、定时器、I/O 都由宿主侧的调度器驱动。入口函数返回后，宿主继续排空微任务与宏任务队列，直到没有待处理工作才结束进程。这意味着 `setTimeout` 注册的回调不会因为主体代码执行完就被丢弃。

> <details><summary>「入口返回 ≠ 进程结束」对写代码的影响</summary>
>
> Node.js 里有 `process.exit()` 提前结束进程的常见写法，wjsm 也支持。区别在于：Node.js 退出前会丢掉所有未完成的异步任务，wjsm 的宿主会继续排空微任务和宏任务队列。
>
> 一个直接的影响是：如果你写了 `setTimeout(() => console.log("late"), 1000)` 然后主体代码跑完，Node 不会等这个回调（因为进程已经退出），但 wjsm 会等——所以「程序没正常退出」在 wjsm 里可能是异步任务还在排队，不是真的有 bug。
>
> 想让 wjsm 提前结束：用 `process.exit(n)`；否则就让它自己排空队列。
>
> </details>

## 深入了解

- [编译编排入口如何串联四个阶段](../../internals/pipeline/orchestration.md)
- [NaN-boxed 值表示的位布局](../../internals/backend/value-representation.md)
- [实例化与执行生命周期的宿主侧细节](../../internals/host-runtime/instantiation-and-lifecycle.md)
- [Promise、微任务与异步调度器的实现](../../internals/runtime-features/async-scheduler.md)
