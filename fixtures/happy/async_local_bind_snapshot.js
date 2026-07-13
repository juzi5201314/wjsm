const { AsyncLocalStorage } = require('async_hooks');
const als = new AsyncLocalStorage();
const bound = als.run(10, function () {
  return AsyncLocalStorage.snapshot();
});
const v = bound(function () {
  return als.getStore();
});
console.log(v);
