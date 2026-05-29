class Base {
  constructor() {
    this.base = true;
  }
}

class Derived extends Base {
  constructor() {
    this.x = 1;
    super();
  }
}

new Derived();
