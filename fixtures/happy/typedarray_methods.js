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

// lastIndexOf
console.log("lastIndexOf: " + arr3.lastIndexOf(2));

// copyWithin
var copy = new Uint8Array([1, 2, 3, 4]);
copy.copyWithin(0, 2);
console.log("copyWithin: " + Array.from(copy));

// filter
var filtered = arr3.filter(function(v) { return v > 1; });
console.log("filter type: " + (Array.isArray(filtered) ? "Array" : "not Array"));
console.log("filter: " + filtered.join(","));

// reduceRight
console.log("reduceRight: " + arr3.reduceRight(function(a, b) { return a - b; }, 0));

// find / findIndex / some / every
console.log("find: " + arr3.find(function(v) { return v > 2; }));
console.log("findIndex: " + arr3.findIndex(function(v) { return v > 2; }));
console.log("some: " + arr3.some(function(v) { return v > 2; }));
console.log("every: " + arr3.every(function(v) { return v > 0; }));

// entries / keys / values / toString
var entryList = Array.from(arr3.entries());
console.log("entries: " + entryList[0].join(":") + "," + entryList[1].join(":"));
console.log("keys: " + Array.from(arr3.keys()).join(","));
console.log("values: " + Array.from(arr3.values()).join(","));
console.log("toString: " + arr3.toString());
var liveValues = arr3.values();
arr3[1] = 8;
console.log("values live: " + Array.from(liveValues).join(","));
var liveEntries = arr3.entries();
arr3[2] = 7;
var liveEntryList = Array.from(liveEntries);
console.log("entries live: " + liveEntryList[2].join(":"));

// sort with compareFn
var desc = new Uint8Array([3, 1, 2]);
desc.sort(function(a, b) { return b - a; });
console.log("sort compare: " + Array.from(desc));

// Uint8ClampedArray uses ToUint8Clamp half-to-even conversion
var clamped = new Uint8ClampedArray([2.5, 3.5, -1, 300]);
console.log("clamped: " + Array.from(clamped));
clamped.fill(2.5);
console.log("clamped fill: " + Array.from(clamped));
