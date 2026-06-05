var sab = new SharedArrayBuffer(16);
console.log(sab.byteLength);
console.log(sab.growable);
console.log(sab.maxByteLength);
var growable = new SharedArrayBuffer(4, { maxByteLength: 12 });
console.log(growable.byteLength);
console.log(growable.growable);
console.log(growable.maxByteLength);