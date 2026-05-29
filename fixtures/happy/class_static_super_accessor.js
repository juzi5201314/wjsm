class Base {
  static get value() {
    return this.suffix + 1;
  }
}

class Derived extends Base {
  static suffix = 41;
  static get value() {
    return super.value + 1;
  }
}

console.log(Derived.value);
