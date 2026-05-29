// Class instance setter — verify side effect on assignment.
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
