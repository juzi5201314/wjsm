function at(array, index) {
  return array[index];
}
function write(array, index, value) {
  array[index] = value;
  return array[index];
}
const view = new Float64Array([1, 2, 3]);
let total = 0;
for (let i = 0; i < 120; i++) {
  total += at(view, i % 3);
  write(view, i % 3, view[i % 3]);
}
console.log(total);
