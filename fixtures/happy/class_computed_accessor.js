// Computed accessor keys — getter/setter with [expr] names (issue #84)
const getterName = "value";
const setterName = "value";

class ComputedAccessor {
  #x = 0;
  get [getterName]() { return this.#x; }
  set [setterName](v) { this.#x = v; }
}

const obj = new ComputedAccessor();
obj.value = 99;
console.log(obj.value);
console.log(obj.value === 99);
