// TypedArray prototype methods test

// fill
var arr1 = new Uint8Array(4);
arr1.fill(5);
console.log("fill: " + Array.from(arr1));

// reverse
var arr2 = new Uint8Array([1, 2, 3]);
arr2.reverse();
console.log("reverse: " + Array.from(arr2));

// indexOf
var arr3 = new Uint8Array([1, 2, 3]);
console.log("indexOf: " + arr3.indexOf(2));

// includes
console.log("includes: " + arr3.includes(2));

// join
console.log("join: " + arr3.join("-"));

// at (positive and negative)
console.log("at: " + arr3.at(-1));

// forEach
var foreachResult = [];
arr3.forEach(function(v) {
    foreachResult.push(v * 2);
});
console.log("forEach: " + foreachResult.join(","));

// map (returns Array, not TypedArray)
var mapped = arr3.map(function(v) { return v + 10; });
console.log("map type: " + (Array.isArray(mapped) ? "Array" : "not Array"));
console.log("map: " + mapped.join(","));

// reduce
var sum = arr3.reduce(function(a, b) { return a + b; }, 0);
console.log("reduce: " + sum);

// sort
var arr4 = new Uint8Array([3, 1, 2]);
arr4.sort();
console.log("sort: " + Array.from(arr4));
