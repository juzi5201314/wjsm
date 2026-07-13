const { AsyncLocalStorage } = require('async_hooks');
const als = new AsyncLocalStorage();
als.run(9, () => {
  queueMicrotask(() => console.log(als.getStore()));
});
