const arr = JSON.parse('[1,2,3]', (k, v) => (k === '1' ? undefined : v));
console.log(0 in arr, 1 in arr, 2 in arr, arr.length);