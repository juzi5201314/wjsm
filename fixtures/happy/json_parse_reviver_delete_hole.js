const arr = JSON.parse("[1,2,3]", (_key, value) => (value === 2 ? undefined : value));
console.log("len:", arr.length);
console.log("in1:", 1 in arr);
console.log("keys:", Object.keys(arr).join(","));
console.log("join:", arr.join("|"));
console.log("json:", JSON.stringify(arr));
