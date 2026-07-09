const { Worker, isMainThread } = require('worker_threads');
if (!isMainThread) throw new Error('expected main');
const w = new Worker(`
  const { parentPort } = require('worker_threads');
  parentPort.postMessage({ ok: 1 + 1 });
`, { eval: true });
w.on('message', (m) => {
  console.log(m.ok);
  w.terminate();
});
w.on('exit', (code) => {
  console.log('exit', code);
  process.exit(0);
});
