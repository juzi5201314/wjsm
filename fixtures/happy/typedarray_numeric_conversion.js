var u8 = new Uint8Array(4);
u8[0] = 257;
u8[1] = Infinity;
u8[2] = -1;
u8[3] = NaN;
console.log(Array.from(u8).join(","));

var i8 = new Int8Array(3);
i8[0] = 255;
i8[1] = 128;
i8[2] = -129;
console.log(Array.from(i8).join(","));

console.log(new Uint8Array(NaN).length);
console.log(new Uint8Array(1.9).length);
