// Issue #390：同一调用点预热为 number 后发生类型漂移，必须保持完整 ECMAScript 语义。
function addOne(value) {
  return value + 1;
}

function invoke(value) {
  return addOne(value);
}

let sum = 0;
for (let i = 0; i < 150; i++) {
  sum += invoke(i);
}
console.log("number", sum);
console.log("string", invoke("x"));

try {
  invoke(1n);
  console.log("bigint", "no throw");
} catch (error) {
  console.log("bigint", error.name);
}

const coercible = {
  valueOf() {
    return 9;
  },
};
console.log("object", invoke(coercible));
console.log("boolean", invoke(true));
