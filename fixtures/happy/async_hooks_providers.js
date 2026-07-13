const { asyncWrapProviders } = require('node:async_hooks');

console.log(asyncWrapProviders.NONE, asyncWrapProviders.PROMISE);
console.log(asyncWrapProviders.HTTP2SESSION === undefined);
console.log(asyncWrapProviders.FSREQPROMISE === undefined);
console.log(Object.isFrozen(asyncWrapProviders));
try {
  asyncWrapProviders.PROMISE = 1;
} catch (_) {}
console.log(asyncWrapProviders.PROMISE);
