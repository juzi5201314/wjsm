function read(object) {
  return object.x;
}
const stable = { x: 1 };
let total = 0;
for (let i = 0; i < 120; i++) {
  total += read(stable);
}
const other = { y: 0, x: 2 };
total += read(other);
console.log(total);
