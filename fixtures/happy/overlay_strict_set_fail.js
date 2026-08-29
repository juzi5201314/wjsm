"use strict";
function write(object, value) {
  object.x = value;
}
const object = { x: 0 };
for (let i = 0; i < 120; i++) {
  write(object, i);
}
Object.freeze(object);
try {
  write(object, 0);
  console.log("no-throw");
} catch (error) {
  console.log(error.name);
}
console.log(object.x);
