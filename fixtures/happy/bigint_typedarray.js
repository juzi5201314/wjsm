// BigInt64Array / BigUint64Array test

// Creation
var b64 = new BigInt64Array(2);
console.log("BigInt64Array length: " + b64.length);

// Element access and assignment
b64[0] = 10n;
b64[1] = -20n;
console.log("b64[0]: " + b64[0]);
console.log("b64[1]: " + b64[1]);

// set method
var b64b = new BigInt64Array(4);
b64b.set([1n, 2n], 1);
console.log("set: " + Array.from(b64b).join(","));

// slice method
var sliced = b64.slice(0, 1);
console.log("slice length: " + sliced.length);
console.log("slice[0]: " + sliced[0]);

// BigUint64Array
var bu64 = new BigUint64Array(3);
bu64[0] = 100n;
bu64[1] = 200n;
console.log("BigUint64Array length: " + bu64.length);
console.log("bu64[0]: " + bu64[0]);
console.log("bu64[1]: " + bu64[1]);

// join
console.log("join: " + bu64.join("-"));

// indexOf
console.log("indexOf: " + bu64.indexOf(200n));

// includes
console.log("includes: " + bu64.includes(200n));
