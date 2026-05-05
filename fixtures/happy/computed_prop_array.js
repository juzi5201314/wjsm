// Computed property access on arrays
let arr = [100, 200, 300];
let idx = 1;
console.log(arr[idx]);
arr[idx] = 999;
console.log(arr[1]);
// Compound assignment with computed index
arr[idx] = arr[idx] + 1;
console.log(arr[1]);
