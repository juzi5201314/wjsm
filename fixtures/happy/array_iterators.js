// Array.prototype.keys / values / entries (ES2015)

const a = [10, 20, 30];

// keys()
const keysIter = a.keys();
console.log(JSON.stringify(keysIter.next()));
console.log(JSON.stringify(keysIter.next()));
console.log(JSON.stringify(keysIter.next()));
console.log(JSON.stringify(keysIter.next()));

// values()
const valuesIter = a.values();
console.log(JSON.stringify(valuesIter.next()));
console.log(JSON.stringify(valuesIter.next()));
console.log(JSON.stringify(valuesIter.next()));
console.log(JSON.stringify(valuesIter.next()));

// entries()
const entriesIter = a.entries();
console.log(JSON.stringify(entriesIter.next()));
console.log(JSON.stringify(entriesIter.next()));
console.log(JSON.stringify(entriesIter.next()));
console.log(JSON.stringify(entriesIter.next()));

// for-of + spread
console.log(JSON.stringify([...["a", "b"].keys()]));
console.log(JSON.stringify([...[10, 20].values()]));
console.log(JSON.stringify([...["x", "y"].entries()]));

// Symbol.iterator === values
const arr = [1, 2, 3];
console.log(arr[Symbol.iterator] === arr.values);
console.log(JSON.stringify([...arr]));
