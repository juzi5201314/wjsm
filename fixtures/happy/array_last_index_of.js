// Array.prototype.lastIndexOf (ES2023 §23.1.3.20)

const a = [1, 2, 3, 2, 1];

console.log(a.lastIndexOf(2));
console.log(a.lastIndexOf(2, 3));
console.log(a.lastIndexOf(2, 2));
console.log(a.lastIndexOf(2, 1));
console.log(a.lastIndexOf(9));

// 负 fromIndex
console.log(a.lastIndexOf(2, -1));
console.log(a.lastIndexOf(2, -2));
console.log(a.lastIndexOf(2, -10));

// 边界
console.log([].lastIndexOf(1));
console.log([5].lastIndexOf(5));
console.log([5].lastIndexOf(5, 0));

// 字面量接收者（generic path via string.last_index_of fallback）
console.log([5, 5, 5].lastIndexOf(5));
console.log([1, 2, 3, 2, 1].lastIndexOf(2, 2));
