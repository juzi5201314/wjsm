let source = {
  n: 1,
  arr: [1, { x: 2 }],
  nested: { ok: true },
  date: new Date(1234),
  re: /ab+/gi,
  ab: new ArrayBuffer(2),
  bytes: new Uint8Array([1, 2, 3]),
  buf: Buffer.from('hi'),
  map: new Map([[{ k: 1 }, { v: 2 }]]),
  set: new Set([{ s: 1 }])
};
let clone = structuredClone(source);
clone.arr[1].x = 9;
clone.bytes[0] = 9;
clone.buf[0] = 120;
console.log(source.arr[1].x === 2, source.bytes[0] === 1, source.buf.toString() === 'hi', clone.date.getTime() === 1234);
console.log(Buffer.isBuffer(clone.buf), clone.map.size === 1 && clone.set.size === 1);
try {
  structuredClone(function () {});
} catch (e) {
  console.log(e.name);
}
