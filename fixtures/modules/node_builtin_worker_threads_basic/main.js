const { Worker, isMainThread } = require('worker_threads');
const { AsyncLocalStorage } = require('async_hooks');
const path = require('path');
if (!isMainThread) throw new Error('expected main');
const als = new AsyncLocalStorage();

als.run('parent-context', () => {
  const worker = new Worker(path.join(__dirname, 'worker.js'), {
    workerData: { name: 'wjsm' },
  });
  worker.on('message', (message) => {
    console.log(message.hello, message.isolated, !message.isMainThread, als.getStore());
    worker.terminate();
  });
  worker.on('exit', (code) => {
    console.log('exit', code);
    process.exit(0);
  });
});
