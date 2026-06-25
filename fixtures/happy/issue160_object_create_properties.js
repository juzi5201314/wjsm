const o = Object.create(null, {
  x: { value: 1, enumerable: true },
});
console.log("x:", o.x);
console.log("keys:", JSON.stringify(Object.keys(o)));