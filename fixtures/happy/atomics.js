var sab = new SharedArrayBuffer(16);
var ta = new Int32Array(sab);
Atomics.store(ta, 0, 42);
console.log(Atomics.load(ta, 0));
