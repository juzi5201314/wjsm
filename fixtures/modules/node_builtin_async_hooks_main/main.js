const ah = require('async_hooks');
const keys = [
  'createHook',
  'executionAsyncId',
  'triggerAsyncId',
  'executionAsyncResource',
  'asyncWrapProviders',
  'AsyncResource',
  'AsyncLocalStorage',
].map((k) => typeof ah[k]).join(',');
console.log(keys);
console.log(ah.executionAsyncId());
console.log(ah.triggerAsyncId());
console.log(typeof ah.asyncWrapProviders.PROMISE);
