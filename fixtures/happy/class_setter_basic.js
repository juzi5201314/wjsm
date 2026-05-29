// Class instance setter — documents actual behavior.
// NOTE: Setter is currently bypassed; assignment creates an own data property instead
// of invoking the setter. The getter then reads the own property, masking the prototype getter.
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
