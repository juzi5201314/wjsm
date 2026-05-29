class Base {
  constructor(x) {
    this.x = x;
  }
}

class Derived extends Base {
  y = this.x + 1;
  constructor(x) {
    super(x);
  }
}

const d = new Derived(5);
console.log(d.x);
console.log(d.y);
