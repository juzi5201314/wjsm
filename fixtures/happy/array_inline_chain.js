let arr = [1, 2, 3, 4, 5, 6];
let r = arr.map(x => x * 2).filter(x => x % 3).reduce((a, b) => a + b, 0);
console.log(r);
