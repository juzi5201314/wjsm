// Array.prototype.findLast / findLastIndex (ES2023)

const a = [1, 2, 3, 4, 5];

console.log(a.findLast(x => x < 3));
console.log(a.findLastIndex(x => x < 3));

console.log(a.findLast(x => x > 10));
console.log(a.findLastIndex(x => x > 10));

// 空数组
console.log([].findLast(x => true));
console.log([].findLastIndex(x => true));

// this 绑定
const arr = [10, 20, 30];
console.log(arr.findLast(function(x, i, a) {
  console.log("this:", this === undefined);
  return x > 15;
}));
