const { AsyncLocalStorage, createHook } = require('node:async_hooks');
const als = new AsyncLocalStorage();
let count = 0;
const hook = createHook({ init() { count++; } }).enable();
let completed = 0;
let restored = true;

// 验证 AsyncLocalStorage 上下文经重复同步读取、Promise 回调与 GC 后仍保持。
// 重复次数只覆盖状态累积，不承担吞吐或压力测试职责。
const ITERATIONS = 64;

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
