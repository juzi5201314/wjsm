const { AsyncLocalStorage, createHook } = require('node:async_hooks');
const als = new AsyncLocalStorage();
let count = 0;
const hook = createHook({ init() { count++; } }).enable();
let completed = 0;
let restored = true;

als.enterWith('load-context');
for (let i = 0; i < 100000; i++) {
  completed++;
  if (als.getStore() !== 'load-context') restored = false;
}

als.run('promise-context', () => {
  Promise.resolve().then(() => {
    if (als.getStore() !== 'promise-context') restored = false;
    gc();
    hook.disable();
    console.log(completed === 100000 && count > 0 && restored);
  });
});
