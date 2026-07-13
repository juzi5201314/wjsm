const { AsyncLocalStorage } = require('async_hooks');
const als = new AsyncLocalStorage();
async function f() {
  await Promise.resolve();
  console.log(als.getStore());
}
als.run('Y', () => {
  f();
});
