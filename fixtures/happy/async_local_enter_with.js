const { AsyncLocalStorage } = require('async_hooks');
const als = new AsyncLocalStorage();
als.enterWith(5);
console.log(als.getStore());
