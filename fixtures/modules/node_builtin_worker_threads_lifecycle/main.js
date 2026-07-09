const { Worker, isMainThread } = require('worker_threads');
if (!isMainThread) throw new Error('expected main');

const thrower = new Worker(`
  throw new Error('boom');
`, { eval: true });

thrower.on('error', (err) => {
  console.log('error', err && err.message ? err.message : String(err));
});

thrower.on('exit', (code) => {
  console.log('throw-exit', code);
  const keeper = new Worker(`
    const { parentPort } = require('worker_threads');
    parentPort.on('message', () => {});
  `, { eval: true });
  keeper.on('online', () => {
    console.log('online');
    keeper.terminate();
  });
  keeper.on('exit', (code2) => {
    console.log('term-exit', code2);
    process.exit(0);
  });
});
