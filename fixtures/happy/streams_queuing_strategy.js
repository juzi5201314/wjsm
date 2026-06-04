// Test: CountQueuingStrategy and ByteLengthQueuingStrategy constructors and size methods
const count = new CountQueuingStrategy({ highWaterMark: 4 });
console.log("count hwm:", count.highWaterMark);
console.log("count size:", count.size("chunk"));

const byteLength = new ByteLengthQueuingStrategy({ highWaterMark: 8 });
console.log("byte hwm:", byteLength.highWaterMark);
console.log("byte size view:", byteLength.size(new Uint8Array(3)));
console.log("byte size object:", byteLength.size({ byteLength: 5 }));
