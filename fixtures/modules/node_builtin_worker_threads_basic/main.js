const { Worker, isMainThread } = require('worker_threads');
const path = require('path');
if (!isMainThread) throw new Error('expected main');
const w = new Worker(path.join(__dirname, 'worker.js'), {
  workerData: { name: 'wjsm' },
});
w.on('message', (m) => {
  console.log(m.hello);
  w.terminate();
});
w.on('exit', (code) => {
  console.log('exit', code);
  process.exit(0);
});
