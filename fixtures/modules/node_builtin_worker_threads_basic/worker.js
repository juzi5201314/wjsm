const { parentPort, workerData, isMainThread } = require('worker_threads');
const { AsyncLocalStorage } = require('async_hooks');
const als = new AsyncLocalStorage({ defaultValue: null });
parentPort.postMessage({
  hello: workerData.name,
  isolated: als.getStore() !== 'parent-context',
  isMainThread,
});
