const { AsyncLocalStorage } = require('async_hooks');
const als = new AsyncLocalStorage();
console.log(als.getStore() === undefined);
als.run(42, () => {
  console.log(als.getStore());
});
console.log(als.getStore() === undefined);
