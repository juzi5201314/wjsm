function sum(array) {
  let total = 0;
  for (let i = 0; i < array.length; i++) {
    const value = array[i];
    if (value !== undefined) {
      total += value;
    }
  }
  return total;
}
const array = [1, , 3];
let total = 0;
for (let i = 0; i < 120; i++) {
  total += sum(array);
}
console.log(total);
