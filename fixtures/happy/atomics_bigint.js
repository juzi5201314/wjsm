var sab = new SharedArrayBuffer(16);
var ta = new BigInt64Array(sab);
console.log(Atomics.store(ta, 0, 5n));
console.log(Atomics.load(ta, 0));
console.log(Atomics.add(ta, 0, 3n));
console.log(Atomics.load(ta, 0));
console.log(Atomics.compareExchange(ta, 0, 8n, 1n));
console.log(Atomics.load(ta, 0));
