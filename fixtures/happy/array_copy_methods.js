// Array.prototype.toSorted / toReversed / toSpliced / with (ES2023)

// toSorted
const a = [3, 1, 2];
console.log(JSON.stringify(a.toSorted()));
console.log(JSON.stringify(a)); // 不变

console.log(JSON.stringify([3, 1, 2].toSorted((x, y) => y - x)));

// toReversed
const b = [1, 2, 3];
console.log(JSON.stringify(b.toReversed()));
console.log(JSON.stringify(b)); // 不变

// toSpliced
const c = [1, 2, 3, 4, 5];
console.log(JSON.stringify(c.toSpliced(1, 2, 99, 88)));
console.log(JSON.stringify(c)); // 不变

console.log(JSON.stringify([1, 2, 3].toSpliced(1, 1)));
console.log(JSON.stringify([1, 2, 3].toSpliced(10, 0, 99)));

// with
const d = [1, 2, 3];
console.log(JSON.stringify(d.with(1, 99)));
console.log(JSON.stringify(d.with(-1, 99)));
console.log(JSON.stringify(d)); // 不变

// 边界
console.log(JSON.stringify([].toSorted()));
console.log(JSON.stringify([5].toReversed()));
