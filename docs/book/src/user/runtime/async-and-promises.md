# 异步任务与 Promise

wjsm 有完整的事件循环：微任务队列、定时器、`process.nextTick`，以及 `async`/`await`。程序在所有待处理任务耗尽后退出，不需要显式保持进程存活。

## 执行顺序

```bash
wjsm run -e 'setTimeout(() => console.log("timeout"), 0);
  queueMicrotask(() => console.log("micro"));
  console.log("sync")'
```

```text
sync
micro
timeout
```

同步代码先跑完，然后是微任务，最后才是定时器回调。这与 ECMAScript 和 Node.js 的任务优先级一致。

`process.nextTick` 在当前操作完成后、微任务之前执行：

```bash
wjsm run -e 'process.nextTick(() => console.log("tick")); console.log("sync")'
```

```text
sync
tick
```

> <details><summary>`process.nextTick` vs `queueMicrotask`：为什么顺序不一样？</summary>
>
> Node.js 历史包袱。`process.nextTick` 比 Promise 微任务还早存在——Node 在 0.x 时代就有这个 API，比 Promise 普及得早。微任务（Promise 用的）后来才被加进 V8/Node 生态。
>
> 实际优先级：`process.nextTick` > Promise 微任务 > `queueMicrotask` 回调 > 定时器 > I/O。Node 官方称之为「nextTick queue 和 microtask queue 是两个独立队列」。
>
> wjsm 忠实复现了这个顺序，因为有些老代码依赖「在所有 Promise 之前执行」的特性——比如 `process.nextTick` 经常被用来「在当前代码结束后、下一次事件循环前」插入一段逻辑。改了顺序就会破坏这些代码的预期。
>
> </details>

## Promise 组合器

`Promise.all`、`Promise.allSettled`、`Promise.race`、`Promise.any` 和 `Promise.withResolvers` 都可用：

```bash
wjsm run -e 'Promise.allSettled([Promise.reject(new Error("x")), 1])
  .then(r => console.log(r[0].status, r[1].status))'
```

```text
rejected fulfilled
```

## 未处理的拒绝

未被捕获的 Promise 拒绝会打印警告，但不会改变退出码：

```bash
wjsm run -e 'new Promise((_, reject) => reject(new Error("boom")))'
```

```text
UnhandledPromiseRejectionWarning: Error: boom
```

如果需要让这种情况导致失败，请自行注册处理逻辑并调用 `process.exit`。

> <details><summary>为什么 unhandled rejection 只警告不报错？</summary>
>
> Node.js 在 v15 之前也是这个行为——unhandled rejection 打印警告但进程以 0 退出。原因是：很多代码在边界处有意「fire and forget」一个 Promise（比如通知类操作），期望失败不重要。
>
> Node 15+ 改为默认报错是因为生态共识变了——unhandled rejection 通常意味着 bug，不是设计选择。
>
> wjsm 当前保持「只警告」的行为。如果你想严格化：
>
> ```js
> process.on('unhandledRejection', (reason) => {
>   console.error('Unhandled rejection:', reason);
>   process.exit(1);
> });
> ```
>
> 放在程序入口顶部，跨整个进程生效。
>
> </details>

## async 函数与生成器

`async function`、`for await...of` 和 async 生成器均已实现：

```bash
wjsm run -e 'async function* g() { yield 1; yield 2 }
  (async () => { let s = 0; for await (const v of g()) s += v; console.log(s) })()'
```

```text
3
```

## 深入了解

- [微任务队列与异步调度器的实现](../../internals/runtime-features/async-scheduler.md)
- [async 上下文如何跨 await 传播](../../internals/runtime-features/async-hooks.md)
- [定时器、事件与流的宿主实现](../../internals/runtime-features/timers-events-and-streams.md)
