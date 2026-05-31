const arr = [1, 2, 3];
Reflect.deleteProperty(arr, 1);
console.log("in1:", 1 in arr);
console.log("keys:", Object.keys(arr).join(","));
console.log("join:", arr.join("|"));
