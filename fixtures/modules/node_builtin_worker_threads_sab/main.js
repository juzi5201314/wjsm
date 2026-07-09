const { Worker, isMainThread } = require('worker_threads');
const path = require('path');
if (!isMainThread) throw new Error('expected main');
const sab = new SharedArrayBuffer(4);
const ta = new Int32Array(sab);
Atomics.store(ta, 0, 1);
const w = new Worker(path.join(__dirname, 'worker.js'), {
  workerData: { sab: sab },
});
w.on('message', (m) => {
  console.log(m.done, m.value, Atomics.load(ta, 0));
  w.terminate();
});
w.on('exit', (code) => {
  console.log('exit', code);
  process.exit(0);
});
