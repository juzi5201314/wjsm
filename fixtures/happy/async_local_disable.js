const { AsyncLocalStorage } = require('async_hooks');
const als = new AsyncLocalStorage({ defaultValue: 7 });
console.log(als.getStore());
als.run(1, () => {
  console.log(als.getStore());
  als.disable();
  console.log(als.getStore());
});
