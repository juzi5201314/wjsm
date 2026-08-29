function read(object) {
  return object.x;
}
const proto = { x: 1 };
const object = Object.create(proto);
let total = 0;
for (let i = 0; i < 120; i++) {
  total += read(object);
}
proto.x = 2;
total += read(object);
console.log(total);
