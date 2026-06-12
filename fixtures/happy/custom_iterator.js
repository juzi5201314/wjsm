const obj = {
  [Symbol.iterator]() {
    let i = 0;
    return {
      next() {
        i++;
        return { value: i, done: i > 2 };
      }
    };
  }
};
const arr = [...obj];
console.log(arr.length);
console.log(arr[0]);
console.log(arr[1]);
let sum = 0;
for (const value of obj) {
  sum += value;
}
console.log(sum);
