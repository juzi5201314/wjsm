function read(object) {
  return object.x;
}
const shapes = [
  { x: 1 },
  { a: 0, x: 1 },
  { a: 0, b: 0, x: 1 },
  { a: 0, b: 0, c: 0, x: 1 },
  { a: 0, b: 0, c: 0, d: 0, x: 1 },
];
let total = 0;
for (let i = 0; i < 120; i++) {
  total += read(shapes[i % 5]);
}
console.log(total);
