const path = require('path');
const { AsyncLocalStorage } = require('async_hooks');
const { Worker } = require('worker_threads');
const als = new AsyncLocalStorage();

als.run('parent-context', () => {
  const worker = new Worker(path.join(__dirname, 'worker.js'));
  worker.on('message', (message) => {
    console.log(message.isolated, als.getStore());
    worker.terminate();
  });
  worker.on('exit', () => process.exit(0));
});
