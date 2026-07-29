# Promise、Proxy 与 Reflect 算法

这一章说明 Promise 链、async 函数和 Proxy/Reflect 的 builtins 实现。

## Promise

`promise.rs` 实现完整的 Promise 链：

- `PromiseResolve`：把值包装为已 fulfilled 的 Promise。
- `PromiseReject`：创建已 rejected 的 Promise。
- `PromiseThen`：注册 fulfill/reject reaction，返回新 Promise。
- `PromiseAll` / `PromiseAllSettled` / `PromiseRace` / `PromiseAny`：组合器，在 `promise_combinators.rs`。
- `PromiseWithResolvers`：返回 `{promise, resolve, reject}` 三元组。

`PromiseEntry` 记录状态（Pending/Fulfilled/Rejected）、reaction 列表、handled 标记和创建时捕获的 async hooks scope。`then` 的 reaction 继承创建时的 scope，这是 `AsyncLocalStorage` 能跨 await 传播的基础。

## async 函数与生成器

`async_fn.rs` 实现 async 函数的驱动：async 函数返回一个 Promise，函数体的 `await` 通过 `Continuation` 机制挂起与恢复。`Continuation` 有独立标签 `TAG_CONTINUATION`。

`async_generator.rs` 和 `generator.rs` 实现生成器协议：`yield` 挂起执行，`next()`/`return()`/`throw()` 恢复。async 生成器同时支持 `for await`。

## Proxy

`proxy_traps.rs` 实现 Proxy 的全部陷阱：

| 陷阱 | 对应操作 |
| --- | --- |
| `get` | 属性读取 |
| `set` | 属性写入 |
| `has` | `in` 操作 |
| `deleteProperty` | `delete` |
| `ownKeys` | `Object.keys` 等 |
| `getOwnPropertyDescriptor` | 属性描述符 |
| `defineProperty` | 属性定义 |
| `preventExtensions` / `isExtensible` | 扩展性控制 |
| `getPrototypeOf` / `setPrototypeOf` | 原型 |
| `apply` | 函数调用 |
| `construct` | `new` |

每个陷阱调用 handler 函数前保存当前执行状态，调用后恢复。陷阱的返回值需要类型检查，不合法时抛 TypeError。

## Reflect

`proxy_reflect.rs` 实现 Reflect API。Reflect 方法与 Proxy trap 一一对应，是 trap 的「默认行为」版本。Reflect 不是独立子系统，而是 Proxy trap 的正向使用。

## 深入了解

- [Promise 组合器的用户侧行为](../../user/runtime/async-and-promises.md)
- [async hooks 与 AsyncLocalStorage 的上下文传播](../runtime-features/async-hooks.md)
- [Proxy 的用户侧限制](../../user/runtime/limitations.md)
