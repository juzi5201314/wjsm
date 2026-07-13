const { AsyncLocalStorage } = require('async_hooks');
const als = new AsyncLocalStorage();
als.run(1, () => {
  als.exit(() => {
    console.log(als.getStore() === undefined);
  });
  console.log(als.getStore());
});
