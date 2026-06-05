var sab = new SharedArrayBuffer(4);
var ta = new Int32Array(sab);
Atomics.store(ta, 0, 11);
console.log(Atomics.load(ta, 0));
