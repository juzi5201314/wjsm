function at(array, index) {
  return array[index];
}
const values = [1, 2, 3];
let total = 0;
for (let i = 0; i < 120; i++) {
  total += at(values, i % 3);
}
console.log(total);
