var sab = new SharedArrayBuffer(4);
var t = new Int32Array(sab);
console.log(t.length);
console.log(Atomics.store(t, 0, 42));
console.log(Atomics.load(t, 0));