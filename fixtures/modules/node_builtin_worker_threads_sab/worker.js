const { parentPort, workerData } = require('worker_threads');
const ta = new Int32Array(workerData.sab);
Atomics.add(ta, 0, 41);
parentPort.postMessage({ done: true, value: Atomics.load(ta, 0) });
