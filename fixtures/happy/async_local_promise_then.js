const { AsyncLocalStorage } = require('async_hooks');
const als = new AsyncLocalStorage();
als.run('X', () => {
  Promise.resolve().then(() => console.log(als.getStore()));
});
