// Atomics.waitAsync + Atomics.notify coverage: t=0 returns {async:false,value}, t>0 returns Promise settled via notify or timeout
const sab = new SharedArrayBuffer(4);
const ta = new Int32Array(sab);
ta[0] = 0;

const r1 = Atomics.waitAsync(ta, 0, 1, 0);
console.log(r1.async);
console.log(r1.value);

const n1 = Atomics.notify(ta, 0, 1);
console.log("n1=" + n1);

const r2 = Atomics.waitAsync(ta, 0, 0, 100);
console.log(r2.async);
const p = r2.value;
console.log(p == null ? "null" : "obj");
p.then(v => console.log("settled:" + v));

const n2 = Atomics.notify(ta, 0, 1);
console.log("n2=" + n2);
