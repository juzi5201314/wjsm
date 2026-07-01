var a = [1, 2];
a.foo = "x";
a.bar = "y";
Object.defineProperty(a, "hidden", { value: 1, enumerable: false, configurable: true });
var sym = Symbol("s");
a[sym] = "sym";

console.log("read:", a.foo);
console.log("keys:", Object.keys(a).join(","));
var forIn = [];
for (var k in a) {
  forIn.push(k);
}
console.log("forin:", forIn.join(","));
console.log("json:", JSON.stringify(a));