const { createHook } = require('node:async_hooks');

process.on('uncaughtException', () => {
  console.log('caught');
});

createHook({
  init() {
    throw new Error('hook boom');
  },
}).enable();

setTimeout(() => {}, 0);
