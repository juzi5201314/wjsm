function write(object, value) {
  object.x = value;
  return object.x;
}
const object = { x: 0 };
let total = 0;
for (let i = 0; i < 120; i++) {
  total += write(object, 1);
}
console.log(total);
