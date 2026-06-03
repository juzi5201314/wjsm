// Class instance setter — setter is invoked on assignment, prototype chain is respected.
// The setter on the prototype is called with the instance as `this`, and the getter
// reads the backing field set by the setter.
class Counter {
  constructor() {
    this._value = 0;
  }
  set value(v) {
    this._value = v * 2;
  }
  get value() {
    return this._value;
  }
}

const c = new Counter();
c.value = 5;
console.log(c.value);
