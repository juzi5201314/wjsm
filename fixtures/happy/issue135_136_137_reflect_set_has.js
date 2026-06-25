// #135 writable data in-place, #136 inherited setter, #137 Reflect.has / in, inherited non-writable set false
const protoSetter = {
  set x(v) {
    this._x = v;
  },
};
const childSetter = Object.create(protoSetter);
const okSetter = Reflect.set(childSetter, "x", 42);
console.log("setter_called:", childSetter._x === 42);
console.log("setter_return:", okSetter);

const obj = { w: 1 };
Reflect.set(obj, "w", 2);
console.log("writable_update:", obj.w);
console.log("own_prop_count:", Object.getOwnPropertyNames(obj).length);

const protoReadonly = {};
Object.defineProperty(protoReadonly, "frozen", {
  value: 0,
  writable: false,
  configurable: true,
  enumerable: true,
});
const childReadonly = Object.create(protoReadonly);
console.log("inherited_has:", Reflect.has(childReadonly, "frozen"));
console.log("in_operator:", "frozen" in childReadonly);
console.log("set_inherited_nonwritable:", Reflect.set(childReadonly, "frozen", 9));