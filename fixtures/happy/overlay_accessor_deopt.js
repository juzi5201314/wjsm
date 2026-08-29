function read(object) {
  return object.x;
}
const object = { x: 1 };
let total = 0;
for (let i = 0; i < 120; i++) {
  total += read(object);
}
Object.defineProperty(object, "x", {
  get() {
    return 99;
  },
});
total += read(object);
console.log(total);
