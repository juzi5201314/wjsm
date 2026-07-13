const { AsyncLocalStorage } = require('async_hooks');
const a = new AsyncLocalStorage({ name: 'ctx', defaultValue: 3 });
console.log(a.name);
console.log(a.getStore());

console.log(a instanceof AsyncLocalStorage);
try { AsyncLocalStorage(); } catch (error) { console.log(error.name); }