const o = {};
Object.defineProperty(o, "x", { value: 1, configurable: false, writable: false });
let caught = "none";
try {
  Object.defineProperty(o, "x", { value: 2 });
} catch (e) {
  caught = e instanceof TypeError ? "TypeError" : "other";
}
console.log("redefine value:", caught, o.x);