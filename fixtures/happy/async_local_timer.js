const { AsyncLocalStorage } = require('async_hooks');
const als = new AsyncLocalStorage();
als.run('A', () => {
  setTimeout(() => {
    console.log(als.getStore());
  }, 0);
});
