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
