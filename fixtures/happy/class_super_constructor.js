class Base {
  constructor(x) {
    this.x = x;
  }
}

class Derived extends Base {
  constructor(x) {
    super(x);
    this.y = 1;
  }
}

const d = new Derived(5);
console.log(d.x);
console.log(d.y);
console.log(d instanceof Base);
console.log(d instanceof Derived);
