const { AsyncLocalStorage } = require('async_hooks');
const als = new AsyncLocalStorage();
als.run('A', () => {
  const p = Promise.resolve();
  setTimeout(() => {
    console.log('t', als.getStore());
  }, 0);
  process.nextTick(() => {
    console.log('n', als.getStore());
  });
  queueMicrotask(() => {
    console.log('q', als.getStore());
  });
  als.enterWith('B');
  p.then(() => {
    console.log('p', als.getStore());
  });
});
