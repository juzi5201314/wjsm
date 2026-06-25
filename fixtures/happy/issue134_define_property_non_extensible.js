const o = {};
Object.preventExtensions(o);
let caught = "none";
try {
  Object.defineProperty(o, "x", { value: 1 });
} catch (e) {
  caught = e instanceof TypeError ? "TypeError" : "other";
}
console.log(caught, o.x === undefined);