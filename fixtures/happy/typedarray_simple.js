// Simple TypedArray methods test (no callbacks)

// fill
var arr1 = new Uint8Array(4);
arr1.fill(5);
console.log("fill: " + arr1.join(","));

// reverse
var arr2 = new Uint8Array(3);
arr2.set([1, 2, 3], 0);
arr2.reverse();
console.log("reverse: " + arr2.join(","));

// indexOf
var arr3 = new Uint8Array(3);
arr3.set([1, 2, 3], 0);
console.log("indexOf: " + arr3.indexOf(2));

// includes
console.log("includes: " + arr3.includes(2));

// join
console.log("join: " + arr3.join("-"));

// at
console.log("at: " + arr3.at(-1));

// Numeric index assignment must not create ordinary object properties
arr3[0] = 9;
console.log("index assignment: " + arr3[0]);
console.log("index property leak: " + arr3.undefined);
arr3[-1] = 7;
console.log("negative index: " + arr3[-1]);
