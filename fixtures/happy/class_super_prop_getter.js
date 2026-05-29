// super access inside getter.
class Base {
  get value() {
    return 42;
  }
}

class Derived extends Base {
  get value() {
    return super.value + 1;
  }
}

const d = new Derived();
console.log(d.value);
