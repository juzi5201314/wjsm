const { AsyncLocalStorage } = require('async_hooks');
const als = new AsyncLocalStorage();
als.run(7, () => {
  process.nextTick(() => console.log(als.getStore()));
});
