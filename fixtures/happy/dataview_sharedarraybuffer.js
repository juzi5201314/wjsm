const sab = new SharedArrayBuffer(8);
const dv = new DataView(sab);
dv.setUint8(0, 77);
console.log(dv.getUint8(0));
console.log(new Uint8Array(sab)[0]);
const u8 = new Uint8Array(sab);
u8[1] = 88;
console.log(dv.getUint8(1));

const ab = new ArrayBuffer(2);
const abv = new DataView(ab);
abv.setUint8(0, 9);
console.log(abv.getUint8(0));
