// Array with objects as elements (tests GC marking and nested property access)
let obj1 = { value: 1 };
let arr = [obj1];
// Read object back from array and access its property
console.log(arr[0].value);
// Compound assignment with computed index on array of numbers
let nums = [100];
nums[0] = nums[0] + 1;
console.log(nums[0]);
