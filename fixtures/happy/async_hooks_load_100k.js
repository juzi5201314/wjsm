const { AsyncLocalStorage, createHook } = require('node:async_hooks');
const als = new AsyncLocalStorage();
let count = 0;
const hook = createHook({ init() { count++; } }).enable();
let completed = 0;
let restored = true;

// 正确性验证：AsyncLocalStorage 上下文在同步循环与 promise 回调中不丢失。
// 迭代次数对语义是任意的（上下文保持与次数无关），取足以驱动 init hook
// 与一次 GC 的量即可；高负载稳定性由 Rust 侧单测覆盖。
const ITERATIONS = 1000;

als.enterWith('load-context');
for (let i = 0; i < ITERATIONS; i++) {
  completed++;
  if (als.getStore() !== 'load-context') restored = false;
}

als.run('promise-context', () => {
  Promise.resolve().then(() => {
    if (als.getStore() !== 'promise-context') restored = false;
    gc();
    hook.disable();
    console.log(completed === ITERATIONS && count > 0 && restored);
  });
});
