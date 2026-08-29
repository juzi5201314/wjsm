function sum(array) {
  let total = 0;
  for (let i = 0; i < array.length; i++) {
    total += array[i];
  }
  return total;
}
const array = [1, 2, 3, 4];
let total = 0;
for (let i = 0; i < 120; i++) {
  total += sum(array);
}
console.log(total);
