// TypedArray 综合测试
var buf = new ArrayBuffer(40);
var arr = new Int32Array(buf, 0, 8);
arr.set([1, 2, 3, 4, 5, 6, 7, 8], 0);
console.log("length: " + arr.length);
console.log("byteLength: " + arr.byteLength);
console.log("byteOffset: " + arr.byteOffset);

// at
console.log("at: " + arr.at(0));

// indexOf
console.log("indexOf: " + arr.indexOf(5));

// includes
console.log("includes: " + arr.includes(5));

// join (works because render_value handles NaN-boxed correctly)
console.log("join: " + arr.join("-"));

// toString
console.log("toString: " + arr.toString());
