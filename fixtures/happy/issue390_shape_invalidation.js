// Issue #390：热属性访问后的 shape、原型、accessor 与 proxy 变化必须使 overlay 失效。
function readValue(object) {
  return object.value;
}

function writeValue(object, value) {
  object.value = value;
  return object.value;
}

const target = { value: 1 };
let warm = 0;
for (let i = 0; i < 150; i++) {
  warm += readValue(target);
}
console.log("warm", warm);

target.extra = 2;
console.log("transition", readValue(target), target.extra);
console.log("write", writeValue(target, 5));

delete target.value;
Object.setPrototypeOf(target, { value: 7 });
console.log("prototype", readValue(target));

Object.defineProperty(target, "value", {
  configurable: true,
  get() {
    return this.extra + 10;
  },
  set(value) {
    this.extra = value;
  },
});
console.log("accessor-read", readValue(target));
console.log("accessor-write", writeValue(target, 20), target.extra);

const proxy = new Proxy(target, {
  get(object, key) {
    if (key === "value") return 99;
    return object[key];
  },
  set(object, key, value) {
    object[key] = value + 1;
    return true;
  },
});
console.log("proxy-read", readValue(proxy));
console.log("proxy-write", writeValue(proxy, 30), target.extra);
