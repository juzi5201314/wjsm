const { parentPort } = require('worker_threads');
const { AsyncLocalStorage } = require('async_hooks');
const als = new AsyncLocalStorage({ defaultValue: null });
parentPort.postMessage({ isolated: als.getStore() !== 'parent-context' });
