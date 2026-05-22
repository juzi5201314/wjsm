var sab = new SharedArrayBuffer(16);
console.log(sab.byteLength);
var sliced = sab.slice(4, 8);
console.log(sliced.byteLength);
